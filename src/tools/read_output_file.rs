//! `ReadOutputFile` pure handler.

use super::paths::{relative_display, resolve_output_path};
use super::ToolCtx;
use anyhow::{bail, Context, Result};
use std::fs;

/// Max bytes returned to the model (design: 256 KiB).
pub const READ_OUTPUT_CAP: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct ReadOutputFileArgs {
    /// Path relative to outputs_path.
    pub path: String,
}

pub async fn read_output_file(ctx: &ToolCtx, args: ReadOutputFileArgs) -> Result<String> {
    let path = resolve_output_path(&ctx.outputs_path, &args.path)?;
    if !path.is_file() {
        bail!("file not found: {}", args.path.trim());
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let rel = relative_display(&ctx.outputs_path, &path);
    let truncated = bytes.len() > READ_OUTPUT_CAP;
    let slice = if truncated {
        &bytes[..READ_OUTPUT_CAP]
    } else {
        &bytes
    };
    let text = String::from_utf8_lossy(slice);
    let mut out = format!("# file: {rel} ({} bytes)\n\n", bytes.len());
    out.push_str(&text);
    if truncated {
        out.push_str(&format!(
            "\n\n[truncated: showing first {READ_OUTPUT_CAP} of {} bytes]\n",
            bytes.len()
        ));
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
    async fn read_happy_and_missing() {
        let mut root = std::env::temp_dir();
        root.push(format!("nfs-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("w/thread")).unwrap();
        std::fs::write(root.join("w/thread/a.md"), b"hello summary\n").unwrap();
        let root = root.canonicalize().unwrap();

        let pool = open_in_memory().await.unwrap();
        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let ctx = ToolCtx::new(pool, index, root.clone(), w, week_window(w));

        let text = read_output_file(
            &ctx,
            ReadOutputFileArgs {
                path: "w/thread/a.md".into(),
            },
        )
        .await
        .unwrap();
        assert!(text.contains("hello summary"));
        assert!(text.contains("w/thread/a.md"));

        let err = read_output_file(
            &ctx,
            ReadOutputFileArgs {
                path: "w/thread/nope.md".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("file not found"));

        let err = read_output_file(
            &ctx,
            ReadOutputFileArgs {
                path: "../etc/passwd".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains(".."));

        let _ = std::fs::remove_dir_all(&root);
    }
}
