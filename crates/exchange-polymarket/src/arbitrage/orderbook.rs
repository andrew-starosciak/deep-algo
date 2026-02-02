//! Order book walking algorithm for fill simulation.
//!
//! Provides the [`simulate_fill`] function to calculate the actual cost
//! of filling an order of a given size through the order book.

use rust_decimal::Decimal;

use super::types::{FillSimulation, L2OrderBook, Side};

/// Walk the order book to calculate actual fill cost for a given size.
///
/// For buy orders, this walks through the ask levels (ascending).
/// For sell orders, this walks through the bid levels (descending).
///
/// # Arguments
///
/// * `book` - The order book to simulate against
/// * `side` - Whether this is a buy or sell order
/// * `target_size` - The desired fill quantity
///
/// # Returns
///
/// Returns `Some(FillSimulation)` if the book has any liquidity,
/// `None` if the book is empty or target size is invalid.
///
/// # Example
///
/// ```
/// use algo_trade_polymarket::arbitrage::{L2OrderBook, Side, simulate_fill};
/// use rust_decimal_macros::dec;
///
/// let mut book = L2OrderBook::new("token-1".to_string());
/// book.apply_snapshot(
///     vec![(dec!(0.48), dec!(100))],  // bids
///     vec![(dec!(0.50), dec!(100))],  // asks
/// );
///
/// // Simulate buying 50 shares
/// let fill = simulate_fill(&book, Side::Buy, dec!(50)).unwrap();
/// assert_eq!(fill.filled, dec!(50));
/// assert_eq!(fill.worst_price, dec!(0.50));
/// assert!(fill.sufficient_depth);
/// ```
#[must_use]
pub fn simulate_fill(
    book: &L2OrderBook,
    side: Side,
    target_size: Decimal,
) -> Option<FillSimulation> {
    if target_size <= Decimal::ZERO {
        return None;
    }

    // Collect price levels based on side
    let levels: Vec<(Decimal, Decimal)> = match side {
        // For buying, we take from asks (sorted ascending)
        Side::Buy => book.asks.iter().map(|(p, s)| (*p, *s)).collect(),
        // For selling, we take from bids (sorted descending via Reverse)
        Side::Sell => book.bids.iter().map(|(r, s)| (r.0, *s)).collect(),
    };

    if levels.is_empty() {
        return None;
    }

    let mut filled = Decimal::ZERO;
    let mut total_cost = Decimal::ZERO;
    let mut worst_price = Decimal::ZERO;
    let best_price = levels.first().map(|(p, _)| *p)?;

    for (price, size) in &levels {
        if filled >= target_size {
            break;
        }
        let remaining = target_size - filled;
        let take = (*size).min(remaining);
        total_cost += take * price;
        filled += take;
        worst_price = *price;
    }

    let sufficient_depth = filled >= target_size;
    let vwap = if filled > Decimal::ZERO {
        total_cost / filled
    } else {
        Decimal::ZERO
    };

    Some(FillSimulation {
        filled,
        total_cost,
        vwap,
        worst_price,
        best_price,
        sufficient_depth,
    })
}

/// Calculate the depth available at or better than a given price.
///
/// # Arguments
///
/// * `book` - The order book to analyze
/// * `side` - Whether to look at bids or asks
/// * `price_limit` - The price threshold
///
/// # Returns
///
/// Total size available at or better than the price limit.
#[must_use]
pub fn depth_at_price(book: &L2OrderBook, side: Side, price_limit: Decimal) -> Decimal {
    match side {
        Side::Buy => {
            // For buying, "better" means lower price
            book.asks
                .iter()
                .filter(|(p, _)| **p <= price_limit)
                .map(|(_, s)| *s)
                .sum()
        }
        Side::Sell => {
            // For selling, "better" means higher price
            book.bids
                .iter()
                .filter(|(r, _)| r.0 >= price_limit)
                .map(|(_, s)| *s)
                .sum()
        }
    }
}

