use crate::domain::ChatRole;
use crate::reactive::UiSnapshot;
use crate::tools::ToolBridge;
use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROMPT_TEMPLATE: &str = include_str!("../docs/research-prompt.md");
const ARTICLE_BRIEF_PROMPT_TEMPLATE: &str = include_str!("../docs/article-brief-prompt.md");
const CLAUDE_DEFAULT_MODEL: &str = "opus";
const CLAUDE_PULSE_TOOLS: &str = "mcp__pulse__*";

type ProgressSink = dyn Fn(&str, &str) + Send + Sync;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchAgent {
    Claude,
    Codex,
}

impl ResearchAgent {
    pub fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("claude") {
            Some(Self::Claude)
        } else if value.eq_ignore_ascii_case("codex") {
            Some(Self::Codex)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn binary(self) -> &'static str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub ready: bool,
    pub checks: Vec<DoctorCheck>,
    pub fixes: Vec<String>,
}

impl DoctorReport {
    pub fn render(&self) -> String {
        let mut lines = vec!["PULSE RESEARCH DOCTOR".to_owned()];
        lines.extend(self.checks.iter().map(|check| {
            format!(
                "{} {:<24} {}",
                if check.ok { "✓" } else { "✗" },
                check.label,
                check.detail
            )
        }));
        if !self.fixes.is_empty() {
            lines.push(String::new());
            lines.push("Fixes:".to_owned());
            lines.extend(self.fixes.iter().map(|fix| format!("  {fix}")));
        }
        lines.push(String::new());
        lines.push(
            "Research runs consume your Claude/ChatGPT subscription quota and start only when you click a research action."
                .to_owned(),
        );
        lines.join("\n")
    }
}

pub fn doctor(port: u16) -> DoctorReport {
    doctor_with_binaries(port, None, None)
}

pub fn doctor_with_binaries(
    port: u16,
    claude_binary: Option<&Path>,
    codex_binary: Option<&Path>,
) -> DoctorReport {
    let mut checks = Vec::new();
    check_harness(ResearchAgent::Claude, claude_binary, &mut checks);
    check_harness(ResearchAgent::Codex, codex_binary, &mut checks);
    let endpoint = SocketAddr::from(([127, 0, 0, 1], port));
    let reachable = TcpStream::connect_timeout(&endpoint, Duration::from_millis(500)).is_ok();
    checks.push(DoctorCheck {
        label: "pulse MCP endpoint".to_owned(),
        ok: reachable,
        detail: if reachable {
            format!("http://127.0.0.1:{port}/mcp reachable")
        } else {
            format!("http://127.0.0.1:{port}/mcp unavailable")
        },
    });

    let ready = checks.iter().all(|check| check.ok);
    let mut fixes = Vec::new();
    if !reachable {
        fixes.push(format!("pulse --fixture app --mcp-port {port}"));
    }
    if checks
        .iter()
        .any(|check| check.label == "claude MCP registration" && !check.ok)
    {
        fixes.push(format!(
            "claude mcp add --transport http pulse http://127.0.0.1:{port}/mcp"
        ));
    }
    if checks
        .iter()
        .any(|check| check.label == "codex MCP registration" && !check.ok)
    {
        fixes.push(format!(
            "codex mcp add pulse -- npx -y mcp-remote http://127.0.0.1:{port}/mcp"
        ));
    }
    DoctorReport {
        ready,
        checks,
        fixes,
    }
}

fn check_harness(
    agent: ResearchAgent,
    binary_override: Option<&Path>,
    checks: &mut Vec<DoctorCheck>,
) {
    let binary = binary_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(agent.binary()));
    let version = Command::new(&binary).arg("--version").output();
    checks.push(DoctorCheck {
        label: format!("{} CLI", agent.as_str()),
        ok: version.as_ref().is_ok_and(|output| output.status.success()),
        detail: version
            .ok()
            .filter(|output| output.status.success())
            .map(|output| first_line(&output.stdout))
            .filter(|line| !line.is_empty())
            .unwrap_or_else(|| format!("{} not available", binary.display())),
    });

