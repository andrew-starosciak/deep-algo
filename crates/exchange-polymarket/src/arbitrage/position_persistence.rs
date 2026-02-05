//! Position persistence for surviving restarts.
//!
//! This module provides the ability to persist `WindowPositionTracker` state to a JSON file
//! so positions survive restarts. It includes:
//!
//! - Save/load roundtrip functionality
//! - Stale window detection (clearing positions from old windows)
//! - Graceful handling of missing/corrupt files
//!
//! # Example
//!
//! ```ignore
//! use algo_trade_polymarket::arbitrage::position_persistence::PositionPersistence;
//! use std::path::PathBuf;
//!
//! let persistence = PositionPersistence::new(PathBuf::from("/tmp/positions.json"));
//!
//! // Load existing positions (returns default if file missing/corrupt)
//! let tracker = persistence.load(current_window_ms)?;
//!
//! // ... modify tracker ...
//!
//! // Save after changes
//! persistence.save(&tracker)?;
//! ```

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use thiserror::Error;
use tracing::{debug, info, warn};

use super::auto_executor::WindowPositionTracker;
use super::gabagool_detector::OpenPosition;

/// Errors from position persistence operations.
#[derive(Error, Debug)]
pub enum PersistenceError {
    /// IO error reading/writing file.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Persisted position state.
///
/// This is the format saved to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedPosition {
    /// Window start timestamp (ms).
    pub window_start_ms: i64,

    /// YES position if any.
    pub yes_position: Option<OpenPosition>,

    /// NO position if any.
    pub no_position: Option<OpenPosition>,

    /// Total cost invested.
    pub total_cost: Decimal,

    /// Timestamp when this was saved.
    pub saved_at: DateTime<Utc>,
}

impl PersistedPosition {
    /// Creates a new persisted position from a tracker.
    #[must_use]
    pub fn from_tracker(tracker: &WindowPositionTracker) -> Self {
        Self {
            window_start_ms: tracker.window_start_ms,
            yes_position: tracker.yes_position.clone(),
            no_position: tracker.no_position.clone(),
            total_cost: tracker.total_cost,
            saved_at: Utc::now(),
        }
    }

    /// Converts to a tracker.
    #[must_use]
    pub fn into_tracker(self) -> WindowPositionTracker {
        WindowPositionTracker {
            window_start_ms: self.window_start_ms,
            yes_position: self.yes_position,
            no_position: self.no_position,
            total_cost: self.total_cost,
        }
    }

    /// Returns true if this position is for a different (stale) window.
    #[must_use]
    pub fn is_stale(&self, current_window_ms: i64) -> bool {
        self.window_start_ms != current_window_ms
    }
}

/// Handles persisting and loading position state.
#[derive(Debug, Clone)]
pub struct PositionPersistence {
    /// Path to the persistence file.
    path: PathBuf,
}

impl PositionPersistence {
    /// Creates a new persistence handler.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the persistence path.
    #[must_use]
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Saves the position tracker state to disk.
    ///
    /// Creates parent directories if they don't exist.
    pub fn save(&self, tracker: &WindowPositionTracker) -> Result<(), PersistenceError> {
        // Create parent directories if needed
        if let Some(parent) = self.path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        let persisted = PersistedPosition::from_tracker(tracker);
        let file = File::create(&self.path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &persisted)?;

        debug!(
            path = %self.path.display(),
            window_ms = tracker.window_start_ms,
            has_yes = tracker.yes_position.is_some(),
            has_no = tracker.no_position.is_some(),
            "Saved position state"
        );

        Ok(())
    }

