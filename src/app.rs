use crate::chat::{AgentConfig, ChatEvent, ChatSession};
use crate::domain::{ChatRole, InterestModel};
use crate::engine::PulseEngine;
use crate::reactive::UiSnapshot;
use crate::tools::ToolBridge;
use anyhow::Result;
use slint::{ModelRc, SharedString, VecModel};
use std::fmt::Write as _;
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
        .map(|card| {
            let chart = spark_geometry(&card.sparkline, None);
            DigestRow {
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
                spark_line: chart.line.into(),
                spark_area: chart.area.into(),
                spark_end_x: chart.end_x,
                spark_end_y: chart.end_y,
            }
        })
        .collect();
    window.set_digest(model(digest));

    let topics = topic_rows(&snapshot.interests, &snapshot.suggested_topics);
    let mixer_topics = topics
        .iter()
        .map(|topic| topic.id.to_string())
        .collect::<std::collections::HashSet<_>>();
    window.set_topics(model(topics));
    window.set_suggested_topics(model(
        snapshot
            .suggested_topics
            .iter()
            .filter(|topic| !mixer_topics.contains(*topic))
            .take(3)
            .cloned()
            .map(SharedString::from)
            .collect(),
    ));
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
        let chart = spark_geometry(&evidence.sparkline, Some(evidence.baseline_mean));
        window.set_has_evidence(true);
        window.set_evidence(EvidenceRow {
            topic: evidence.topic.into(),
            velocity: format!(
                "velocity {} / 1h · {} / 6h · {} / 24h",
                evidence.mentions_1h, evidence.mentions_6h, evidence.mentions_24h
            )
            .into(),
            baseline: if evidence.baseline_stddev < 1.0 {
                format!(
                    "baseline μ {:.1} · σ {:.1} · z floors σ at 1.0",
                    evidence.baseline_mean, evidence.baseline_stddev
                )
            } else {
                format!(
                    "baseline μ {:.1} · σ {:.1}",
                    evidence.baseline_mean, evidence.baseline_stddev
                )
            }
            .into(),
            z_score: format!("z = {:+.2}", evidence.z_score).into(),
            spark_line: chart.line.into(),
            spark_area: chart.area.into(),
            spark_end_x: chart.end_x,
            spark_end_y: chart.end_y,
            baseline_y: chart.baseline_y,
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

fn topic_rows(interests: &InterestModel, _suggested: &[String]) -> Vec<TopicRow> {
    let mut topics = ["rust", "local-first", "ai-infra", "wasm-runtimes", "crypto"]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let extra_topics = interests
        .0
        .keys()
        .filter(|topic| !topics.contains(topic))
        .cloned()
        .collect::<Vec<_>>();
    topics.extend(extra_topics);
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

struct SparkGeometry {
    line: String,
    area: String,
    end_x: f32,
    end_y: f32,
    baseline_y: f32,
}

fn spark_geometry(points: &[usize], baseline: Option<f64>) -> SparkGeometry {
    const WIDTH: f64 = 116.0;
    const HEIGHT: f64 = 34.0;
    const PAD: f64 = 3.0;

    let mut low = points.iter().copied().min().unwrap_or_default() as f64;
    let mut high = points.iter().copied().max().unwrap_or(1) as f64;
    if let Some(baseline) = baseline {
        low = low.min(baseline);
        high = high.max(baseline);
    }
    if (high - low).abs() < f64::EPSILON {
        high = low + 1.0;
    }
    let y_for = |value: f64| HEIGHT - PAD - ((value - low) / (high - low)) * (HEIGHT - PAD * 2.0);
    let step = if points.len() > 1 {
        (WIDTH - PAD * 2.0) / (points.len() - 1) as f64
    } else {
        0.0
    };
    let mut line = String::new();
    let mut end_x = PAD;
    let mut end_y = HEIGHT - PAD;
    for (index, value) in points.iter().enumerate() {
        let x = PAD + step * index as f64;
        let y = y_for(*value as f64);
        let _ = write!(
            line,
            "{}{:0.2} {:0.2}",
            if index == 0 { "M" } else { " L" },
            x,
            y
        );
        end_x = x;
        end_y = y;
    }
    if line.is_empty() {
        line.push_str("M3 31 L113 31");
        end_x = WIDTH - PAD;
    }
    let area = format!("{line} L{end_x:.2} 31 L3 31 Z");
    SparkGeometry {
        line,
        area,
        end_x: end_x as f32,
        end_y: end_y as f32,
        baseline_y: baseline.map(y_for).unwrap_or(-1.0) as f32,
    }
}

fn compact_result(value: &serde_json::Value) -> String {
    if value.get("error").and_then(serde_json::Value::as_bool) == Some(true) {
        return format!(
            "tool failed: {}",
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error")
        );
    }
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
