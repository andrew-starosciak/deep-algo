use algo_trade_core::events::{MarketEvent, SignalDirection, SignalEvent};
use algo_trade_core::traits::Strategy;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::collections::VecDeque;

pub struct MaCrossoverStrategy {
    symbol: String,
    fast_period: usize,
    slow_period: usize,
    fast_prices: VecDeque<Decimal>,
    slow_prices: VecDeque<Decimal>,
    last_signal: Option<SignalDirection>,
}

impl MaCrossoverStrategy {
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // String cannot be used in const fn
    pub fn new(symbol: String, fast_period: usize, slow_period: usize) -> Self {
        Self {
            symbol,
            fast_period,
            slow_period,
            fast_prices: VecDeque::new(),
            slow_prices: VecDeque::new(),
            last_signal: None,
        }
    }

    fn calculate_ma(prices: &VecDeque<Decimal>) -> Decimal {
        let sum: Decimal = prices.iter().sum();
        sum / Decimal::from(prices.len())
    }
}

#[async_trait]
impl Strategy for MaCrossoverStrategy {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        let (symbol, price) = match event {
            MarketEvent::Bar { symbol, close, .. } => (symbol, close),
            MarketEvent::Trade { symbol, price, .. } => (symbol, price),
            MarketEvent::Quote { .. } => return Ok(None),
        };

        if symbol != &self.symbol {
            return Ok(None);
        }

        self.fast_prices.push_back(*price);
        self.slow_prices.push_back(*price);

        if self.fast_prices.len() > self.fast_period {
            self.fast_prices.pop_front();
        }
        if self.slow_prices.len() > self.slow_period {
            self.slow_prices.pop_front();
        }

        if self.fast_prices.len() < self.fast_period || self.slow_prices.len() < self.slow_period {
            return Ok(None);
        }

        let fast_ma = Self::calculate_ma(&self.fast_prices);
        let slow_ma = Self::calculate_ma(&self.slow_prices);

        let new_signal = match fast_ma.cmp(&slow_ma) {
            Ordering::Greater => Some(SignalDirection::Long),
            Ordering::Less => Some(SignalDirection::Short),
            Ordering::Equal => None,
        };

        // Only emit signal on crossover (direction change)
        if new_signal != self.last_signal && new_signal.is_some() {
            let signal = SignalEvent {
                symbol: self.symbol.clone(),
                direction: new_signal.clone().unwrap(),
                strength: 1.0,
                timestamp: Utc::now(),
            };
            self.last_signal = new_signal;
            Ok(Some(signal))
        } else {
            Ok(None)
        }
    }

    fn name(&self) -> &'static str {
        "MA Crossover"
    }
}
