use crate::domain::{
    Citation, CommunityPost, MAX_BUDGET, ResearchEnrichment, ResearchSection, ResearchSubmission,
};
use crate::engine::{PulseEngine, canonical_topic};
use crate::reactive::{PulseState, UiSnapshot};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path};
use std::sync::{Arc, Mutex};

const MAX_RESEARCH_MARKDOWN_BYTES: usize = 16 * 1024;
const MAX_RESEARCH_CITATIONS: usize = 20;
const MAX_RESEARCH_WINDOW_HOURS: usize = 24 * 30;
const MAX_RESEARCH_POSTS: usize = 100;
const MAX_SERIES_BUCKETS: usize = 168;
const MAX_SERIES_BUCKET_HOURS: usize = 24;

#[derive(Clone)]
pub struct ToolBridge {
    engine: Arc<Mutex<PulseEngine>>,
    state: PulseState,
}

impl ToolBridge {
    pub fn new(engine: PulseEngine) -> Result<Self> {
        let budget = engine.budget()?;
        let interests = engine.load_interests()?;
        let subscriptions = engine.subscriptions()?;
        let suggested = engine.suggested_topics(8)?;
        let delta_chips = engine.digest_delta_chips(&interests)?;
        let research = engine.list_research(None)?;
        Ok(Self {
            engine: Arc::new(Mutex::new(engine)),
            state: PulseState::new(
                budget,
                interests,
                subscriptions,
                suggested,
                delta_chips,
                research,
            ),
        })
    }

    pub fn state(&self) -> &PulseState {
        &self.state
    }

    pub fn snapshot(&self) -> UiSnapshot {
        self.state.snapshot()
    }

    pub fn get_pulse(&self, limit: Option<usize>) -> Result<Value> {
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let cards = engine.get_pulse(&self.state.interests(), limit, Utc::now())?;
        let budget = engine.budget()?;
        self.state.set_budget(budget);
        self.state.set_digest(cards.clone());
        Ok(json!({
            "attention_budget": budget,
            "count": cards.len(),
            "digest": cards,
        }))
    }

    pub fn refresh_scores(&self) -> Result<Value> {
        let mut engine = self.engine.lock().expect("pulse engine lock poisoned");
        engine.recompute(Utc::now())?;
        let cards = engine.get_pulse(&self.state.interests(), None, Utc::now())?;
        let suggested = engine.suggested_topics(8)?;
        self.state.set_digest(cards.clone());
        self.state.set_suggested_topics(suggested);
        Ok(json!({ "count": cards.len(), "digest": cards }))
    }

    pub fn ingest_sources(
        &self,
        sources: &[(String, Vec<CommunityPost>)],
        now: chrono::DateTime<Utc>,
    ) -> Result<usize> {
        let mut engine = self.engine.lock().expect("pulse engine lock poisoned");
        let mut ingested = 0;
        for (_, posts) in sources {
            ingested += engine.ingest(posts)?;
        }
        engine.recompute(now)?;
        let interests = self.state.interests();
        let cards = engine.get_pulse(&interests, None, now)?;
        let suggested = engine.suggested_topics(8)?;
        let delta_chips = engine.digest_delta_chips(&interests)?;
        let tracked = engine.subscriptions()?;
        let alert = cards
            .iter()
            .filter(|card| tracked.iter().any(|topic| topic == &card.id))
            .filter(|card| card.z_score >= 2.0)
            .max_by(|left, right| left.z_score.total_cmp(&right.z_score))
            .map(|card| {
                format!(
                    "{} just spiked {:+.1}σ · tap to inspect",
                    card.topic, card.z_score
                )
            })
            .unwrap_or_default();
        self.state.set_digest(cards);
        self.state.set_suggested_topics(suggested);
        self.state.set_delta_chips(delta_chips);
        self.state.set_alert(alert);
        Ok(ingested)
    }

