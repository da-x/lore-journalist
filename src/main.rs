mod agent;
mod config;
mod content_cleaner;
mod db;
mod email_index;
mod git_handler;
mod grep_cmd;
mod html;
mod ids;
mod lock;
mod lore;
mod models;
mod outputs;
mod summarize;
mod tools;
mod week;

use crate::agent::llm::client_from_config;
use crate::config::Config;
use crate::db::open_db;
use crate::email_index::EmailIndex;
use crate::git_handler::GitHandler;
use crate::summarize::{AgentRunOpts, MaterializeResult, require_outputs_path, run_summarize_week};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
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
    /// Load all email metadata from the database into memory.
    Meta {},
    /// Search email threads with a regex; matching spans are green on a TTY.
    Grep {
        /// Regular expression to search for in composed Subject + Body text.
        pattern: String,
    },
    /// Produce one week edition end-to-end (lock, order, threads, overview, `.complete`).
    ///
    /// Does **not** write per-message markdown. Cleaned bodies stay in the DB for
    /// inference; published summaries link messages to lore.kernel.org.
    SummarizeWeek {
        /// Bootstrap only when no complete weeks exist under outputs_path.
        #[arg(long)]
        start_week: Option<String>,
        /// Explicit week end date (YYYY-MM-DD); wins over auto-resolve.
        #[arg(long)]
        week: Option<String>,
        /// Skip LLM agents (layout / empty stub only).
        #[arg(long, default_value_t = false)]
        prepare_only: bool,
    },
    /// Convert published markdown under outputs_path into static HTML.
    ///
    /// Requires `outputs_path`. HTML dest is `html_outputs_path` or `--html-dir`.
    RenderHtml {
        /// Override config.html_outputs_path.
        #[arg(long)]
        html_dir: Option<PathBuf>,
        /// Override config.outputs_path.
        #[arg(long)]
        outputs: Option<PathBuf>,
    },
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
        Commands::Meta {} => {
            meta(config).await?;
        }
        Commands::Grep { pattern } => {
            let pool = open_db(&config.db_path, false).await?;
            grep_cmd::run_grep(&pool, &pattern).await?;
        }
        Commands::SummarizeWeek {
            start_week,
            week,
            prepare_only,
        } => {
            summarize_week_cmd(config, start_week.as_deref(), week.as_deref(), prepare_only)
                .await?;
        }
        Commands::RenderHtml { html_dir, outputs } => {
            render_html_cmd(&config, outputs, html_dir)?;
        }
    }

    Ok(())
}

async fn summarize_week_cmd(
    config: Config,
    start_week: Option<&str>,
    week: Option<&str>,
    prepare_only: bool,
) -> Result<()> {
    let outputs = require_outputs_path(&config.outputs_path)?;
    let pool = open_db(&config.db_path, false).await?;

    let opts = AgentRunOpts {
        client: if prepare_only {
            None
        } else {
            Some(client_from_config(&config))
        },
        order_inference: None,
        thread_inference: None,
        week_inference: None,
        prepare_only,
    };

    let result = run_summarize_week(
        &pool,
        &outputs,
        week,
        start_week,
        &config.lore_base_url,
        &config.list,
        opts,
    )
    .await?;
    match &result {
        MaterializeResult::AlreadyComplete { week } => {
            info!(%week, "already complete; nothing to do");
        }
        MaterializeResult::EmptyWeekComplete { week } => {
            info!(%week, "empty week stub written and marked complete");
        }
        MaterializeResult::WeekPrepared {
            week,
            message_count,
            thread_count,
        } => {
            info!(
                %week,
                message_count,
                thread_count,
                "week prepared only (--prepare-only); agents not run"
            );
        }
        MaterializeResult::AgentsFinished {
            week,
            threads_ok,
            threads_failed,
            failed_thread_ids,
            week_complete,
            headline,
        } => {
            info!(
                %week,
                threads_ok,
                threads_failed,
                week_complete,
                ?headline,
                ?failed_thread_ids,
                "summarize-week agents finished"
            );
        }
    }

    let html_dir = crate::html::html_dir_from_config(&config.html_outputs_path);
    let published = matches!(
        result,
        MaterializeResult::EmptyWeekComplete { .. }
            | MaterializeResult::AgentsFinished {
                week_complete: true,
                ..
            }
    );
    if published {
        crate::html::maybe_render_html(&outputs, html_dir, config.list.short_title())?;
    }
    Ok(())
}

fn render_html_cmd(
    config: &Config,
    outputs_override: Option<PathBuf>,
    html_override: Option<PathBuf>,
) -> Result<()> {
    let outputs = match outputs_override {
        Some(p) => p,
        None => require_outputs_path(&config.outputs_path)?,
    };
    let html_dir = html_override.or_else(|| {
        crate::html::html_dir_from_config(&config.html_outputs_path).map(PathBuf::from)
    });
    let Some(html_dir) = html_dir else {
        anyhow::bail!(
            "html_outputs_path is required for render-html (set it in config or pass --html-dir)"
        );
    };
    crate::html::render_html_tree(&outputs, &html_dir, config.list.short_title())?;
    info!(
        outputs = %outputs.display(),
        html_dir = %html_dir.display(),
        "render-html finished"
    );
    Ok(())
}

async fn meta(config: Config) -> Result<()> {
    let pool = open_db(&config.db_path, false).await?;

    info!("Loading email metadata into memory...");
    let index = EmailIndex::load(&pool).await?;

    info!("Loaded {} emails into memory", index.len());
    if let (Some(first), Some(last)) = (index.emails().first(), index.emails().last()) {
        info!(
            "Date range: {} .. {}",
            first.date.format("%Y-%m-%d"),
            last.date.format("%Y-%m-%d")
        );
    }

    Ok(())
}

async fn build_db(config: Config) -> Result<()> {
    info!("Opening git repository at: {}", config.git_repo_path);
    let g = GitHandler::open(&config.git_repo_path)?;

    // open_db runs sqlx migrations (creates/updates schema).
    let pool = open_db(&config.db_path, true).await?;

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

        // Compile-time checked insert (sqlx::query!); skip existing PKs.
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
