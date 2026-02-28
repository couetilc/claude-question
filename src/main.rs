mod commands;
mod db;
mod models;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "claude-track", about = "Claude Code usage analytics tracker")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Hook entrypoint — dispatches by event, writes to SQLite (reads JSON from stdin)
    Hook,
    /// Show usage statistics
    Stats,
    /// Register all hooks in Claude Code settings
    Install,
    /// Remove all hooks and optionally delete data
    Uninstall,
    /// Import legacy JSONL data into SQLite
    Migrate,
    /// Run an ad-hoc SQL query against the tracking database
    Query {
        /// The SQL query to execute
        sql: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Hook => commands::hook::run(),
        Commands::Stats => commands::stats::run(),
        Commands::Install => commands::install::run(),
        Commands::Uninstall => commands::uninstall::run(),
        Commands::Migrate => commands::migrate::run(),
        Commands::Query { ref sql } => commands::query::run(sql),
    }
}
