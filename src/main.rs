mod cache;
mod engine;
mod query;
mod results;
mod sources;

use chrono::{NaiveDate, TimeZone, Utc};
use clap::{Parser, Subcommand};
use engine::RecallEngine;
use query::{MatchMode, RecallQuery};

#[derive(Parser)]
#[command(name = "remember")]
#[command(about = "Memory recall for agents — search across conversation histories and perception data")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Free-text query to search for (use quotes for exact phrases: '"exact phrase"')
    #[arg(trailing_var_arg = true, num_args = 0..)]
    query: Vec<String>,

    /// Keyword filters (can specify multiple)
    #[arg(short, long, num_args = 1..)]
    keywords: Vec<String>,

    /// Match ANY term instead of ALL terms (default is AND)
    #[arg(long)]
    any: bool,

    /// Only return results after this date (YYYY-MM-DD)
    #[arg(long)]
    after: Option<String>,

    /// Only return results before this date (YYYY-MM-DD)
    #[arg(long)]
    before: Option<String>,

    /// Maximum results per source
    #[arg(short, long, default_value = "20")]
    limit: usize,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    format: OutputFormat,
}

#[derive(Clone, clap::ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// List available data sources and their status
    Sources,
    /// Clear the result cache
    ClearCache,
}

fn parse_date(s: &str) -> anyhow::Result<chrono::DateTime<Utc>> {
    let naive = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("Invalid date '{}': {} (expected YYYY-MM-DD)", s, e))?;
    Ok(Utc.from_utc_datetime(&naive.and_hms_opt(0, 0, 0).unwrap()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let engine = RecallEngine::new();

    match cli.command {
        Some(Commands::Sources) => {
            let sources = engine.list_sources();
            println!("Available memory sources:");
            for (name, available) in sources {
                let status = if available { "✓" } else { "✗" };
                println!("  {} {}", status, name);
            }
            return Ok(());
        }
        Some(Commands::ClearCache) => {
            engine.clear_cache()?;
            println!("Cache cleared.");
            return Ok(());
        }
        None => {}
    }

    // Build the query
    let query_text = if cli.query.is_empty() {
        None
    } else {
        Some(cli.query.join(" "))
    };

    let after = cli.after.as_deref().map(parse_date).transpose()?;
    let before = cli.before.as_deref().map(parse_date).transpose()?;

    let mode = if cli.any { MatchMode::Or } else { MatchMode::And };

    let query = RecallQuery {
        text: query_text,
        keywords: cli.keywords,
        after,
        before,
        limit: cli.limit,
        mode,
    };

    if !query.has_constraints() {
        eprintln!("Error: Please provide a query, keywords, or date range.");
        eprintln!("Usage: remember <query text>");
        eprintln!("       remember -k rust -k sqlite --after 2025-01-01");
        eprintln!("       remember --help");
        std::process::exit(1);
    }

    let results = engine.recall(&query).await;

    match cli.format {
        OutputFormat::Text => print!("{}", results.format_text()),
        OutputFormat::Json => println!("{}", results.format_json()),
    }

    Ok(())
}
