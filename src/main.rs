use anyhow::{Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use community_pulse::{PulseEngine, app, ingest};
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
    /// Print the ranked digest (always capped at five).
    Top {
        #[arg(long, default_value_t = 5)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Print the evidence behind one trend id.
    Explain { id: String },
    /// Launch the Slint topic composer, digest, evidence view, and chat.
    App,
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
            let cards = engine.get_pulse(&engine.load_interests()?, Some(limit), Utc::now())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&cards)?);
            } else {
                println!("COMMUNITY PULSE  ·  {} / 5", cards.len());
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
        Command::App => {
            ensure_data(&engine)?;
            engine.recompute(Utc::now())?;
            app::run(engine, cli.replay)?;
        }
    }
    Ok(())
}

fn ensure_data(engine: &PulseEngine) -> Result<()> {
    if engine.post_count()? == 0 {
        bail!("the database is empty; run `pulse ingest` or add `--fixture`")
    }
    Ok(())
}
