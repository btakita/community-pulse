use chrono::{TimeZone, Utc};
use community_pulse::mcp::{PROTOCOL_VERSION, handle};
use community_pulse::{PulseEngine, ToolBridge};
use serde_json::{Value, json};

fn fixture_bridge() -> ToolBridge {
    let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
    let mut engine = PulseEngine::in_memory().unwrap();
    engine.load_fixture(now).unwrap();
    ToolBridge::new(engine).unwrap()
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

#[test]
fn initialize_advertises_the_supported_protocol_and_tools() {
    let bridge = fixture_bridge();
    let response = handle(&bridge, &request(1, "initialize", json!({}))).unwrap();

    assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(response["result"]["capabilities"]["tools"].is_object());
}

#[test]
fn tools_list_contains_exactly_the_shared_bridge_surface() {
    let bridge = fixture_bridge();
    let response = handle(&bridge, &request(2, "tools/list", json!({}))).unwrap();
    let names = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        names,
        [
            "explain_trend",
            "get_series",
            "get_pulse",
            "list_research",
            "list_topics",
            "set_interests",
            "submit_research",
            "subscribe_topic",
            "topic_posts"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(names.len(), 9);
}

#[test]
fn tools_call_mutates_the_same_reactive_state() {
    let bridge = fixture_bridge();
    let response = handle(
        &bridge,
        &request(
            3,
            "tools/call",
            json!({ "name": "get_pulse", "arguments": { "limit": 3 } }),
        ),
    )
    .unwrap();
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let result: Value = serde_json::from_str(text).unwrap();
    let count = result["count"].as_u64().unwrap() as usize;

    assert!(count <= 5);
    assert_eq!(bridge.snapshot().digest.len(), count);
}

#[test]
fn set_interests_clamps_attention_budget_without_an_mcp_error() {
    let bridge = fixture_bridge();
    let response = handle(
        &bridge,
        &request(
            5,
            "tools/call",
            json!({
                "name": "set_interests",
                "arguments": { "attention_budget": 50 }
            }),
        ),
    )
    .unwrap();
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let result: Value = serde_json::from_str(text).unwrap();

    assert_eq!(response["result"]["isError"], false);
    assert_eq!(result["attention_budget"], 10);
    assert_eq!(bridge.snapshot().budget, 10);
    assert_eq!(bridge.snapshot().digest.len(), 7);
    assert!(bridge.snapshot().status.contains("7/10 signals"));
}

#[test]
fn tool_failures_use_mcp_is_error_instead_of_json_rpc_errors() {
    let bridge = fixture_bridge();
    let response = handle(
        &bridge,
        &request(
            4,
            "tools/call",
            json!({ "name": "not_a_tool", "arguments": {} }),
        ),
    )
    .unwrap();

    assert_eq!(response["result"]["isError"], true);
    assert!(response.get("error").is_none());
}

#[test]
fn initialized_notification_has_no_json_rpc_response() {
    let bridge = fixture_bridge();
    assert!(
        handle(
            &bridge,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .is_none()
    );
}
