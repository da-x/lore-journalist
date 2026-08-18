//! `GrepOutputs` pure handler: regex over files under outputs_path.

use super::ToolCtx;
use super::paths::{path_glob_match, relative_display, resolve_output_path};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::fs;
use walkdir::WalkDir;

const DEFAULT_MAX_MATCHES: usize = 50;
const MAX_FILE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct GrepOutputsArgs {
    pub pattern: String,
    /// Optional glob filter, default `**/*` (all files under outputs).
    pub glob: Option<String>,
    pub max_matches: Option<usize>,
}

pub async fn grep_outputs(ctx: &ToolCtx, args: GrepOutputsArgs) -> Result<String> {
    if args.pattern.is_empty() {
        bail!("pattern is required");
    }
    let re =
        Regex::new(&args.pattern).with_context(|| format!("invalid regex: {}", args.pattern))?;
    let glob_pat = args.glob.as_deref().unwrap_or("**/*");
    if glob_pat.contains("..") || glob_pat.starts_with('/') {
        bail!("invalid glob filter");
    }
    let max_matches = args.max_matches.unwrap_or(DEFAULT_MAX_MATCHES);

    if !ctx.outputs_path.exists() {
        return Ok(format!(
            "GrepOutputs: outputs_path missing (0 matches for {:?})\n",
            args.pattern
        ));
    }
    let root = ctx
        .outputs_path
        .canonicalize()
        .with_context(|| format!("canonicalize {}", ctx.outputs_path.display()))?;

    let mut match_count = 0usize;
    let mut files_scanned = 0usize;
    let mut out = format!(
        "GrepOutputs pattern={:?} glob={glob_pat:?} max_matches={max_matches}\n\n",
        args.pattern
    );

    let mut files: Vec<_> = WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    files.sort();

    'files: for abs in files {
        let rel = relative_display(&root, &abs);
        if !path_glob_match(glob_pat, &rel) {
            continue;
        }
        // Re-validate via sandbox (relative path).
        let checked = match resolve_output_path(&root, &rel) {
            Ok(p) => p,
            Err(_) => continue,
        };
        files_scanned += 1;
        let bytes = match fs::read(&checked) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let text = if bytes.len() > MAX_FILE_BYTES {
            String::from_utf8_lossy(&bytes[..MAX_FILE_BYTES]).into_owned()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };

        let mut file_header = false;
        for (i, line) in text.lines().enumerate() {
            if match_count >= max_matches {
                break 'files;
            }
            if re.is_match(line) {
                if !file_header {
                    out.push_str(&format!("=== {rel} ===\n"));
                    file_header = true;
                }
                out.push_str(&format!("  L{}: {line}\n", i + 1));
                match_count += 1;
            }
        }
    }

    out.push_str(&format!(
        "\nSummary: match_lines={match_count} files_scanned={files_scanned}"
    ));
    if match_count >= max_matches {
        out.push_str(" truncated=max_matches");
    }
    out.push('\n');
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
    async fn grep_finds_line_in_thread_md() {
        let mut root = std::env::temp_dir();
        root.push(format!("lore-grepo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("2026-07-20/thread")).unwrap();
        std::fs::write(
            root.join("2026-07-20/thread/t.md"),
            b"# Title\nunique_output_token here\n",
        )
        .unwrap();
        let root = root.canonicalize().unwrap();

        let pool = open_in_memory().await.unwrap();
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, root.clone(), w, week_window(w));

        let out = grep_outputs(
            &ctx,
            GrepOutputsArgs {
                pattern: "unique_output_token".into(),
                glob: Some("*/thread/*.md".into()),
                max_matches: None,
            },
        )
        .await
        .unwrap();
        assert!(out.contains("unique_output_token"));
        assert!(out.contains("2026-07-20/thread/t.md"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
