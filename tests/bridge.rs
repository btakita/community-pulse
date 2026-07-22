use chrono::{TimeZone, Utc};
use community_pulse::chat::{ChatEvent, ChatSession};
use community_pulse::domain::{
    Citation, ResearchEnrichment, ResearchQuote, ResearchSection, ResearchSubmission,
};
use community_pulse::{PulseEngine, ToolBridge};

fn fixture_bridge() -> ToolBridge {
    let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
    let mut engine = PulseEngine::in_memory().unwrap();
    engine.load_fixture(now).unwrap();
    ToolBridge::new(engine).unwrap()
}

#[test]
fn ui_and_agent_tools_share_the_same_reactive_state() {
    let bridge = fixture_bridge();
    bridge.get_pulse(Some(5)).unwrap();
    bridge
        .set_interests(&["rust".to_owned()], &["crypto".to_owned()], None)
        .unwrap();
    bridge.explain_trend("rust").unwrap();
    bridge.subscribe_topic("wasm runtimes").unwrap();

    let snapshot = bridge.snapshot();
    assert_eq!(snapshot.digest.len(), 5);
    assert_eq!(snapshot.interests.weight("rust"), 1.0);
    assert_eq!(snapshot.interests.weight("crypto"), -1.0);
    assert!(snapshot.digest.iter().all(|card| card.id != "crypto"));
    assert_eq!(snapshot.evidence.unwrap().id, "rust");
    assert_eq!(snapshot.tracked_topics, vec!["wasm-runtimes"]);
    assert!(snapshot.alert.contains("spiked"));
    assert!(!snapshot.delta_chips.is_empty());
    assert!(snapshot.status.contains("1 interests"));
}

#[test]
fn master_budget_updates_digest_meter_and_shared_state() {
    let bridge = fixture_bridge();
    let result = bridge.set_budget(8).unwrap();
    let snapshot = bridge.snapshot();

    assert_eq!(result["attention_budget"], 8);
    assert_eq!(snapshot.budget, 8);
    assert_eq!(snapshot.digest.len(), 7);
    assert!(snapshot.status.contains("7/8 signals"));
}

#[tokio::test]
async fn replay_chat_calls_the_real_bridge_and_streams_narration() {
    let bridge = fixture_bridge();
    let session = ChatSession::replay(bridge.clone());
    let mut events = Vec::new();

    session
        .respond("more Rust, less crypto", |event| events.push(event))
        .await
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        ChatEvent::ToolCall { name, .. } if name == "set_interests"
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ChatEvent::Delta(delta) if !delta.is_empty()))
    );
    assert_eq!(bridge.snapshot().interests.weight("rust"), 1.0);
    assert_eq!(bridge.snapshot().interests.weight("crypto"), -1.0);
}

#[tokio::test]
async fn replay_chat_can_set_the_user_owned_attention_budget() {
    let bridge = fixture_bridge();
    let session = ChatSession::replay(bridge.clone());

    session
        .respond("give me eight today", |_| {})
        .await
        .unwrap();

    assert_eq!(bridge.snapshot().budget, 8);
    assert_eq!(bridge.snapshot().digest.len(), 7);
}

#[test]
fn direct_fader_weights_snap_to_neutral_and_clamp_to_the_real_domain() {
    let bridge = fixture_bridge();

    bridge.set_interest("rust", 0.04).unwrap();
    assert_eq!(bridge.snapshot().interests.weight("rust"), 0.0);

    bridge.set_interest("rust", 4.0).unwrap();
    assert_eq!(bridge.snapshot().interests.weight("rust"), 2.0);

    bridge.set_interest("rust", -3.0).unwrap();
    assert_eq!(bridge.snapshot().interests.weight("rust"), -1.0);
}

#[test]
fn evidence_selection_uses_the_stable_id_and_can_be_cleared() {
    let bridge = fixture_bridge();

    bridge.explain_trend("rust").unwrap();
    let evidence = bridge.snapshot().evidence.unwrap();
    assert_eq!(evidence.id, "rust");
    assert!(evidence.posts.iter().all(|post| !post.url.is_empty()));

    bridge.clear_evidence();
    assert!(bridge.snapshot().evidence.is_none());
}

