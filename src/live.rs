use crate::domain::{CommunityPost, SourceStatus};
use crate::ingest;
use crate::reactive::UiSnapshot;
use crate::tools::ToolBridge;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MIN_INGEST_INTERVAL: Duration = Duration::from_secs(120);
pub const DEFAULT_INGEST_INTERVAL: Duration = Duration::from_secs(300);
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

pub type FetchResult = Vec<(String, Result<Vec<CommunityPost>>)>;

#[async_trait]
pub trait IngestFeed: Send + Sync {
    async fn fetch(&self) -> FetchResult;
}

pub struct PublicFeed;

#[async_trait]
impl IngestFeed for PublicFeed {
    async fn fetch(&self) -> FetchResult {
        ingest::fetch_all()
            .await
            .into_iter()
            .map(|(name, result)| (name.to_owned(), result))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LivePolicy {
    interval: Duration,
}

impl LivePolicy {
    pub fn new(requested_seconds: u64) -> Self {
        Self {
            interval: Duration::from_secs(requested_seconds).max(MIN_INGEST_INTERVAL),
        }
    }

    pub fn interval(self) -> Duration {
        self.interval
    }

    pub fn delay_after_failures(self, failures: u32) -> Duration {
        let multiplier = 1_u32.checked_shl(failures.min(8)).unwrap_or(u32::MAX);
        self.interval
            .checked_mul(multiplier)
            .unwrap_or(MAX_BACKOFF)
            .min(MAX_BACKOFF)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TriggerOutcome {
    Started,
    Busy,
    Disabled,
    Cooldown(Duration),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngestSummary {
    pub ingested: usize,
    pub succeeded_sources: usize,
    pub failed_sources: usize,
}

#[derive(Default)]
struct Gate {
    running: bool,
    last_started: Option<Instant>,
}

#[derive(Clone)]
pub struct IngestController {
    bridge: ToolBridge,
    fixture_mode: bool,
    gate: Arc<Mutex<Gate>>,
}

impl IngestController {
    pub fn new(bridge: ToolBridge, fixture_mode: bool) -> Self {
        let controller = Self {
            bridge,
            fixture_mode,
            gate: Arc::new(Mutex::new(Gate::default())),
        };
        if fixture_mode {
            controller
                .bridge
                .state()
                .configure_ingest(false, "fixture mode — ingest off");
        }
        controller
    }

    pub fn trigger(
        &self,
        feed: Arc<dyn IngestFeed>,
        on_change: Arc<dyn Fn(UiSnapshot) + Send + Sync>,
    ) -> TriggerOutcome {
        let outcome = self.reserve(Instant::now());
        if outcome != TriggerOutcome::Started {
            self.report_outcome(&outcome);
            on_change(self.bridge.snapshot());
            return outcome;
        }

        self.bridge.state().begin_ingest();
        on_change(self.bridge.snapshot());
        let controller = self.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build ingest runtime");
            let _ = runtime.block_on(controller.run_reserved(feed.as_ref()));
            on_change(controller.bridge.snapshot());
        });
        outcome
    }

    pub fn start_live(&self, policy: LivePolicy, on_change: Arc<dyn Fn(UiSnapshot) + Send + Sync>) {
        if self.fixture_mode {
            self.report_outcome(&TriggerOutcome::Disabled);
            on_change(self.bridge.snapshot());
            return;
        }
        let controller = self.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build live ingest runtime");
            let feed = PublicFeed;
            let mut failures = 0_u32;
            loop {
                if controller.reserve(Instant::now()) == TriggerOutcome::Started {
                    controller.bridge.state().begin_ingest();
                    on_change(controller.bridge.snapshot());
                    let result = runtime.block_on(controller.run_reserved(&feed));
                    on_change(controller.bridge.snapshot());
                    failures = if result.is_ok() {
                        0
                    } else {
                        failures.saturating_add(1)
                    };
                }
                std::thread::sleep(policy.delay_after_failures(failures));
            }
        });
    }

    fn reserve(&self, now: Instant) -> TriggerOutcome {
        if self.fixture_mode {
            return TriggerOutcome::Disabled;
        }
        let mut gate = self.gate.lock().expect("ingest gate lock poisoned");
        if gate.running {
            return TriggerOutcome::Busy;
        }
        if let Some(last_started) = gate.last_started {
            let elapsed = now.saturating_duration_since(last_started);
            if elapsed < MIN_INGEST_INTERVAL {
                return TriggerOutcome::Cooldown(MIN_INGEST_INTERVAL - elapsed);
            }
        }
        gate.running = true;
        gate.last_started = Some(now);
        TriggerOutcome::Started
    }

    fn report_outcome(&self, outcome: &TriggerOutcome) {
        let message = match outcome {
            TriggerOutcome::Started => return,
            TriggerOutcome::Busy => "ingest already running".to_owned(),
            TriggerOutcome::Disabled => "fixture mode — ingest off".to_owned(),
            TriggerOutcome::Cooldown(remaining) => {
                format!("next ingest in {}s", remaining.as_secs().max(1))
            }
        };
        self.bridge.state().set_ingest_message(message);
    }

    async fn run_reserved(&self, feed: &dyn IngestFeed) -> Result<IngestSummary> {
        let results = feed.fetch().await;
        let mut statuses = Vec::with_capacity(results.len());
        let mut successful = Vec::new();
        let mut failed_sources = 0;
        for (name, result) in results {
            match result {
                Ok(posts) => {
                    statuses.push(SourceStatus {
                        name: name.clone(),
                        ok: true,
                        count: posts.len(),
                        error: String::new(),
                    });
                    successful.push((name, posts));
                }
                Err(error) => {
                    failed_sources += 1;
                    statuses.push(SourceStatus {
                        name,
                        ok: false,
                        count: 0,
                        error: format!("{error:#}"),
                    });
                }
            }
        }

        let result = if successful.is_empty() {
            Err(anyhow!("all ingesters failed"))
        } else {
            let succeeded_sources = successful.len();
            self.bridge
                .ingest_sources(&successful, Utc::now())
                .map(|ingested| IngestSummary {
                    ingested,
                    succeeded_sources,
                    failed_sources,
                })
        };

        match &result {
            Ok(summary) => self.bridge.state().finish_ingest(
                Some(Utc::now()),
                statuses,
                format!("+{} posts", summary.ingested),
            ),
            Err(error) => self.bridge.state().finish_ingest(
                None,
                statuses,
                format!("ingest failed · {error:#}"),
            ),
        }
        self.gate.lock().expect("ingest gate lock poisoned").running = false;
        result
    }
}

impl Default for LivePolicy {
    fn default() -> Self {
        Self {
            interval: DEFAULT_INGEST_INTERVAL,
        }
    }
}
