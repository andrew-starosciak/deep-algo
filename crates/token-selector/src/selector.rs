use algo_trade_core::config::TokenSelectorConfig;
use algo_trade_data::{BacktestResultRecord, DatabaseClient};
use anyhow::Result;
use rust_decimal::Decimal;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Clone)]
pub struct TokenSelectionCriteria {
    pub min_sharpe_ratio: f64,
    pub min_win_rate: f64,
    pub max_drawdown: Decimal,
    pub min_num_trades: i32,
}

impl From<&TokenSelectorConfig> for TokenSelectionCriteria {
    fn from(config: &TokenSelectorConfig) -> Self {
        Self {
            min_sharpe_ratio: config.min_sharpe_ratio,
            min_win_rate: config.min_win_rate,
            max_drawdown: Decimal::try_from(config.max_drawdown).unwrap_or(Decimal::ZERO),
            min_num_trades: config.min_num_trades,
        }
    }
}

pub struct TokenSelector {
    config: TokenSelectorConfig,
    db_client: Arc<DatabaseClient>,
}

impl TokenSelector {
    /// Creates a new token selector.
    #[must_use]
    pub const fn new(config: TokenSelectorConfig, db_client: Arc<DatabaseClient>) -> Self {
        Self { config, db_client }
    }

    /// Selects tokens that meet the performance criteria based on recent backtest results.
    ///
    /// # Errors
    /// Returns an error if database query fails.
    pub async fn select_tokens(&self, strategy_name: &str) -> Result<Vec<String>> {
        let criteria = TokenSelectionCriteria::from(&self.config);

        // Query latest backtest results
        let results = self
            .db_client
            .query_latest_backtest_results(strategy_name, self.config.lookback_hours)
            .await?;

        info!("Evaluating {} backtest results for token selection", results.len());

        // Filter and rank tokens
        let mut approved_tokens: Vec<(String, f64)> = results
            .iter()
            .filter(|r| meets_criteria(r, &criteria))
            .map(|r| (r.symbol.clone(), r.sharpe_ratio))
            .collect();

        // Sort by Sharpe ratio (descending)
        approved_tokens.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let token_list: Vec<String> = approved_tokens.iter().map(|(symbol, _)| symbol.clone()).collect();

        info!(
            "Token selection complete: {} tokens approved out of {}",
            token_list.len(),
            results.len()
        );

        Ok(token_list)
    }

    /// Gets detailed selection results with metrics for display.
    ///
    /// # Errors
    /// Returns an error if database query fails.
    pub async fn get_selection_details(
        &self,
        strategy_name: &str,
    ) -> Result<Vec<TokenSelectionResult>> {
        let criteria = TokenSelectionCriteria::from(&self.config);

        let results = self
            .db_client
            .query_latest_backtest_results(strategy_name, self.config.lookback_hours)
            .await?;

        let mut selection_results: Vec<TokenSelectionResult> = results
            .iter()
            .map(|r| {
                let approved = meets_criteria(r, &criteria);
                TokenSelectionResult {
                    symbol: r.symbol.clone(),
                    sharpe_ratio: r.sharpe_ratio,
                    win_rate: r.win_rate,
                    max_drawdown: r.max_drawdown,
                    num_trades: r.num_trades,
                    total_pnl: r.total_pnl,
                    approved,
                }
            })
            .collect();

        // Sort by Sharpe ratio
        selection_results.sort_by(|a, b| b.sharpe_ratio.partial_cmp(&a.sharpe_ratio).unwrap_or(std::cmp::Ordering::Equal));

        Ok(selection_results)
    }
}

fn meets_criteria(record: &BacktestResultRecord, criteria: &TokenSelectionCriteria) -> bool {
    record.sharpe_ratio >= criteria.min_sharpe_ratio
        && record.win_rate >= criteria.min_win_rate
        && record.max_drawdown <= criteria.max_drawdown
        && record.num_trades >= criteria.min_num_trades
}

#[derive(Debug, Clone)]
pub struct TokenSelectionResult {
    pub symbol: String,
    pub sharpe_ratio: f64,
    pub win_rate: f64,
    pub max_drawdown: Decimal,
    pub num_trades: i32,
    pub total_pnl: Decimal,
    pub approved: bool,
}
