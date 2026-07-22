use chrono::{TimeZone, Utc};
use community_pulse::PulseEngine;
use community_pulse::domain::{DEFAULT_BUDGET, InterestModel, MAX_BUDGET};
use community_pulse::engine::extract_topics;

fn fixture_engine() -> (PulseEngine, chrono::DateTime<Utc>) {
    let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
    let mut engine = PulseEngine::in_memory().unwrap();
    engine.load_fixture(now).unwrap();
    (engine, now)
}

#[test]
fn database_snapshot_restores_posts_and_user_budget() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("live.db");
    let snapshot = directory.path().join("demo-live-snapshot.db");
    let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
    let mut engine = PulseEngine::open(&source).unwrap();
    engine.load_fixture(now).unwrap();
    engine.set_budget(8).unwrap();
    engine.snapshot_to(&snapshot).unwrap();
    drop(engine);

    let restored = PulseEngine::open(&snapshot).unwrap();
    assert_eq!(restored.post_count().unwrap(), 30);
    assert_eq!(restored.budget().unwrap(), 8);
}

#[test]
fn fixture_produces_a_ranked_default_budget_digest() {
    let (engine, now) = fixture_engine();
    let cards = engine
        .get_pulse(&InterestModel::default(), None, now)
        .unwrap();

    assert_eq!(engine.post_count().unwrap(), 30);
    assert_eq!(cards.len(), DEFAULT_BUDGET);
    assert!(cards.windows(2).all(|pair| pair[0].score >= pair[1].score));
    assert!(cards.iter().all(|card| !card.sources.is_empty()));
    assert!(cards.iter().all(|card| !card.headline_url.is_empty()));
    assert!(cards.iter().all(|card| card.sparkline.len() == 12));
    let sparklines = cards
        .iter()
        .map(|card| card.sparkline.clone())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(sparklines.len(), cards.len());
    let headlines = cards
        .iter()
        .map(|card| card.headline.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(headlines.len(), cards.len());
    assert!(cards.iter().any(|card| card.id == "wasm-runtimes"));
}

#[test]
fn explicit_limits_clamp_to_the_engine_ceiling() {
    let (engine, now) = fixture_engine();
    let cards = engine
        .get_pulse(&InterestModel::default(), Some(99), now)
        .unwrap();

    assert!(cards.len() <= MAX_BUDGET);
    assert!(cards.len() > DEFAULT_BUDGET);
}

#[test]
fn attention_budget_persists_and_drives_the_default_digest() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("pulse.db");
    let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
    {
        let mut engine = PulseEngine::open(&database).unwrap();
        engine.load_fixture(now).unwrap();
        assert_eq!(engine.budget().unwrap(), DEFAULT_BUDGET);
        assert_eq!(engine.set_budget(8).unwrap(), 8);
        let stored_default = engine
            .get_pulse(&InterestModel::default(), None, now)
            .unwrap();
        let explicit_eight = engine
            .get_pulse(&InterestModel::default(), Some(8), now)
            .unwrap();
        assert_eq!(stored_default, explicit_eight);
    }

    let engine = PulseEngine::open(&database).unwrap();
    assert_eq!(engine.budget().unwrap(), 8);
    assert_eq!(engine.set_budget(50).unwrap(), MAX_BUDGET);
}

#[test]
fn interests_rerank_and_muted_topics_disappear() {
    let (engine, now) = fixture_engine();
    let mut interests = InterestModel::default();
    interests.set("rust", 2.0);
    interests.set("crypto", -1.0);

    let cards = engine.get_pulse(&interests, Some(5), now).unwrap();
    let rust = cards.iter().find(|card| card.id == "rust").unwrap();

    assert!((rust.interest_affinity - 2.3).abs() < 0.001);
    assert!((rust.score - rust.trend_score * 2.3).abs() < 0.001);
    assert!(cards.iter().all(|card| card.id != "crypto"));
}

#[test]
fn explanation_contains_velocity_baseline_sparkline_and_posts() {
    let (engine, now) = fixture_engine();
    let evidence = engine.explain_trend("wasm-runtimes", now).unwrap();

    assert_eq!(evidence.sparkline.len(), 12);
    assert!(evidence.mentions_6h >= evidence.mentions_1h);
    assert!(evidence.mentions_24h >= evidence.mentions_6h);
    assert!(!evidence.posts.is_empty());
    assert!(evidence.z_score.is_finite());
    assert!(evidence.baseline_mean.is_finite());
}

#[test]
fn topic_extraction_is_deterministic_and_tag_aware() {
    let topics = extract_topics(
        "A tiny runtime written from scratch",
        &["Rust".to_owned(), "WebAssembly".to_owned()],
    );
    assert_eq!(topics, vec!["rust", "wasm-runtimes"]);
}

#[test]
fn fixture_exposes_a_truthful_delta_from_the_previous_snapshot() {
    let (engine, _) = fixture_engine();
    let chips = engine
        .digest_delta_chips(&InterestModel::default())
        .unwrap();

    assert!(chips.iter().any(|chip| chip.contains("Wasm Runtimes")));
    assert!(chips.iter().any(|chip| chip.contains("cooled")));
}
