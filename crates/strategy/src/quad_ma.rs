use algo_trade_core::events::{MarketEvent, SignalDirection, SignalEvent};
use algo_trade_core::traits::Strategy;
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::io::Write;

/// Quad Moving Average strategy with crossover detection and fixed TP/SL.
///
/// Industry-standard implementation using:
/// - **Crossover events**: MA5 crossing MA10 triggers entry consideration
/// - **MA alignment**: Both shorts above/below MA3 (simplified from 4 checks)
/// - **Trend filter**: 100-period MA slope and price position
/// - **Optional volume filter**: 1.5x average volume confirmation (configurable)
/// - **Fixed TP/SL**: 2% take profit, 1% stop loss (2:1 risk/reward)
///
/// # Signal Generation
///
/// **Long Entry**: MA5 crosses above MA10 + both shorts > MA3 + uptrend + volume
/// **Short Entry**: MA5 crosses below MA10 + both shorts < MA3 + downtrend + volume
/// **Exit**: TP/SL hit OR conditions become neutral
///
/// # Configuration
///
/// Default periods: 5/10/20/50 MAs + 100 trend + 1.5x volume
pub struct QuadMaStrategy {
    symbol: String,

    // MA periods
    short_period_1: usize, // default 5
    short_period_2: usize, // default 10
    long_period_1: usize,  // default 20
    long_period_2: usize,  // default 50
    trend_period: usize,   // default 100

    // Filters
    volume_filter_enabled: bool, // default true
    volume_factor: f64,          // default 1.5
    take_profit_pct: f64,        // default 0.02 (2%)
    stop_loss_pct: f64,          // default 0.01 (1%)

    // Price buffers for MAs
    short_1_prices: VecDeque<Decimal>,
    short_2_prices: VecDeque<Decimal>,
    long_1_prices: VecDeque<Decimal>,
    long_2_prices: VecDeque<Decimal>,
    trend_prices: VecDeque<Decimal>,

    // Volume buffers
    volumes: VecDeque<Decimal>,

    // For trend slope calculation
    prev_trend_ma: Option<Decimal>,

    // For crossover detection
    prev_short_1: Option<Decimal>,
    prev_short_2: Option<Decimal>,
    prev_long_1: Option<Decimal>,
    prev_long_2: Option<Decimal>,

    last_signal: Option<SignalDirection>,
    entry_price: Option<Decimal>, // Track entry price for TP/SL

    // Reversal confirmation to reduce whipsaws
    reversal_confirmation_bars: usize, // How many bars to confirm opposite signal (default 2)
    bars_since_entry: usize,           // Bars since last position entry
    pending_reversal: Option<SignalDirection>, // Opposite signal waiting for confirmation
    pending_reversal_count: usize,     // How many consecutive bars opposite signal persisted
}

