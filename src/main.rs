mod commands;
mod config;
mod credential;
mod plugin_runtime;
mod recommendation;
mod snapshot;

use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser)]
#[command(
    name = "au",
    about = "AgentUsage — AI provider usage monitor",
    long_about = "Monitor AI provider usage across multiple providers.\nUsable as an agent skill for automated model-selection decisions.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Show current usage for all configured providers
    Status(commands::status::StatusArgs),
    /// Configure provider credentials
    Configure(commands::configure::ConfigureArgs),
    /// List configured providers and their sync status
    Providers,
    /// Manage plugins
    #[command(subcommand)]
    Plugins(commands::plugins::PluginsCommand),
}

fn main() {
    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::from_env("AU_LOG").add_directive(tracing::Level::ERROR.into())
    };

    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let result = match cli.command {
        Commands::Status(args) => commands::status::run(args, cli.verbose),
        Commands::Configure(args) => commands::configure::run(args),
        Commands::Providers => commands::providers::run(),
        Commands::Plugins(cmd) => commands::plugins::run(cmd),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