    /// Loads position state from disk.
    ///
    /// # Arguments
    /// * `current_window_ms` - The current window timestamp. If the saved position
    ///   is from a different window, it will be discarded and a fresh tracker returned.
    ///
    /// # Returns
    /// - The loaded tracker if file exists and is for current window
    /// - A fresh tracker if file is missing, corrupt, or stale
    pub fn load(&self, current_window_ms: i64) -> Result<WindowPositionTracker, PersistenceError> {
        // Handle missing file
        if !self.path.exists() {
            info!(
                path = %self.path.display(),
                "No persisted position file found, starting fresh"
            );
            return Ok(WindowPositionTracker::new(current_window_ms));
        }

        // Try to load and parse
        let result = self.load_internal();

        match result {
            Ok(persisted) => {
                // Check if stale
                if persisted.is_stale(current_window_ms) {
                    info!(
                        saved_window = persisted.window_start_ms,
                        current_window = current_window_ms,
                        "Loaded position is from stale window, starting fresh"
                    );
                    Ok(WindowPositionTracker::new(current_window_ms))
                } else {
                    info!(
                        window_ms = persisted.window_start_ms,
                        has_yes = persisted.yes_position.is_some(),
                        has_no = persisted.no_position.is_some(),
                        total_cost = %persisted.total_cost,
                        "Loaded persisted position"
                    );
                    Ok(persisted.into_tracker())
                }
            }
            Err(e) => {
                warn!(
                    path = %self.path.display(),
                    error = %e,
                    "Failed to load persisted position, starting fresh"
                );
                Ok(WindowPositionTracker::new(current_window_ms))
            }
        }
    }