#[test]
fn research_reports_round_trip_and_update_shared_state() {
    let bridge = fixture_bridge();
    let markdown = "## What changed\n\n- Runtime adoption accelerated.\n\n`component-model`";
    let citations = vec![Citation {
        url: "https://example.com/runtime".to_owned(),
        note: Some("Primary discussion".to_owned()),
    }];

    let submitted = bridge
        .submit_research(
            "wasm runtimes",
            "claude",
            "Why component runtimes moved",
            markdown,
            &citations,
            None,
        )
        .unwrap();
    let listed = bridge.list_research(Some("wasm-runtimes")).unwrap();

    assert_eq!(submitted["count"], 1);
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["reports"][0]["markdown"], markdown);
    assert_eq!(
        listed["reports"][0]["citations"][0]["url"],
        citations[0].url
    );
    assert_eq!(bridge.snapshot().research.len(), 1);
    assert_eq!(
        bridge.snapshot().research[0].title,
        "Why component runtimes moved"
    );
}

#[test]
fn research_submission_reconciles_agent_display_name_with_running_family() {
    let bridge = fixture_bridge();
    let run_id = bridge.start_research_run("privacy", "claude");

    bridge
        .submit_research(
            "privacy",
            "Claude (Opus 4.8)",
            "Privacy research",
            "## Verdict\n\nOrganic.",
            &[],
            None,
        )
        .unwrap();

    let run = bridge
        .snapshot()
        .research_runs
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    assert_eq!(run.status, "done");
    assert_eq!(run.progress, "Report submitted");
}

