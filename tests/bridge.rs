use chrono::{TimeZone, Utc};
use community_pulse::chat::{ChatEvent, ChatSession};
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
        .set_interests(&["rust".to_owned()], &["crypto".to_owned()])
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
    assert!(snapshot.status.contains("1 interests"));
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
