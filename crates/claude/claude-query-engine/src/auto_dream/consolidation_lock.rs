//! File-based consolidation lock for auto-dream.
//!
//! The lock file's **mtime** serves double duty as `lastConsolidatedAt`.
//! Stale threshold: 60 minutes (guards against PID reuse).
//! Contention: last-writer-wins; loser bails on re-read.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Name of the lock file within the memory directory.
const LOCK_FILENAME: &str = ".consolidate-lock";

/// Stale threshold in seconds (60 minutes).
const STALE_SECS: u64 = 3600;

/// Consolidation lock state.
#[derive(Debug)]
pub struct ConsolidationLock {
    lock_path: PathBuf,
}

impl ConsolidationLock {
    /// Create a consolidation lock handle for the given memory directory.
    pub fn new(memory_dir: &Path) -> Self {
        Self {
            lock_path: memory_dir.join(LOCK_FILENAME),
        }
    }

    /// Read the `lastConsolidatedAt` timestamp (lock file mtime in ms).
    /// Returns 0 if the lock file does not exist.
    pub fn read_last_consolidated_at(&self) -> u128 {
        fs::metadata(&self.lock_path)
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }

    /// Try to acquire the consolidation lock.
    ///
    /// Writes current PID, verifies we won the race.
    /// Returns `Ok(prior_mtime_ms)` on success.
    pub fn try_acquire(&self) -> anyhow::Result<u128> {
        let prior_ms = self.read_last_consolidated_at();

        // Check for stale lock
        if let Ok(meta) = fs::metadata(&self.lock_path)
            && let Ok(modified) = meta.modified()
            && let Ok(elapsed) = modified.elapsed()
            && elapsed < Duration::from_secs(STALE_SECS)
        {
            // Lock is held and not stale — bail
            anyhow::bail!("consolidation lock is held by another process");
        }

        // Write our PID
        let pid = std::process::id();
        fs::write(&self.lock_path, pid.to_string())?;

        // Verify we won the race (re-read and check PID matches)
        let contents = fs::read_to_string(&self.lock_path).unwrap_or_default();
        if contents.trim() != pid.to_string() {
            anyhow::bail!("lost consolidation lock race");
        }

        Ok(prior_ms)
    }

    /// Roll back the lock to the prior mtime on failure.
    /// If prior was 0, removes the lock file entirely.
    pub fn rollback(&self, prior_mtime_ms: u128) {
        if prior_mtime_ms == 0 {
            let _ = fs::remove_file(&self.lock_path);
        } else {
            // Restore mtime by setting file times
            let prior_time = UNIX_EPOCH + Duration::from_millis(prior_mtime_ms as u64);
            let ft = filetime::FileTime::from_system_time(prior_time);
            let _ = filetime::set_file_mtime(&self.lock_path, ft);
        }
    }

    /// Record a successful consolidation by touching the lock file.
    pub fn record_consolidation(&self) -> anyhow::Result<()> {
        let pid = std::process::id();
        fs::write(&self.lock_path, pid.to_string())?;
        Ok(())
    }

    /// Return the lock file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.lock_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn read_returns_zero_when_no_lock_file() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let lock = ConsolidationLock::new(dir.path());
        assert_eq!(lock.read_last_consolidated_at(), 0);
    }

    #[test]
    fn acquire_creates_lock_file() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let lock = ConsolidationLock::new(dir.path());
        let prior = lock
            .try_acquire()
            .expect("lock acquisition should create lock file");
        assert_eq!(prior, 0);
        assert!(lock.path().exists());
    }

    #[test]
    fn acquire_twice_with_stale_gap_succeeds() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let lock = ConsolidationLock::new(dir.path());
        lock.try_acquire()
            .expect("initial lock acquisition should succeed");
        // Artificially age the lock file past stale threshold
        let old_time = UNIX_EPOCH + Duration::from_secs(1000);
        let ft = filetime::FileTime::from_system_time(old_time);
        filetime::set_file_mtime(lock.path(), ft).expect("lock mtime should be settable");

        // Should succeed because lock is stale
        assert!(lock.try_acquire().is_ok());
    }

    #[test]
    fn rollback_removes_file_when_prior_zero() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let lock = ConsolidationLock::new(dir.path());
        lock.try_acquire()
            .expect("initial lock acquisition should succeed");
        lock.rollback(0);
        assert!(!lock.path().exists());
    }

    #[test]
    fn read_mtime_increases_after_consolidation() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let lock = ConsolidationLock::new(dir.path());
        let before = lock.read_last_consolidated_at();
        thread::sleep(Duration::from_millis(50));
        lock.record_consolidation()
            .expect("consolidation timestamp should be written");
        let after = lock.read_last_consolidated_at();
        assert!(after > before);
    }
}
