use algo_trade_core::events::{MarketEvent, SignalDirection, SignalEvent};
use algo_trade_core::traits::Strategy;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::VecDeque;

pub struct QuadMaStrategy {
    symbol: String,
    ma1_period: usize,  // Fastest (e.g., 5)
    ma2_period: usize,  // Fast (e.g., 8)
    ma3_period: usize,  // Slow (e.g., 13)
    ma4_period: usize,  // Slowest (e.g., 21)
    ma1_prices: VecDeque<Decimal>,
    ma2_prices: VecDeque<Decimal>,
    ma3_prices: VecDeque<Decimal>,
    ma4_prices: VecDeque<Decimal>,
    last_signal: Option<SignalDirection>,
}

impl QuadMaStrategy {
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // String cannot be used in const fn
    pub fn new(symbol: String) -> Self {
        // Fibonacci sequence periods
        Self {
            symbol,
            ma1_period: 5,
            ma2_period: 8,
            ma3_period: 13,
            ma4_period: 21,
            ma1_prices: VecDeque::new(),
            ma2_prices: VecDeque::new(),
            ma3_prices: VecDeque::new(),
            ma4_prices: VecDeque::new(),
            last_signal: None,
        }
    }

    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // String cannot be used in const fn
    pub fn with_periods(symbol: String, ma1: usize, ma2: usize, ma3: usize, ma4: usize) -> Self {
        Self {
            symbol,
            ma1_period: ma1,
            ma2_period: ma2,
            ma3_period: ma3,
            ma4_period: ma4,
            ma1_prices: VecDeque::new(),
            ma2_prices: VecDeque::new(),
            ma3_prices: VecDeque::new(),
            ma4_prices: VecDeque::new(),
            last_signal: None,
        }
    }

    fn calculate_ma(prices: &VecDeque<Decimal>) -> Decimal {
        let sum: Decimal = prices.iter().sum();
        sum / Decimal::from(prices.len())
    }

    fn is_bullish_alignment(ma1: Decimal, ma2: Decimal, ma3: Decimal, ma4: Decimal) -> bool {
        // Bullish: MA1 > MA2 > MA3 > MA4 (all MAs in ascending order)
        ma1 > ma2 && ma2 > ma3 && ma3 > ma4
    }

    fn is_bearish_alignment(ma1: Decimal, ma2: Decimal, ma3: Decimal, ma4: Decimal) -> bool {
        // Bearish: MA1 < MA2 < MA3 < MA4 (all MAs in descending order)
        ma1 < ma2 && ma2 < ma3 && ma3 < ma4
    }
}

#[async_trait]
impl Strategy for QuadMaStrategy {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        let (symbol, price) = match event {
            MarketEvent::Bar { symbol, close, .. } => (symbol, close),
            MarketEvent::Trade { symbol, price, .. } => (symbol, price),
            MarketEvent::Quote { .. } => return Ok(None),
        };

        if symbol != &self.symbol {
            return Ok(None);
        }

        // Update all MA price buffers
        self.ma1_prices.push_back(*price);
        self.ma2_prices.push_back(*price);
        self.ma3_prices.push_back(*price);
        self.ma4_prices.push_back(*price);

        // Maintain buffer sizes
        if self.ma1_prices.len() > self.ma1_period {
            self.ma1_prices.pop_front();
        }
        if self.ma2_prices.len() > self.ma2_period {
            self.ma2_prices.pop_front();
        }
        if self.ma3_prices.len() > self.ma3_period {
            self.ma3_prices.pop_front();
        }
        if self.ma4_prices.len() > self.ma4_period {
            self.ma4_prices.pop_front();
        }

        // Wait until all MAs are ready (need longest period)
        if self.ma1_prices.len() < self.ma1_period
            || self.ma2_prices.len() < self.ma2_period
            || self.ma3_prices.len() < self.ma3_period
            || self.ma4_prices.len() < self.ma4_period
        {
            return Ok(None);
        }

        // Calculate all MAs
        let ma1 = Self::calculate_ma(&self.ma1_prices);
        let ma2 = Self::calculate_ma(&self.ma2_prices);
        let ma3 = Self::calculate_ma(&self.ma3_prices);
        let ma4 = Self::calculate_ma(&self.ma4_prices);

        // Determine signal based on alignment
        let new_signal = if Self::is_bullish_alignment(ma1, ma2, ma3, ma4) {
            Some(SignalDirection::Long)
        } else if Self::is_bearish_alignment(ma1, ma2, ma3, ma4) {
            Some(SignalDirection::Short)
        } else {
            None
        };

        // Only emit signal on alignment change (not continuous)
        if new_signal != self.last_signal && new_signal.is_some() {
            let signal = SignalEvent {
                symbol: self.symbol.clone(),
                direction: new_signal.clone().unwrap(),
                strength: 1.0,
                price: *price,
                timestamp: Utc::now(),
            };
            self.last_signal = new_signal;
            Ok(Some(signal))
        } else {
            Ok(None)
        }
    }

    fn name(&self) -> &'static str {
        "Quad MA"
    }
}