impl QuadMaStrategy {
    /// Creates a new `QuadMaStrategy` with Python default parameters.
    ///
    /// Defaults: `short_1=5`, `short_2=10`, `long_1=20`, `long_2=50`, `trend=100`,
    /// `volume_factor=1.5`, `take_profit=2%`, `stop_loss=1%`
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // String cannot be used in const fn
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            short_period_1: 5,
            short_period_2: 10,
            long_period_1: 20,
            long_period_2: 50,
            trend_period: 100,
            volume_filter_enabled: true,
            volume_factor: 1.5,
            take_profit_pct: 0.02,
            stop_loss_pct: 0.01,
            short_1_prices: VecDeque::new(),
            short_2_prices: VecDeque::new(),
            long_1_prices: VecDeque::new(),
            long_2_prices: VecDeque::new(),
            trend_prices: VecDeque::new(),
            volumes: VecDeque::new(),
            prev_trend_ma: None,
            prev_short_1: None,
            prev_short_2: None,
            prev_long_1: None,
            prev_long_2: None,
            last_signal: None,
            entry_price: None,
            reversal_confirmation_bars: 2,
            bars_since_entry: 0,
            pending_reversal: None,
            pending_reversal_count: 0,
        }
    }

    /// Creates a `QuadMaStrategy` with custom MA periods (for TUI compatibility).
    ///
    /// Maps old 4-period API to new structure:
    /// - `ma1` → `short_period_1`
    /// - `ma2` → `short_period_2`
    /// - `ma3` → `long_period_1`
    /// - `ma4` → `long_period_2`
    ///
    /// Uses default `trend_period=100`, `volume_factor=1.5`, etc.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // String cannot be used in const fn
    pub fn with_periods(symbol: String, ma1: usize, ma2: usize, ma3: usize, ma4: usize) -> Self {
        Self {
            symbol,
            short_period_1: ma1,
            short_period_2: ma2,
            long_period_1: ma3,
            long_period_2: ma4,
            trend_period: 100,
            volume_filter_enabled: true,
            volume_factor: 1.5,
            take_profit_pct: 0.02,
            stop_loss_pct: 0.01,
            short_1_prices: VecDeque::new(),
            short_2_prices: VecDeque::new(),
            long_1_prices: VecDeque::new(),
            long_2_prices: VecDeque::new(),
            trend_prices: VecDeque::new(),
            volumes: VecDeque::new(),
            prev_trend_ma: None,
            prev_short_1: None,
            prev_short_2: None,
            prev_long_1: None,
            prev_long_2: None,
            last_signal: None,
            entry_price: None,
            reversal_confirmation_bars: 2,
            bars_since_entry: 0,
            pending_reversal: None,
            pending_reversal_count: 0,
        }
    }

    /// Creates a `QuadMaStrategy` with full custom configuration.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // String cannot be used in const fn
    #[allow(clippy::too_many_arguments)]
    pub fn with_full_config(
        symbol: String,
        short_1: usize,
        short_2: usize,
        long_1: usize,
        long_2: usize,
        trend: usize,
        volume_filter_enabled: bool,
        volume_factor: f64,
        take_profit_pct: f64,
        stop_loss_pct: f64,
        reversal_confirmation_bars: usize,
    ) -> Self {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("quad_ma_debug.log")
        {
            let _ = writeln!(
                file,
                "[QuadMA CONFIG] {symbol} - MA: {short_1}/{short_2}/{long_1}/{long_2}, trend={trend}, vol_filter={volume_filter_enabled}, vol_factor={volume_factor:.2}, tp={take_profit_pct:.4}, sl={stop_loss_pct:.4}"
            );
        }
        Self {
            symbol,
            short_period_1: short_1,
            short_period_2: short_2,
            long_period_1: long_1,
            long_period_2: long_2,
            trend_period: trend,
            volume_filter_enabled,
            volume_factor,
            take_profit_pct,
            stop_loss_pct,
            short_1_prices: VecDeque::new(),
            short_2_prices: VecDeque::new(),
            long_1_prices: VecDeque::new(),
            long_2_prices: VecDeque::new(),
            trend_prices: VecDeque::new(),
            volumes: VecDeque::new(),
            prev_trend_ma: None,
            prev_short_1: None,
            prev_short_2: None,
            prev_long_1: None,
            prev_long_2: None,
            last_signal: None,
            entry_price: None,
            reversal_confirmation_bars,
            bars_since_entry: 0,
            pending_reversal: None,
            pending_reversal_count: 0,
        }
    }

    fn calculate_ma(prices: &VecDeque<Decimal>) -> Decimal {
        let sum: Decimal = prices.iter().sum();
        sum / Decimal::from(prices.len())
    }

    /// Updates all price and volume buffers, maintaining max sizes
    fn update_buffers(&mut self, price: Decimal, volume: Decimal) {
        // Update all price buffers
        self.short_1_prices.push_back(price);
        self.short_2_prices.push_back(price);
        self.long_1_prices.push_back(price);
        self.long_2_prices.push_back(price);
        self.trend_prices.push_back(price);

        // Update volume buffer
        self.volumes.push_back(volume);

        // Maintain buffer sizes
        if self.short_1_prices.len() > self.short_period_1 {
            self.short_1_prices.pop_front();
        }
        if self.short_2_prices.len() > self.short_period_2 {
            self.short_2_prices.pop_front();
        }
        if self.long_1_prices.len() > self.long_period_1 {
            self.long_1_prices.pop_front();
        }
        if self.long_2_prices.len() > self.long_period_2 {
            self.long_2_prices.pop_front();
        }
        if self.trend_prices.len() > self.trend_period {
            self.trend_prices.pop_front();
        }
        if self.volumes.len() > self.long_period_2 {
            self.volumes.pop_front();
        }
    }

    /// Check if conditions are met for entering a long position.
    ///
    /// Entry requirements (all must be true):
    /// - **Crossover event**: MA5 crossed above MA10 (dynamic trigger)
    /// - **MA alignment**: Both shorts above MA3 (bullish structure)
    /// - **Trend filter**: Trend MA slope positive AND price above trend MA
    /// - **Volume filter** (optional): Current volume > average × `volume_factor`
    #[allow(clippy::too_many_arguments)]
    fn should_enter_long(
        short_1: Decimal,
        short_2: Decimal,
        long_1: Decimal,
        long_2: Decimal,
        trend_ma: Decimal,
        prev_trend_ma: Decimal,
        price: Decimal,
        volume: Decimal,
        avg_volume: Decimal,
        volume_factor: Decimal,
        volume_filter_enabled: bool,
        prev_short_1: Option<Decimal>,
        prev_short_2: Option<Decimal>,
    ) -> bool {
        // Check for bullish crossover (MA5 crossing above MA10)
        let has_crossover =
            Self::detect_crossover(prev_short_1, short_1, prev_short_2, short_2) == Some(true);

        if !has_crossover {
            return false;
        }

        // MA alignment: both shorts above both longs (4 checks - stricter filter)
        let bullish = short_1 > long_1 && short_1 > long_2 && short_2 > long_1 && short_2 > long_2;

        // Trend filter: slope positive and price above trend
        let trend_slope = trend_ma - prev_trend_ma;
        let in_uptrend = trend_slope > Decimal::ZERO && price > trend_ma;

        // Volume filter (optional)
        let volume_ok = if volume_filter_enabled {
            volume > avg_volume * volume_factor
        } else {
            true
        };

        bullish && in_uptrend && volume_ok
    }

    /// Check if conditions are met for entering a short position.
    ///
    /// Entry requirements (all must be true):
    /// - **Crossover event**: MA5 crossed below MA10 (dynamic trigger)
    /// - **MA alignment**: Both shorts below MA3 (bearish structure)
    /// - **Trend filter**: Trend MA slope negative AND price below trend MA
    /// - **Volume filter** (optional): Current volume > average × `volume_factor`
    #[allow(clippy::too_many_arguments)]
    fn should_enter_short(
        short_1: Decimal,
        short_2: Decimal,
        long_1: Decimal,
        long_2: Decimal,
        trend_ma: Decimal,
        prev_trend_ma: Decimal,
        price: Decimal,
        volume: Decimal,
        avg_volume: Decimal,
        volume_factor: Decimal,
        volume_filter_enabled: bool,
        prev_short_1: Option<Decimal>,
        prev_short_2: Option<Decimal>,
    ) -> bool {
        // Check for bearish crossover (MA5 crossing below MA10)
        let has_crossover =
            Self::detect_crossover(prev_short_1, short_1, prev_short_2, short_2) == Some(false);

        if !has_crossover {
            return false;
        }

        // MA alignment: both shorts below both longs (4 checks - stricter filter)
        let bearish = short_1 < long_1 && short_1 < long_2 && short_2 < long_1 && short_2 < long_2;

        // Trend filter: slope negative and price below trend
        let trend_slope = trend_ma - prev_trend_ma;
        let in_downtrend = trend_slope < Decimal::ZERO && price < trend_ma;

        // Volume filter (optional)
        let volume_ok = if volume_filter_enabled {
            volume > avg_volume * volume_factor
        } else {
            true
        };

        bearish && in_downtrend && volume_ok
    }

    /// Detects if MA1 crossed above or below MA2 between previous and current bars.
    ///
    /// Industry-standard crossover detection comparing relative positions across
    /// consecutive bars to identify dynamic momentum shifts (not static alignment).
    ///
    /// # Arguments
    ///
    /// * `prev_ma1` - Previous value of first MA (`None` if insufficient data)
    /// * `curr_ma1` - Current value of first MA
    /// * `prev_ma2` - Previous value of second MA (`None` if insufficient data)
    /// * `curr_ma2` - Current value of second MA
    ///
    /// # Returns
    ///
    /// * `Some(true)` - Bullish crossover (MA1 crossed **above** MA2)
    /// * `Some(false)` - Bearish crossover (MA1 crossed **below** MA2)
    /// * `None` - No crossover detected OR insufficient historical data
    fn detect_crossover(
        prev_ma1: Option<Decimal>,
        curr_ma1: Decimal,
        prev_ma2: Option<Decimal>,
        curr_ma2: Decimal,
    ) -> Option<bool> {
        match (prev_ma1, prev_ma2) {
            (Some(prev1), Some(prev2)) => {
                let was_below = prev1 <= prev2;
                let is_above = curr_ma1 > curr_ma2;
                let was_above = prev1 > prev2;
                let is_below = curr_ma1 <= curr_ma2;

                if was_below && is_above {
                    Some(true) // Bullish crossover
                } else if was_above && is_below {
                    Some(false) // Bearish crossover
                } else {
                    None // No crossover
                }
            }
            _ => None, // Insufficient historical data
        }
    }
}

