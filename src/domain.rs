use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_BUDGET: usize = 5;
pub const MAX_BUDGET: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommunityPost {
    pub id: String,
    pub source: String,
    pub title: String,
    pub url: String,
    pub author: String,
    pub published_at: DateTime<Utc>,
    pub points: i64,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DigestCard {
    pub id: String,
    pub topic: String,
    pub headline: String,
    pub headline_url: String,
    pub sources: Vec<String>,
    pub score: f64,
    pub trend_score: f64,
    pub interest_affinity: f64,
    pub z_score: f64,
    pub mentions_1h: usize,
    pub mentions_6h: usize,
    pub mentions_24h: usize,
    pub sparkline: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidencePost {
    pub source: String,
    pub title: String,
    pub url: String,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub url: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchEnrichment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchQuote {
    pub text: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchSection {
    pub kind: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quotes: Vec<ResearchQuote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchSubmission {
    pub topic_id: String,
    pub agent: String,
    pub title: String,
    pub markdown: String,
    #[serde(default)]
    pub citations: Vec<Citation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_report: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<ResearchSection>,
    #[serde(default, flatten)]
    pub enrichment: ResearchEnrichment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchReport {
    pub id: i64,
    pub topic_id: String,
    pub agent: String,
    pub title: String,
    pub markdown: String,
    pub citations: Vec<Citation>,
    pub web_report: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<ResearchSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchTopic {
    pub id: String,
    pub display: String,
    pub z: f64,
    pub trend: f64,
    pub mentions: usize,
    pub window_hours: usize,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchPost {
    pub source: String,
    pub title: String,
    pub url: String,
    pub points: i64,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchSeries {
    pub id: String,
    pub buckets: usize,
    pub bucket_hours: usize,
    pub counts: Vec<usize>,
    pub baseline_mean: f64,
    pub baseline_stddev: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchRun {
    pub id: u64,
    pub topic_id: String,
    pub agent: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub progress: String,
    pub stderr_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStatus {
    pub name: String,
    pub ok: bool,
    pub count: usize,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrendEvidence {
    pub id: String,
    pub topic: String,
    pub mentions_1h: usize,
    pub mentions_6h: usize,
    pub mentions_24h: usize,
    pub baseline_mean: f64,
    pub baseline_stddev: f64,
    pub z_score: f64,
    pub sparkline: Vec<usize>,
    pub posts: Vec<EvidencePost>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InterestModel(pub BTreeMap<String, f64>);

impl InterestModel {
    pub fn weight(&self, topic: &str) -> f64 {
        self.0.get(topic).copied().unwrap_or_default()
    }

    pub fn set(&mut self, topic: impl Into<String>, weight: f64) {
        let topic = topic.into();
        if weight.abs() < f64::EPSILON {
            self.0.remove(&topic);
        } else {
            self.0.insert(topic, weight.clamp(-1.0, 2.0));
        }
    }

    pub fn affinity(&self, topic: &str) -> f64 {
        let weight = self.weight(topic);
        if weight < 0.0 {
            0.0
        } else {
            1.0 + (weight * 0.65)
        }
    }

    pub fn active_count(&self) -> usize {
        self.0.values().filter(|weight| **weight > 0.0).count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub role: ChatRole,
    pub body: String,
    pub tool: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicScore {
    pub id: String,
    pub topic: String,
    pub mentions_1h: usize,
    pub mentions_6h: usize,
    pub mentions_24h: usize,
    pub baseline_mean: f64,
    pub baseline_stddev: f64,
    pub z_score: f64,
    pub trend_score: f64,
    pub captured_at: DateTime<Utc>,
}
