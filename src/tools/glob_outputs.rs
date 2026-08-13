//! `GlobOutputs` pure handler.

use super::ToolCtx;
use super::paths::{path_glob_match, relative_display};
use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct GlobOutputsArgs {
    /// Glob relative to outputs_path, e.g. `*/thread/*.md`.
    pub pattern: String,
}

const MAX_RESULTS: usize = 500;

pub async fn glob_outputs(ctx: &ToolCtx, args: GlobOutputsArgs) -> Result<String> {
    let pattern = args.pattern.trim();
    if pattern.is_empty() {
        bail!("pattern is required");
    }
    if pattern.contains("..") {
        bail!("'..' not allowed in glob pattern");
    }
    if pattern.starts_with('/') {
        bail!("absolute glob patterns not allowed");
    }

    let root = if ctx.outputs_path.exists() {
        ctx.outputs_path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", ctx.outputs_path.display()))?
    } else {
        return Ok(format!(
            "GlobOutputs pattern={pattern:?}: outputs_path does not exist yet (0 matches)\n"
        ));
    };

    let mut matches = Vec::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path();
        let rel = relative_display(&root, abs);
        if path_glob_match(pattern, &rel) {
            matches.push(rel);
            if matches.len() >= MAX_RESULTS {
                break;
            }
        }
    }
    matches.sort();

    let mut out = format!(
        "GlobOutputs pattern={pattern:?} matches={}{}\n",
        matches.len(),
        if matches.len() >= MAX_RESULTS {
            " (capped)"
        } else {
            ""
        }
    );
    for m in &matches {
        out.push_str(m);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::email_index::EmailIndex;
    use crate::week::week_window;
    use chrono::NaiveDate;
    use std::sync::Arc;

    #[tokio::test]
    async fn glob_finds_thread_files() {
        let mut root = std::env::temp_dir();
        root.push(format!("nfs-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("2026-07-13/thread")).unwrap();
        std::fs::create_dir_all(root.join("2026-07-20/thread")).unwrap();
        std::fs::write(root.join("2026-07-13/thread/a.md"), b"x").unwrap();
        std::fs::write(root.join("2026-07-20/thread/a.md"), b"y").unwrap();
        std::fs::write(root.join("2026-07-20/index.md"), b"z").unwrap();
        let root = root.canonicalize().unwrap();

        let pool = open_in_memory().await.unwrap();
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, root.clone(), w, week_window(w));

        let out = glob_outputs(
            &ctx,
            GlobOutputsArgs {
                pattern: "*/thread/a.md".into(),
            },
        )
        .await
        .unwrap();
        assert!(out.contains("2026-07-13/thread/a.md"));
        assert!(out.contains("2026-07-20/thread/a.md"));
        assert!(!out.contains("index.md") || out.matches("index.md").count() == 0);

        let bad = glob_outputs(
            &ctx,
            GlobOutputsArgs {
                pattern: "../x".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(bad.to_string().contains(".."));

        let _ = std::fs::remove_dir_all(&root);
    }
}