    /// Loads without stale checking (internal helper).
    fn load_internal(&self) -> Result<PersistedPosition, PersistenceError> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let persisted: PersistedPosition = serde_json::from_reader(reader)?;
        Ok(persisted)
    }

    /// Loads the raw persisted data without any validation.
    ///
    /// This is useful for inspection/debugging.
    pub fn load_raw(&self) -> Result<PersistedPosition, PersistenceError> {
        self.load_internal()
    }

    /// Deletes the persistence file if it exists.
    pub fn clear(&self) -> Result<(), PersistenceError> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
            debug!(path = %self.path.display(), "Cleared persisted position file");
        }
        Ok(())
    }

    /// Returns true if the persistence file exists.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.path.exists()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitrage::gabagool_detector::GabagoolDirection;
    use rust_decimal_macros::dec;
    use std::io::Write;
    use tempfile::TempDir;

    /// Creates a temp directory and returns path to a positions file.
    fn temp_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("positions.json");
        (dir, path)
    }

    /// Creates a test position.
    fn make_position(direction: GabagoolDirection, price: Decimal) -> OpenPosition {
        OpenPosition {
            direction,
            entry_price: price,
            quantity: dec!(100),
            entry_time_ms: 1000,
            window_start_ms: 900_000,
        }
    }

    // =========================================================================
    // Save/Load Roundtrip Tests
    // =========================================================================

    #[test]
    fn test_save_load_roundtrip_empty_tracker() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        // Create empty tracker
        let tracker = WindowPositionTracker::new(900_000);

        // Save
        persistence.save(&tracker).unwrap();

        // Load with same window
        let loaded = persistence.load(900_000).unwrap();

        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(loaded.yes_position.is_none());
        assert!(loaded.no_position.is_none());
        assert_eq!(loaded.total_cost, Decimal::ZERO);
    }

    #[test]
    fn test_save_load_roundtrip_with_yes_position() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        // Create tracker with YES position
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(make_position(GabagoolDirection::Yes, dec!(0.35)));

        // Save
        persistence.save(&tracker).unwrap();

        // Load with same window
        let loaded = persistence.load(900_000).unwrap();

        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(loaded.yes_position.is_some());
        let yes_pos = loaded.yes_position.unwrap();
        assert_eq!(yes_pos.direction, GabagoolDirection::Yes);
        assert_eq!(yes_pos.entry_price, dec!(0.35));
        assert_eq!(yes_pos.quantity, dec!(100));
        assert!(loaded.no_position.is_none());
        assert_eq!(loaded.total_cost, dec!(35)); // 0.35 * 100
    }

    #[test]
    fn test_save_load_roundtrip_with_no_position() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        // Create tracker with NO position
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(make_position(GabagoolDirection::No, dec!(0.40)));

        // Save
        persistence.save(&tracker).unwrap();

        // Load
        let loaded = persistence.load(900_000).unwrap();

        assert!(loaded.no_position.is_some());
        let no_pos = loaded.no_position.unwrap();
        assert_eq!(no_pos.direction, GabagoolDirection::No);
        assert_eq!(no_pos.entry_price, dec!(0.40));
        assert!(loaded.yes_position.is_none());
    }

    #[test]
    fn test_save_load_roundtrip_hedged_position() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        // Create hedged tracker (both YES and NO)
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(make_position(GabagoolDirection::Yes, dec!(0.35)));
        tracker.record_hedge(make_position(GabagoolDirection::No, dec!(0.60)));

        // Save
        persistence.save(&tracker).unwrap();

        // Load
        let loaded = persistence.load(900_000).unwrap();

        assert!(loaded.yes_position.is_some());
        assert!(loaded.no_position.is_some());
        assert_eq!(loaded.total_cost, dec!(95)); // 35 + 60
        assert!(loaded.is_hedged());
    }

    // =========================================================================
    // Stale Window Detection Tests
    // =========================================================================

    #[test]
    fn test_stale_window_returns_fresh_tracker() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        // Save with window 900_000
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(make_position(GabagoolDirection::Yes, dec!(0.35)));
        persistence.save(&tracker).unwrap();

        // Load with different window 1_800_000 (next 15-min window)
        let loaded = persistence.load(1_800_000).unwrap();

        // Should get fresh tracker for new window
        assert_eq!(loaded.window_start_ms, 1_800_000);
        assert!(loaded.yes_position.is_none());
        assert!(loaded.no_position.is_none());
        assert_eq!(loaded.total_cost, Decimal::ZERO);
    }

    #[test]
    fn test_same_window_preserves_position() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        // Save with window 900_000
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(make_position(GabagoolDirection::Yes, dec!(0.35)));
        persistence.save(&tracker).unwrap();

        // Load with SAME window
        let loaded = persistence.load(900_000).unwrap();

        // Should preserve position
        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(loaded.yes_position.is_some());
        assert_eq!(loaded.total_cost, dec!(35));
    }

    #[test]
    fn test_is_stale_method() {
        let persisted = PersistedPosition {
            window_start_ms: 900_000,
            yes_position: None,
            no_position: None,
            total_cost: Decimal::ZERO,
            saved_at: Utc::now(),
        };

        assert!(!persisted.is_stale(900_000)); // Same window
        assert!(persisted.is_stale(1_800_000)); // Different window
        assert!(persisted.is_stale(0)); // Different window
    }

    // =========================================================================
    // Missing File Handling Tests
    // =========================================================================

    #[test]
    fn test_missing_file_returns_fresh_tracker() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        // File doesn't exist
        assert!(!path.exists());

        // Load should return fresh tracker
        let loaded = persistence.load(900_000).unwrap();

        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(!loaded.has_position());
    }

    #[test]
    fn test_exists_method() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        assert!(!persistence.exists());

        // Save something
        let tracker = WindowPositionTracker::new(900_000);
        persistence.save(&tracker).unwrap();

        assert!(persistence.exists());
    }

    // =========================================================================
    // Corrupt File Handling Tests
    // =========================================================================

    #[test]
    fn test_corrupt_file_returns_fresh_tracker() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        // Write garbage to file
        let mut file = File::create(&path).unwrap();
        file.write_all(b"not valid json {{{").unwrap();

        // Load should return fresh tracker (not error)
        let loaded = persistence.load(900_000).unwrap();

        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(!loaded.has_position());
    }

    #[test]
    fn test_empty_file_returns_fresh_tracker() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        // Create empty file
        File::create(&path).unwrap();

        // Load should return fresh tracker
        let loaded = persistence.load(900_000).unwrap();

        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(!loaded.has_position());
    }

    #[test]
    fn test_partial_json_returns_fresh_tracker() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        // Write partial JSON
        let mut file = File::create(&path).unwrap();
        file.write_all(b"{\"window_start_ms\": 900000, \"yes_position\":").unwrap();

        // Load should return fresh tracker
        let loaded = persistence.load(900_000).unwrap();

        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(!loaded.has_position());
    }

    #[test]
    fn test_wrong_json_structure_returns_fresh_tracker() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        // Write valid JSON but wrong structure
        let mut file = File::create(&path).unwrap();
        file.write_all(b"{\"foo\": \"bar\", \"baz\": 123}").unwrap();

        // Load should return fresh tracker
        let loaded = persistence.load(900_000).unwrap();

        assert_eq!(loaded.window_start_ms, 900_000);
        assert!(!loaded.has_position());
    }

    // =========================================================================
    // Clear Tests
    // =========================================================================

    #[test]
    fn test_clear_removes_file() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        // Save something
        let tracker = WindowPositionTracker::new(900_000);
        persistence.save(&tracker).unwrap();
        assert!(path.exists());

        // Clear
        persistence.clear().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn test_clear_on_nonexistent_file_ok() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        assert!(!path.exists());

        // Should not error
        persistence.clear().unwrap();
    }

    // =========================================================================
    // Directory Creation Tests
    // =========================================================================

    #[test]
    fn test_save_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("deep").join("positions.json");
        let persistence = PositionPersistence::new(path.clone());

        // Parent doesn't exist
        assert!(!path.parent().unwrap().exists());

        // Save should create it
        let tracker = WindowPositionTracker::new(900_000);
        persistence.save(&tracker).unwrap();

        assert!(path.exists());
    }

    // =========================================================================
    // load_raw Tests
    // =========================================================================

    #[test]
    fn test_load_raw_returns_persisted_data() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        // Save
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(make_position(GabagoolDirection::Yes, dec!(0.35)));
        persistence.save(&tracker).unwrap();

        // Load raw
        let raw = persistence.load_raw().unwrap();

        assert_eq!(raw.window_start_ms, 900_000);
        assert!(raw.yes_position.is_some());
        assert!(raw.saved_at <= Utc::now());
    }

    #[test]
    fn test_load_raw_errors_on_missing_file() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path);

        let result = persistence.load_raw();
        assert!(result.is_err());
    }

    // =========================================================================
    // JSON Format Tests
    // =========================================================================

    #[test]
    fn test_json_format_matches_spec() {
        let (_dir, path) = temp_path();
        let persistence = PositionPersistence::new(path.clone());

        // Save with position
        let mut tracker = WindowPositionTracker::new(1234567890000);
        tracker.record_entry(make_position(GabagoolDirection::Yes, dec!(0.35)));
        persistence.save(&tracker).unwrap();

        // Read raw JSON
        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Verify expected fields exist
        assert!(json.get("window_start_ms").is_some());
        assert!(json.get("yes_position").is_some());
        assert!(json.get("no_position").is_some());
        assert!(json.get("total_cost").is_some());
        assert!(json.get("saved_at").is_some());

        // Verify types
        assert!(json["window_start_ms"].is_i64());
        assert!(json["saved_at"].is_string()); // ISO 8601 format
    }

    // =========================================================================
    // PersistedPosition Tests
    // =========================================================================

    #[test]
    fn test_persisted_position_from_tracker() {
        let mut tracker = WindowPositionTracker::new(900_000);
        tracker.record_entry(make_position(GabagoolDirection::Yes, dec!(0.35)));

        let persisted = PersistedPosition::from_tracker(&tracker);

        assert_eq!(persisted.window_start_ms, 900_000);
        assert!(persisted.yes_position.is_some());
        assert!(persisted.no_position.is_none());
        assert_eq!(persisted.total_cost, dec!(35));
        assert!(persisted.saved_at <= Utc::now());
    }

    #[test]
    fn test_persisted_position_into_tracker() {
        let persisted = PersistedPosition {
            window_start_ms: 900_000,
            yes_position: Some(make_position(GabagoolDirection::Yes, dec!(0.35))),
            no_position: None,
            total_cost: dec!(35),
            saved_at: Utc::now(),
        };

        let tracker = persisted.into_tracker();

        assert_eq!(tracker.window_start_ms, 900_000);
        assert!(tracker.yes_position.is_some());
        assert!(tracker.no_position.is_none());
        assert_eq!(tracker.total_cost, dec!(35));
    }
}