    pub fn set_interests(
        &self,
        add: &[String],
        remove: &[String],
        attention_budget: Option<usize>,
    ) -> Result<Value> {
        let mut interests = self.state.interests();
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        for topic in add {
            let topic = canonical_topic(topic);
            interests.set(&topic, 1.0);
            engine.set_interest(&topic, 1.0)?;
        }
        for topic in remove {
            let topic = canonical_topic(topic);
            interests.set(&topic, -1.0);
            engine.set_interest(&topic, -1.0)?;
        }
        let budget = if let Some(budget) = attention_budget {
            engine.set_budget(budget)?
        } else {
            engine.budget()?
        };
        self.state.set_interests(interests.clone());
        self.state.set_budget(budget);
        let cards = engine.get_pulse(&interests, None, Utc::now())?;
        self.state.set_digest(cards.clone());
        Ok(json!({ "attention_budget": budget, "interests": interests, "digest": cards }))
    }

    pub fn set_budget(&self, budget: usize) -> Result<Value> {
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let budget = engine.set_budget(budget)?;
        let cards = engine.get_pulse(&self.state.interests(), None, Utc::now())?;
        self.state.set_budget(budget);
        self.state.set_digest(cards.clone());
        Ok(json!({
            "attention_budget": budget,
            "count": cards.len(),
            "digest": cards
        }))
    }

    pub fn set_interest(&self, topic: &str, weight: f64) -> Result<Value> {
        let topic = canonical_topic(topic);
        let weight = normalize_interest_weight(weight);
        let mut interests = self.state.interests();
        interests.set(&topic, weight);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        engine.set_interest(&topic, weight)?;
        let cards = engine.get_pulse(&interests, None, Utc::now())?;
        self.state.set_interests(interests.clone());
        self.state.set_digest(cards.clone());
        Ok(json!({ "topic": topic, "weight": weight, "digest": cards }))
    }

    pub fn explain_trend(&self, id: &str) -> Result<Value> {
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let evidence = engine.explain_trend(&canonical_topic(id), Utc::now())?;
        self.state.set_evidence(Some(evidence.clone()));
        Ok(serde_json::to_value(evidence)?)
    }

    pub fn list_topics(&self, window_hours: Option<usize>, min_z: Option<f64>) -> Result<Value> {
        let window_hours = window_hours
            .unwrap_or(6)
            .clamp(1, MAX_RESEARCH_WINDOW_HOURS);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let topics = engine.list_topics(Utc::now(), window_hours, min_z)?;
        Ok(json!({
            "count": topics.len(),
            "window_hours": window_hours,
            "topics": topics
        }))
    }

    pub fn topic_posts(
        &self,
        id: &str,
        window_hours: Option<usize>,
        limit: Option<usize>,
    ) -> Result<Value> {
        let id = canonical_topic(id);
        let window_hours = window_hours
            .unwrap_or(24 * 7)
            .clamp(1, MAX_RESEARCH_WINDOW_HOURS);
        let limit = limit.unwrap_or(20).clamp(1, MAX_RESEARCH_POSTS);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let posts = engine.topic_posts(&id, Utc::now(), window_hours, limit)?;
        Ok(json!({
            "id": id,
            "count": posts.len(),
            "window_hours": window_hours,
            "posts": posts
        }))
    }

    pub fn get_series(
        &self,
        id: &str,
        buckets: Option<usize>,
        bucket_hours: Option<usize>,
    ) -> Result<Value> {
        let id = canonical_topic(id);
        let buckets = buckets.unwrap_or(24).clamp(1, MAX_SERIES_BUCKETS);
        let bucket_hours = bucket_hours.unwrap_or(1).clamp(1, MAX_SERIES_BUCKET_HOURS);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        Ok(serde_json::to_value(engine.get_series(
            &id,
            Utc::now(),
            buckets,
            bucket_hours,
        )?)?)
    }

    pub fn clear_evidence(&self) {
        self.state.set_evidence(None);
    }

