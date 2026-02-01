//! Polymarket 15-minute window timing utilities.
//!
//! Polymarket BTC binary options settle at fixed 15-minute intervals:
//! - :00, :15, :30, :45 of each hour
//!
//! This module provides utilities to:
//! - Determine the current window
//! - Calculate time remaining in window
//! - Decide if we should trade this window or wait

use chrono::{DateTime, Duration, Timelike, Utc};

/// A 15-minute trading window on Polymarket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradingWindow {
    /// Start of the window
    pub start: DateTime<Utc>,
    /// End of the window (settlement time)
    pub end: DateTime<Utc>,
    /// Window duration (always 15 minutes for Polymarket BTC)
    pub duration: Duration,
}

impl TradingWindow {
    /// Creates a new trading window.
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            start,
            end,
            duration: end - start,
        }
    }

    /// Returns the settlement time (same as end).
    #[allow(dead_code)]
    pub fn settlement_time(&self) -> DateTime<Utc> {
        self.end
    }

    /// Calculates time remaining in this window from the given timestamp.
    pub fn time_remaining(&self, now: DateTime<Utc>) -> Duration {
        if now >= self.end {
            Duration::zero()
        } else if now < self.start {
            self.duration
        } else {
            self.end - now
        }
    }

    /// Calculates the offset into this window from the given timestamp.
    pub fn time_elapsed(&self, now: DateTime<Utc>) -> Duration {
        if now <= self.start {
            Duration::zero()
        } else if now >= self.end {
            self.duration
        } else {
            now - self.start
        }
    }

    /// Returns true if the given timestamp is within this window.
    #[allow(dead_code)]
    pub fn contains(&self, now: DateTime<Utc>) -> bool {
        now >= self.start && now < self.end
    }

    /// Returns the percentage of the window that has elapsed (0.0 to 1.0).
    pub fn progress(&self, now: DateTime<Utc>) -> f64 {
        let elapsed = self.time_elapsed(now);
        elapsed.num_milliseconds() as f64 / self.duration.num_milliseconds() as f64
    }
}

/// Window timing calculator for Polymarket 15-minute binaries.
#[derive(Debug, Clone)]
pub struct WindowTimer {
    /// Window duration in minutes (15 for Polymarket BTC)
    window_minutes: i64,
    /// Minimum time remaining to enter a trade (cutoff)
    entry_cutoff: Duration,
}

impl Default for WindowTimer {
    fn default() -> Self {
        Self {
            window_minutes: 15,
            entry_cutoff: Duration::minutes(2), // Don't enter with < 2 mins remaining
        }
    }
}

impl WindowTimer {
    /// Creates a new window timer with custom settings.
    pub fn new(window_minutes: i64, entry_cutoff_mins: i64) -> Self {
        Self {
            window_minutes,
            entry_cutoff: Duration::minutes(entry_cutoff_mins),
        }
    }

    /// Gets the current trading window for the given timestamp.
    ///
    /// Windows align to clock boundaries:
    /// - For 15-min windows: :00, :15, :30, :45
    ///
    /// # Example
    /// ```
    /// use chrono::{TimeZone, Utc};
    ///
    /// let timer = WindowTimer::default();
    /// let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 7, 32).unwrap();
    /// let window = timer.current_window(now);
    ///
    /// assert_eq!(window.start.minute(), 0);  // 14:00
    /// assert_eq!(window.end.minute(), 15);   // 14:15
    /// ```
    pub fn current_window(&self, now: DateTime<Utc>) -> TradingWindow {
        // Calculate which window we're in based on minutes
        let minute = now.minute() as i64;
        let window_start_minute = (minute / self.window_minutes) * self.window_minutes;

        // Build window start time (truncate to window boundary)
        let start = now
            .with_minute(window_start_minute as u32)
            .unwrap()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();

        let end = start + Duration::minutes(self.window_minutes);

        TradingWindow::new(start, end)
    }

    /// Gets the next trading window after the given timestamp.
    pub fn next_window(&self, now: DateTime<Utc>) -> TradingWindow {
        let current = self.current_window(now);
        TradingWindow::new(
            current.end,
            current.end + Duration::minutes(self.window_minutes),
        )
    }

    /// Determines if we should trade the current window or wait.
    ///
    /// Returns `Some(window)` if we should trade, `None` if we should wait.
    #[allow(dead_code)]
    pub fn should_trade_now(&self, now: DateTime<Utc>) -> Option<TradingWindow> {
        let window = self.current_window(now);
        let remaining = window.time_remaining(now);

        if remaining >= self.entry_cutoff {
            Some(window)
        } else {
            None
        }
    }

    /// Gets the window to trade (current if enough time, otherwise next).
    #[allow(dead_code)]
    pub fn get_tradeable_window(&self, now: DateTime<Utc>) -> (TradingWindow, bool) {
        let current = self.current_window(now);
        let remaining = current.time_remaining(now);

        if remaining >= self.entry_cutoff {
            (current, false) // Trade current window, not waiting
        } else {
            (self.next_window(now), true) // Wait for next window
        }
    }

    /// Calculates how long until the next tradeable window opens.
    ///
    /// If current window is tradeable, returns 0.
    /// Otherwise returns time until next window starts.
    #[allow(dead_code)]
    pub fn time_until_next_entry(&self, now: DateTime<Utc>) -> Duration {
        let current = self.current_window(now);
        let remaining = current.time_remaining(now);

        if remaining >= self.entry_cutoff {
            Duration::zero()
        } else {
            remaining // Wait until current window ends
        }
    }