#[test]
fn research_reads_are_unbudgeted_and_expose_posts_and_series() {
    let bridge = fixture_bridge();
    bridge.get_pulse(Some(3)).unwrap();

    let topics = bridge.list_topics(Some(24), None).unwrap();
    let posts = bridge.topic_posts("rust", Some(24 * 7), Some(10)).unwrap();
    let series = bridge.get_series("rust", Some(12), Some(1)).unwrap();

    assert!(topics["count"].as_u64().unwrap() > 3);
    assert_eq!(topics["window_hours"], 24);
    assert!(
        topics["topics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|topic| topic["id"] == "rust" && topic["sources"].is_array())
    );
    assert!(posts["count"].as_u64().unwrap() > 0);
    assert!(
        posts["posts"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://")
    );
    assert!(posts["posts"][0]["points"].is_i64());
    assert_eq!(series["counts"].as_array().unwrap().len(), 12);
    assert!(series["baseline_mean"].is_number());
    assert!(series["baseline_stddev"].is_number());
}

#[test]
fn web_reports_are_restricted_to_reviewable_local_or_claude_targets() {
    let bridge = fixture_bridge();

    let error = bridge
        .submit_research(
            "rust",
            "codex",
            "Unsafe target",
            "Finding",
            &[],
            Some("https://example.com/untrusted"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("web_report must be"));

    bridge
        .submit_research(
            "rust",
            "claude",
            "Artifact target",
            "Finding",
            &[],
            Some("https://claude.ai/artifacts/example"),
        )
        .unwrap();
    assert_eq!(
        bridge.snapshot().research[0].web_report.as_deref(),
        Some("https://claude.ai/artifacts/example")
    );
}

#[test]
fn structured_research_round_trips_alerts_and_never_reranks() {
    let bridge = fixture_bridge();
    bridge.get_pulse(None).unwrap();
    bridge.subscribe_topic("rust").unwrap();
    let before = bridge
        .snapshot()
        .digest
        .iter()
        .map(|card| card.id.clone())
        .collect::<Vec<_>>();

    bridge
        .submit_research_enriched(ResearchSubmission {
            topic_id: "rust".to_owned(),
            agent: "claude".to_owned(),
            title: "Authenticity check".to_owned(),
            markdown: "The spike is coordinated.".to_owned(),
            citations: vec![],
            web_report: None,
            article_url: None,
            sections: vec![],
            enrichment: ResearchEnrichment {
                verdict: Some("MANUFACTURED".to_owned()),
                summary: Some("A narrow referral burst is driving the spike.".to_owned()),
                watch: vec!["AI Infra".to_owned(), "wasm runtimes".to_owned()],
            },
        })
        .unwrap();

    let snapshot = bridge.snapshot();
    let after = snapshot
        .digest
        .iter()
        .map(|card| card.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(after, before, "research must annotate without reranking");
    assert_eq!(
        snapshot.research[0].verdict.as_deref(),
        Some("manufactured")
    );
    assert_eq!(snapshot.research[0].watch, ["ai-infra", "wasm-runtimes"]);
    assert!(snapshot.alert.contains("rust"));
    assert!(snapshot.alert.contains("manufactured"));

    let listed = bridge.list_research(Some("rust")).unwrap();
    assert_eq!(listed["reports"][0]["verdict"], "manufactured");
    assert_eq!(listed["reports"][0]["watch"][0], "ai-infra");
}

#[test]
fn article_briefs_match_evidence_round_trip_sections_and_never_rerank() {
    let bridge = fixture_bridge();
    bridge.get_pulse(None).unwrap();
    let before = bridge
        .snapshot()
        .digest
        .iter()
        .map(|card| card.id.clone())
        .collect::<Vec<_>>();
    let posts = bridge.topic_posts("rust", Some(24 * 7), Some(1)).unwrap();
    let article_url = posts["posts"][0]["url"].as_str().unwrap().to_owned();
    let quote_url = format!("{article_url}#discussion");
    let citations = vec![
        Citation {
            url: article_url.clone(),
            note: Some("Article".to_owned()),
        },
        Citation {
            url: quote_url.clone(),
            note: Some("Direct discussion reference".to_owned()),
        },
    ];

    let submitted = bridge
        .submit_research_enriched(ResearchSubmission {
            topic_id: "rust".to_owned(),
            agent: "claude".to_owned(),
            title: "Article brief".to_owned(),
            markdown: "## What it is\n\nA complete markdown fallback.".to_owned(),
            citations,
            web_report: None,
            article_url: Some(article_url.clone()),
            sections: vec![ResearchSection {
                kind: "reaction".to_owned(),
                body: "The thread is constructively skeptical.".to_owned(),
                quotes: vec![ResearchQuote {
                    text: "Show us the benchmark and the failure modes.".to_owned(),
                    url: quote_url,
                    author: Some("community member".to_owned()),
                }],
            }],
            enrichment: ResearchEnrichment {
                verdict: Some("manufactured".to_owned()),
                summary: Some("This summary belongs only to the article brief.".to_owned()),
                watch: vec!["article-only-watch".to_owned()],
            },
        })
        .unwrap();

    assert!(submitted["warning"].is_null());
    assert_eq!(submitted["report"]["article_url"], article_url);
    assert_eq!(submitted["report"]["sections"][0]["kind"], "reaction");
    assert_eq!(
        submitted["report"]["sections"][0]["quotes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        bridge
            .snapshot()
            .digest
            .iter()
            .map(|card| card.id.clone())
            .collect::<Vec<_>>(),
        before,
        "article briefs must not change ranking"
    );

    let unmatched = bridge
        .submit_research_enriched(ResearchSubmission {
            topic_id: "rust".to_owned(),
            agent: "codex".to_owned(),
            title: "Unmatched brief".to_owned(),
            markdown: "Fallback".to_owned(),
            citations: vec![],
            web_report: None,
            article_url: Some("https://example.com/not-an-evidence-post".to_owned()),
            sections: vec![],
            enrichment: ResearchEnrichment::default(),
        })
        .unwrap();
    assert!(
        unmatched["warning"]
            .as_str()
            .unwrap()
            .contains("stored as topic-level research")
    );
    assert!(unmatched["report"]["article_url"].is_null());
}

#[test]
fn article_quote_urls_must_be_exact_citations() {
    let bridge = fixture_bridge();
    let error = bridge
        .submit_research_enriched(ResearchSubmission {
            topic_id: "rust".to_owned(),
            agent: "codex".to_owned(),
            title: "Untraceable quote".to_owned(),
            markdown: "Fallback".to_owned(),
            citations: vec![],
            web_report: None,
            article_url: None,
            sections: vec![ResearchSection {
                kind: "reaction".to_owned(),
                body: "Reaction".to_owned(),
                quotes: vec![ResearchQuote {
                    text: "This quote has no matching citation.".to_owned(),
                    url: "https://example.com/comment/1".to_owned(),
                    author: None,
                }],
            }],
            enrichment: ResearchEnrichment::default(),
        })
        .unwrap_err();
    assert!(error.to_string().contains("must also appear in citations"));
}