    pub fn subscribe_topic(&self, topic: &str) -> Result<Value> {
        let topic = canonical_topic(topic);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let subscriptions = engine.subscribe_topic(&topic, Utc::now())?;
        self.state.set_tracked_topics(subscriptions.clone());
        let alert = self
            .state
            .snapshot()
            .digest
            .into_iter()
            .find(|card| card.id == topic)
            .map(|card| {
                if card.z_score >= 2.0 {
                    format!(
                        "{} just spiked {:+.1}σ · alert armed",
                        card.topic, card.z_score
                    )
                } else {
                    format!("Tracking {} · alerts start at +2.0σ", card.topic)
                }
            })
            .unwrap_or_else(|| format!("Tracking {topic} · waiting for a scored signal"));
        self.state.set_alert(alert.clone());
        Ok(json!({ "subscribed": topic, "tracked_topics": subscriptions, "alert": alert }))
    }

    pub fn submit_research(
        &self,
        topic_id: &str,
        agent: &str,
        title: &str,
        markdown: &str,
        citations: &[Citation],
        web_report: Option<&str>,
    ) -> Result<Value> {
        self.submit_research_enriched(ResearchSubmission {
            topic_id: topic_id.to_owned(),
            agent: agent.to_owned(),
            title: title.to_owned(),
            markdown: markdown.to_owned(),
            citations: citations.to_vec(),
            web_report: web_report.map(str::to_owned),
            article_url: None,
            sections: vec![],
            enrichment: ResearchEnrichment::default(),
        })
    }

    pub fn submit_research_enriched(&self, submission: ResearchSubmission) -> Result<Value> {
        let article_url = validate_optional_source_url(submission.article_url.as_deref())?;
        let sections = validate_sections(submission.sections, &submission.citations)?;
        let submission = ResearchSubmission {
            topic_id: canonical_topic(&submission.topic_id),
            agent: submission.agent.trim().to_owned(),
            title: submission.title.trim().to_owned(),
            markdown: submission.markdown,
            citations: submission.citations,
            web_report: validate_web_report(submission.web_report.as_deref())?,
            article_url,
            sections,
            enrichment: validate_enrichment(submission.enrichment)?,
        };
        if submission.topic_id.is_empty()
            || submission.agent.is_empty()
            || submission.title.is_empty()
            || submission.markdown.trim().is_empty()
        {
            bail!("topic_id, agent, title, and markdown are required");
        }
        if submission.markdown.len() > MAX_RESEARCH_MARKDOWN_BYTES {
            bail!("research markdown exceeds the 16 KiB limit");
        }
        if submission.citations.len() > MAX_RESEARCH_CITATIONS {
            bail!("research reports accept at most 20 citations");
        }
        if submission
            .citations
            .iter()
            .any(|citation| !is_source_url(&citation.url))
        {
            bail!("research citation URLs must use http or https");
        }
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let mut submission = submission;
        let warning = if let Some(url) = submission.article_url.as_deref() {
            if engine.topic_has_post_url(&submission.topic_id, url)? {
                None
            } else {
                let unmatched = submission.article_url.take().unwrap_or_default();
                Some(format!(
                    "article_url did not match a post for {}; stored as topic-level research: {unmatched}",
                    submission.topic_id
                ))
            }
        } else {
            None
        };
        let report = engine.submit_research(&submission, Utc::now())?;
        let research = engine.list_research(None)?;
        self.state.set_research(research.clone());
        self.state
            .mark_research_submitted(&submission.topic_id, &submission.agent);
        if submission.article_url.is_none()
            && submission.enrichment.verdict.as_deref() == Some("manufactured")
            && self
                .state
                .snapshot()
                .tracked_topics
                .iter()
                .any(|tracked| tracked == &submission.topic_id)
        {
            self.state.set_alert(format!(
                "research flags {} as manufactured — tap to read",
                submission.topic_id
            ));
        }
        Ok(json!({ "report": report, "count": research.len(), "warning": warning }))
    }

    pub fn start_research_run(&self, topic_id: &str, agent: &str) -> u64 {
        self.state.start_research_run(
            canonical_topic(topic_id),
            agent.trim().to_owned(),
            Utc::now(),
        )
    }

    pub fn fail_research_run(&self, id: u64, stderr_tail: impl Into<String>) {
        self.state.fail_research_run(id, stderr_tail);
    }

