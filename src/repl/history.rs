/// SQL history file management for the REPL.
use std::path::PathBuf;

use crate::error::{PgCliError, Result};

/// Default history file name, relative to the user's home directory.
const DEFAULT_HISTORY_FILE: &str = ".pgcli-rs_history";
/// Default maximum number of history entries.
const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// Manages a persistent history file for REPL sessions.
///
/// Stores complete SQL statements (not individual lines). Deduplicates
/// consecutive identical entries before appending.
pub struct HistoryManager {
    /// Absolute path to the history file.
    pub path: PathBuf,
    /// Maximum number of entries retained.
    pub max_entries: usize,
    /// In-memory cache of recent entries.
    entries: Vec<String>,
    /// The last entry added (used for deduplication).
    last_entry: Option<String>,
}

impl HistoryManager {
    /// Create a new `HistoryManager` using the default history file location.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Config` if the home directory cannot be determined.
    pub fn new_default() -> Result<Self> {
        let path = dirs::home_dir()
            .ok_or_else(|| PgCliError::Config("cannot determine home directory".to_string()))?
            .join(DEFAULT_HISTORY_FILE);
        Ok(Self::with_path(path, DEFAULT_MAX_ENTRIES))
    }

    /// Create a `HistoryManager` with an explicit path and entry limit.
    pub fn with_path(path: PathBuf, max_entries: usize) -> Self {
        Self {
            path,
            max_entries,
            entries: Vec::new(),
            last_entry: None,
        }
    }

    /// Load history entries from the file into memory.
    ///
    /// Silently succeeds if the file does not yet exist.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Io` if the file exists but cannot be read.
    pub fn load(&mut self) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.path)?;
        self.entries = content.lines().map(|l| l.to_string()).collect();
        // Honour max_entries by keeping only the tail.
        if self.entries.len() > self.max_entries {
            let drain_len = self.entries.len() - self.max_entries;
            self.entries.drain(0..drain_len);
        }
        self.last_entry = self.entries.last().cloned();
        Ok(())
    }

    /// Append `entry` to the history, skipping consecutive duplicates.
    ///
    /// Automatically flushes to disk.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Io` if the file cannot be written.
    pub fn add(&mut self, entry: &str) -> Result<()> {
        let trimmed = entry.trim().to_string();
        if trimmed.is_empty() {
            return Ok(());
        }
        // Skip consecutive duplicate.
        if self.last_entry.as_deref() == Some(&trimmed) {
            return Ok(());
        }
        self.entries.push(trimmed.clone());
        self.last_entry = Some(trimmed);

        // Evict oldest if over the limit.
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }

        self.flush()
    }

    /// Write all in-memory entries to disk.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Io` if the write fails.
    pub fn flush(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = self.entries.join("\n");
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    /// Return the history entries in chronological order (oldest first).
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn temp_manager() -> HistoryManager {
        let f = NamedTempFile::new().unwrap();
        HistoryManager::with_path(f.path().to_path_buf(), 5)
    }

    #[test]
    fn add_and_retrieve() {
        let mut mgr = temp_manager();
        mgr.add("SELECT 1").unwrap();
        mgr.add("SELECT 2").unwrap();
        assert_eq!(mgr.entries().len(), 2);
    }

    #[test]
    fn consecutive_duplicates_are_skipped() {
        let mut mgr = temp_manager();
        mgr.add("SELECT 1").unwrap();
        mgr.add("SELECT 1").unwrap();
        assert_eq!(mgr.entries().len(), 1);
    }

    #[test]
    fn non_consecutive_duplicates_are_kept() {
        let mut mgr = temp_manager();
        mgr.add("SELECT 1").unwrap();
        mgr.add("SELECT 2").unwrap();
        mgr.add("SELECT 1").unwrap();
        assert_eq!(mgr.entries().len(), 3);
    }

    #[test]
    fn max_entries_enforced() {
        let mut mgr = temp_manager(); // max = 5
        for i in 0..10 {
            mgr.add(&format!("SELECT {i}")).unwrap();
        }
        assert_eq!(mgr.entries().len(), 5);
    }

    #[test]
    fn empty_string_not_added() {
        let mut mgr = temp_manager();
        mgr.add("").unwrap();
        mgr.add("   ").unwrap();
        assert_eq!(mgr.entries().len(), 0);
    }

    #[test]
    fn load_round_trip() {
        let f = NamedTempFile::new().unwrap();
        let path = f.path().to_path_buf();
        {
            let mut mgr = HistoryManager::with_path(path.clone(), 100);
            mgr.add("SELECT 1").unwrap();
            mgr.add("SELECT 2").unwrap();
        }
        let mut mgr2 = HistoryManager::with_path(path, 100);
        mgr2.load().unwrap();
        assert_eq!(mgr2.entries().len(), 2);
    }
}