#[async_trait]
impl Strategy for QuadMaStrategy {
    #[allow(clippy::too_many_lines)]
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        // Only process Bar events (need volume data)
        let MarketEvent::Bar {
            symbol,
            close: price,
            volume,
            ..
        } = event
        else {
            return Ok(None);
        };

        if symbol != &self.symbol {
            return Ok(None);
        }

        // Update all buffers
        self.update_buffers(*price, *volume);

        // Wait until longest period ready (trend_period)
        if self.trend_prices.len() < self.trend_period {
            return Ok(None);
        }

        // Wait until we have at least 2 trend values for slope calculation
        if self.prev_trend_ma.is_none() && self.trend_prices.len() < self.trend_period + 1 {
            // Calculate first trend MA and store it
            let trend_ma = Self::calculate_ma(&self.trend_prices);
            self.prev_trend_ma = Some(trend_ma);
            return Ok(None);
        }

        // Calculate all MAs
        let short_1 = Self::calculate_ma(&self.short_1_prices);
        let short_2 = Self::calculate_ma(&self.short_2_prices);
        let long_1 = Self::calculate_ma(&self.long_1_prices);
        let long_2 = Self::calculate_ma(&self.long_2_prices);
        let trend_ma = Self::calculate_ma(&self.trend_prices);

