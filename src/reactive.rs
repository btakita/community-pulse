use crate::domain::{
    ChatMessage, ChatRole, DigestCard, InterestModel, ResearchReport, ResearchRun, SourceStatus,
    SourceWeight, TrendEvidence,
};
use chrono::{DateTime, Utc};
use lazily::{Computed, Source, ThreadSafeContext};

#[derive(Debug, Clone, PartialEq)]
pub struct UiSnapshot {
    pub digest: Vec<DigestCard>,
    pub budget: usize,
    pub interests: InterestModel,
    pub evidence: Option<TrendEvidence>,
    pub research: Vec<ResearchReport>,
    pub research_runs: Vec<ResearchRun>,
    pub tracked_topics: Vec<String>,
    pub suggested_topics: Vec<String>,
    pub delta_chips: Vec<String>,
    pub alert: String,
    pub chat: Vec<ChatMessage>,
    pub loading: bool,
    pub ingesting: bool,
    pub ingest_enabled: bool,
    pub last_ingest_at: Option<DateTime<Utc>>,
    pub source_status: Vec<SourceStatus>,
    pub source_weights: Vec<SourceWeight>,
    pub ingest_message: String,
    pub status: String,
}

#[derive(Clone)]
pub struct PulseState {
    context: ThreadSafeContext,
    digest: Source<Vec<DigestCard>>,
    budget: Source<usize>,
    interests: Source<InterestModel>,
    evidence: Source<Option<TrendEvidence>>,
    research: Source<Vec<ResearchReport>>,
    research_runs: Source<Vec<ResearchRun>>,
    tracked_topics: Source<Vec<String>>,
    suggested_topics: Source<Vec<String>>,
    delta_chips: Source<Vec<String>>,
    alert: Source<String>,
    chat: Source<Vec<ChatMessage>>,
    loading: Source<bool>,
    ingesting: Source<bool>,
    ingest_enabled: Source<bool>,
    last_ingest_at: Source<Option<DateTime<Utc>>>,
    source_status: Source<Vec<SourceStatus>>,
    source_weights: Source<Vec<SourceWeight>>,
    ingest_message: Source<String>,
    next_message_id: Source<u64>,
    status: Computed<String>,
}

impl PulseState {
    pub fn new(
        budget: usize,
        interests: InterestModel,
        tracked_topics: Vec<String>,
        suggested_topics: Vec<String>,
        delta_chips: Vec<String>,
        source_weights: Vec<SourceWeight>,
        research: Vec<ResearchReport>,
    ) -> Self {
        let context = ThreadSafeContext::new();
        let digest = context.source(Vec::<DigestCard>::new());
        let budget = context.source(budget);
        let interests = context.source(interests);
        let evidence = context.source(None::<TrendEvidence>);
        let research = context.source(research);
        let research_runs = context.source(Vec::<ResearchRun>::new());
        let tracked_topics = context.source(tracked_topics);
        let suggested_topics = context.source(suggested_topics);
        let delta_chips = context.source(delta_chips);
        let alert = context.source(String::new());
        let chat = context.source(vec![ChatMessage {
            id: 0,
            role: ChatRole::Assistant,
            body: "Ask what is moving, tune an interest, or open the evidence behind a trend."
                .to_owned(),
            tool: None,
        }]);
        let loading = context.source(false);
        let ingesting = context.source(false);
        let ingest_enabled = context.source(true);
        let last_ingest_at = context.source(None::<DateTime<Utc>>);
        let source_status = context.source(Vec::<SourceStatus>::new());
        let source_weights = context.source(source_weights);
        let ingest_message = context.source("ingest ready".to_owned());
        let next_message_id = context.source(1_u64);
        let status = context.computed(move |compute| {
            let cards = compute.get(&digest).len();
            let budget = compute.get(&budget);
            let tracked = compute.get(&tracked_topics).len();
            let active = compute.get(&interests).active_count();
            format!("{cards}/{budget} signals · {active} interests · {tracked} tracked")
        });

        Self {
            context,
            digest,
            budget,
            interests,
            evidence,
            research,
            research_runs,
            tracked_topics,
            suggested_topics,
            delta_chips,
            alert,
            chat,
            loading,
            ingesting,
            ingest_enabled,
            last_ingest_at,
            source_status,
            source_weights,
            ingest_message,
            next_message_id,
            status,
        }
    }

