use chrono::{TimeZone, Utc};
use community_pulse::PulseEngine;
use community_pulse::domain::{ATTENTION_BUDGET, InterestModel};
use community_pulse::engine::extract_topics;

fn fixture_engine() -> (PulseEngine, chrono::DateTime<Utc>) {
    let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
    let mut engine = PulseEngine::in_memory().unwrap();
    engine.load_fixture(now).unwrap();
    (engine, now)
}

#[test]
fn fixture_produces_a_ranked_five_card_digest() {
    let (engine, now) = fixture_engine();
    let cards = engine
        .get_pulse(&InterestModel::default(), Some(99), now)
        .unwrap();

    assert_eq!(engine.post_count().unwrap(), 30);
    assert_eq!(cards.len(), ATTENTION_BUDGET);
    assert!(cards.windows(2).all(|pair| pair[0].score >= pair[1].score));
    assert!(cards.iter().all(|card| !card.sources.is_empty()));
    assert!(cards.iter().all(|card| card.sparkline.len() == 12));
    let headlines = cards
        .iter()
        .map(|card| card.headline.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(headlines.len(), cards.len());
    assert!(cards.iter().any(|card| card.id == "wasm-runtimes"));
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
