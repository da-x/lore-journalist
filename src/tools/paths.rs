//! Path sandbox for output tools (KD22).

use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

/// Resolve a relative path under `outputs_root` (must already be canonical when possible).
///
/// - Rejects absolute paths and `..` components.
/// - If the target exists, canonicalizes and requires it stay under `outputs_root`.
/// - If missing, returns the logical join without requiring canonicalize (caller reports not found).
pub fn resolve_output_path(outputs_root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    if rel.is_empty() {
        bail!("path is empty");
    }
    // Reject absolute paths (Unix and Windows-style).
    let p = Path::new(rel);
    if p.is_absolute() {
        bail!("absolute paths not allowed: {rel:?}");
    }
    if rel.starts_with('/') {
        bail!("absolute paths not allowed: {rel:?}");
    }

    for c in p.components() {
        match c {
            Component::ParentDir => bail!("'..' not allowed in path: {rel:?}"),
            Component::RootDir | Component::Prefix(_) => {
                bail!("absolute/prefix paths not allowed: {rel:?}")
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }

    // Normalize logical path (drop `.`, reject escape via redundant checks).
    let mut logical = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => logical.push(s),
            Component::CurDir => {}
            _ => unreachable!("filtered above"),
        }
    }

    let candidate = outputs_root.join(&logical);

    if candidate.exists() {
        let canon = candidate
            .canonicalize()
            .with_context(|| format!("canonicalize {}", candidate.display()))?;
        let root_canon = if outputs_root.exists() {
            outputs_root
                .canonicalize()
                .with_context(|| format!("canonicalize root {}", outputs_root.display()))?
        } else {
            outputs_root.to_path_buf()
        };
        if !canon.starts_with(&root_canon) {
            bail!("path escapes outputs_path: {rel:?}");
        }
        Ok(canon)
    } else {
        // Missing: ensure no symlink escape on parents that exist.
        if let Some(parent) = candidate.parent() {
            if parent.exists() {
                let parent_canon = parent
                    .canonicalize()
                    .with_context(|| format!("canonicalize parent {}", parent.display()))?;
                let root_canon = outputs_root
                    .canonicalize()
                    .with_context(|| format!("canonicalize root {}", outputs_root.display()))?;
                if !parent_canon.starts_with(&root_canon) {
                    bail!("path escapes outputs_path: {rel:?}");
                }
            }
        }
        Ok(candidate)
    }
}

/// Relative path from outputs root for display (forward slashes).
pub fn relative_display(outputs_root: &Path, absolute: &Path) -> String {
    let root = outputs_root
        .canonicalize()
        .unwrap_or_else(|_| outputs_root.to_path_buf());
    let abs = absolute
        .canonicalize()
        .unwrap_or_else(|_| absolute.to_path_buf());
    abs.strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| absolute.display().to_string())
}

/// Simple path glob: `*` matches within a single path segment; `**` matches across segments.
pub fn path_glob_match(pattern: &str, rel_path: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    let rel_path = rel_path.trim_start_matches("./");
    glob_match_segments(
        &pattern.split('/').collect::<Vec<_>>(),
        &rel_path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>(),
    )
}

fn glob_match_segments(pat: &[&str], path: &[&str]) -> bool {
    match (pat.first().copied(), path.first().copied()) {
        (None, None) => true,
        (Some("**"), _) => {
            // ** matches zero or more segments
            if pat.len() == 1 {
                return true;
            }
            // Try matching rest at each position
            for i in 0..=path.len() {
                if glob_match_segments(&pat[1..], &path[i..]) {
                    return true;
                }
            }
            false
        }
        (Some(p), Some(s)) => {
            if segment_match(p, s) {
                glob_match_segments(&pat[1..], &path[1..])
            } else {
                false
            }
        }
        (Some(_), None) | (None, Some(_)) => false,
    }
}

fn segment_match(pat: &str, seg: &str) -> bool {
    if pat == "*" {
        return true;
    }
    // Character-level * within segment
    let mut pi = 0;
    let mut si = 0;
    let pb = pat.as_bytes();
    let sb = seg.as_bytes();
    let mut star = None::<(usize, usize)>;
    while si < sb.len() {
        if pi < pb.len() && (pb[pi] == sb[si] || pb[pi] == b'?') {
            pi += 1;
            si += 1;
        } else if pi < pb.len() && pb[pi] == b'*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, mut ss)) = star {
            ss += 1;
            star = Some((sp, ss));
            pi = sp + 1;
            si = ss;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == b'*' {
        pi += 1;
    }
    pi == pb.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "lore-path-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    }

    #[test]
    fn rejects_absolute_and_dotdot() {
        let root = temp_root();
        assert!(resolve_output_path(&root, "/etc/passwd").is_err());
        assert!(resolve_output_path(&root, "../secret").is_err());
        assert!(resolve_output_path(&root, "a/../../x").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn allows_relative_existing_and_missing() {
        let root = temp_root();
        fs::create_dir_all(root.join("2026-07-20/thread")).unwrap();
        fs::write(root.join("2026-07-20/thread/foo.md"), b"hi").unwrap();

        let ok = resolve_output_path(&root, "2026-07-20/thread/foo.md").unwrap();
        assert!(ok.is_file());
        assert!(ok.starts_with(&root));

        let missing = resolve_output_path(&root, "2026-07-20/thread/missing.md").unwrap();
        assert!(!missing.exists());
        assert!(missing.starts_with(&root) || missing.starts_with(root.as_path()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn glob_segment_star() {
        assert!(path_glob_match(
            "*/thread/foo.md",
            "2026-07-20/thread/foo.md"
        ));
        assert!(!path_glob_match(
            "*/thread/foo.md",
            "2026-07-20/messages/foo.md"
        ));
        assert!(path_glob_match("**/*.md", "a/b/c.md"));
        assert!(path_glob_match("2026-07-20/**", "2026-07-20/thread/x.md"));
    }
}