/// Calculate the price impact of a given order size.
///
/// Returns the difference between worst fill price and best price.
#[must_use]
pub fn price_impact(book: &L2OrderBook, side: Side, size: Decimal) -> Option<Decimal> {
    let fill = simulate_fill(book, side, size)?;
    if fill.sufficient_depth {
        Some((fill.worst_price - fill.best_price).abs())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn create_test_orderbook() -> L2OrderBook {
        let mut book = L2OrderBook::new("test-token".to_string());
        book.apply_snapshot(
            // Bids: 0.48 x 100, 0.47 x 200, 0.46 x 300
            vec![
                (dec!(0.48), dec!(100)),
                (dec!(0.47), dec!(200)),
                (dec!(0.46), dec!(300)),
            ],
            // Asks: 0.50 x 100, 0.51 x 200, 0.52 x 300
            vec![
                (dec!(0.50), dec!(100)),
                (dec!(0.51), dec!(200)),
                (dec!(0.52), dec!(300)),
            ],
        );
        book
    }

    #[test]
    fn test_simulate_fill_buy_single_level() {
        let book = create_test_orderbook();
        let fill = simulate_fill(&book, Side::Buy, dec!(50)).unwrap();

        assert_eq!(fill.filled, dec!(50));
        assert_eq!(fill.total_cost, dec!(25)); // 50 * 0.50
        assert_eq!(fill.vwap, dec!(0.50));
        assert_eq!(fill.best_price, dec!(0.50));
        assert_eq!(fill.worst_price, dec!(0.50));
        assert!(fill.sufficient_depth);
    }

    #[test]
    fn test_simulate_fill_buy_multiple_levels() {
        let book = create_test_orderbook();
        let fill = simulate_fill(&book, Side::Buy, dec!(150)).unwrap();

        assert_eq!(fill.filled, dec!(150));
        // 100 @ 0.50 + 50 @ 0.51 = 50 + 25.5 = 75.5
        assert_eq!(fill.total_cost, dec!(75.5));
        assert_eq!(fill.best_price, dec!(0.50));
        assert_eq!(fill.worst_price, dec!(0.51));
        assert!(fill.sufficient_depth);
    }

    #[test]
    fn test_simulate_fill_buy_all_levels() {
        let book = create_test_orderbook();
        let fill = simulate_fill(&book, Side::Buy, dec!(600)).unwrap();

        assert_eq!(fill.filled, dec!(600));
        // 100 @ 0.50 + 200 @ 0.51 + 300 @ 0.52 = 50 + 102 + 156 = 308
        assert_eq!(fill.total_cost, dec!(308));
        assert_eq!(fill.worst_price, dec!(0.52));
        assert!(fill.sufficient_depth);
    }

    #[test]
    fn test_simulate_fill_buy_insufficient_depth() {
        let book = create_test_orderbook();
        let fill = simulate_fill(&book, Side::Buy, dec!(700)).unwrap();

        assert_eq!(fill.filled, dec!(600)); // Only 600 available
        assert!(!fill.sufficient_depth);
    }

    #[test]
    fn test_simulate_fill_sell_single_level() {
        let book = create_test_orderbook();
        let fill = simulate_fill(&book, Side::Sell, dec!(50)).unwrap();

        assert_eq!(fill.filled, dec!(50));
        assert_eq!(fill.total_cost, dec!(24)); // 50 * 0.48
        assert_eq!(fill.vwap, dec!(0.48));
        assert_eq!(fill.best_price, dec!(0.48));
        assert_eq!(fill.worst_price, dec!(0.48));
        assert!(fill.sufficient_depth);
    }

    #[test]
    fn test_simulate_fill_sell_multiple_levels() {
        let book = create_test_orderbook();
        let fill = simulate_fill(&book, Side::Sell, dec!(200)).unwrap();

        assert_eq!(fill.filled, dec!(200));
        // 100 @ 0.48 + 100 @ 0.47 = 48 + 47 = 95
        assert_eq!(fill.total_cost, dec!(95));
        assert_eq!(fill.best_price, dec!(0.48));
        assert_eq!(fill.worst_price, dec!(0.47));
        assert!(fill.sufficient_depth);
    }

    #[test]
    fn test_simulate_fill_zero_size() {
        let book = create_test_orderbook();
        assert!(simulate_fill(&book, Side::Buy, Decimal::ZERO).is_none());
        assert!(simulate_fill(&book, Side::Buy, dec!(-10)).is_none());
    }

    #[test]
    fn test_simulate_fill_empty_book() {
        let book = L2OrderBook::new("empty".to_string());
        assert!(simulate_fill(&book, Side::Buy, dec!(100)).is_none());
        assert!(simulate_fill(&book, Side::Sell, dec!(100)).is_none());
    }

    #[test]
    fn test_depth_at_price_buy() {
        let book = create_test_orderbook();
        // Depth at 0.50 or better (lower) - only first level
        assert_eq!(depth_at_price(&book, Side::Buy, dec!(0.50)), dec!(100));
        // Depth at 0.51 or better - first two levels
        assert_eq!(depth_at_price(&book, Side::Buy, dec!(0.51)), dec!(300));
        // Depth at 0.52 or better - all levels
        assert_eq!(depth_at_price(&book, Side::Buy, dec!(0.52)), dec!(600));
    }

    #[test]
    fn test_depth_at_price_sell() {
        let book = create_test_orderbook();
        // Depth at 0.48 or better (higher) - only first level
        assert_eq!(depth_at_price(&book, Side::Sell, dec!(0.48)), dec!(100));
        // Depth at 0.47 or better - first two levels
        assert_eq!(depth_at_price(&book, Side::Sell, dec!(0.47)), dec!(300));
        // Depth at 0.46 or better - all levels
        assert_eq!(depth_at_price(&book, Side::Sell, dec!(0.46)), dec!(600));
    }

    #[test]
    fn test_price_impact() {
        let book = create_test_orderbook();

        // Small order - no impact
        assert_eq!(
            price_impact(&book, Side::Buy, dec!(50)),
            Some(Decimal::ZERO)
        );

        // Order crossing levels - has impact
        assert_eq!(price_impact(&book, Side::Buy, dec!(150)), Some(dec!(0.01)));

        // Too large - insufficient depth
        assert!(price_impact(&book, Side::Buy, dec!(1000)).is_none());
    }

    #[test]
    fn test_vwap_calculation() {
        let book = create_test_orderbook();
        let fill = simulate_fill(&book, Side::Buy, dec!(300)).unwrap();

        // 100 @ 0.50 + 200 @ 0.51 = 50 + 102 = 152
        // VWAP = 152 / 300 = 0.50666...
        let expected_vwap = dec!(152) / dec!(300);
        assert_eq!(fill.vwap, expected_vwap);
    }
}
