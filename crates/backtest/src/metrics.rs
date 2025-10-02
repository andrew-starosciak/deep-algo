use rust_decimal::Decimal;

pub struct PerformanceMetrics {
    pub total_return: Decimal,
    pub sharpe_ratio: f64,
    pub max_drawdown: Decimal,
    pub num_trades: usize,
    pub win_rate: f64,
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
