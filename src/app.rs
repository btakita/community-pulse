use crate::chat::{AgentConfig, ChatEvent, ChatSession};
use crate::domain::{ChatMessage, ChatRole, InterestModel, ResearchReport, ResearchRun};
use crate::engine::PulseEngine;
use crate::live::{IngestController, LivePolicy, PublicFeed};
use crate::mcp;
use crate::reactive::UiSnapshot;
use crate::research::{self, ResearchAgent};
use crate::setup;
use crate::tools::ToolBridge;
use anyhow::{Context, Result, bail};
use arboard::Clipboard;
use chrono::{Local, Utc};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

slint::include_modules!();

thread_local! {
    static MOBILE_CLOCK: Timer = Timer::default();
    static DESKTOP_INGEST_CLOCK: Timer = Timer::default();
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewMode {
    Desktop,
    Mobile,
    Companion,
}

pub fn run(
    engine: PulseEngine,
    replay: bool,
    view: ViewMode,
    mcp_port: Option<u16>,
    fixture_mode: bool,
    live_policy: Option<LivePolicy>,
    agent_terminals: &[ResearchAgent],
) -> Result<()> {
    let bridge = ToolBridge::new(engine)?;
    let ingest_controller = IngestController::new(bridge.clone(), fixture_mode);
    bridge.get_pulse(None)?;
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
    let desktop_window = matches!(view, ViewMode::Desktop | ViewMode::Companion)
        .then(AppWindow::new)
        .transpose()?;
    let mobile_window = matches!(view, ViewMode::Mobile | ViewMode::Companion)
        .then(MobileWindow::new)
        .transpose()?;
    let targets = UiTargets {
        desktop: desktop_window.as_ref().map(ComponentHandle::as_weak),
        mobile: mobile_window.as_ref().map(ComponentHandle::as_weak),
    };
    if let Some(window) = &desktop_window {
        window.set_mcp_status(
            mcp_port
                .map(|port| format!("mcp ● :{port}"))
                .unwrap_or_else(|| "mcp off".to_owned())
                .into(),
        );
    }
    render_now(&targets, &bridge);
    if let Some(desktop_window) = &desktop_window {
        start_desktop_ingest_clock(desktop_window, &bridge);
        wire_desktop(
            desktop_window,
            &bridge,
            &ingest_controller,
            &session,
            &targets,
            mcp_port,
        );
    }
    if let Some(mobile_window) = &mobile_window {
        start_mobile_clock(mobile_window);
        wire_mobile(mobile_window, &bridge, &session, &targets);
    }
    if let Some(port) = mcp_port {
        start_mcp(bridge.clone(), targets.clone(), port);
    }
    let mut agent_terminal_guards = Vec::new();
    for agent in agent_terminals {
        match setup::spawn_agent_terminal(*agent) {
            Ok(terminal) => agent_terminal_guards.push(terminal),
            Err(error) => eprintln!("agent terminal: {error:#}"),
        }
    }
    if let Some(policy) = live_policy {
        let render_targets = targets.clone();
        ingest_controller.start_live(
            policy,
            Arc::new(move |snapshot| render_later(&render_targets, snapshot)),
        );
    }

    let run_result = match (desktop_window, mobile_window) {
        (Some(desktop), Some(mobile)) => {
            mobile.show()?;
            desktop.run()
        }
        (Some(desktop), None) => desktop.run(),
        (None, Some(mobile)) => mobile.run(),
        (None, None) => unreachable!("every view mode creates at least one window"),
    };
    drop(agent_terminal_guards);
    run_result?;
    Ok(())
}

fn start_mobile_clock(window: &MobileWindow) {
    window.set_sysbar_time(Local::now().format("%H:%M").to_string().into());
    let weak = window.as_weak();
    MOBILE_CLOCK.with(|timer| {
        timer.start(TimerMode::Repeated, Duration::from_secs(30), move || {
            if let Some(window) = weak.upgrade() {
                window.set_sysbar_time(Local::now().format("%H:%M").to_string().into());
            }
        });
    });
}

fn start_desktop_ingest_clock(window: &AppWindow, bridge: &ToolBridge) {
    let weak = window.as_weak();
    let bridge = bridge.clone();
    DESKTOP_INGEST_CLOCK.with(|timer| {
        timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            if let Some(window) = weak.upgrade() {
                let snapshot = bridge.snapshot();
                window.set_ingest_label(ingest_label(&snapshot).into());
                apply_research_run_state(&window, &snapshot);
                window.set_research_progress_phase((window.get_research_progress_phase() + 1) % 5);
            }
        });
    });
}

#[derive(Clone)]
struct UiTargets {
    desktop: Option<slint::Weak<AppWindow>>,
    mobile: Option<slint::Weak<MobileWindow>>,
}

macro_rules! wire_callbacks {
    ($window:expr, $bridge:expr, $session:expr, $targets:expr) => {{
        {
            let bridge = $bridge.clone();
            let targets = $targets.clone();
            $window.on_set_budget(move |budget| {
                if let Err(error) = bridge.set_budget(budget.max(1) as usize) {
                    bridge.state().append_chat(
                        ChatRole::System,
                        format!("Attention budget update failed: {error:#}"),
                        None,
                    );
                }
                render_now(&targets, &bridge);
            });
        }
        {
            let bridge = $bridge.clone();
            let targets = $targets.clone();
            $window.on_set_interest(move |topic, weight| {
                if let Err(error) = bridge.set_interest(topic.as_str(), weight as f64) {
                    bridge.state().append_chat(
                        ChatRole::System,
                        format!("Interest update failed: {error:#}"),
                        None,
                    );
                }
                render_now(&targets, &bridge);
            });
        }
        {
            let bridge = $bridge.clone();
            let targets = $targets.clone();
            $window.on_explain_requested(move |topic| {
                toggle_evidence(&bridge, topic.as_str());
                render_now(&targets, &bridge);
            });
        }
        {
            let bridge = $bridge.clone();
            let targets = $targets.clone();
            $window.on_clear_evidence(move || {
                bridge.clear_evidence();
                render_now(&targets, &bridge);
            });
        }
        {
            let bridge = $bridge.clone();
            let targets = $targets.clone();
            $window.on_subscribe_requested(move |topic| {
                if let Err(error) = bridge.subscribe_topic(topic.as_str()) {
                    bridge.state().append_chat(
                        ChatRole::System,
                        format!("Subscription failed: {error:#}"),
                        None,
                    );
                }
                render_now(&targets, &bridge);
            });
        }
        {
            let bridge = $bridge.clone();
            let targets = $targets.clone();
            $window.on_open_url(move |url| {
                if url.is_empty() {
                    return;
                }
                let target = match resolve_open_target(
                    url.as_str(),
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                ) {
                    Ok(target) => target,
                    Err(error) => {
                        eprintln!("open-url: rejected target {url:?} ({error:#})");
                        return;
                    }
                };
                if let Err(error) = open_target_detached(&target) {
                    bridge.state().append_chat(
                        ChatRole::System,
                        format!("Could not open link: {error}"),
                        None,
                    );
                    render_now(&targets, &bridge);
                }
            });
        }
        {
            let bridge = $bridge.clone();
            let session = Arc::clone($session);
            let targets = $targets.clone();
            $window.on_send_message(move |message| {
                start_chat(
                    targets.clone(),
                    bridge.clone(),
                    Arc::clone(&session),
                    message.to_string(),
                );
            });
        }
    }};
}