    let registration = Command::new(&binary).args(["mcp", "list"]).output();
    let registration_text = registration
        .as_ref()
        .ok()
        .map(|output| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .unwrap_or_default();
    let registered = registration
        .as_ref()
        .is_ok_and(|output| output.status.success())
        && registration_text.to_ascii_lowercase().contains("pulse");
    checks.push(DoctorCheck {
        label: format!("{} MCP registration", agent.as_str()),
        ok: registered,
        detail: if registered {
            "pulse registered".to_owned()
        } else {
            "pulse registration missing".to_owned()
        },
    });
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

pub fn launch<F>(
    bridge: ToolBridge,
    topic_id: &str,
    agent: ResearchAgent,
    port: u16,
    binary_override: Option<&Path>,
    on_change: F,
) -> Result<u64>
where
    F: Fn(UiSnapshot) + Send + Sync + 'static,
{
    launch_prompt(
        bridge,
        topic_id,
        agent,
        binary_override,
        on_change,
        render_prompt(topic_id, port),
        topic_id,
    )
}

pub fn launch_article_brief<F>(
    bridge: ToolBridge,
    topic_id: &str,
    article_url: &str,
    agent: ResearchAgent,
    port: u16,
    binary_override: Option<&Path>,
    on_change: F,
) -> Result<u64>
where
    F: Fn(UiSnapshot) + Send + Sync + 'static,
{
    launch_prompt(
        bridge,
        topic_id,
        agent,
        binary_override,
        on_change,
        render_article_brief_prompt(topic_id, article_url, port),
        article_url,
    )
}

fn launch_prompt<F>(
    bridge: ToolBridge,
    topic_id: &str,
    agent: ResearchAgent,
    binary_override: Option<&Path>,
    on_change: F,
    prompt: String,
    target: &str,
) -> Result<u64>
where
    F: Fn(UiSnapshot) + Send + Sync + 'static,
{
    let run_id = bridge.start_research_run(topic_id, agent.as_str());
    let on_change = Arc::new(on_change);
    let log_directory = research_log_directory(binary_override);
    let binary = binary_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(agent.binary()));
    let (log_path, log) =
        match create_research_log(&log_directory, run_id, topic_id, target, agent, &binary) {
            Ok(log) => log,
            Err(error) => {
                bridge
                    .fail_research_run(run_id, format!("could not create research log: {error:#}"));
                on_change(bridge.snapshot());
                return Err(error);
            }
        };
    let log_display = display_log_path(&log_path);
    bridge.state().append_chat(
        ChatRole::System,
        format!(
            "{} research started for {topic_id} · headless process · log {log_display}",
            agent.as_str().to_uppercase()
        ),
        None,
    );
    on_change(bridge.snapshot());

    let mut command = research_command(&binary, agent, prompt);
    let child = command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            append_log(&log, &format!("[pulse] launch failed: {error}"));
            bridge.fail_research_run(
                run_id,
                format!(
                    "could not launch {}: {error} · log {log_display}",
                    binary.display()
                ),
            );
            bridge.state().append_chat(
                ChatRole::System,
                format!(
                    "{} research failed to launch · log {log_display}",
                    agent.as_str().to_uppercase()
                ),
                None,
            );
            on_change(bridge.snapshot());
            return Err(error).with_context(|| format!("launch {} research", agent.as_str()));
        }
    };

    let progress_bridge = bridge.clone();
    let progress_on_change = Arc::clone(&on_change);
    let progress_sink: Arc<ProgressSink> = Arc::new(move |label, output| {
        let Some(progress) = progress_from_output(label, output) else {
            return;
        };
        let changed = progress_bridge
            .state()
            .update_research_progress(run_id, progress);
        if changed {
            progress_on_change(progress_bridge.snapshot());
        }
    });
    let stdout_pump = child.stdout.take().map(|stdout| {
        spawn_log_pump(
            stdout,
            "stdout",
            Arc::clone(&log),
            Arc::clone(&progress_sink),
        )
    });
    let stderr_pump = child
        .stderr
        .take()
        .map(|stderr| spawn_log_pump(stderr, "stderr", Arc::clone(&log), progress_sink));
    let topic_id = topic_id.to_owned();
    let agent_label = agent.as_str().to_uppercase();
    std::thread::spawn(move || {
        let process_result = child.wait();
        if let Some(pump) = stdout_pump {
            let _ = pump.join();
        }
        if let Some(pump) = stderr_pump {
            let _ = pump.join();
        }
        let exit_detail = process_result
            .as_ref()
            .map_or_else(|error| format!("wait error: {error}"), ToString::to_string);
        append_log(&log, &format!("[pulse] process finished: {exit_detail}"));

        let run_status = bridge
            .snapshot()
            .research_runs
            .iter()
            .find(|run| run.id == run_id)
            .map(|run| run.status.clone());
        if run_status.as_deref() == Some("done") {
            bridge.state().append_chat(
                ChatRole::System,
                format!(
                    "{agent_label} research submitted for {topic_id} · {exit_detail} · log {log_display}"
                ),
                None,
            );
        } else {
            let tail = research_log_tail(&log_path);
            let failure = match process_result {
                Ok(status) if status.success() => {
                    format!("process exited without submitting a report · log {log_display}")
                }
                Ok(status) if tail.is_empty() => {
                    format!("process exited with {status} · log {log_display}")
                }
                Ok(status) => format!(
                    "process exited with {status}: {} · log {log_display}",
                    one_line(&tail)
                ),
                Err(error) => {
                    format!("could not wait for research process: {error} · log {log_display}")
                }
            };
            bridge.fail_research_run(run_id, failure.clone());
            bridge.state().append_chat(
                ChatRole::System,
                format!("{agent_label} research failed for {topic_id}: {failure}"),
                None,
            );
        }
        on_change(bridge.snapshot());
    });
    Ok(run_id)
}