    pub fn list_research(&self, topic_id: Option<&str>) -> Result<Value> {
        let topic_id = topic_id.map(canonical_topic);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let reports = engine.list_research(topic_id.as_deref())?;
        let all_reports = if topic_id.is_some() {
            engine.list_research(None)?
        } else {
            reports.clone()
        };
        self.state.set_research(all_reports);
        Ok(json!({ "count": reports.len(), "reports": reports }))
    }

    pub fn call(&self, name: &str, arguments: &str) -> Result<Value> {
        match name {
            "get_pulse" => {
                let arguments: GetPulseArgs = parse_arguments(arguments)?;
                self.get_pulse(arguments.limit)
            }
            "set_interests" => {
                let arguments: SetInterestsArgs = parse_arguments(arguments)?;
                self.set_interests(
                    &arguments.add,
                    &arguments.remove,
                    arguments.attention_budget,
                )
            }
            "explain_trend" => {
                let arguments: TrendArgs = parse_arguments(arguments)?;
                self.explain_trend(&arguments.id)
            }
            "subscribe_topic" => {
                let arguments: SubscribeArgs = parse_arguments(arguments)?;
                self.subscribe_topic(&arguments.topic)
            }
            "list_topics" => {
                let arguments: ListTopicsArgs = parse_arguments(arguments)?;
                self.list_topics(arguments.window, arguments.min_z)
            }
            "topic_posts" => {
                let arguments: TopicPostsArgs = parse_arguments(arguments)?;
                self.topic_posts(&arguments.id, arguments.window_hours, arguments.limit)
            }
            "get_series" => {
                let arguments: GetSeriesArgs = parse_arguments(arguments)?;
                self.get_series(&arguments.id, arguments.buckets, arguments.bucket_hours)
            }
            "submit_research" => {
                let arguments: SubmitResearchArgs = parse_arguments(arguments)?;
                self.submit_research_enriched(ResearchSubmission {
                    topic_id: arguments.topic_id,
                    agent: arguments.agent,
                    title: arguments.title,
                    markdown: arguments.markdown,
                    citations: arguments.citations,
                    web_report: arguments.web_report,
                    article_url: arguments.article_url,
                    sections: arguments.sections,
                    enrichment: ResearchEnrichment {
                        verdict: arguments.verdict,
                        summary: arguments.summary,
                        watch: arguments.watch,
                    },
                })
            }
            "list_research" => {
                let arguments: ListResearchArgs = parse_arguments(arguments)?;
                self.list_research(arguments.topic_id.as_deref())
            }
            _ => bail!("unknown tool: {name}"),
        }
    }