    /// Returns a human-readable status of the current window.
    pub fn status(&self, now: DateTime<Utc>) -> WindowStatus {
        let window = self.current_window(now);
        let remaining = window.time_remaining(now);
        let can_trade = remaining >= self.entry_cutoff;

        WindowStatus {
            window_start: window.start,
            window_end: window.end,
            time_remaining: remaining,
            can_trade,
            progress_pct: window.progress(now) * 100.0,
        }
    }
}

/// Status of the current trading window.
#[derive(Debug, Clone)]
pub struct WindowStatus {
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub time_remaining: Duration,
    pub can_trade: bool,
    pub progress_pct: f64,
}

impl std::fmt::Display for WindowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let remaining_secs = self.time_remaining.num_seconds();
        let mins = remaining_secs / 60;
        let secs = remaining_secs % 60;

        write!(
            f,
            "Window {:02}:{:02} → {:02}:{:02} | {:.0}% | {}m {}s remaining | {}",
            self.window_start.hour(),
            self.window_start.minute(),
            self.window_end.hour(),
            self.window_end.minute(),
            self.progress_pct,
            mins,
            secs,
            if self.can_trade {
                "CAN TRADE"
            } else {
                "WAIT FOR NEXT"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_current_window_at_start() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 0, 0).unwrap();

        let window = timer.current_window(now);

        assert_eq!(window.start.hour(), 14);
        assert_eq!(window.start.minute(), 0);
        assert_eq!(window.end.hour(), 14);
        assert_eq!(window.end.minute(), 15);
    }

    #[test]
    fn test_current_window_mid_window() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 7, 32).unwrap();

        let window = timer.current_window(now);

        assert_eq!(window.start.hour(), 14);
        assert_eq!(window.start.minute(), 0);
        assert_eq!(window.end.hour(), 14);
        assert_eq!(window.end.minute(), 15);
    }

    #[test]
    fn test_current_window_near_end() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 14, 30).unwrap();

        let window = timer.current_window(now);

        assert_eq!(window.start.minute(), 0);
        assert_eq!(window.end.minute(), 15);
    }

    #[test]
    fn test_current_window_second_window() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 22, 0).unwrap();

        let window = timer.current_window(now);

        assert_eq!(window.start.minute(), 15);
        assert_eq!(window.end.minute(), 30);
    }

    #[test]
    fn test_current_window_third_window() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 35, 0).unwrap();

        let window = timer.current_window(now);

        assert_eq!(window.start.minute(), 30);
        assert_eq!(window.end.minute(), 45);
    }

    #[test]
    fn test_current_window_fourth_window() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 55, 0).unwrap();

        let window = timer.current_window(now);

        assert_eq!(window.start.minute(), 45);
        assert_eq!(window.end.hour(), 15);
        assert_eq!(window.end.minute(), 0);
    }

    #[test]
    fn test_time_remaining() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 10, 0).unwrap();

        let window = timer.current_window(now);
        let remaining = window.time_remaining(now);

        assert_eq!(remaining.num_minutes(), 5);
    }

    #[test]
    fn test_should_trade_now_enough_time() {
        let timer = WindowTimer::new(15, 2); // 2 min cutoff
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 10, 0).unwrap();

        let result = timer.should_trade_now(now);

        assert!(result.is_some());
    }

    #[test]
    fn test_should_trade_now_not_enough_time() {
        let timer = WindowTimer::new(15, 2); // 2 min cutoff
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 14, 0).unwrap();

        let result = timer.should_trade_now(now);

        assert!(result.is_none()); // Only 1 min left, below 2 min cutoff
    }

    #[test]
    fn test_next_window() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 7, 0).unwrap();

        let next = timer.next_window(now);

        assert_eq!(next.start.minute(), 15);
        assert_eq!(next.end.minute(), 30);
    }

    #[test]
    fn test_get_tradeable_window_current() {
        let timer = WindowTimer::new(15, 2);
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 5, 0).unwrap();

        let (window, waiting) = timer.get_tradeable_window(now);

        assert!(!waiting);
        assert_eq!(window.start.minute(), 0);
    }

    #[test]
    fn test_get_tradeable_window_next() {
        let timer = WindowTimer::new(15, 2);
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 14, 0).unwrap();

        let (window, waiting) = timer.get_tradeable_window(now);

        assert!(waiting);
        assert_eq!(window.start.minute(), 15);
    }

    #[test]
    fn test_window_progress() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 7, 30).unwrap();

        let window = timer.current_window(now);
        let progress = window.progress(now);

        assert!((progress - 0.5).abs() < 0.01); // ~50% through window
    }

    #[test]
    fn test_status_display() {
        let timer = WindowTimer::default();
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 7, 30).unwrap();

        let status = timer.status(now);

        assert!(status.can_trade);
        assert!((status.progress_pct - 50.0).abs() < 1.0);
        assert!(status.time_remaining.num_minutes() >= 7);
    }

    #[test]
    fn test_time_until_next_entry_can_trade() {
        let timer = WindowTimer::new(15, 2);
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 5, 0).unwrap();

        let wait = timer.time_until_next_entry(now);

        assert_eq!(wait.num_seconds(), 0);
    }

    #[test]
    fn test_time_until_next_entry_must_wait() {
        let timer = WindowTimer::new(15, 2);
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 14, 14, 0).unwrap();

        let wait = timer.time_until_next_entry(now);

        assert_eq!(wait.num_minutes(), 1); // Wait 1 minute until 14:15
    }
}
