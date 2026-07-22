//! Explicit, one-time agent configuration and optional terminal launch helpers.

use crate::research::ResearchAgent;
use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use toml_edit::{Array, DocumentMut, Item, Table, value};

pub const DEFAULT_MCP_PORT: u16 = 7432;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupOutcome {
    Configured,
    AlreadyConfigured,
}

impl SetupOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::AlreadyConfigured => "already configured",
        }
    }
}

pub fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

pub fn setup_claude(port: u16) -> Result<SetupOutcome> {
    setup_claude_with_binary(Path::new("claude"), port)
}

pub fn setup_claude_with_binary(binary: &Path, port: u16) -> Result<SetupOutcome> {
    let listing = Command::new(binary)
        .args(["mcp", "list"])
        .output()
        .with_context(|| format!("run `{}` to inspect Claude MCP servers", binary.display()))?;
    let listing_text = format!(
        "{}{}",
        String::from_utf8_lossy(&listing.stdout),
        String::from_utf8_lossy(&listing.stderr)
    );
    if listing.status.success() && registration_mentions_pulse(&listing_text) {
        return Ok(SetupOutcome::AlreadyConfigured);
    }

    let endpoint = endpoint(port);
    let output = Command::new(binary)
        .args([
            "mcp",
            "add",
            "-s",
            "user",
            "--transport",
            "http",
            "pulse",
            endpoint.as_str(),
        ])
        .output()
        .with_context(|| format!("run `{}` to configure Claude MCP", binary.display()))?;
    if !output.status.success() {
        bail!(
            "Claude MCP setup failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(SetupOutcome::Configured)
}

fn registration_mentions_pulse(listing: &str) -> bool {
    listing.lines().any(|line| {
        let first = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '-');
        first.eq_ignore_ascii_case("pulse")
    })
}

pub fn codex_config_path() -> Result<PathBuf> {
    if let Some(root) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root).join("config.toml"));
    }
    let home =
        env::var_os("HOME").context("HOME is not set; cannot locate ~/.codex/config.toml")?;
    Ok(PathBuf::from(home).join(".codex/config.toml"))
}

pub fn setup_codex(port: u16) -> Result<SetupOutcome> {
    setup_codex_at(&codex_config_path()?, port)
}

pub fn setup_codex_at(path: &Path, port: u16) -> Result<SetupOutcome> {
    let existing = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let mut document = if existing.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing
            .parse::<DocumentMut>()
            .with_context(|| format!("{} is malformed; refusing to modify it", path.display()))?
    };

    let endpoint = endpoint(port);
    if codex_entry_matches(&document, &endpoint) {
        return Ok(SetupOutcome::AlreadyConfigured);
    }
    ensure_table(&mut document, "mcp_servers", path)?;
    ensure_nested_table(&mut document, "mcp_servers", "pulse", path)?;

    document["mcp_servers"]["pulse"]["command"] = value("npx");
    let mut args = Array::new();
    args.push("-y");
    args.push("mcp-remote");
    args.push(endpoint);
    document["mcp_servers"]["pulse"]["args"] = value(args);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create Codex config directory {}", parent.display()))?;
    }
    fs::write(path, document.to_string())
        .with_context(|| format!("write Codex config {}", path.display()))?;
    Ok(SetupOutcome::Configured)
}

fn ensure_table(document: &mut DocumentMut, key: &str, path: &Path) -> Result<()> {
    if document.get(key).is_none() {
        document[key] = Item::Table(Table::new());
    } else if !document[key].is_table() {
        bail!(
            "{} has a non-table `{key}` value; refusing to overwrite it",
            path.display()
        );
    }
    Ok(())
}

fn ensure_nested_table(
    document: &mut DocumentMut,
    parent: &str,
    key: &str,
    path: &Path,
) -> Result<()> {
    if document[parent].get(key).is_none() {
        document[parent][key] = Item::Table(Table::new());
    } else if !document[parent][key].is_table() {
        bail!(
            "{} has a non-table `{parent}.{key}` value; refusing to overwrite it",
            path.display()
        );
    }
    Ok(())
}

fn codex_entry_matches(document: &DocumentMut, endpoint: &str) -> bool {
    let Some(entry) = document
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .and_then(|servers| servers.get("pulse"))
        .and_then(Item::as_table_like)
    else {
        return false;
    };
    let command_matches = entry
        .get("command")
        .and_then(Item::as_str)
        .is_some_and(|command| command == "npx");
    let args_match = entry
        .get("args")
        .and_then(Item::as_array)
        .is_some_and(|args| {
            args.iter()
                .filter_map(|item| item.as_str())
                .eq(["-y", "mcp-remote", endpoint])
        });
    command_matches && args_match
}

pub fn spawn_agent_terminal(agent: ResearchAgent) -> Result<()> {
    let terminal = env::var_os("TERMINAL")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "i3-sensible-terminal".into());
    Command::new(&terminal)
        .arg("-e")
        .arg(agent.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| {
            format!(
                "launch {} in terminal {}",
                agent.as_str(),
                PathBuf::from(terminal).display()
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn codex_setup_merges_and_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "model = \"gpt-5\"\n").unwrap();

        assert_eq!(
            setup_codex_at(&path, 7432).unwrap(),
            SetupOutcome::Configured
        );
        let configured = fs::read_to_string(&path).unwrap();
        assert!(configured.contains("model = \"gpt-5\""));
        assert!(configured.contains("[mcp_servers.pulse]"));
        assert!(configured.contains("http://127.0.0.1:7432/mcp"));
        assert_eq!(
            setup_codex_at(&path, 7432).unwrap(),
            SetupOutcome::AlreadyConfigured
        );
    }

    #[test]
    fn codex_setup_refuses_malformed_input_without_replacing_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let malformed = "[mcp_servers\n";
        fs::write(&path, malformed).unwrap();

        let error = setup_codex_at(&path, 7432).unwrap_err();
        assert!(error.to_string().contains("malformed"));
        assert_eq!(fs::read_to_string(path).unwrap(), malformed);
    }

    #[test]
    fn pulse_registration_requires_a_pulse_name_column() {
        assert!(registration_mentions_pulse(
            "pulse  http://127.0.0.1:7432/mcp"
        ));
        assert!(!registration_mentions_pulse(
            "other  https://example.test/pulse"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn claude_setup_uses_user_scoped_http_registration() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("claude");
        let capture = directory.path().join("args.txt");
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"mcp\" ] && [ \"$2\" = \"list\" ]; then exit 0; fi\nprintf '%s\\n' \"$@\" > '{}'\n",
                capture.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();

        assert_eq!(
            setup_claude_with_binary(&binary, 7432).unwrap(),
            SetupOutcome::Configured
        );
        assert_eq!(
            fs::read_to_string(capture).unwrap(),
            "mcp\nadd\n-s\nuser\n--transport\nhttp\npulse\nhttp://127.0.0.1:7432/mcp\n"
        );
    }
}
