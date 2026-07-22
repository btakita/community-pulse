use anyhow::{Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use community_pulse::research::ResearchAgent;
use community_pulse::{PulseEngine, app, ingest, live, research, setup};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "pulse",
    version,
    about = "Community signals under an attention budget"
)]
struct Cli {
    /// SQLite database used for normalized posts, scores, and user state.
    #[arg(long, default_value = "community-pulse.db", global = true)]
    database: PathBuf,

    /// Replace the database with the bundled deterministic community snapshot.
    #[arg(long, global = true)]
    fixture: bool,

    /// Use the deterministic local agent instead of an external LLM API.
    #[arg(long, global = true)]
    replay: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch Hacker News, Lobsters, and Product Hunt, then recompute scores.
    Ingest,
    /// Print the ranked digest using the stored attention budget by default.
    Top {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Print the evidence behind one trend id.
    Explain { id: String },
    /// Launch the Slint topic composer, digest, evidence view, and chat.
    App {
        /// Open only the 390x740 phone-frame view.
        #[arg(long, conflicts_with = "companion")]
        mobile: bool,
        /// Open synchronized desktop and phone-frame windows.
        #[arg(long, conflicts_with = "mobile")]
        companion: bool,
        /// Expose the shared tools over MCP streamable HTTP on localhost.
        #[arg(long, default_value_t = setup::DEFAULT_MCP_PORT)]
        mcp_port: u16,
        /// Run the app without its default localhost MCP endpoint.
        #[arg(long)]
        no_mcp: bool,
        /// Open a visible interactive agent session in $TERMINAL (repeatable).
        #[arg(
            long = "agent-terminal",
            value_parser = ["claude", "codex"],
            num_args = 0..=1,
            default_missing_value = "claude",
            action = clap::ArgAction::Append
        )]
        agent_terminals: Vec<String>,
        /// Keep public-source data fresh in the background.
        #[arg(long)]
        live: bool,
        /// Seconds between live ingests (clamped to the 120-second source floor).
        #[arg(long, requires = "live")]
        ingest_interval: Option<u64>,
    },
    /// Write a consistent, standalone SQLite copy for demo fallback or transfer.
    Snapshot {
        /// New database path. Refuses to overwrite an existing file.
        output: PathBuf,
    },
    /// Check local Claude/Codex research delegation and MCP readiness.
    Research {
        #[command(subcommand)]
        command: ResearchCommand,
    },
    /// Configure Claude and/or Codex to use the default Pulse MCP endpoint.
    Setup {
        /// Agent to configure; omit to configure both.
        #[arg(value_parser = ["claude", "codex"])]
        agent: Option<String>,
        /// MCP port written into the agent configuration.
        #[arg(long, default_value_t = setup::DEFAULT_MCP_PORT)]
        mcp_port: u16,
    },
}