fn wire_desktop(
    window: &AppWindow,
    bridge: &ToolBridge,
    ingest_controller: &IngestController,
    session: &Arc<ChatSession>,
    targets: &UiTargets,
    mcp_port: Option<u16>,
) {
    wire_callbacks!(window, bridge, session, targets);
    {
        let controller = ingest_controller.clone();
        let targets = targets.clone();
        window.on_refresh_requested(move || {
            let render_targets = targets.clone();
            controller.trigger(
                Arc::new(PublicFeed),
                Arc::new(move |snapshot| render_later(&render_targets, snapshot)),
            );
        });
    }
    {
        let weak = window.as_weak();
        let bridge = bridge.clone();
        window.on_select_research(move |id| {
            if let Some(window) = weak.upgrade() {
                window.set_research_view_active(true);
                apply_research_selection(&window, &bridge.snapshot().research, i64::from(id));
            }
        });
    }
    {
        let bridge = bridge.clone();
        let targets = targets.clone();
        window.on_copy_text(move |text| {
            if let Err(error) = copy_to_clipboard(text.as_str()) {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Could not copy text: {error}"),
                    None,
                );
                render_now(&targets, &bridge);
            }
        });
    }
    {
        let weak = window.as_weak();
        let bridge = bridge.clone();
        window.on_view_research_requested(move |topic| {
            if let Some(window) = weak.upgrade() {
                let research = bridge.snapshot().research;
                window.set_research_view_active(true);
                if let Some(report) = research
                    .iter()
                    .find(|report| report.topic_id == topic.as_str())
                {
                    apply_research_selection(&window, &research, report.id);
                } else {
                    window.set_selected_research_id(-1);
                }
            }
        });
    }
    {
        let bridge = bridge.clone();
        let targets = targets.clone();
        window.on_research_requested(move |topic, agent| {
            let Some(agent) = ResearchAgent::parse(agent.as_str()) else {
                return;
            };
            if bridge.snapshot().research_runs.iter().any(|run| {
                run.topic_id == topic.as_str()
                    && run.agent.eq_ignore_ascii_case(agent.as_str())
                    && run.status == "running"
            }) {
                return;
            }
            let Some(port) = mcp_port else {
                let id = bridge.start_research_run(topic.as_str(), agent.as_str());
                bridge.fail_research_run(id, "launch the app with --mcp-port to delegate research");
                bridge.state().append_chat(
                    ChatRole::System,
                    "Research delegation needs `app --mcp-port <PORT>`.",
                    None,
                );
                render_now(&targets, &bridge);
                return;
            };
            let render_targets = targets.clone();
            if let Err(error) = research::launch(
                bridge.clone(),
                topic.as_str(),
                agent,
                port,
                None,
                move |snapshot| render_later(&render_targets, snapshot),
            ) {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Research launch failed: {error:#}"),
                    None,
                );
                render_now(&targets, &bridge);
            }
        });
    }
    {
        let bridge = bridge.clone();
        let targets = targets.clone();
        window.on_article_brief_requested(move |topic, article_url, agent| {
            let Some(agent) = ResearchAgent::parse(agent.as_str()) else {
                return;
            };
            if bridge.snapshot().research_runs.iter().any(|run| {
                run.topic_id == topic.as_str()
                    && run.agent.eq_ignore_ascii_case(agent.as_str())
                    && run.status == "running"
            }) {
                return;
            }
            let Some(port) = mcp_port else {
                let id = bridge.start_research_run(topic.as_str(), agent.as_str());
                bridge.fail_research_run(id, "launch the app with MCP enabled to delegate a brief");
                bridge.state().append_chat(
                    ChatRole::System,
                    "Article briefs need the in-app MCP server; relaunch without `--no-mcp`.",
                    None,
                );
                render_now(&targets, &bridge);
                return;
            };
            let render_targets = targets.clone();
            if let Err(error) = research::launch_article_brief(
                bridge.clone(),
                topic.as_str(),
                article_url.as_str(),
                agent,
                port,
                None,
                move |snapshot| render_later(&render_targets, snapshot),
            ) {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Article brief launch failed: {error:#}"),
                    None,
                );
                render_now(&targets, &bridge);
            }
        });
    }
}

fn wire_mobile(
    window: &MobileWindow,
    bridge: &ToolBridge,
    session: &Arc<ChatSession>,
    targets: &UiTargets,
) {
    wire_callbacks!(window, bridge, session, targets);
    {
        let bridge = bridge.clone();
        let targets = targets.clone();
        window.on_refresh_requested(move || {
            bridge.state().set_loading(true);
            render_now(&targets, &bridge);
            let result = bridge.refresh_scores();
            bridge.state().set_loading(false);
            if let Err(error) = result {
                bridge.state().append_chat(
                    ChatRole::System,
                    format!("Refresh failed: {error:#}"),
                    None,
                );
            }
            render_now(&targets, &bridge);
        });
    }
    let weak = window.as_weak();
    window.on_rotate_requested(move || {
        if let Some(window) = weak.upgrade() {
            let landscape = !window.get_landscape();
            window.set_landscape(landscape);
            let size = if landscape {
                slint::LogicalSize::new(872.0, 418.0)
            } else {
                slint::LogicalSize::new(418.0, 872.0)
            };
            window.window().set_size(size);
        }
    });
}

fn toggle_evidence(bridge: &ToolBridge, topic: &str) {
    let selected = bridge
        .snapshot()
        .evidence
        .is_some_and(|evidence| evidence.id == topic);
    if selected {
        bridge.clear_evidence();
    } else if let Err(error) = bridge.explain_trend(topic) {
        bridge.state().append_chat(
            ChatRole::System,
            format!("Evidence lookup failed: {error:#}"),
            None,
        );
    }
}

fn start_mcp(bridge: ToolBridge, targets: UiTargets, port: u16) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build MCP runtime");
        let render_targets = targets.clone();
        let known_report_ids = Arc::new(Mutex::new(
            bridge
                .snapshot()
                .research
                .iter()
                .map(|report| report.id)
                .collect::<HashSet<_>>(),
        ));
        let result = runtime.block_on(mcp::serve(bridge.clone(), port, move |snapshot| {
            let new_report_id = known_report_ids
                .lock()
                .ok()
                .and_then(|mut known| newest_unseen_report(&mut known, &snapshot.research));
            flash_mcp_later(&render_targets, snapshot, new_report_id);
        }));
        if let Err(error) = result {
            eprintln!("mcp: unavailable on 127.0.0.1:{port} ({error:#}); app continuing");
            set_mcp_status_later(&targets, format!("mcp ○ :{port}"));
            render_later(&targets, bridge.snapshot());
        }
    });
}

fn flash_mcp_later(targets: &UiTargets, snapshot: UiSnapshot, new_report_id: Option<i64>) {
    let targets = targets.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = targets.desktop.as_ref().and_then(slint::Weak::upgrade) {
            apply_snapshot(&window, snapshot.clone());
            if let Some(id) = new_report_id {
                window.set_research_view_active(true);
                apply_research_selection(&window, &snapshot.research, id);
            }
            window.set_mcp_active(true);
            let weak = window.as_weak();
            Timer::single_shot(Duration::from_millis(900), move || {
                if let Some(window) = weak.upgrade() {
                    window.set_mcp_active(false);
                }
            });
        }
        if let Some(window) = targets.mobile.as_ref().and_then(slint::Weak::upgrade) {
            apply_mobile_snapshot(&window, snapshot);
        }
    });
}