fn research_command(binary: &Path, agent: ResearchAgent, prompt: String) -> Command {
    let mut command = Command::new(binary);
    match agent {
        ResearchAgent::Claude => {
            command
                .args(["--model", CLAUDE_DEFAULT_MODEL])
                .args(["--allowedTools", CLAUDE_PULSE_TOOLS])
                .arg("-p")
                .arg(prompt)
                .args(["--permission-mode", "dontAsk"]);
        }
        ResearchAgent::Codex => {
            command.arg("exec").arg(prompt);
        }
    }
    command
}

fn research_log_directory(binary_override: Option<&Path>) -> PathBuf {
    binary_override
        .and_then(Path::parent)
        .map(|parent| parent.join("research-logs"))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("research/logs"))
}

fn create_research_log(
    directory: &Path,
    run_id: u64,
    topic_id: &str,
    target: &str,
    agent: ResearchAgent,
    binary: &Path,
) -> Result<(PathBuf, Arc<Mutex<File>>)> {
    fs::create_dir_all(directory)
        .with_context(|| format!("create research log directory {}", directory.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let filename = format!(
        "{timestamp}-{run_id}-{}-{}.log",
        safe_log_fragment(topic_id),
        agent.as_str()
    );
    let path = directory.join(filename);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("create research log {}", path.display()))?;
    writeln!(file, "[pulse] run_id={run_id}")?;
    writeln!(file, "[pulse] agent={}", agent.as_str())?;
    writeln!(
        file,
        "[pulse] model={}",
        if agent == ResearchAgent::Claude {
            CLAUDE_DEFAULT_MODEL
        } else {
            "cli-default"
        }
    )?;
    writeln!(file, "[pulse] topic={topic_id}")?;
    writeln!(file, "[pulse] target={target}")?;
    writeln!(file, "[pulse] binary={}", binary.display())?;
    file.flush()?;
    Ok((path, Arc::new(Mutex::new(file))))
}

fn safe_log_fragment(value: &str) -> String {
    let safe = value
        .chars()
        .take(48)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "research".to_owned()
    } else {
        safe
    }
}