    pub fn snapshot(&self) -> UiSnapshot {
        UiSnapshot {
            digest: self.context.get(&self.digest),
            budget: self.context.get(&self.budget),
            interests: self.context.get(&self.interests),
            evidence: self.context.get(&self.evidence),
            research: self.context.get(&self.research),
            research_runs: self.context.get(&self.research_runs),
            tracked_topics: self.context.get(&self.tracked_topics),
            suggested_topics: self.context.get(&self.suggested_topics),
            delta_chips: self.context.get(&self.delta_chips),
            alert: self.context.get(&self.alert),
            chat: self.context.get(&self.chat),
            loading: self.context.get(&self.loading),
            ingesting: self.context.get(&self.ingesting),
            ingest_enabled: self.context.get(&self.ingest_enabled),
            last_ingest_at: self.context.get(&self.last_ingest_at),
            source_status: self.context.get(&self.source_status),
            source_weights: self.context.get(&self.source_weights),
            ingest_message: self.context.get(&self.ingest_message),
            status: self.context.get(&self.status),
        }
    }

    pub fn interests(&self) -> InterestModel {
        self.context.get(&self.interests)
    }

    pub fn budget(&self) -> usize {
        self.context.get(&self.budget)
    }

    pub fn set_digest(&self, digest: Vec<DigestCard>) {
        self.context.set(&self.digest, digest);
    }

    pub fn set_source_weights(&self, source_weights: Vec<SourceWeight>) {
        self.context.set(&self.source_weights, source_weights);
    }

    pub fn set_budget(&self, budget: usize) {
        self.context.set(&self.budget, budget);
    }

    pub fn set_interests(&self, interests: InterestModel) {
        self.context.set(&self.interests, interests);
    }

    pub fn set_evidence(&self, evidence: Option<TrendEvidence>) {
        self.context.set(&self.evidence, evidence);
    }

    pub fn set_research(&self, research: Vec<ResearchReport>) {
        self.context.set(&self.research, research);
    }

