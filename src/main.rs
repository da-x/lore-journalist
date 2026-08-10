mod config;
mod content_cleaner;
mod git_handler;
mod mail_processor;
mod models;
mod openai_client;

use crate::config::Config;
use crate::git_handler::GitHandler;
use crate::mail_processor::process_threads;
use crate::openai_client::OpenAIClient;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, default_value = "config.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    BuildDB {},
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let cli = Cli::parse();

    let config_content = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("Failed to read config file: {:?}", cli.config))?;
    let config: Config = toml::from_str(&config_content)?;

    match cli.command {
        Commands::BuildDB {} => {
            build_db(config).await?;
        }
    }

    Ok(())
}

async fn build_db(config: Config) -> Result<()> {
    let g = GitHandler::open(&config.git_repo_path);

    // TODO: Let's add sqlx here and build an sqlite database using sqlx::query! that will
    // include all the mails, based on the fields in the `EmailMessage` struct, and where
    // the body is compressed in zstd.

    Ok(())
}
