use crate::domain::{ChatMessage, ChatRole, DigestCard, InterestModel, TrendEvidence};
use lazily::{Computed, Source, ThreadSafeContext};

#[derive(Debug, Clone, PartialEq)]
pub struct UiSnapshot {
    pub digest: Vec<DigestCard>,
    pub interests: InterestModel,
    pub evidence: Option<TrendEvidence>,
    pub tracked_topics: Vec<String>,
    pub suggested_topics: Vec<String>,
    pub chat: Vec<ChatMessage>,
    pub loading: bool,
    pub status: String,
}

#[derive(Clone)]
pub struct PulseState {
    context: ThreadSafeContext,
    digest: Source<Vec<DigestCard>>,
    interests: Source<InterestModel>,
    evidence: Source<Option<TrendEvidence>>,
    tracked_topics: Source<Vec<String>>,
    suggested_topics: Source<Vec<String>>,
    chat: Source<Vec<ChatMessage>>,
    loading: Source<bool>,
    next_message_id: Source<u64>,
    status: Computed<String>,
}

impl PulseState {
    pub fn new(
        interests: InterestModel,
        tracked_topics: Vec<String>,
        suggested_topics: Vec<String>,
    ) -> Self {
        let context = ThreadSafeContext::new();
        let digest = context.source(Vec::<DigestCard>::new());
        let interests = context.source(interests);
        let evidence = context.source(None::<TrendEvidence>);
        let tracked_topics = context.source(tracked_topics);
        let suggested_topics = context.source(suggested_topics);
        let chat = context.source(vec![ChatMessage {
            id: 0,
            role: ChatRole::Assistant,
            body: "Ask what is moving, tune an interest, or open the evidence behind a trend."
                .to_owned(),
            tool: None,
        }]);
        let loading = context.source(false);
        let next_message_id = context.source(1_u64);
        let status = context.computed(move |compute| {
            let cards = compute.get(&digest).len();
            let tracked = compute.get(&tracked_topics).len();
            let active = compute.get(&interests).active_count();
            format!("{cards}/5 signals · {active} interests · {tracked} tracked")
        });

        Self {
            context,
            digest,
            interests,
            evidence,
            tracked_topics,
            suggested_topics,
            chat,
            loading,
            next_message_id,
            status,
        }
    }

    pub fn snapshot(&self) -> UiSnapshot {
        UiSnapshot {
            digest: self.context.get(&self.digest),
            interests: self.context.get(&self.interests),
            evidence: self.context.get(&self.evidence),
            tracked_topics: self.context.get(&self.tracked_topics),
            suggested_topics: self.context.get(&self.suggested_topics),
            chat: self.context.get(&self.chat),
            loading: self.context.get(&self.loading),
            status: self.context.get(&self.status),
        }
    }

    pub fn interests(&self) -> InterestModel {
        self.context.get(&self.interests)
    }

    pub fn set_digest(&self, digest: Vec<DigestCard>) {
        self.context.set(&self.digest, digest);
    }

    pub fn set_interests(&self, interests: InterestModel) {
        self.context.set(&self.interests, interests);
    }

    pub fn set_evidence(&self, evidence: Option<TrendEvidence>) {
        self.context.set(&self.evidence, evidence);
    }

    pub fn set_tracked_topics(&self, topics: Vec<String>) {
        self.context.set(&self.tracked_topics, topics);
    }

    pub fn set_suggested_topics(&self, topics: Vec<String>) {
        self.context.set(&self.suggested_topics, topics);
    }

    pub fn set_loading(&self, loading: bool) {
        self.context.set(&self.loading, loading);
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
