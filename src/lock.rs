//! Exclusive process lock for `summarize-week` (KD13).

use crate::outputs::summarize_lock_path;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Holds an exclusive non-blocking flock for the duration of a summarize run.
/// Released on drop.
pub struct SummarizeLock {
    _file: File,
    path: PathBuf,
}

impl SummarizeLock {
    /// Open `{outputs}/.summarize-week.lock` and take exclusive non-blocking flock.
    /// Fails immediately if another process holds the lock.
    pub fn try_acquire(outputs_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(outputs_path)
            .with_context(|| format!("create outputs_path {}", outputs_path.display()))?;
        let path = summarize_lock_path(outputs_path);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open lock file {}", path.display()))?;
        file.try_lock_exclusive().map_err(|e| {
            warn!(
                summarize_lock_busy = true,
                path = %path.display(),
                error = %e,
                "could not acquire exclusive summarize-week lock"
            );
            anyhow::anyhow!(
                "another summarize-week is running (could not lock {}): {e}",
                path.display()
            )
        })?;
        info!(path = %path.display(), "acquired exclusive summarize-week lock");
        Ok(Self { _file: file, path })
    }
}

impl Drop for SummarizeLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
        info!(path = %self.path.display(), "released summarize-week lock");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "nfs-lock-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn second_lock_fails_while_first_held() {
        let dir = temp_dir();
        let a = SummarizeLock::try_acquire(&dir).expect("first lock");
        let b = SummarizeLock::try_acquire(&dir);
        assert!(b.is_err(), "second lock should fail");
        drop(a);
        let c = SummarizeLock::try_acquire(&dir).expect("lock after release");
        drop(c);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