#[derive(Subcommand)]
enum ResearchCommand {
    /// Verify harness binaries, pulse MCP registration, and endpoint reachability.
    Doctor {
        #[arg(long, default_value_t = 7432)]
        mcp_port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut engine = PulseEngine::open(&cli.database)?;
    if cli.fixture {
        let loaded = engine.load_fixture(Utc::now())?;
        eprintln!("loaded {loaded} deterministic fixture posts");
    }

    match cli.command {
        Command::Ingest => {
            let mut total = 0;
            for (source, result) in ingest::fetch_all().await {
                match result {
                    Ok(posts) => {
                        let count = engine.ingest(&posts)?;
                        total += count;
                        eprintln!("{source}: normalized {count} posts");
                    }
                    Err(error) => eprintln!("{source}: skipped ({error:#})"),
                }
            }
            if total == 0 && engine.post_count()? == 0 {
                bail!("all ingesters failed and the database is empty; retry or use --fixture")
            }
            let scores = engine.recompute(Utc::now())?;
            println!("ingested {total} posts; scored {} topics", scores.len());
        }
        Command::Top { limit, json } => {
            ensure_data(&engine)?;
            engine.recompute(Utc::now())?;
            let cards = engine.get_pulse(&engine.load_interests()?, limit, Utc::now())?;
            let budget = engine.budget()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&cards)?);
            } else {
                println!("COMMUNITY PULSE  ·  {} / {budget}", cards.len());
                for (index, card) in cards.iter().enumerate() {
                    println!(
                        "{}. {:<16} {:>6.1}  {:+.1}σ  {}",
                        index + 1,
                        card.topic,
                        card.score,
                        card.z_score,
                        card.headline
                    );
                    println!(
                        "   {} · {} now / {} in 6h / {} in 24h",
                        card.sources.join(" + "),
                        card.mentions_1h,
                        card.mentions_6h,
                        card.mentions_24h
                    );
                }
            }
        }
        Command::Explain { id } => {
            ensure_data(&engine)?;
            engine.recompute(Utc::now())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&engine.explain_trend(&id, Utc::now())?)?
            );
        }
        Command::App {
            mobile,
            companion,
            mcp_port,
            no_mcp,
            agent_terminals,
            live,
            ingest_interval,
        } => {
            ensure_data(&engine)?;
            engine.recompute(Utc::now())?;
            let view = if companion {
                app::ViewMode::Companion
            } else if mobile {
                app::ViewMode::Mobile
            } else {
                app::ViewMode::Desktop
            };
            let live_policy = live.then(|| {
                live::LivePolicy::new(
                    ingest_interval.unwrap_or(live::DEFAULT_INGEST_INTERVAL.as_secs()),
                )
            });
            let mcp_port = (!no_mcp).then_some(mcp_port);
            let agent_terminals = agent_terminals
                .iter()
                .filter_map(|agent| ResearchAgent::parse(agent))
                .collect::<Vec<_>>();
            app::run(
                engine,
                cli.replay,
                view,
                mcp_port,
                cli.fixture,
                live_policy,
                &agent_terminals,
            )?;
        }
        Command::Snapshot { output } => {
            engine.snapshot_to(&output)?;
            println!("snapshot written to {}", output.display());
        }
        Command::Research { command } => match command {
            ResearchCommand::Doctor { mcp_port } => {
                let report = research::doctor(mcp_port);
                println!("{}", report.render());
                if !report.ready {
                    bail!("research doctor found missing prerequisites");
                }
            }
        },
        Command::Setup { agent, mcp_port } => run_setup(agent.as_deref(), mcp_port)?,
    }
    Ok(())
}

fn run_setup(agent: Option<&str>, port: u16) -> Result<()> {
    let agents = match agent.and_then(ResearchAgent::parse) {
        Some(agent) => vec![agent],
        None if agent.is_none() => vec![ResearchAgent::Claude, ResearchAgent::Codex],
        None => unreachable!("clap limits setup agents to claude or codex"),
    };
    println!("PULSE AGENT SETUP");
    let mut failures = Vec::new();
    for agent in agents {
        let result = match agent {
            ResearchAgent::Claude => setup::setup_claude(port),
            ResearchAgent::Codex => setup::setup_codex(port),
        };
        match result {
            Ok(outcome) => println!("✓ {:<8} {}", agent.as_str(), outcome.label()),
            Err(error) => {
                println!("✗ {:<8} {error:#}", agent.as_str());
                failures.push(agent.as_str());
            }
        }
    }
    println!();
    println!("Endpoint: {}", setup::endpoint(port));
    println!("Next: pulse app (MCP starts automatically)");
    if !failures.is_empty() {
        bail!("setup failed for {}", failures.join(", "));
    }
    Ok(())
}

fn ensure_data(engine: &PulseEngine) -> Result<()> {
    if engine.post_count()? == 0 {
        bail!("the database is empty; run `pulse ingest` or add `--fixture`")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_defaults_to_local_mcp_and_claude_terminal_when_value_is_omitted() {
        let cli = Cli::try_parse_from(["pulse", "app", "--agent-terminal"]).unwrap();
        let Command::App {
            mcp_port,
            no_mcp,
            agent_terminals,
            ..
        } = cli.command
        else {
            panic!("expected app command");
        };
        assert_eq!(mcp_port, setup::DEFAULT_MCP_PORT);
        assert!(!no_mcp);
        assert_eq!(agent_terminals, ["claude"]);
    }

    #[test]
    fn app_accepts_two_agent_terminals_and_no_mcp() {
        let cli = Cli::try_parse_from([
            "pulse",
            "app",
            "--no-mcp",
            "--agent-terminal=claude",
            "--agent-terminal=codex",
        ])
        .unwrap();
        let Command::App {
            no_mcp,
            agent_terminals,
            ..
        } = cli.command
        else {
            panic!("expected app command");
        };
        assert!(no_mcp);
        assert_eq!(agent_terminals, ["claude", "codex"]);
    }
}
