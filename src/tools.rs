use crate::domain::ATTENTION_BUDGET;
use crate::engine::{PulseEngine, canonical_topic};
use crate::reactive::{PulseState, UiSnapshot};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct ToolBridge {
    engine: Arc<Mutex<PulseEngine>>,
    state: PulseState,
}

impl ToolBridge {
    pub fn new(engine: PulseEngine) -> Result<Self> {
        let interests = engine.load_interests()?;
        let subscriptions = engine.subscriptions()?;
        let suggested = engine.suggested_topics(8)?;
        Ok(Self {
            engine: Arc::new(Mutex::new(engine)),
            state: PulseState::new(interests, subscriptions, suggested),
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
        let cards = engine.get_pulse(&self.state.interests(), limit)?;
        self.state.set_digest(cards.clone());
        Ok(json!({
            "attention_budget": ATTENTION_BUDGET,
            "count": cards.len(),
            "digest": cards,
        }))
    }

    pub fn refresh_scores(&self) -> Result<Value> {
        let mut engine = self.engine.lock().expect("pulse engine lock poisoned");
        engine.recompute(Utc::now())?;
        let cards = engine.get_pulse(&self.state.interests(), Some(ATTENTION_BUDGET))?;
        let suggested = engine.suggested_topics(8)?;
        self.state.set_digest(cards.clone());
        self.state.set_suggested_topics(suggested);
        Ok(json!({ "count": cards.len(), "digest": cards }))
    }

    pub fn set_interests(&self, add: &[String], remove: &[String]) -> Result<Value> {
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
        self.state.set_interests(interests.clone());
        let cards = engine.get_pulse(&interests, Some(ATTENTION_BUDGET))?;
        self.state.set_digest(cards.clone());
        Ok(json!({ "interests": interests, "digest": cards }))
    }

    pub fn set_interest(&self, topic: &str, weight: f64) -> Result<Value> {
        let topic = canonical_topic(topic);
        let mut interests = self.state.interests();
        interests.set(&topic, weight);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        engine.set_interest(&topic, weight)?;
        let cards = engine.get_pulse(&interests, Some(ATTENTION_BUDGET))?;
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

    pub fn subscribe_topic(&self, topic: &str) -> Result<Value> {
        let topic = canonical_topic(topic);
        let engine = self.engine.lock().expect("pulse engine lock poisoned");
        let subscriptions = engine.subscribe_topic(&topic, Utc::now())?;
        self.state.set_tracked_topics(subscriptions.clone());
        Ok(json!({ "subscribed": topic, "tracked_topics": subscriptions }))
    }

    pub fn call(&self, name: &str, arguments: &str) -> Result<Value> {
        match name {
            "get_pulse" => {
                let arguments: GetPulseArgs = parse_arguments(arguments)?;
                self.get_pulse(arguments.limit)
            }
            "set_interests" => {
                let arguments: SetInterestsArgs = parse_arguments(arguments)?;
                self.set_interests(&arguments.add, &arguments.remove)
            }
            "explain_trend" => {
                let arguments: TrendArgs = parse_arguments(arguments)?;
                self.explain_trend(&arguments.id)
            }
            "subscribe_topic" => {
                let arguments: SubscribeArgs = parse_arguments(arguments)?;
                self.subscribe_topic(&arguments.topic)
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
                    "description": "Return at most five currently ranked community trends.",
                    "parameters": {
                        "type": "object",
                        "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 5 } },
                        "additionalProperties": false
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "set_interests",
                    "description": "Boost or mute topics and immediately rerank the digest.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "add": { "type": "array", "items": { "type": "string" } },
                            "remove": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["add", "remove"],
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
        ]
    }
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
}

#[derive(Deserialize)]
struct TrendArgs {
    id: String,
}

#[derive(Deserialize)]
struct SubscribeArgs {
    topic: String,
}
