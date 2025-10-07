use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub total_return: Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: Decimal,
    pub num_trades: usize,
    pub win_rate: f64,
}

impl PerformanceMetrics {
    /// Converts performance metrics to a database record format.
    ///
    /// # Arguments
    /// * `timestamp` - The timestamp when the backtest was run
    /// * `symbol` - The trading symbol (e.g., "BTC")
    /// * `exchange` - The exchange name (e.g., "hyperliquid")
    /// * `strategy_name` - The name of the strategy (e.g., `quad_ma`)
    /// * `total_pnl` - The total profit/loss from the backtest
    #[must_use]
    pub const fn to_db_record(
        &self,
        timestamp: DateTime<Utc>,
        symbol: String,
        exchange: String,
        strategy_name: String,
        total_pnl: Decimal,
    ) -> BacktestResultRecord {
        BacktestResultRecord {
            timestamp,
            symbol,
            exchange,
            strategy_name,
            sharpe_ratio: self.sharpe_ratio,
            sortino_ratio: None, // Can be added later
            total_pnl,
            total_return: self.total_return,
            win_rate: self.win_rate,
            max_drawdown: self.max_drawdown,
            num_trades: self.num_trades,
            parameters: None, // Can be added later for strategy params
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResultRecord {
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub exchange: String,
    pub strategy_name: String,
    pub sharpe_ratio: f64,
    pub sortino_ratio: Option<f64>,
    pub total_pnl: Decimal,
    pub total_return: Decimal,
    pub win_rate: f64,
    pub max_drawdown: Decimal,
    pub num_trades: usize,
    pub parameters: Option<serde_json::Value>,
}

pub struct MetricsCalculator {
    returns: Vec<Decimal>,
    equity_curve: Vec<Decimal>,
    wins: usize,
    losses: usize,
}

impl MetricsCalculator {
    /// Creates a new `MetricsCalculator` with the specified initial capital.
    #[must_use]
    pub fn new(initial_capital: Decimal) -> Self {
        Self {
            returns: Vec::new(),
            equity_curve: vec![initial_capital],
            wins: 0,
            losses: 0,
        }
    }

    /// Adds a trade to the performance tracking.
    ///
    /// # Panics
    ///
    /// Panics if `equity_curve` is empty (should never happen with proper initialization).
    pub fn add_trade(&mut self, pnl: Decimal) {
        let current_equity = *self.equity_curve.last().unwrap();
        let new_equity = current_equity + pnl;

        self.equity_curve.push(new_equity);

        if pnl > Decimal::ZERO {
            self.wins += 1;
        } else if pnl < Decimal::ZERO {
            self.losses += 1;
        }

        let return_pct = pnl / current_equity;
        self.returns.push(return_pct);
    }

    /// Calculates and returns performance metrics.
    ///
    /// # Panics
    ///
    /// Panics if `equity_curve` is empty (should never happen with proper initialization).
    #[must_use]
    pub fn calculate(&self) -> PerformanceMetrics {
        let total_return = (self.equity_curve.last().unwrap() - self.equity_curve.first().unwrap())
            / self.equity_curve.first().unwrap();

        #[allow(clippy::cast_precision_loss)]
        let returns_len_f64 = self.returns.len() as f64;

        let mean_return: f64 = self.returns.iter()
            .map(|r| r.to_string().parse::<f64>().unwrap_or(0.0))
            .sum::<f64>() / returns_len_f64;

        let variance: f64 = self.returns.iter()
            .map(|r| {
                let val = r.to_string().parse::<f64>().unwrap_or(0.0);
                (val - mean_return).powi(2)
            })
            .sum::<f64>() / returns_len_f64;

        let std_dev = variance.sqrt();
        let sharpe_ratio = if std_dev > 0.0 {
            mean_return / std_dev * (252.0_f64).sqrt() // Annualized
        } else {
            0.0
        };

        let max_drawdown = self.calculate_max_drawdown();

        let total_trades = self.wins + self.losses;
        #[allow(clippy::cast_precision_loss)]
        let win_rate = if total_trades > 0 {
            self.wins as f64 / total_trades as f64
        } else {
            0.0
        };

        PerformanceMetrics {
            total_return,
            sharpe_ratio,
            max_drawdown,
            num_trades: total_trades,
            win_rate,
        }
    }

    fn calculate_max_drawdown(&self) -> Decimal {
        let mut max_drawdown = Decimal::ZERO;
        let mut peak = self.equity_curve[0];

        for &equity in &self.equity_curve {
            if equity > peak {
                peak = equity;
            }
            let drawdown = (peak - equity) / peak;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }

        max_drawdown
    }
}