fn newest_unseen_report(known: &mut HashSet<i64>, reports: &[ResearchReport]) -> Option<i64> {
    reports
        .iter()
        .filter_map(|report| known.insert(report.id).then_some(report.id))
        .max()
}

fn set_mcp_status_later(targets: &UiTargets, status: String) {
    let targets = targets.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = targets.desktop.as_ref().and_then(slint::Weak::upgrade) {
            window.set_mcp_status(status.into());
            window.set_mcp_active(false);
        }
    });
}

fn start_chat(targets: UiTargets, bridge: ToolBridge, session: Arc<ChatSession>, message: String) {
    let message = message.trim().to_owned();
    if message.is_empty() || bridge.snapshot().loading {
        return;
    }
    bridge
        .state()
        .append_chat(ChatRole::User, message.clone(), None);
    let assistant_id = bridge.state().append_chat(ChatRole::Assistant, "", None);
    bridge.state().set_loading(true);
    render_now(&targets, &bridge);

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
            render_later(&targets, bridge.snapshot());
        }));
        if let Err(error) = result {
            bridge
                .state()
                .replace_chat(assistant_id, format!("I couldn't complete that: {error:#}"));
        }
        bridge.state().set_loading(false);
        render_later(&targets, bridge.snapshot());
    });
}

fn render_now(targets: &UiTargets, bridge: &ToolBridge) {
    let snapshot = bridge.snapshot();
    if let Some(window) = targets.desktop.as_ref().and_then(slint::Weak::upgrade) {
        apply_snapshot(&window, snapshot.clone());
    }
    if let Some(window) = targets.mobile.as_ref().and_then(slint::Weak::upgrade) {
        apply_mobile_snapshot(&window, snapshot);
    }
}

fn render_later(targets: &UiTargets, snapshot: UiSnapshot) {
    let targets = targets.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = targets.desktop.as_ref().and_then(slint::Weak::upgrade) {
            apply_snapshot(&window, snapshot.clone());
        }
        if let Some(window) = targets.mobile.as_ref().and_then(slint::Weak::upgrade) {
            apply_mobile_snapshot(&window, snapshot);
        }
    });
}

