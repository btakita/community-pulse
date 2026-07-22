use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ATTENTION_BUDGET: usize = 5;

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
    pub sources: Vec<String>,
    pub score: f64,
    pub trend_score: f64,
    pub interest_affinity: f64,
    pub z_score: f64,
    pub mentions_1h: usize,
    pub mentions_6h: usize,
    pub mentions_24h: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidencePost {
    pub source: String,
    pub title: String,
    pub url: String,
    pub published_at: DateTime<Utc>,
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