    pub fn start_research_run(
        &self,
        topic_id: impl Into<String>,
        agent: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> u64 {
        let mut id = 1;
        let topic_id = topic_id.into();
        let agent = agent.into();
        self.context.batch(|context| {
            let mut runs = context.get(&self.research_runs);
            id = runs.iter().map(|run| run.id).max().unwrap_or_default() + 1;
            runs.push(ResearchRun {
                id,
                topic_id,
                agent,
                status: "running".to_owned(),
                started_at,
                finished_at: None,
                progress: "Launching research harness".to_owned(),
                stderr_tail: String::new(),
            });
            context.set(&self.research_runs, runs);
        });
        id
    }

    pub fn mark_research_submitted(&self, topic_id: &str, agent: &str) {
        self.context.batch(|context| {
            let mut runs = context.get(&self.research_runs);
            if let Some(run) = runs.iter_mut().rev().find(|run| {
                run.topic_id == topic_id
                    && research_agents_match(&run.agent, agent)
                    && run.status == "running"
            }) {
                run.status = "done".to_owned();
                run.finished_at = Some(Utc::now());
                run.progress = "Report submitted".to_owned();
                run.stderr_tail.clear();
                context.set(&self.research_runs, runs);
            }
        });
    }

    pub fn update_research_progress(&self, id: u64, progress: impl Into<String>) -> bool {
        let progress = progress.into();
        let mut changed = false;
        self.context.batch(|context| {
            let mut runs = context.get(&self.research_runs);
            if let Some(run) = runs
                .iter_mut()
                .find(|run| run.id == id && run.status == "running")
            {
                if run.progress == progress {
                    return;
                }
                run.progress = progress;
                context.set(&self.research_runs, runs);
                changed = true;
            }
        });
        changed
    }

    pub fn fail_research_run(&self, id: u64, stderr_tail: impl Into<String>) {
        let stderr_tail = stderr_tail.into();
        self.context.batch(|context| {
            let mut runs = context.get(&self.research_runs);
            if let Some(run) = runs
                .iter_mut()
                .find(|run| run.id == id && run.status == "running")
            {
                run.status = "failed".to_owned();
                run.finished_at = Some(Utc::now());
                run.progress = "Research failed".to_owned();
                run.stderr_tail = stderr_tail;
                context.set(&self.research_runs, runs);
            }
        });
    }

    pub fn set_tracked_topics(&self, topics: Vec<String>) {
        self.context.set(&self.tracked_topics, topics);
    }

    pub fn set_suggested_topics(&self, topics: Vec<String>) {
        self.context.set(&self.suggested_topics, topics);
    }

    pub fn set_delta_chips(&self, chips: Vec<String>) {
        self.context.set(&self.delta_chips, chips);
    }

    pub fn set_alert(&self, alert: impl Into<String>) {
        self.context.set(&self.alert, alert.into());
    }

    pub fn set_loading(&self, loading: bool) {
        self.context.set(&self.loading, loading);
    }

    pub fn configure_ingest(&self, enabled: bool, message: impl Into<String>) {
        let message = message.into();
        self.context.batch(|context| {
            context.set(&self.ingest_enabled, enabled);
            context.set(&self.ingest_message, message);
        });
    }

    pub fn begin_ingest(&self) {
        self.context.batch(|context| {
            context.set(&self.ingesting, true);
            context.set(&self.ingest_message, "ingesting…".to_owned());
        });
    }

    pub fn finish_ingest(
        &self,
        completed_at: Option<DateTime<Utc>>,
        statuses: Vec<SourceStatus>,
        message: impl Into<String>,
    ) {
        let message = message.into();
        self.context.batch(|context| {
            context.set(&self.ingesting, false);
            if let Some(completed_at) = completed_at {
                context.set(&self.last_ingest_at, Some(completed_at));
            }
            context.set(&self.source_status, statuses);
            context.set(&self.ingest_message, message);
        });
    }

    pub fn set_ingest_message(&self, message: impl Into<String>) {
        self.context.set(&self.ingest_message, message.into());
    }

    pub fn append_chat(
        &self,
        role: ChatRole,
        body: impl Into<String>,
        tool: Option<String>,
    ) -> u64 {
        let id = self.context.get(&self.next_message_id);
        self.context.batch(|context| {
            let mut messages = context.get(&self.chat);
            messages.push(ChatMessage {
                id,
                role,
                body: body.into(),
                tool,
            });
            context.set(&self.chat, messages);
            context.set(&self.next_message_id, id + 1);
        });
        id
    }

    pub fn append_to_chat(&self, id: u64, delta: &str) {
        self.context.batch(|context| {
            let mut messages = context.get(&self.chat);
            if let Some(message) = messages.iter_mut().find(|message| message.id == id) {
                message.body.push_str(delta);
                context.set(&self.chat, messages);
            }
        });
    }

    pub fn replace_chat(&self, id: u64, body: impl Into<String>) {
        let body = body.into();
        self.context.batch(|context| {
            let mut messages = context.get(&self.chat);
            if let Some(message) = messages.iter_mut().find(|message| message.id == id) {
                message.body = body;
                context.set(&self.chat, messages);
            }
        });
    }
}

fn research_agents_match(run_agent: &str, submitted_agent: &str) -> bool {
    if run_agent.eq_ignore_ascii_case(submitted_agent) {
        return true;
    }

    let run_agent = run_agent.to_ascii_lowercase();
    let submitted_agent = submitted_agent.to_ascii_lowercase();
    ["claude", "codex"]
        .iter()
        .any(|family| run_agent.contains(family) && submitted_agent.contains(family))
}
