use crate::chat::{AgentConfig, ChatEvent, ChatSession};
use crate::domain::{ChatRole, InterestModel};
use crate::engine::PulseEngine;
use crate::reactive::UiSnapshot;
use crate::tools::ToolBridge;
use anyhow::Result;
use slint::{ModelRc, SharedString, VecModel};
use std::collections::BTreeSet;
use std::sync::Arc;

slint::include_modules!();

pub fn run(engine: PulseEngine, replay: bool) -> Result<()> {
    let bridge = ToolBridge::new(engine)?;
    bridge.get_pulse(Some(5))?;
    let session = if replay {
        ChatSession::replay(bridge.clone())
    } else {
        match AgentConfig::from_env() {
            Ok(config) => ChatSession::live(config, bridge.clone())?,
            Err(error) => {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Live chat unavailable ({error:#}); using deterministic replay."),
                    None,
                );
                ChatSession::replay(bridge.clone())
            }
        }
    };
    let session = Arc::new(session);
    let window = AppWindow::new()?;
    apply_snapshot(&window, bridge.snapshot());

    {
        let bridge = bridge.clone();
        let weak = window.as_weak();
        window.on_refresh_requested(move || {
            bridge.state().set_loading(true);
            render_now(&weak, &bridge);
            let result = bridge.refresh_scores();
            bridge.state().set_loading(false);
            if let Err(error) = result {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Refresh failed: {error:#}"),
                    None,
                );
            }
            render_now(&weak, &bridge);
        });
    }
    {
        let bridge = bridge.clone();
        let weak = window.as_weak();
        window.on_set_interest(move |topic, weight| {
            if let Err(error) = bridge.set_interest(topic.as_str(), weight as f64) {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Interest update failed: {error:#}"),
                    None,
                );
            }
            render_now(&weak, &bridge);
        });
    }
    {
        let bridge = bridge.clone();
        let weak = window.as_weak();
        window.on_explain_requested(move |topic| {
            if let Err(error) = bridge.explain_trend(topic.as_str()) {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Evidence lookup failed: {error:#}"),
                    None,
                );
            }
            render_now(&weak, &bridge);
        });
    }
    {
        let bridge = bridge.clone();
        let weak = window.as_weak();
        window.on_subscribe_requested(move |topic| {
            if let Err(error) = bridge.subscribe_topic(topic.as_str()) {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Subscription failed: {error:#}"),
                    None,
                );
            }
            render_now(&weak, &bridge);
        });
    }
    {
        let bridge = bridge.clone();
        let session = Arc::clone(&session);
        let weak = window.as_weak();
        window.on_send_message(move |message| {
            start_chat(
                weak.clone(),
                bridge.clone(),
                Arc::clone(&session),
                message.to_string(),
            );
        });
    }

    window.run()?;
    Ok(())
}

fn start_chat(
    weak: slint::Weak<AppWindow>,
    bridge: ToolBridge,
    session: Arc<ChatSession>,
    message: String,
) {
    let message = message.trim().to_owned();
    if message.is_empty() || bridge.snapshot().loading {
        return;
    }
    bridge
        .state()
        .append_chat(ChatRole::User, message.clone(), None);
    let assistant_id = bridge.state().append_chat(ChatRole::Assistant, "", None);
    bridge.state().set_loading(true);
    render_now(&weak, &bridge);

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build chat runtime");
        let result = runtime.block_on(session.respond(&message, |event| {
            match event {
                ChatEvent::Delta(delta) => bridge.state().append_to_chat(assistant_id, &delta),
                ChatEvent::ToolCall { name, result } => {
                    bridge
                        .state()
                        .append_chat(ChatRole::Tool, compact_result(&result), Some(name));
                }
            }
            render_later(&weak, bridge.snapshot());
        }));
        if let Err(error) = result {
            bridge
                .state()
                .replace_chat(assistant_id, format!("I couldn't complete that: {error:#}"));
        }
        bridge.state().set_loading(false);
        render_later(&weak, bridge.snapshot());
    });
}

fn render_now(weak: &slint::Weak<AppWindow>, bridge: &ToolBridge) {
    if let Some(window) = weak.upgrade() {
        apply_snapshot(&window, bridge.snapshot());
    }
}

fn render_later(weak: &slint::Weak<AppWindow>, snapshot: UiSnapshot) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = weak.upgrade() {
            apply_snapshot(&window, snapshot);
        }
    });
}