    pub fn tool_definitions() -> Vec<Value> {
        vec![
            json!({
                "type": "function",
                "function": {
                    "name": "get_pulse",
                    "description": "Return the ranked pulse using the user's stored attention budget unless a bounded limit is supplied.",
                    "parameters": {
                        "type": "object",
                        "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": MAX_BUDGET } },
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "set_interests",
                    "description": "Boost or mute topics, optionally set the user-owned attention budget, and immediately rerank the digest.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "add": { "type": "array", "items": { "type": "string" } },
                            "remove": { "type": "array", "items": { "type": "string" } },
                            "attention_budget": { "type": "integer", "minimum": 1, "maximum": MAX_BUDGET }
                        },
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "explain_trend",
                    "description": "Show velocity, baseline, z-score, sparkline, and source posts for a trend.",
                    "parameters": {
                        "type": "object",
                        "properties": { "id": { "type": "string" } },
                        "required": ["id"],
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "subscribe_topic",
                    "description": "Track a topic for future personal alerts.",
                    "parameters": {
                        "type": "object",
                        "properties": { "topic": { "type": "string" } },
                        "required": ["topic"],
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "list_topics",
                    "description": "Return the full unbudgeted ranked topic long tail for research.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "window": { "type": "integer", "minimum": 1, "maximum": MAX_RESEARCH_WINDOW_HOURS, "description": "Mention window in hours (default 6)." },
                            "min_z": { "type": "number" }
                        },
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "topic_posts",
                    "description": "Return source posts and URLs for one topic; research reads are not attention-budget capped.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "window_hours": { "type": "integer", "minimum": 1, "maximum": MAX_RESEARCH_WINDOW_HOURS },
                            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESEARCH_POSTS }
                        },
                        "required": ["id"],
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "get_series",
                    "description": "Return raw topic counts plus the stored baseline mean and standard deviation.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "buckets": { "type": "integer", "minimum": 1, "maximum": MAX_SERIES_BUCKETS },
                            "bucket_hours": { "type": "integer", "minimum": 1, "maximum": MAX_SERIES_BUCKET_HOURS }
                        },
                        "required": ["id"],
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "submit_research",
                    "description": "Persist agent-attributed research. Optional verdict, summary, and watch fields annotate the UI but never change ranking.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "topic_id": { "type": "string" },
                            "agent": { "type": "string" },
                            "title": { "type": "string" },
                            "markdown": { "type": "string", "maxLength": MAX_RESEARCH_MARKDOWN_BYTES },
                            "citations": {
                                "type": "array",
                                "maxItems": MAX_RESEARCH_CITATIONS,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "url": { "type": "string" },
                                        "note": { "type": "string" }
                                    },
                                    "required": ["url"],
                                    "additionalProperties": false
                                }
                            },
                            "web_report": { "type": "string" },
                            "article_url": { "type": "string", "description": "Evidence post URL for an article-level brief." },
                            "sections": {
                                "type": "array",
                                "maxItems": 5,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "kind": { "type": "string", "enum": ["what", "substance", "reaction", "credibility", "watch"] },
                                        "body": { "type": "string" },
                                        "quotes": {
                                            "type": "array",
                                            "maxItems": 3,
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "text": { "type": "string", "maxLength": 280 },
                                                    "url": { "type": "string" },
                                                    "author": { "type": "string" }
                                                },
                                                "required": ["text", "url"],
                                                "additionalProperties": false
                                            }
                                        }
                                    },
                                    "required": ["kind", "body"],
                                    "additionalProperties": false
                                }
                            },
                            "verdict": { "type": "string", "enum": ["organic", "manufactured", "unclear"] },
                            "summary": { "type": "string", "maxLength": 140 },
                            "watch": {
                                "type": "array",
                                "maxItems": 3,
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["topic_id", "agent", "title", "markdown"],
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "list_research",
                    "description": "List stored research reports, optionally filtered to one topic.",
                    "parameters": {
                        "type": "object",
                        "properties": { "topic_id": { "type": "string" } },
                        "additionalProperties": false
                    }
                }
            }),
        ]
    }
}

fn normalize_interest_weight(weight: f64) -> f64 {
    let weight = weight.clamp(-1.0, 2.0);
    if weight.abs() < 0.05 { 0.0 } else { weight }
}

fn validate_web_report(web_report: Option<&str>) -> Result<Option<String>> {
    let Some(web_report) = web_report.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if web_report.starts_with("https://claude.ai/") {
        return Ok(Some(web_report.to_owned()));
    }

    let path = Path::new(web_report);
    let reports_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("research/reports");
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || !path.starts_with(&reports_root)
    {
        bail!(
            "web_report must be an https://claude.ai/ URL or an absolute path inside {}",
            reports_root.display()
        );
    }
    Ok(Some(web_report.to_owned()))
}

fn validate_optional_source_url(url: Option<&str>) -> Result<Option<String>> {
    let Some(url) = url.map(str::trim).filter(|url| !url.is_empty()) else {
        return Ok(None);
    };
    if !is_source_url(url) {
        bail!("article_url must use http or https");
    }
    Ok(Some(url.to_owned()))
}

fn is_source_url(url: &str) -> bool {
    let url = url.trim();
    (url.starts_with("https://") && url.len() > "https://".len())
        || (url.starts_with("http://") && url.len() > "http://".len())
}