        // Calculate volume MA (use long_period_2 like Python)
        let avg_volume = if self.volumes.len() >= self.long_period_2 {
            Self::calculate_ma(&self.volumes)
        } else {
            return Ok(None); // Not enough volume data yet
        };

        // Get previous trend MA for slope calculation
        let prev_trend = self.prev_trend_ma.unwrap_or(trend_ma);

        // Update previous trend MA for next iteration
        self.prev_trend_ma = Some(trend_ma);

        // Convert volume_factor to Decimal
        let volume_factor = Decimal::from_f64(self.volume_factor)
            .unwrap_or_else(|| Decimal::from(15) / Decimal::from(10));

        // Check TP/SL if in a position
        if let (Some(entry), Some(direction)) = (self.entry_price, &self.last_signal) {
            let tp_pct = Decimal::from_f64(self.take_profit_pct)
                .unwrap_or_else(|| Decimal::from(2) / Decimal::from(100));
            let sl_pct = Decimal::from_f64(self.stop_loss_pct)
                .unwrap_or_else(|| Decimal::from(1) / Decimal::from(100));

            // Debug: Log TP/SL check
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("quad_ma_debug.log")
            {
                let profit = match direction {
                    SignalDirection::Long => (*price - entry) / entry,
                    SignalDirection::Short => (entry - *price) / entry,
                    SignalDirection::Exit => Decimal::ZERO,
                };
                let _ = writeln!(
                    file,
                    "[QuadMA CHECK] {} - Entry: {}, Current: {}, Profit: {:.6}, TP: {:.6}, SL: {:.6}",
                    self.symbol, entry, price, profit, tp_pct, sl_pct
                );
            }

            let hit_tp_or_sl = match direction {
                SignalDirection::Long => {
                    let profit = (*price - entry) / entry;
                    profit >= tp_pct || profit <= -sl_pct
                }
                SignalDirection::Short => {
                    let profit = (entry - *price) / entry;
                    profit >= tp_pct || profit <= -sl_pct
                }
                SignalDirection::Exit => false, // Already exiting
            };

            if hit_tp_or_sl {
                let profit = match direction {
                    SignalDirection::Long => (*price - entry) / entry,
                    SignalDirection::Short => (entry - *price) / entry,
                    SignalDirection::Exit => Decimal::ZERO,
                };
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("quad_ma_debug.log")
                {
                    let _ = writeln!(
                        file,
                        "[QuadMA TP/SL] {} - Entry: {}, Current: {}, Profit: {:.4}, TP: {:.4}, SL: {:.4} - EXIT TRIGGERED",
                        self.symbol, entry, price, profit, tp_pct, sl_pct
                    );
                }
                self.entry_price = None;
                self.last_signal = None;
                return Ok(Some(SignalEvent {
                    symbol: self.symbol.clone(),
                    direction: SignalDirection::Exit,
                    strength: 1.0,
                    price: *price,
                    timestamp: event.timestamp(),
                }));
            }
        }