fn apply_snapshot(window: &AppWindow, snapshot: UiSnapshot) {
    let digest = snapshot
        .digest
        .into_iter()
        .map(|card| DigestRow {
            id: card.id.into(),
            topic: card.topic.into(),
            headline: card.headline.into(),
            sources: card.sources.join(" + ").into(),
            score: format!("{:.1}", card.score).into(),
            delta: format!("{:+.1}σ", card.z_score).into(),
            mentions: format!(
                "{} now · {} / 6h · {} / 24h",
                card.mentions_1h, card.mentions_6h, card.mentions_24h
            )
            .into(),
        })
        .collect();
    window.set_digest(model(digest));

    window.set_topics(model(topic_rows(
        &snapshot.interests,
        &snapshot.suggested_topics,
    )));
    window.set_tracked_topics(model(
        snapshot
            .tracked_topics
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect(),
    ));
    window.set_tracked_summary(if snapshot.tracked_topics.is_empty() {
        "Nothing tracked yet".into()
    } else {
        snapshot.tracked_topics.join("  ·  ").into()
    });
    window.set_suggested_prompts(model(vec![
        "What's moving?".into(),
        "More Rust".into(),
        "Why?".into(),
    ]));
    window.set_chat(model(
        snapshot
            .chat
            .into_iter()
            .map(|message| ChatRow {
                role: match message.role {
                    ChatRole::User => "YOU",
                    ChatRole::Assistant => "PULSE",
                    ChatRole::Tool => "TOOL",
                    ChatRole::System => "SYSTEM",
                }
                .into(),
                body: message.body.into(),
                tool: message.tool.unwrap_or_default().into(),
            })
            .collect(),
    ));
    window.set_busy(snapshot.loading);
    window.set_status(snapshot.status.into());

    if let Some(evidence) = snapshot.evidence {
        window.set_has_evidence(true);
        window.set_evidence(EvidenceRow {
            topic: evidence.topic.into(),
            velocity: format!(
                "velocity {} / 1h · {} / 6h · {} / 24h",
                evidence.mentions_1h, evidence.mentions_6h, evidence.mentions_24h
            )
            .into(),
            baseline: format!(
                "baseline μ {:.1} · σ {:.1}",
                evidence.baseline_mean, evidence.baseline_stddev
            )
            .into(),
            z_score: format!("z = {:+.2}", evidence.z_score).into(),
            sparkline: sparkline(&evidence.sparkline).into(),
            posts: evidence
                .posts
                .iter()
                .map(|post| format!("{}: {}", post.source, post.title))
                .collect::<Vec<_>>()
                .join("  ·  ")
                .into(),
        });
    } else {
        window.set_has_evidence(false);
    }
}

fn topic_rows(interests: &InterestModel, suggested: &[String]) -> Vec<TopicRow> {
    let mut topics = suggested.iter().cloned().collect::<BTreeSet<_>>();
    topics.extend(interests.0.keys().cloned());
    topics.extend(
        ["rust", "wasm-runtimes", "local-first", "ai-infra", "crypto"]
            .into_iter()
            .map(str::to_owned),
    );
    topics
        .into_iter()
        .map(|topic| {
            let weight = interests.weight(&topic);
            TopicRow {
                id: topic.clone().into(),
                topic: display_topic(&topic).into(),
                weight: if weight > 0.0 {
                    format!("+{weight:.1}")
                } else {
                    format!("{weight:.1}")
                }
                .into(),
                state: if weight < 0.0 {
                    "muted"
                } else if weight > 0.0 {
                    "boosted"
                } else {
                    "neutral"
                }
                .into(),
                weight_value: weight as f32,
                active: weight > 0.0,
                muted: weight < 0.0,
            }
        })
        .collect()
}

fn display_topic(topic: &str) -> String {
    topic
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sparkline(points: &[usize]) -> String {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = points.iter().copied().max().unwrap_or(1).max(1);
    points
        .iter()
        .map(|point| LEVELS[point * (LEVELS.len() - 1) / max])
        .collect()
}

fn compact_result(value: &serde_json::Value) -> String {
    if let Some(count) = value.get("count").and_then(serde_json::Value::as_u64) {
        return format!("updated {count} digest cards");
    }
    if let Some(topic) = value.get("subscribed").and_then(serde_json::Value::as_str) {
        return format!("tracking {topic}");
    }
    if let Some(topic) = value.get("topic").and_then(serde_json::Value::as_str) {
        return format!("updated {topic}");
    }
    "state synchronized".to_owned()
}

fn model<T: Clone + 'static>(values: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(values))
}
