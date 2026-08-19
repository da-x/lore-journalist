//! SQLite pool setup and sqlx migrations.

use anyhow::{Context, Result};
use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use tracing::info;

/// Embedded migrations from the crate `migrations/` directory.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Open a SQLite pool at `db_path` and apply pending migrations.
///
/// When `create_if_missing` is false, the file must already exist (or the
/// connection fails). Migrations always run after connect.
pub async fn open_db(db_path: &str, create_if_missing: bool) -> Result<SqlitePool> {
    info!("Connecting to SQLite database at: {db_path}");
    let options = SqliteConnectOptions::from_str(&format!("sqlite:{db_path}"))?
        .create_if_missing(create_if_missing)
        .disable_statement_logging();

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .with_context(|| format!("Failed to open SQLite database: {db_path}"))?;

    run_migrations(&pool).await?;
    Ok(pool)
}

/// Apply embedded migrations to `pool`.
pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    MIGRATOR
        .run(pool)
        .await
        .context("Failed to run database migrations")?;
    Ok(())
}

/// In-memory SQLite pool with migrations applied (tests and output-only CLI tools).
pub async fn open_in_memory() -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str("sqlite::memory:")?
        .create_if_missing(true)
        .disable_statement_logging();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .context("Failed to open in-memory SQLite")?;
    run_migrations(&pool).await?;
    Ok(pool)
}

/// Insert a fixture email (compile-time checked query). Tests only.
#[cfg(test)]
pub async fn insert_test_email(
    pool: &SqlitePool,
    message_id: &str,
    subject: &str,
    from_addr: &str,
    date: &str,
    body: &str,
    in_reply_to: Option<&str>,
    references: &str,
) -> Result<()> {
    let compressed = zstd::encode_all(body.as_bytes(), 3).context("zstd compress")?;
    sqlx::query!(
        r#"
            INSERT INTO emails
                (message_id, subject, from_addr, date, body, in_reply_to, "references")
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        message_id,
        subject,
        from_addr,
        date,
        compressed,
        in_reply_to,
        references,
    )
    .execute(pool)
    .await
    .context("insert_test_email")?;
    Ok(())
}