fn display_log_path(path: &Path) -> String {
    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
        .unwrap_or(path)
        .display()
        .to_string()
}

fn spawn_log_pump<R>(
    mut reader: R,
    label: &'static str,
    log: Arc<Mutex<File>>,
    progress_sink: Arc<ProgressSink>,
) -> std::thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let text = String::from_utf8_lossy(&buffer[..count]);
                    append_log(&log, &format!("[{label}] {text}"));
                    progress_sink(label, &text);
                }
                Err(error) => {
                    append_log(&log, &format!("[pulse] {label} read error: {error}"));
                    break;
                }
            }
        }
    })
}

fn progress_from_output(label: &str, output: &str) -> Option<String> {
    let line = output.lines().rev().find_map(|line| {
        let line = one_line(line);
        (!line.is_empty()).then_some(line)
    })?;
    let line = truncate_chars(&line, 120);
    Some(if label == "stderr" {
        format!("Diagnostic · {line}")
    } else {
        line
    })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn append_log(log: &Arc<Mutex<File>>, message: &str) {
    if let Ok(mut file) = log.lock() {
        let _ = writeln!(file, "{}", message.trim_end());
        let _ = file.flush();
    }
}

fn render_prompt(topic_id: &str, port: u16) -> String {
    PROMPT_TEMPLATE
        .replace("{{topic}}", topic_id)
        .replace("{{port}}", &port.to_string())
}

fn render_article_brief_prompt(topic_id: &str, article_url: &str, port: u16) -> String {
    ARTICLE_BRIEF_PROMPT_TEMPLATE
        .replace("{{topic}}", topic_id)
        .replace("{{article_url}}", article_url)
        .replace("{{port}}", &port.to_string())
}

fn research_log_tail(path: &Path) -> String {
    fs::read(path)
        .map(|contents| text_tail(&contents))
        .unwrap_or_default()
}

fn text_tail(text: &[u8]) -> String {
    let text = String::from_utf8_lossy(text);
    let chars = text.chars().collect::<Vec<_>>();
    chars[chars.len().saturating_sub(2_000)..].iter().collect()
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_brief_prompt_fills_all_context() {
        let prompt = render_article_brief_prompt("rust", "https://example.com/article", 7432);
        assert!(prompt.contains("rust"));
        assert!(prompt.contains("https://example.com/article"));
        assert!(prompt.contains("http://127.0.0.1:7432/mcp"));
        assert!(prompt.contains("structured `sections`"));
        assert!(prompt.contains("Always produce a self-contained `web_report`"));
        assert!(prompt.contains("series: { label, points, baseline? }"));
        assert!(prompt.contains("research/reports/assets/"));
        assert!(!prompt.contains("{{"));
    }

    #[test]
    fn topic_prompt_always_requires_a_rich_web_report() {
        let prompt = render_prompt("rust", 7432);
        assert!(prompt.contains("Always produce a self-contained web report"));
        assert!(prompt.contains("series: { label, points, baseline? }"));
        assert!(prompt.contains("research/reports/assets/"));
    }

    #[test]
    fn claude_research_defaults_to_opus() {
        let command = research_command(
            Path::new("claude"),
            ResearchAgent::Claude,
            "research prompt".to_owned(),
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "--model",
                "opus",
                "--allowedTools",
                "mcp__pulse__*",
                "-p",
                "research prompt",
                "--permission-mode",
                "dontAsk",
            ]
        );
    }

    #[test]
    fn progress_uses_the_latest_nonempty_output_line() {
        assert_eq!(
            progress_from_output("stdout", "looking up sources\n\nreading posts\n"),
            Some("reading posts".to_owned())
        );
        assert_eq!(
            progress_from_output("stderr", "warning from harness"),
            Some("Diagnostic · warning from harness".to_owned())
        );
    }

    #[test]
    fn log_filenames_do_not_accept_path_syntax() {
        assert_eq!(safe_log_fragment("../AI infra/🔥"), "___AI_infra__");
    }
}
