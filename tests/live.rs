use anyhow::anyhow;
use async_trait::async_trait;
use chrono::Utc;
use community_pulse::domain::CommunityPost;
use community_pulse::live::{
    FetchResult, IngestController, IngestFeed, LivePolicy, MIN_INGEST_INTERVAL, TriggerOutcome,
};
use community_pulse::{PulseEngine, ToolBridge};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

struct StubFeed {
    calls: Arc<AtomicUsize>,
}

struct FailingFeed;

#[async_trait]
impl IngestFeed for FailingFeed {
    async fn fetch(&self) -> FetchResult {
        vec![("failed source".to_owned(), Err(anyhow!("offline")))]
    }
}

#[async_trait]
impl IngestFeed for StubFeed {
    async fn fetch(&self) -> FetchResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        vec![
            (
                "working source".to_owned(),
                Ok(vec![CommunityPost {
                    id: "stub:controller-1".to_owned(),
                    source: "Stub".to_owned(),
                    title: "Controller test signal".to_owned(),
                    url: "https://example.com/controller".to_owned(),
                    author: "fixture".to_owned(),
                    published_at: Utc::now(),
                    points: 7,
                    tags: vec!["controller-test".to_owned()],
                }]),
            ),
            ("failed source".to_owned(), Err(anyhow!("offline"))),
        ]
    }
}

fn fixture_bridge() -> ToolBridge {
    let mut engine = PulseEngine::in_memory().unwrap();
    engine.load_fixture(Utc::now()).unwrap();
    ToolBridge::new(engine).unwrap()
}

#[test]
fn controller_ingests_partial_success_and_enforces_one_floor_for_every_trigger() {
    let bridge = fixture_bridge();
    let controller = IngestController::new(bridge.clone(), false);
    let calls = Arc::new(AtomicUsize::new(0));
    let feed = Arc::new(StubFeed {
        calls: Arc::clone(&calls),
    });
    let (sender, receiver) = mpsc::channel();
    let on_change = Arc::new(move |snapshot| sender.send(snapshot).unwrap());

    assert_eq!(
        controller.trigger(feed.clone(), on_change.clone()),
        TriggerOutcome::Started
    );
    let completed = loop {
        let snapshot = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        if !snapshot.ingesting && !snapshot.source_status.is_empty() {
            break snapshot;
        }
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.source_status.len(), 2);
    assert!(completed.source_status[0].ok);
    assert!(!completed.source_status[1].ok);
    assert_eq!(completed.ingest_message, "+1 posts");
    assert!(completed.last_ingest_at.is_some());

    assert!(matches!(
        controller.trigger(feed, on_change),
        TriggerOutcome::Cooldown(remaining) if remaining <= MIN_INGEST_INTERVAL
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn fixture_mode_disables_ingest_without_calling_sources() {
    let bridge = fixture_bridge();
    let controller = IngestController::new(bridge, true);
    let calls = Arc::new(AtomicUsize::new(0));
    let feed = Arc::new(StubFeed {
        calls: Arc::clone(&calls),
    });

    assert_eq!(
        controller.trigger(feed, Arc::new(|_| {})),
        TriggerOutcome::Disabled
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn all_source_failure_releases_the_running_gate_and_reports_the_error() {
    let bridge = fixture_bridge();
    let controller = IngestController::new(bridge, false);
    let (sender, receiver) = mpsc::channel();
    let on_change = Arc::new(move |snapshot| sender.send(snapshot).unwrap());

    assert_eq!(
        controller.trigger(Arc::new(FailingFeed), on_change.clone()),
        TriggerOutcome::Started
    );
    let completed = loop {
        let snapshot = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        if !snapshot.ingesting && !snapshot.source_status.is_empty() {
            break snapshot;
        }
    };
    assert!(completed.ingest_message.contains("all ingesters failed"));
    assert!(matches!(
        controller.trigger(Arc::new(FailingFeed), on_change),
        TriggerOutcome::Cooldown(_)
    ));
}

#[test]
fn live_policy_clamps_to_the_source_floor_and_backs_off() {
    let policy = LivePolicy::new(1);
    assert_eq!(policy.interval(), MIN_INGEST_INTERVAL);
    assert_eq!(policy.delay_after_failures(1), MIN_INGEST_INTERVAL * 2);
    assert!(policy.delay_after_failures(20) <= Duration::from_secs(30 * 60));
}
