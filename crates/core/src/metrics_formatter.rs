#![allow(clippy::format_push_string)]
#![allow(clippy::uninlined_format_args)]

use crate::engine::PerformanceMetrics;

pub struct MetricsFormatter;

impl MetricsFormatter {
    #[must_use]
    pub fn format(metrics: &PerformanceMetrics) -> String {
        let mut output = String::new();

        output.push('\n');
        output.push_str("═══════════════════════════════════════════════════════════════\n");
        output.push_str("                    BACKTEST RESULTS                           \n");
        output.push_str("═══════════════════════════════════════════════════════════════\n");
        output.push('\n');

        // Time Period
        output.push_str("Time Period\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");
        output.push_str(&format!(
            "Start:                 {}\n",
            metrics.start_time.format("%Y-%m-%d %H:%M:%S UTC")
        ));
        output.push_str(&format!(
            "End:                   {}\n",
            metrics.end_time.format("%Y-%m-%d %H:%M:%S UTC")
        ));

        let days = metrics.duration.num_days();
        let hours = metrics.duration.num_hours() % 24;
        let minutes = metrics.duration.num_minutes() % 60;
        output.push_str(&format!(
            "Duration:              {} days {} hours {} minutes\n",
            days, hours, minutes
        ));
        output.push('\n');

        // Portfolio Performance
        output.push_str("Portfolio Performance\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");
        output.push_str(&format!(
            "Initial Capital:       ${:.2}\n",
            metrics.initial_capital
        ));
        output.push_str(&format!(
            "Final Capital:         ${:.2}\n",
            metrics.final_capital
        ));
        output.push_str(&format!(
            "Equity Peak:           ${:.2}\n",
            metrics.equity_peak
        ));
        output.push_str(&format!(
            "Total Return:          {:.2}%\n",
            metrics.total_return * rust_decimal::Decimal::from(100)
        ));
        output.push_str(&format!(
            "Buy & Hold Return:     {:.2}%\n",
            metrics.buy_hold_return * rust_decimal::Decimal::from(100)
        ));
        output.push_str(&format!(
            "Sharpe Ratio:          {:.4}\n",
            metrics.sharpe_ratio
        ));
        output.push_str(&format!(
            "Max Drawdown:          {:.2}%\n",
            metrics.max_drawdown * rust_decimal::Decimal::from(100)
        ));
        output.push_str(&format!(
            "Exposure Time:         {:.2}%\n",
            metrics.exposure_time * 100.0
        ));
        output.push('\n');

        // Trade Statistics
        output.push_str("Trade Statistics\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");
        output.push_str(&format!("Total Trades:          {}\n", metrics.num_trades));

        if metrics.num_trades > 0 {
            output.push_str(&format!(
                "Win Rate:              {:.2}%\n",
                metrics.win_rate * 100.0
            ));
        } else {
            output.push_str("Win Rate:              N/A (no trades)\n");
        }

        output.push('\n');
        output.push_str("═══════════════════════════════════════════════════════════════\n");

        if metrics.num_trades == 0 {
            output.push_str("\n⚠️  No trades were made during this backtest.\n");
            output.push_str("    Consider adjusting strategy parameters or data range.\n\n");
        }

        output
    }
}
