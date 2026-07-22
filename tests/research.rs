#![cfg(unix)]

use chrono::{TimeZone, Utc};
use community_pulse::mcp;
use community_pulse::research::{self, ResearchAgent};
use community_pulse::{PulseEngine, ToolBridge};
use std::fs;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn fixture_bridge() -> ToolBridge {
    let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
    let mut engine = PulseEngine::in_memory().unwrap();
    engine.load_fixture(now).unwrap();
    ToolBridge::new(engine).unwrap()
}

fn start_mcp(bridge: ToolBridge) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(mcp::serve(bridge, port, |_| {})).unwrap();
    });
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
            return port;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("test MCP endpoint did not start");
}

fn write_fake_harness(path: &Path, port: u16) {
    let script = format!(
        r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "fake research harness 1.0"
  exit 0
fi
if [ "$1" = "mcp" ]; then
  echo "pulse http://127.0.0.1:{port}/mcp"
  exit 0
fi
sleep 0.2
echo "fake harness stdout"
echo "fake harness stderr" >&2
curl -fsS -X POST http://127.0.0.1:{port}/mcp \
  -H 'content-type: application/json' \
  -d '{{"jsonrpc":"2.0","id":31,"method":"tools/call","params":{{"name":"submit_research","arguments":{{"topic_id":"rust","agent":"codex","title":"Fake CLI finding","markdown":"## Verified\n\nThe fake harness completed the full loop.","citations":[]}}}}}}'
"###
    );
    fs::write(path, script).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn doctor_and_fake_cli_complete_the_research_loop_without_accounts() {
    let bridge = fixture_bridge();
    let port = start_mcp(bridge.clone());
    let directory = tempfile::tempdir().unwrap();
    let fake = directory.path().join("fake-codex");
    write_fake_harness(&fake, port);

    let doctor = research::doctor_with_binaries(port, Some(&fake), Some(&fake));
    assert!(doctor.ready, "{}", doctor.render());

    let (sender, receiver) = mpsc::channel();
    let run_id = research::launch(
        bridge.clone(),
        "rust",
        ResearchAgent::Codex,
        port,
        Some(&fake),
        move |snapshot| {
            let _ = sender.send(snapshot);
        },
    )
    .unwrap();
    assert!(
        bridge
            .snapshot()
            .research_runs
            .iter()
            .any(|run| { run.id == run_id && run.agent == "codex" && run.status == "running" })
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut completed = false;
    while Instant::now() < deadline {
        let Ok(snapshot) = receiver.recv_timeout(Duration::from_millis(250)) else {
            continue;
        };
        completed = snapshot
            .research_runs
            .iter()
            .any(|run| run.id == run_id && run.status == "done")
            && snapshot
                .research
                .iter()
                .any(|report| report.title == "Fake CLI finding");
        if completed {
            break;
        }
    }
    assert!(completed, "fake CLI did not submit and complete its run");
    assert!(bridge.snapshot().chat.iter().any(|message| {
        message
            .tool
            .as_deref()
            .is_some_and(|tool| tool == "submit_research · via mcp")
    }));
    let log_directory = directory.path().join("research-logs");
    let log_path = fs::read_dir(log_directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains("[pulse] agent=codex"));
    assert!(log.contains("[stdout] fake harness stdout"));
    assert!(log.contains("[stderr] fake harness stderr"));
    assert!(log.contains("[pulse] process finished: exit status: 0"));
}