        // Check entry conditions
        let should_long = Self::should_enter_long(
            short_1,
            short_2,
            long_1,
            long_2,
            trend_ma,
            prev_trend,
            *price,
            *volume,
            avg_volume,
            volume_factor,
            self.volume_filter_enabled,
            self.prev_short_1,
            self.prev_short_2,
        );

        let should_short = Self::should_enter_short(
            short_1,
            short_2,
            long_1,
            long_2,
            trend_ma,
            prev_trend,
            *price,
            *volume,
            avg_volume,
            volume_factor,
            self.volume_filter_enabled,
            self.prev_short_1,
            self.prev_short_2,
        );

        // Determine desired signal based on entry conditions
        let desired_signal = if should_long {
            Some(SignalDirection::Long)
        } else if should_short {
            Some(SignalDirection::Short)
        } else {
            // Neutral: if already in position, keep it (only exit via TP/SL)
            // If flat, stay flat
            self.last_signal.clone()
        };

        // Store current MAs as previous values for NEXT iteration's crossover detection
        self.prev_short_1 = Some(short_1);
        self.prev_short_2 = Some(short_2);
        self.prev_long_1 = Some(long_1);
        self.prev_long_2 = Some(long_2);

        // Increment bars since entry if in a position
        if self.last_signal.is_some() {
            self.bars_since_entry += 1;
        }

        // Determine if this is a reversal (opposite direction) or new entry
        let is_reversal = matches!(
            (&self.last_signal, &desired_signal),
            (Some(SignalDirection::Long), Some(SignalDirection::Short))
                | (Some(SignalDirection::Short), Some(SignalDirection::Long))
        );

        // Apply reversal confirmation logic
        let final_signal = if is_reversal {
            // Check if this is a new reversal or continuation of pending reversal
            if self.pending_reversal == desired_signal {
                // Same reversal signal - increment count
                self.pending_reversal_count += 1;

                // Check if we've reached confirmation threshold
                if self.pending_reversal_count >= self.reversal_confirmation_bars {
                    // Reversal confirmed - allow flip
                    self.pending_reversal = None;
                    self.pending_reversal_count = 0;
                    desired_signal
                } else {
                    // Not confirmed yet - hold current position
                    self.last_signal.clone()
                }
            } else {
                // New reversal signal - start confirmation counter
                self.pending_reversal.clone_from(&desired_signal);
                self.pending_reversal_count = 1;
                self.last_signal.clone()
            }
        } else {
            // Not a reversal - reset pending and allow immediate entry
            self.pending_reversal = None;
            self.pending_reversal_count = 0;
            desired_signal
        };

        // Only generate signal event if direction actually changed
        if final_signal == self.last_signal {
            // No change, no signal
            Ok(None)
        } else {
            self.last_signal.clone_from(&final_signal);
            self.bars_since_entry = 0; // Reset counter on new position

            if let Some(direction) = final_signal {
                self.entry_price = Some(*price);
                Ok(Some(SignalEvent {
                    symbol: self.symbol.clone(),
                    direction,
                    strength: 1.0,
                    price: *price,
                    timestamp: event.timestamp(),
                }))
            } else {
                // Should never happen with the logic above
                Ok(None)
            }
        }
    }

    fn name(&self) -> &'static str {
        "Quad MA (Python)"
    }
}
