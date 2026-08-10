mod config;
mod content_cleaner;
mod email_index;
mod git_handler;
mod mail_processor;
mod models;
mod openai_client;

use crate::config::Config;
use crate::git_handler::GitHandler;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::ConnectOptions;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::info;
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
    info!("Opening git repository at: {}", config.git_repo_path);
    let g = GitHandler::open(&config.git_repo_path)?;

    info!("Connecting to SQLite database at: {}", config.db_path);
    let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", config.db_path))?
        .create_if_missing(true)
        .disable_statement_logging();

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("Failed to open SQLite database: {}", config.db_path))?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS emails (
            message_id TEXT PRIMARY KEY NOT NULL,
            subject TEXT NOT NULL,
            from_addr TEXT NOT NULL,
            date TEXT NOT NULL,
            body BLOB NOT NULL,
            in_reply_to TEXT,
            "references" TEXT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("Failed to create emails table")?;

    info!("Scanning git repository for messages...");
    let messages = g.get_all_messages()?;
    let total = messages.len();
    info!("Found {total} messages; inserting new ones into database...");

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )
        .context("Invalid progress bar template")?
        .progress_chars("=>-"),
    );
    pb.set_message("inserting emails");

    let mut tx = pool.begin().await?;
    let mut inserted = 0usize;
    let mut skipped = 0usize;

    for msg in messages {
        let msg = msg?;
        let body_compressed = zstd::encode_all(msg.body.as_bytes(), 3)
            .context("Failed to zstd-compress email body")?;
        let references_json =
            serde_json::to_string(&msg.references).context("Failed to serialize references")?;
        let date = msg.date.to_rfc3339();

        // Skip rows that already exist (by message_id primary key).
        let result = sqlx::query!(
            r#"
            INSERT OR IGNORE INTO emails
                (message_id, subject, from_addr, date, body, in_reply_to, "references")
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
            msg.message_id,
            msg.subject,
            msg.from,
            date,
            body_compressed,
            msg.in_reply_to,
            references_json,
        )
        .execute(&mut *tx)
        .await
        .with_context(|| format!("Failed to insert message {}", msg.message_id))?;

        if result.rows_affected() > 0 {
            inserted += 1;
        } else {
            skipped += 1;
        }
        pb.inc(1);
    }

    tx.commit().await.context("Failed to commit transaction")?;
    pb.finish_with_message("done");

    info!(
        "Wrote {inserted} new emails to {} (skipped {skipped} already present)",
        config.db_path
    );

    Ok(())
}