fn validate_sections(
    sections: Vec<ResearchSection>,
    citations: &[Citation],
) -> Result<Vec<ResearchSection>> {
    if sections.len() > 5 {
        bail!("article briefs accept at most 5 structured sections");
    }
    let mut kinds = std::collections::HashSet::new();
    sections
        .into_iter()
        .map(|section| {
            let kind = section.kind.trim().to_ascii_lowercase();
            if !matches!(
                kind.as_str(),
                "what" | "substance" | "reaction" | "credibility" | "watch"
            ) {
                bail!("unknown article brief section kind: {kind}");
            }
            if !kinds.insert(kind.clone()) {
                bail!("article brief section kind repeated: {kind}");
            }
            let body = section.body.trim().to_owned();
            if body.is_empty() {
                bail!("article brief section bodies cannot be empty");
            }
            if section.quotes.len() > 3 {
                bail!("article brief sections accept at most 3 quotes");
            }
            let quotes = section
                .quotes
                .into_iter()
                .map(|quote| {
                    let text = quote.text.trim().to_owned();
                    if text.is_empty() || text.chars().count() > 280 {
                        bail!("article brief quote text must be 1–280 characters");
                    }
                    let url = quote.url.trim().to_owned();
                    if !is_source_url(&url) {
                        bail!("article brief quote URLs must use http or https");
                    }
                    if !citations.iter().any(|citation| citation.url.trim() == url) {
                        bail!("every article brief quote URL must also appear in citations");
                    }
                    Ok(crate::domain::ResearchQuote {
                        text,
                        url,
                        author: quote
                            .author
                            .map(|author| author.trim().to_owned())
                            .filter(|author| !author.is_empty()),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(ResearchSection { kind, body, quotes })
        })
        .collect()
}

fn validate_enrichment(enrichment: ResearchEnrichment) -> Result<ResearchEnrichment> {
    let verdict = enrichment
        .verdict
        .map(|verdict| verdict.trim().to_ascii_lowercase())
        .filter(|verdict| !verdict.is_empty());
    if verdict
        .as_deref()
        .is_some_and(|verdict| !matches!(verdict, "organic" | "manufactured" | "unclear"))
    {
        bail!("research verdict must be organic, manufactured, or unclear");
    }
    let summary = enrichment
        .summary
        .map(|summary| summary.trim().to_owned())
        .filter(|summary| !summary.is_empty());
    if summary
        .as_deref()
        .is_some_and(|summary| summary.chars().count() > 140)
    {
        bail!("research summary exceeds the 140-character limit");
    }
    if enrichment.watch.len() > 3 {
        bail!("research watch accepts at most 3 topic slugs");
    }
    let mut seen = std::collections::HashSet::new();
    let watch = enrichment
        .watch
        .into_iter()
        .map(|topic| canonical_topic(&topic))
        .filter(|topic| !topic.is_empty() && seen.insert(topic.clone()))
        .collect();
    Ok(ResearchEnrichment {
        verdict,
        summary,
        watch,
    })
}

fn parse_arguments<'a, T: Deserialize<'a>>(arguments: &'a str) -> Result<T> {
    serde_json::from_str(if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    })
    .with_context(|| format!("invalid tool arguments: {arguments}"))
}

#[derive(Deserialize, Default)]
struct GetPulseArgs {
    limit: Option<usize>,
}

#[derive(Deserialize, Default)]
struct SetInterestsArgs {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    attention_budget: Option<usize>,
}

#[derive(Deserialize)]
struct TrendArgs {
    id: String,
}

#[derive(Deserialize)]
struct SubscribeArgs {
    topic: String,
}

#[derive(Deserialize, Default)]
struct ListTopicsArgs {
    window: Option<usize>,
    min_z: Option<f64>,
}

#[derive(Deserialize)]
struct TopicPostsArgs {
    id: String,
    window_hours: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct GetSeriesArgs {
    id: String,
    buckets: Option<usize>,
    bucket_hours: Option<usize>,
}

#[derive(Deserialize)]
struct SubmitResearchArgs {
    topic_id: String,
    agent: String,
    title: String,
    markdown: String,
    #[serde(default)]
    citations: Vec<Citation>,
    web_report: Option<String>,
    article_url: Option<String>,
    #[serde(default)]
    sections: Vec<ResearchSection>,
    verdict: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    watch: Vec<String>,
}

#[derive(Deserialize, Default)]
struct ListResearchArgs {
    topic_id: Option<String>,
}