fn apply_snapshot(window: &AppWindow, snapshot: UiSnapshot) {
    let busy = snapshot.loading || snapshot.ingesting;
    apply_research_run_state(window, &snapshot);
    window.set_ingest_label(ingest_label(&snapshot).into());
    window.set_ingesting(snapshot.ingesting);
    window.set_ingest_enabled(snapshot.ingest_enabled);
    window.set_ingest_message(snapshot.ingest_message.clone().into());
    window.set_source_statuses(model(
        snapshot
            .source_status
            .iter()
            .map(|source| SourceStatusRow {
                name: source.name.clone().into(),
                ok: source.ok,
                count: source.count as i32,
                error: source.error.clone().into(),
            })
            .collect(),
    ));
    let research_counts = snapshot.research.iter().fold(
        std::collections::HashMap::<&str, i32>::new(),
        |mut counts, report| {
            if report.article_url.is_none() {
                *counts.entry(report.topic_id.as_str()).or_default() += 1;
            }
            counts
        },
    );
    let research_annotations = card_research_annotations(&snapshot.research);
    let digest = snapshot
        .digest
        .into_iter()
        .map(|card| {
            let chart = spark_geometry(&card.sparkline, None);
            let research_count = research_counts
                .get(card.id.as_str())
                .copied()
                .unwrap_or_default();
            let annotation = research_annotations.get(card.id.as_str());
            DigestRow {
                id: card.id.into(),
                topic: card.topic.into(),
                headline: card.headline.into(),
                headline_tooltip: source_summary_tooltip(
                    &card.headline_source,
                    &card.headline_summary,
                )
                .into(),
                url: card.headline_url.into(),
                sources: card.sources.join(" + ").into(),
                score: format!("{:.1}", card.score).into(),
                delta: if card.z_score >= 0.0 {
                    format!("z {:+.1} ▲", card.z_score)
                } else {
                    format!("z {:+.1} ▼", card.z_score)
                }
                .into(),
                z_tooltip: z_tooltip(
                    card.z_score,
                    card.mentions_6h,
                    card.baseline_mean,
                    card.baseline_stddev,
                )
                .into(),
                mentions: format!(
                    "{} now · {} / 6h · {} / 24h",
                    card.mentions_1h, card.mentions_6h, card.mentions_24h
                )
                .into(),
                spark_line: chart.line.into(),
                spark_area: chart.area.into(),
                spark_end_x: chart.end_x,
                spark_end_y: chart.end_y,
                research_count,
                research_id: annotation
                    .map(|report| i32::try_from(report.id).unwrap_or(i32::MAX))
                    .unwrap_or(-1),
                research_agent: annotation
                    .map(|report| report.agent.to_uppercase())
                    .unwrap_or_default()
                    .into(),
                research_verdict: annotation
                    .and_then(|report| report.verdict.as_deref())
                    .map(verdict_label)
                    .unwrap_or_default()
                    .into(),
                research_summary: annotation
                    .and_then(|report| report.summary.as_deref())
                    .unwrap_or_default()
                    .into(),
            }
        })
        .collect();
    window.set_digest(model(digest));
    window.set_attention_budget(snapshot.budget as i32);

    let topics = topic_rows(&snapshot.interests);
    let mixer_topics = topics
        .iter()
        .map(|topic| topic.id.to_string())
        .collect::<std::collections::HashSet<_>>();
    window.set_topics(model(topics));
    window.set_suggested_topics(model(suggested_topic_rows(
        &snapshot.suggested_topics,
        &mixer_topics,
        &snapshot.research,
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
    window.set_chat(model(snapshot.chat.into_iter().map(chat_row).collect()));
    window.set_busy(busy);
    window.set_status(snapshot.status.into());
    window.set_research_reports(model(research_summary_rows(&snapshot.research)));
    if window.get_selected_research_id() >= 0 {
        apply_research_selection(
            window,
            &snapshot.research,
            i64::from(window.get_selected_research_id()),
        );
    }

    let evidence_topic = snapshot
        .evidence
        .as_ref()
        .map(|evidence| evidence.id.as_str());
    window.set_evidence_research(model(evidence_research_rows(
        &snapshot.research,
        evidence_topic,
    )));

    if let Some(evidence) = snapshot.evidence {
        let chart = spark_geometry(&evidence.sparkline, Some(evidence.baseline_mean));
        let first_seen = evidence
            .posts
            .iter()
            .map(|post| post.published_at)
            .min()
            .map(|timestamp| timestamp.format("%H:%M").to_string())
            .unwrap_or_else(|| "—".to_owned());
        let source_count = evidence
            .posts
            .iter()
            .map(|post| post.source.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        window.set_evidence_posts(model(
            evidence
                .posts
                .iter()
                .take(3)
                .map(|post| EvidencePostRow {
                    source: post.source.clone().into(),
                    title: post.title.clone().into(),
                    url: post.url.clone().into(),
                    detail: post.published_at.format("%H:%M UTC").to_string().into(),
                    summary: post.summary.clone().into(),
                    summary_tooltip: source_summary_tooltip(&post.source, &post.summary).into(),
                    claude_brief_id: article_brief_id(&snapshot.research, &post.url, "claude"),
                    codex_brief_id: article_brief_id(&snapshot.research, &post.url, "codex"),
                })
                .collect(),
        ));
        window.set_has_evidence(true);
        window.set_evidence(EvidenceRow {
            id: evidence.id.into(),
            topic: evidence.topic.into(),
            mentions: evidence.mentions_6h.to_string().into(),
            baseline_value: format!("{:.1}", evidence.baseline_mean).into(),
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
            z_score: format!("{:+.1}σ", evidence.z_score).into(),
            z_tooltip: z_tooltip(
                evidence.z_score,
                evidence.mentions_6h,
                evidence.baseline_mean,
                evidence.baseline_stddev,
            )
            .into(),
            first_seen: first_seen.into(),
            source_count: source_count.to_string().into(),
            spark_line: chart.line.into(),
            spark_area: chart.area.into(),
            spark_end_x: chart.end_x,
            spark_end_y: chart.end_y,
            baseline_y: chart.baseline_y,
        });
    } else {
        window.set_has_evidence(false);
        window.set_evidence_posts(model(Vec::new()));
    }
}

fn card_research_annotations(
    reports: &[ResearchReport],
) -> std::collections::HashMap<&str, &ResearchReport> {
    let mut newest = std::collections::HashMap::new();
    for report in reports.iter().filter(|report| {
        report.article_url.is_none()
            && report
                .summary
                .as_deref()
                .is_some_and(|summary| !summary.is_empty())
    }) {
        newest
            .entry(report.topic_id.as_str())
            .and_modify(|current: &mut &ResearchReport| {
                if (report.created_at, report.id) > (current.created_at, current.id) {
                    *current = report;
                }
            })
            .or_insert(report);
    }
    newest
}

fn suggested_topic_rows(
    suggested: &[String],
    mixer_topics: &std::collections::HashSet<String>,
    reports: &[ResearchReport],
) -> Vec<SuggestedTopicRow> {
    let mut seen = mixer_topics.clone();
    let mut rows = suggested
        .iter()
        .filter(|topic| seen.insert((*topic).clone()))
        .take(3)
        .map(|topic| SuggestedTopicRow {
            topic: topic.clone().into(),
            agent: "trend".into(),
            from_research: false,
        })
        .collect::<Vec<_>>();
    let mut research_count = 0;
    for report in reports {
        if report.article_url.is_some() {
            continue;
        }
        for topic in &report.watch {
            if research_count >= 2 {
                return rows;
            }
            if seen.insert(topic.clone()) {
                rows.push(SuggestedTopicRow {
                    topic: topic.clone().into(),
                    agent: report.agent.to_lowercase().into(),
                    from_research: true,
                });
                research_count += 1;
            }
        }
    }
    rows
}

fn evidence_research_rows(
    reports: &[ResearchReport],
    topic_id: Option<&str>,
) -> Vec<ResearchTitleRow> {
    let mut agents = std::collections::HashSet::new();
    reports
        .iter()
        .filter(|report| report.article_url.is_none() && Some(report.topic_id.as_str()) == topic_id)
        .filter(|report| agents.insert(report.agent.to_ascii_lowercase()))
        .take(2)
        .map(|report| ResearchTitleRow {
            id: i32::try_from(report.id).unwrap_or(i32::MAX),
            agent: report.agent.to_uppercase().into(),
            title: report.title.clone().into(),
            verdict: report
                .verdict
                .as_deref()
                .map(verdict_label)
                .unwrap_or("— unstructured")
                .into(),
            summary: report.summary.clone().unwrap_or_default().into(),
        })
        .collect()
}

fn verdict_label(verdict: &str) -> &'static str {
    match verdict {
        "organic" => "● organic",
        "manufactured" => "⚠ manufactured",
        "unclear" => "? unclear",
        _ => "? unclear",
    }
}

fn research_summary_rows(reports: &[ResearchReport]) -> Vec<ResearchSummaryRow> {
    let mut reports = reports.iter().collect::<Vec<_>>();
    reports.sort_by(|left, right| {
        left.topic_id
            .cmp(&right.topic_id)
            .then_with(|| right.created_at.cmp(&left.created_at))
    });
    let mut previous_topic = "";
    reports
        .into_iter()
        .map(|report| {
            let group = if report.topic_id == previous_topic {
                String::new()
            } else {
                previous_topic = &report.topic_id;
                report.topic_id.replace('-', " ").to_uppercase()
            };
            ResearchSummaryRow {
                id: i32::try_from(report.id).unwrap_or(i32::MAX),
                group: group.into(),
                agent: report.agent.to_uppercase().into(),
                title: report.title.clone().into(),
                age: relative_age(report.created_at).into(),
                status: report.status.to_uppercase().into(),
            }
        })
        .collect()
}

fn relative_age(created_at: chrono::DateTime<Utc>) -> String {
    let seconds = (Utc::now() - created_at).num_seconds().max(0);
    if seconds < 60 {
        "now".to_owned()
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn research_run_label(runs: &[ResearchRun], topic_id: Option<&str>, agent: &str) -> String {
    let label = if agent.eq_ignore_ascii_case("claude") {
        "Claude"
    } else {
        "Codex"
    };
    let run = latest_research_run(runs, topic_id, agent);
    match run.map(|run| run.status.as_str()) {
        Some("running") => format!("{label}  ·  …"),
        Some("done") => format!("{label}  ·  ✓ {}", research_run_elapsed(run.unwrap())),
        Some("failed") => format!("{label}  ·  ✗ {}", research_run_elapsed(run.unwrap())),
        _ => format!("Research with {label}"),
    }
}

fn latest_research_run<'a>(
    runs: &'a [ResearchRun],
    topic_id: Option<&str>,
    agent: &str,
) -> Option<&'a ResearchRun> {
    topic_id.and_then(|topic_id| {
        runs.iter()
            .rev()
            .find(|run| run.topic_id == topic_id && run.agent.eq_ignore_ascii_case(agent))
    })
}

fn research_run_elapsed(run: &ResearchRun) -> String {
    let finished_at = run.finished_at.unwrap_or_else(Utc::now);
    let seconds = (finished_at - run.started_at).num_seconds().max(0);
    if seconds < 3_600 {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3_600,
            (seconds % 3_600) / 60,
            seconds % 60
        )
    }
}

fn research_run_progress(
    runs: &[ResearchRun],
    topic_id: Option<&str>,
    agent: &str,
) -> (String, bool) {
    let Some(run) = latest_research_run(runs, topic_id, agent) else {
        return (String::new(), false);
    };
    match run.status.as_str() {
        "running" => (
            format!("{} · {}", research_run_elapsed(run), run.progress),
            true,
        ),
        "failed" => (
            format!(
                "Failed · {}",
                truncate_label(&one_line(&run.stderr_tail), 140)
            ),
            false,
        ),
        _ => (String::new(), false),
    }
}

fn apply_research_run_state(window: &AppWindow, snapshot: &UiSnapshot) {
    let topic_id = snapshot
        .evidence
        .as_ref()
        .map(|evidence| evidence.id.as_str());
    let (claude_progress, claude_running) =
        research_run_progress(&snapshot.research_runs, topic_id, "claude");
    let (codex_progress, codex_running) =
        research_run_progress(&snapshot.research_runs, topic_id, "codex");
    window.set_evidence_claude_status(
        research_run_label(&snapshot.research_runs, topic_id, "claude").into(),
    );
    window.set_evidence_codex_status(
        research_run_label(&snapshot.research_runs, topic_id, "codex").into(),
    );
    window.set_evidence_claude_progress(claude_progress.into());
    window.set_evidence_codex_progress(codex_progress.into());
    window.set_evidence_claude_running(claude_running);
    window.set_evidence_codex_running(codex_running);
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_label(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn apply_research_selection(window: &AppWindow, reports: &[ResearchReport], id: i64) {
    let Some(selected) = reports.iter().find(|report| report.id == id) else {
        window.set_selected_research_id(-1);
        window.set_research_comparative(false);
        return;
    };
    let claude = reports.iter().find(|report| {
        report.topic_id == selected.topic_id
            && report.article_url == selected.article_url
            && report.agent.eq_ignore_ascii_case("claude")
    });
    let codex = reports.iter().find(|report| {
        report.topic_id == selected.topic_id
            && report.article_url == selected.article_url
            && report.agent.eq_ignore_ascii_case("codex")
    });
    let (left, right) = match (claude, codex) {
        (Some(claude), Some(codex)) => (claude, Some(codex)),
        _ => (selected, None),
    };

    window.set_selected_research_id(i32::try_from(selected.id).unwrap_or(i32::MAX));
    window.set_research_topic(
        if selected.article_url.is_some() {
            format!(
                "ARTICLE BRIEF · {}",
                selected.topic_id.replace('-', " ").to_uppercase()
            )
        } else {
            selected.topic_id.replace('-', " ").to_uppercase()
        }
        .into(),
    );
    window.set_research_comparative(right.is_some());
    set_left_research(window, left);
    if let Some(right) = right {
        set_right_research(window, right);
    } else {
        clear_right_research(window);
    }
}

fn set_left_research(window: &AppWindow, report: &ResearchReport) {
    window.set_research_left_agent(report.agent.to_uppercase().into());
    window.set_research_left_title(report.title.clone().into());
    window.set_research_left_blocks(model(report_block_rows(&report.markdown)));
    window.set_research_left_raw_markdown(research_copy_text(report).into());
    window.set_research_left_web_report(report.web_report.clone().unwrap_or_default().into());
    window.set_research_left_citations(model(citation_rows(report)));
    window.set_research_left_structured(uses_structured_sections(report));
    window.set_research_left_sections(model(research_section_rows(report)));
}

fn set_right_research(window: &AppWindow, report: &ResearchReport) {
    window.set_research_right_agent(report.agent.to_uppercase().into());
    window.set_research_right_title(report.title.clone().into());
    window.set_research_right_blocks(model(report_block_rows(&report.markdown)));
    window.set_research_right_raw_markdown(research_copy_text(report).into());
    window.set_research_right_web_report(report.web_report.clone().unwrap_or_default().into());
    window.set_research_right_citations(model(citation_rows(report)));
    window.set_research_right_structured(uses_structured_sections(report));
    window.set_research_right_sections(model(research_section_rows(report)));
}

fn clear_right_research(window: &AppWindow) {
    window.set_research_right_agent("".into());
    window.set_research_right_title("".into());
    window.set_research_right_blocks(model(Vec::new()));
    window.set_research_right_raw_markdown("".into());
    window.set_research_right_web_report("".into());
    window.set_research_right_citations(model(Vec::new()));
    window.set_research_right_structured(false);
    window.set_research_right_sections(model(Vec::new()));
}

fn research_section_rows(report: &ResearchReport) -> Vec<ResearchSectionRow> {
    report
        .sections
        .iter()
        .map(|section| ResearchSectionRow {
            kind: section.kind.clone().into(),
            heading: match section.kind.as_str() {
                "what" => "WHAT IT SAYS",
                "substance" => "TECHNICAL SUBSTANCE",
                "reaction" => "COMMUNITY REACTION",
                "credibility" => "CREDIBILITY",
                "watch" => "WHY IT MATTERS / WATCH",
                _ => "SECTION",
            }
            .into(),
            body: styled_markdown(&section.body),
            quotes: model(
                section
                    .quotes
                    .iter()
                    .map(|quote| ResearchQuoteRow {
                        text: quote.text.clone().into(),
                        url: quote.url.clone().into(),
                        author: quote.author.clone().unwrap_or_default().into(),
                    })
                    .collect(),
            ),
        })
        .collect()
}

fn article_brief_id(reports: &[ResearchReport], article_url: &str, agent: &str) -> i32 {
    reports
        .iter()
        .find(|report| {
            report.article_url.as_deref() == Some(article_url)
                && report.agent.eq_ignore_ascii_case(agent)
        })
        .map(|report| i32::try_from(report.id).unwrap_or(i32::MAX))
        .unwrap_or(-1)
}

fn uses_structured_sections(report: &ResearchReport) -> bool {
    !report.sections.is_empty()
}

fn citation_rows(report: &ResearchReport) -> Vec<CitationRow> {
    report
        .citations
        .iter()
        .enumerate()
        .map(|(index, citation)| CitationRow {
            label: format!("SOURCE {}", index + 1).into(),
            url: citation.url.clone().into(),
            note: citation.note.clone().unwrap_or_default().into(),
        })
        .collect()
}

fn markdown_lite(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| {
            let escaped = line
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let heading_depth = escaped.bytes().take_while(|byte| *byte == b'#').count();
            if (1..=6).contains(&heading_depth)
                && escaped.as_bytes().get(heading_depth) == Some(&b' ')
            {
                let heading = &escaped[heading_depth + 1..];
                if heading_depth == 1 {
                    format!("<u>**{heading}**</u>")
                } else {
                    format!("**{heading}**")
                }
            } else if escaped.starts_with("![")
                || escaped.starts_with('|')
                || escaped.starts_with("```")
                || line.starts_with("  - ")
            {
                format!("\\{escaped}")
            } else {
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReportBlock {
    Heading { level: u8, text: String },
    Paragraph(String),
    BulletList(Vec<String>),
    Code(String),
}

fn parse_report_blocks(markdown: &str) -> Vec<ReportBlock> {
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }

        if line.trim_start().starts_with("```") {
            index += 1;
            let mut code = Vec::new();
            while index < lines.len() && !lines[index].trim_start().starts_with("```") {
                code.push(lines[index]);
                index += 1;
            }
            if index < lines.len() {
                index += 1;
            }
            blocks.push(ReportBlock::Code(code.join("\n")));
            continue;
        }

        if let Some((level, text)) = report_heading(line) {
            blocks.push(ReportBlock::Heading {
                level,
                text: text.to_owned(),
            });
            index += 1;
            continue;
        }

        if report_bullet(line).is_some() {
            let mut items = Vec::new();
            while index < lines.len() {
                let Some(item) = report_bullet(lines[index]) else {
                    break;
                };
                items.push(item.to_owned());
                index += 1;
            }
            blocks.push(ReportBlock::BulletList(items));
            continue;
        }

        let mut paragraph = Vec::new();
        while index < lines.len() {
            let candidate = lines[index];
            if candidate.trim().is_empty()
                || candidate.trim_start().starts_with("```")
                || report_heading(candidate).is_some()
                || report_bullet(candidate).is_some()
            {
                break;
            }
            paragraph.push(candidate.trim());
            index += 1;
        }
        blocks.push(ReportBlock::Paragraph(paragraph.join(" ")));
    }

    blocks
}

fn report_heading(line: &str) -> Option<(u8, &str)> {
    let line = line.trim();
    let depth = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&depth) || line.as_bytes().get(depth) != Some(&b' ') {
        return None;
    }
    Some((depth as u8, line[depth + 1..].trim()))
}

fn report_bullet(line: &str) -> Option<&str> {
    let line = line.trim();
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("· "))
        .map(str::trim)
}

fn report_block_rows(markdown: &str) -> Vec<ReportBlockRow> {
    let mut rows = parse_report_blocks(markdown)
        .into_iter()
        .flat_map(|block| match block {
            ReportBlock::Heading { text, .. } => vec![ReportBlockRow {
                kind: "heading".into(),
                content: styled_markdown(&format!("**{text}**")),
                code: "".into(),
                gap_before: 0.0,
            }],
            ReportBlock::Paragraph(text) => vec![ReportBlockRow {
                kind: "paragraph".into(),
                content: styled_markdown(&text),
                code: "".into(),
                gap_before: 0.0,
            }],
            ReportBlock::BulletList(items) => items
                .into_iter()
                .map(|text| ReportBlockRow {
                    kind: "bullet".into(),
                    content: styled_markdown(&text),
                    code: "".into(),
                    gap_before: 0.0,
                })
                .collect(),
            ReportBlock::Code(code) => vec![ReportBlockRow {
                kind: "code".into(),
                content: slint::StyledText::default(),
                code: code.into(),
                gap_before: 0.0,
            }],
        })
        .collect::<Vec<_>>();

    for index in 1..rows.len() {
        let previous = rows[index - 1].kind.to_string();
        let current = rows[index].kind.as_str();
        rows[index].gap_before = if current == "heading" {
            18.0
        } else if previous == "heading" {
            6.0
        } else if current == "bullet" && previous == "bullet" {
            5.0
        } else {
            10.0
        };
    }
    rows
}

fn styled_markdown(markdown: &str) -> slint::StyledText {
    let markdown = markdown_lite(markdown);
    slint::StyledText::from_markdown(&markdown)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(&markdown))
}

fn z_tooltip(z: f64, mentions_6h: usize, baseline_mean: f64, baseline_stddev: f64) -> String {
    let band = if z >= 3.0 {
        format!("rare: {z:.1}σ above its own weekly norm")
    } else if z >= 2.0 {
        "unusual: well above its typical week".to_owned()
    } else if z >= 1.0 {
        "elevated: above its usual range".to_owned()
    } else if z > -1.0 {
        "normal range for this topic".to_owned()
    } else {
        "cooling: below its weekly norm".to_owned()
    };
    let mut lines = vec![
        format!(
            "{mentions_6h} mentions this 6h vs typical {baseline_mean:.1} ± {baseline_stddev:.1}"
        ),
        band,
    ];
    if baseline_stddev < 1.0 {
        lines.push("quiet topic: z uses a σ floor of 1.0".to_owned());
    }
    if baseline_mean < 1.0 {
        lines.push("small baseline — z can overstate; check mentions + sources".to_owned());
    }
    lines.push("z compares a topic to its own history, not to other topics".to_owned());
    lines.join("\n")
}

fn source_summary_tooltip(source: &str, summary: &str) -> String {
    if summary.trim().is_empty() {
        return String::new();
    }
    let excerpt = summary.trim().chars().take(200).collect::<String>();
    format!("author's text · {source}\n{excerpt}")
}

#[derive(Debug, Eq, PartialEq)]
enum OpenTarget {
    Web(String),
    Local(PathBuf),
}

#[derive(Debug, Eq, PartialEq)]
struct ArtifactLink {
    path: String,
    label: &'static str,
}

fn chat_artifact_link(body: &str, tool: Option<&str>) -> Option<ArtifactLink> {
    body.split_whitespace()
        .chain(tool.into_iter().flat_map(str::split_whitespace))
        .filter_map(|token| {
            let token = token.trim_matches(|character: char| {
                matches!(
                    character,
                    '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
                )
            });
            let normalized = token.replace('\\', "/");
            if normalized.starts_with("research/logs/") || normalized.contains("/research/logs/") {
                Some(ArtifactLink {
                    path: token.trim_end_matches('.').to_owned(),
                    label: "open log ↗",
                })
            } else if normalized.starts_with("research/reports/")
                || normalized.contains("/research/reports/")
            {
                Some(ArtifactLink {
                    path: token.trim_end_matches('.').to_owned(),
                    label: "open report ↗",
                })
            } else {
                None
            }
        })
        .next()
}

fn chat_row(message: ChatMessage) -> ChatRow {
    let artifact = chat_artifact_link(&message.body, message.tool.as_deref());
    ChatRow {
        role: match message.role {
            ChatRole::User => "YOU",
            ChatRole::Assistant => "PULSE",
            ChatRole::Tool => "TOOL",
            ChatRole::System => "SYSTEM",
        }
        .into(),
        body: message.body.into(),
        tool: message.tool.unwrap_or_default().into(),
        artifact_path: artifact
            .as_ref()
            .map_or("", |artifact| artifact.path.as_str())
            .into(),
        artifact_label: artifact
            .as_ref()
            .map_or("", |artifact| artifact.label)
            .into(),
    }
}

fn resolve_open_target(input: &str, repo_root: &Path) -> Result<OpenTarget> {
    let input = input.trim();
    if input.starts_with("https://") || input.starts_with("http://") {
        return Ok(OpenTarget::Web(input.to_owned()));
    }
    if input.contains("://") {
        bail!("unsupported URL scheme");
    }

    let path = Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_owned()
    } else {
        repo_root.join(path)
    }
    .canonicalize()
    .with_context(|| format!("canonicalize local open target {input}"))?;
    let allowed_roots = ["research/logs", "research/reports"]
        .into_iter()
        .filter_map(|relative| repo_root.join(relative).canonicalize().ok())
        .collect::<Vec<_>>();
    if !allowed_roots
        .iter()
        .any(|allowed| candidate.starts_with(allowed))
    {
        bail!("local target is outside the research artifact allowlist");
    }
    Ok(OpenTarget::Local(candidate))
}

fn open_target_detached(target: &OpenTarget) -> Result<()> {
    match target {
        OpenTarget::Web(url) => open::that_detached(url).context("open web link"),
        OpenTarget::Local(path) => open::that_detached(path).context("open local artifact"),
    }
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    Clipboard::new()
        .context("connect to clipboard")?
        .set_text(text.to_owned())
        .context("write clipboard text")
}

fn research_copy_text(report: &ResearchReport) -> String {
    report.markdown.clone()
}

fn apply_mobile_snapshot(window: &MobileWindow, snapshot: UiSnapshot) {
    let digest = snapshot
        .digest
        .into_iter()
        .map(|card| {
            let chart = spark_geometry(&card.sparkline, None);
            DigestRow {
                id: card.id.into(),
                topic: card.topic.into(),
                headline: card.headline.into(),
                headline_tooltip: source_summary_tooltip(
                    &card.headline_source,
                    &card.headline_summary,
                )
                .into(),
                url: card.headline_url.into(),
                sources: card.sources.join(" + ").into(),
                score: format!("{:.1}", card.score).into(),
                delta: if card.z_score >= 0.0 {
                    format!("z {:+.1} ▲", card.z_score)
                } else {
                    format!("z {:+.1} ▼", card.z_score)
                }
                .into(),
                z_tooltip: z_tooltip(
                    card.z_score,
                    card.mentions_6h,
                    card.baseline_mean,
                    card.baseline_stddev,
                )
                .into(),
                mentions: format!(
                    "{} now · {} / 6h · {} / 24h",
                    card.mentions_1h, card.mentions_6h, card.mentions_24h
                )
                .into(),
                spark_line: chart.line.into(),
                spark_area: chart.area.into(),
                spark_end_x: chart.end_x,
                spark_end_y: chart.end_y,
                research_count: 0,
                research_id: -1,
                research_agent: "".into(),
                research_verdict: "".into(),
                research_summary: "".into(),
            }
        })
        .collect();
    window.set_digest(model(digest));
    window.set_attention_budget(snapshot.budget as i32);

    let topics = topic_rows(&snapshot.interests);
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
    window.set_suggested_prompts(model(vec![
        "What's moving?".into(),
        "More Rust".into(),
        "Why?".into(),
    ]));
    window.set_delta_chips(model(
        snapshot
            .delta_chips
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect(),
    ));
    window.set_alert(snapshot.alert.into());
    window.set_chat(model(snapshot.chat.into_iter().map(chat_row).collect()));
    window.set_busy(snapshot.loading || snapshot.ingesting);
    window.set_status(snapshot.status.into());

    if let Some(evidence) = snapshot.evidence {
        let chart = spark_geometry(&evidence.sparkline, Some(evidence.baseline_mean));
        let first_seen = evidence
            .posts
            .iter()
            .map(|post| post.published_at)
            .min()
            .map(|timestamp| timestamp.format("%H:%M").to_string())
            .unwrap_or_else(|| "—".to_owned());
        let source_count = evidence
            .posts
            .iter()
            .map(|post| post.source.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        window.set_evidence_posts(model(
            evidence
                .posts
                .iter()
                .take(3)
                .map(|post| EvidencePostRow {
                    source: post.source.clone().into(),
                    title: post.title.clone().into(),
                    url: post.url.clone().into(),
                    detail: post.published_at.format("%H:%M UTC").to_string().into(),
                    summary: post.summary.clone().into(),
                    summary_tooltip: source_summary_tooltip(&post.source, &post.summary).into(),
                    claude_brief_id: -1,
                    codex_brief_id: -1,
                })
                .collect(),
        ));
        window.set_has_evidence(true);
        window.set_evidence(EvidenceRow {
            id: evidence.id.into(),
            topic: evidence.topic.into(),
            mentions: evidence.mentions_6h.to_string().into(),
            baseline_value: format!("{:.1}", evidence.baseline_mean).into(),
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
            z_score: format!("{:+.1}σ", evidence.z_score).into(),
            z_tooltip: z_tooltip(
                evidence.z_score,
                evidence.mentions_6h,
                evidence.baseline_mean,
                evidence.baseline_stddev,
            )
            .into(),
            first_seen: first_seen.into(),
            source_count: source_count.to_string().into(),
            spark_line: chart.line.into(),
            spark_area: chart.area.into(),
            spark_end_x: chart.end_x,
            spark_end_y: chart.end_y,
            baseline_y: chart.baseline_y,
        });
    } else {
        window.set_has_evidence(false);
        window.set_evidence_posts(model(Vec::new()));
    }
}

fn ingest_label(snapshot: &UiSnapshot) -> String {
    if snapshot.ingesting {
        return "now".to_owned();
    }
    snapshot
        .last_ingest_at
        .map(|timestamp| {
            let seconds = Utc::now()
                .signed_duration_since(timestamp)
                .num_seconds()
                .max(0);
            if seconds < 60 {
                format!("{seconds}s ago")
            } else {
                format!("{}m ago", seconds / 60)
            }
        })
        .unwrap_or_else(|| "ready".to_owned())
}

fn topic_rows(interests: &InterestModel) -> Vec<TopicRow> {
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
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return format!("tool failed: {error}");
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

#[cfg(test)]
mod tests {
    use super::{
        OpenTarget, ReportBlock, article_brief_id, card_research_annotations, chat_artifact_link,
        compact_result, evidence_research_rows, markdown_lite, newest_unseen_report,
        parse_report_blocks, research_copy_text, research_run_label, research_run_progress,
        resolve_open_target, source_summary_tooltip, suggested_topic_rows,
        uses_structured_sections, z_tooltip,
    };
    use crate::domain::{ResearchReport, ResearchRun, ResearchSection};
    use chrono::{Duration, Utc};
    use serde_json::json;
    use std::collections::HashSet;
    use std::fs;

    fn report(
        id: i64,
        topic: &str,
        agent: &str,
        verdict: &str,
        summary: &str,
        watch: &[&str],
        age_minutes: i64,
    ) -> ResearchReport {
        ResearchReport {
            id,
            topic_id: topic.to_owned(),
            agent: agent.to_owned(),
            title: format!("{agent} report"),
            markdown: "Finding".to_owned(),
            citations: vec![],
            web_report: None,
            article_url: None,
            sections: vec![],
            verdict: Some(verdict.to_owned()),
            summary: Some(summary.to_owned()),
            watch: watch.iter().map(|topic| (*topic).to_owned()).collect(),
            created_at: Utc::now() - Duration::minutes(age_minutes),
            status: "submitted".to_owned(),
        }
    }

    #[test]
    fn compact_tool_errors_keep_the_failure_visible() {
        assert_eq!(
            compact_result(&json!({ "error": "unknown trend: nope" })),
            "tool failed: unknown trend: nope"
        );
    }

    #[test]
    fn markdown_lite_keeps_supported_inline_markup_and_normalizes_headings() {
        let rendered = markdown_lite(
            "# Finding\n## Detail\n### The spike\n- **organic** spike with `code` and [source](https://example.com)\n![ignored](x.png)",
        );

        assert!(rendered.contains("<u>**Finding**</u>"));
        assert!(rendered.contains("**Detail**"));
        assert!(rendered.contains("**The spike**"));
        assert!(!rendered.contains("###"));
        assert!(rendered.contains("- **organic** spike with `code`"));
        assert!(rendered.contains("[source](https://example.com)"));
        assert!(rendered.contains("\\![ignored](x.png)"));
        slint::StyledText::from_markdown(&rendered).unwrap();
    }

    #[test]
    fn report_markdown_parses_into_distinct_readable_blocks() {
        let blocks = parse_report_blocks(
            "# Finding\n\nFirst line\ncontinues here.\n\nSecond paragraph with `code`.\n\n- one\n· two\n\n```rust\nlet x = 1;\n```",
        );

        assert_eq!(
            blocks,
            vec![
                ReportBlock::Heading {
                    level: 1,
                    text: "Finding".to_owned(),
                },
                ReportBlock::Paragraph("First line continues here.".to_owned()),
                ReportBlock::Paragraph("Second paragraph with `code`.".to_owned()),
                ReportBlock::BulletList(vec!["one".to_owned(), "two".to_owned()]),
                ReportBlock::Code("let x = 1;".to_owned()),
            ]
        );
    }

    #[test]
    fn z_tooltip_selects_every_interpretation_band() {
        let cases = [
            (3.0, "rare:"),
            (2.0, "unusual:"),
            (1.0, "elevated:"),
            (0.0, "normal range"),
            (-0.9, "normal range"),
            (-1.0, "cooling:"),
        ];
        for (z, expected) in cases {
            assert!(z_tooltip(z, 12, 4.0, 2.0).contains(expected));
        }
    }

    #[test]
    fn z_tooltip_includes_only_applicable_caveats() {
        let typical = z_tooltip(1.5, 12, 4.0, 2.0);
        assert!(typical.starts_with("12 mentions this 6h vs typical 4.0 ± 2.0"));
        assert!(!typical.contains("σ floor"));
        assert!(!typical.contains("small baseline"));
        assert!(typical.ends_with("z compares a topic to its own history, not to other topics"));

        let quiet = z_tooltip(1.5, 12, 4.0, 0.5);
        assert!(quiet.contains("quiet topic: z uses a σ floor of 1.0"));
        assert!(!quiet.contains("small baseline"));

        let small = z_tooltip(1.5, 12, 0.5, 2.0);
        assert!(!small.contains("σ floor"));
        assert!(small.contains("small baseline — z can overstate; check mentions + sources"));
    }

    #[test]
    fn source_summary_tooltip_labels_and_bounds_author_text() {
        assert_eq!(source_summary_tooltip("Hacker News", "   "), "");
        assert_eq!(
            source_summary_tooltip("Lobsters", "Written by the submitter."),
            "author's text · Lobsters\nWritten by the submitter."
        );

        let long = "x".repeat(240);
        let tooltip = source_summary_tooltip("Product Hunt", &long);
        let excerpt = tooltip.split_once('\n').unwrap().1;
        assert_eq!(excerpt.chars().count(), 200);
    }

    #[test]
    fn selectable_log_path_resolves_only_inside_the_artifact_allowlist() {
        let directory = tempfile::tempdir().unwrap();
        let logs = directory.path().join("research/logs");
        let reports = directory.path().join("research/reports");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&reports).unwrap();
        let log = logs.join("run.log");
        fs::write(&log, "submitted").unwrap();

        let link = chat_artifact_link(
            "CLAUDE finished · log research/logs/run.log",
            Some("research result"),
        )
        .unwrap();
        assert_eq!(link.path, "research/logs/run.log");
        assert_eq!(link.label, "open log ↗");
        assert_eq!(
            resolve_open_target(&link.path, directory.path()).unwrap(),
            OpenTarget::Local(log.canonicalize().unwrap())
        );

        let outside = directory.path().join("secrets.txt");
        fs::write(&outside, "never open this").unwrap();
        assert!(resolve_open_target("secrets.txt", directory.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn artifact_allowlist_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let logs = directory.path().join("research/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(directory.path().join("research/reports")).unwrap();
        let outside = directory.path().join("outside.log");
        fs::write(&outside, "private").unwrap();
        symlink(&outside, logs.join("escape.log")).unwrap();

        assert!(resolve_open_target("research/logs/escape.log", directory.path()).is_err());
    }

    #[test]
    fn report_copy_text_round_trips_exact_markdown() {
        let mut report = report(1, "rust", "claude", "organic", "Diverse", &[], 1);
        report.markdown = "# Finding\n\n- exact `markdown`\n".to_owned();

        assert_eq!(research_copy_text(&report), report.markdown);
    }

    #[test]
    fn report_path_resolves_even_when_the_logs_directory_is_absent() {
        let directory = tempfile::tempdir().unwrap();
        let reports = directory.path().join("research/reports");
        fs::create_dir_all(&reports).unwrap();
        let report = reports.join("pulse.html");
        fs::write(&report, "report").unwrap();

        assert_eq!(
            resolve_open_target("research/reports/pulse.html", directory.path()).unwrap(),
            OpenTarget::Local(report.canonicalize().unwrap())
        );
    }

    #[test]
    fn card_annotation_uses_the_newest_structured_report() {
        let reports = vec![
            report(1, "rust", "claude", "unclear", "Older", &[], 20),
            report(2, "rust", "codex", "organic", "Newest", &[], 1),
        ];
        let annotations = card_research_annotations(&reports);
        assert_eq!(annotations["rust"].id, 2);
        assert_eq!(annotations["rust"].summary.as_deref(), Some("Newest"));
    }

    #[test]
    fn evidence_keeps_both_agents_verdicts_side_by_side() {
        let reports = vec![
            report(1, "rust", "claude", "organic", "Diverse", &[], 1),
            report(2, "rust", "codex", "manufactured", "Narrow", &[], 2),
        ];
        let rows = evidence_research_rows(&reports, Some("rust"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].verdict.as_str(), "● organic");
        assert_eq!(rows[1].verdict.as_str(), "⚠ manufactured");
    }

    #[test]
    fn research_watch_suggestions_are_provenanced_and_capped_at_two() {
        let reports = vec![
            report(
                1,
                "rust",
                "claude",
                "organic",
                "Diverse",
                &["watch-a", "watch-b", "watch-c"],
                1,
            ),
            report(2, "rust", "codex", "unclear", "Mixed", &["watch-d"], 2),
        ];
        let suggested = vec![
            "trend-a".to_owned(),
            "trend-b".to_owned(),
            "trend-c".to_owned(),
        ];
        let rows = suggested_topic_rows(&suggested, &HashSet::new(), &reports);
        assert_eq!(rows.len(), 5);
        assert!(rows[..3].iter().all(|row| !row.from_research));
        assert_eq!(rows.iter().filter(|row| row.from_research).count(), 2);
        assert_eq!(rows[3].agent.as_str(), "claude");
    }

    #[test]
    fn article_briefs_are_badged_but_do_not_enrich_topic_cards() {
        let mut brief = report(
            7,
            "rust",
            "claude",
            "manufactured",
            "Article-only summary",
            &["article-only-watch"],
            1,
        );
        brief.article_url = Some("https://example.com/article".to_owned());
        brief.sections = vec![ResearchSection {
            kind: "what".to_owned(),
            body: "A native section".to_owned(),
            quotes: vec![],
        }];

        assert_eq!(
            article_brief_id(&[brief.clone()], "https://example.com/article", "claude"),
            7
        );
        assert!(uses_structured_sections(&brief));
        assert!(card_research_annotations(&[brief.clone()]).is_empty());
        assert!(evidence_research_rows(&[brief.clone()], Some("rust")).is_empty());
        assert!(suggested_topic_rows(&[], &HashSet::new(), &[brief.clone()]).is_empty());

        brief.sections.clear();
        assert!(!uses_structured_sections(&brief));
    }

    #[test]
    fn mcp_submission_focuses_only_a_new_report() {
        let reports = vec![
            report(2, "rust", "codex", "organic", "Newest", &[], 1),
            report(1, "rust", "claude", "unclear", "Existing", &[], 2),
        ];
        let mut known = HashSet::from([1]);
        assert_eq!(newest_unseen_report(&mut known, &reports), Some(2));
        assert_eq!(newest_unseen_report(&mut known, &reports), None);
    }

    #[test]
    fn research_progress_exposes_activity_failure_and_final_duration() {
        let started_at = Utc::now() - Duration::seconds(42);
        let mut run = ResearchRun {
            id: 1,
            topic_id: "rust".to_owned(),
            agent: "claude".to_owned(),
            status: "running".to_owned(),
            started_at,
            finished_at: None,
            progress: "Reading source posts".to_owned(),
            stderr_tail: String::new(),
        };

        let (progress, running) = research_run_progress(&[run.clone()], Some("rust"), "claude");
        assert!(running);
        assert!(progress.ends_with("Reading source posts"));

        run.status = "failed".to_owned();
        run.finished_at = Some(started_at + Duration::seconds(47));
        run.stderr_tail = "permission denied for pulse MCP".to_owned();
        let (progress, running) = research_run_progress(&[run.clone()], Some("rust"), "claude");
        assert!(!running);
        assert_eq!(progress, "Failed · permission denied for pulse MCP");
        assert_eq!(
            research_run_label(&[run], Some("rust"), "claude"),
            "Claude  ·  ✗ 00:47"
        );
    }
}
