mod commands;
mod models;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "claude-track", about = "Claude Code tool usage tracker")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Log a tool call (reads JSON from stdin, appends to JSONL log)
    Log,
    /// Show tool usage statistics
    Stats,
    /// Install the PostToolUse hook into Claude Code settings
    Install,
    /// Uninstall the hook and optionally delete the log
    Uninstall,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Log => commands::log::run(),
        Commands::Stats => commands::stats::run(),
        Commands::Install => commands::install::run(),
        Commands::Uninstall => commands::uninstall::run(),
    }
}
