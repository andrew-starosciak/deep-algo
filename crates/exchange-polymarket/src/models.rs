//! Polymarket data models.
//!
//! Models for Polymarket CLOB API responses and internal representations.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A Polymarket binary outcome market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    /// Unique market condition ID
    pub condition_id: String,
    /// Market question/title
    pub question: String,
    /// Optional description
    pub description: Option<String>,
    /// Market end/resolution date
    pub end_date: Option<DateTime<Utc>>,
    /// Tokens representing outcomes
    pub tokens: Vec<Token>,
    /// Whether the market is currently active
    pub active: bool,
    /// Tags/categories for the market
    pub tags: Option<Vec<String>>,
    /// 24-hour volume in USD
    pub volume_24h: Option<Decimal>,
    /// Current liquidity in USD
    pub liquidity: Option<Decimal>,
}

impl Market {
    /// Returns the "Yes" token if present.
    #[must_use]
    pub fn yes_token(&self) -> Option<&Token> {
        self.tokens
            .iter()
            .find(|t| t.outcome.eq_ignore_ascii_case("yes"))
    }

    /// Returns the "No" token if present.
    #[must_use]
    pub fn no_token(&self) -> Option<&Token> {
        self.tokens
            .iter()
            .find(|t| t.outcome.eq_ignore_ascii_case("no"))
    }

    /// Returns the yes price (0.0 to 1.0).
    #[must_use]
    pub fn yes_price(&self) -> Option<Decimal> {
        self.yes_token().map(|t| t.price)
    }

    /// Returns the no price (0.0 to 1.0).
    #[must_use]
    pub fn no_price(&self) -> Option<Decimal> {
        self.no_token().map(|t| t.price)
    }

    /// Returns true if the market is related to BTC/Bitcoin.
    #[must_use]
    pub fn is_btc_related(&self) -> bool {
        let question_lower = self.question.to_lowercase();
        let btc_keywords = ["bitcoin", "btc", "satoshi"];

        btc_keywords.iter().any(|kw| question_lower.contains(kw))
    }

    /// Returns true if the market has sufficient liquidity.
    #[must_use]
    pub fn has_sufficient_liquidity(&self, min_liquidity: Decimal) -> bool {
        self.liquidity.map(|l| l >= min_liquidity).unwrap_or(false)
    }

    /// Returns true if the market is tradeable (active and has prices).
    #[must_use]
    pub fn is_tradeable(&self) -> bool {
        self.active && self.yes_price().is_some() && self.no_price().is_some()
    }

    /// Returns the "Up" token if present (for 15-min markets).
    #[must_use]
    pub fn up_token(&self) -> Option<&Token> {
        self.tokens
            .iter()
            .find(|t| t.outcome.eq_ignore_ascii_case("up"))
    }

    /// Returns the "Down" token if present (for 15-min markets).
    #[must_use]
    pub fn down_token(&self) -> Option<&Token> {
        self.tokens
            .iter()
            .find(|t| t.outcome.eq_ignore_ascii_case("down"))
    }

    /// Returns the up price (for 15-min markets, 0.0 to 1.0).
    #[must_use]
    pub fn up_price(&self) -> Option<Decimal> {
        self.up_token().map(|t| t.price)
    }

    /// Returns the down price (for 15-min markets, 0.0 to 1.0).
    #[must_use]
    pub fn down_price(&self) -> Option<Decimal> {
        self.down_token().map(|t| t.price)
    }

    /// Returns true if this is a 15-minute Up/Down market.
    #[must_use]
    pub fn is_15min_market(&self) -> bool {
        self.up_token().is_some() && self.down_token().is_some()
    }
}

/// A token representing an outcome in a binary market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    /// Unique token ID
    pub token_id: String,
    /// Outcome name ("Yes" or "No")
    pub outcome: String,
    /// Current price (0.0 to 1.0, representing probability)
    pub price: Decimal,
    /// Whether this token won (Some(true) = won, Some(false) = lost, None = not resolved)
    pub winner: Option<bool>,
}

/// Filter for market discovery.
#[derive(Debug, Clone, Default)]
pub struct MarketFilter {
    /// Search query string
    pub query: Option<String>,
    /// Only active markets
    pub active_only: bool,
    /// Minimum liquidity requirement
    pub min_liquidity: Option<Decimal>,
    /// Tags to filter by
    pub tags: Option<Vec<String>>,
}

impl MarketFilter {
    /// Creates a new filter for BTC-related markets.
    #[must_use]
    pub fn btc_markets() -> Self {
        Self {
            query: Some("bitcoin OR btc".to_string()),
            active_only: true,
            min_liquidity: None,
            tags: None,
        }
    }

    /// Adds a minimum liquidity requirement.
    #[must_use]
    pub fn with_min_liquidity(mut self, liquidity: Decimal) -> Self {
        self.min_liquidity = Some(liquidity);
        self
    }
}

/// Price information from the CLOB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Price {
    /// Token ID
    pub token_id: String,
    /// Best bid price
    pub bid: Option<Decimal>,
    /// Best ask price
    pub ask: Option<Decimal>,
    /// Last traded price
    pub last: Option<Decimal>,
    /// Bid-ask spread
    pub spread: Option<Decimal>,
}

impl Price {
    /// Returns the mid price if both bid and ask are available.
    #[must_use]
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.bid, self.ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            (Some(bid), None) => Some(bid),
            (None, Some(ask)) => Some(ask),
            (None, None) => self.last,
        }
    }

    /// Returns the spread in basis points.
    #[must_use]
    pub fn spread_bps(&self) -> Option<Decimal> {
        let mid = self.mid_price()?;
        if mid == Decimal::ZERO {
            return None;
        }
        let spread = self.spread?;
        Some(spread / mid * Decimal::from(10000))
    }
}

/// Raw API response for markets endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct MarketsResponse {
    pub data: Option<Vec<RawMarket>>,
    pub next_cursor: Option<String>,
}

/// Raw market data from API.
#[derive(Debug, Clone, Deserialize)]
pub struct RawMarket {
    pub condition_id: String,
    pub question: String,
    pub description: Option<String>,
    pub end_date_iso: Option<String>,
    pub tokens: Vec<RawToken>,
    pub active: bool,
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub volume_num_24hr: Option<f64>,
    #[serde(default)]
    pub liquidity_num: Option<f64>,
}

/// Raw token data from API.
#[derive(Debug, Clone, Deserialize)]
pub struct RawToken {
    pub token_id: String,
    pub outcome: String,
    #[serde(default)]
    pub price: Option<f64>,
    pub winner: Option<bool>,
}

// ============================================================================
// Gamma API Types (for 15-minute market discovery)
// ============================================================================

/// Supported coins for 15-minute markets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Coin {
    Btc,
    Eth,
    Sol,
    Xrp,
}

impl Coin {
    /// Returns the slug prefix for this coin.
    #[must_use]
    pub fn slug_prefix(&self) -> &'static str {
        match self {
            Coin::Btc => "btc",
            Coin::Eth => "eth",
            Coin::Sol => "sol",
            Coin::Xrp => "xrp",
        }
    }

    /// Returns all supported coins.
    #[must_use]
    pub fn all() -> &'static [Coin] {
        &[Coin::Btc, Coin::Eth, Coin::Sol, Coin::Xrp]
    }
}

/// Gamma API event response (contains markets).
#[derive(Debug, Clone, Deserialize)]
pub struct GammaEvent {
    pub slug: String,
    pub title: Option<String>,
    pub markets: Vec<GammaMarket>,
}

/// Gamma API market data for a 15-minute binary option.
#[derive(Debug, Clone, Deserialize)]
pub struct GammaMarket {
    /// Condition ID for the market
    #[serde(rename = "conditionId")]
    pub condition_id: String,

    /// JSON string array: "[\"0.53\", \"0.47\"]"
    #[serde(rename = "outcomePrices")]
    pub outcome_prices: String,

    /// JSON string array: "[\"token_id_1\", \"token_id_2\"]"
    #[serde(rename = "clobTokenIds")]
    pub clob_token_ids: String,

    /// Liquidity as string (e.g., "11888.4997")
    pub liquidity: Option<String>,

    /// End date in ISO format
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,

    /// Start date in ISO format
    #[serde(rename = "startDate")]
    pub start_date: Option<String>,

    /// Question/title of the market
    pub question: Option<String>,

    /// Whether the market is active
    pub active: Option<bool>,
}

impl GammaMarket {
    /// Parses outcome prices from JSON string array.
    #[must_use]
    pub fn parse_outcome_prices(&self) -> Option<(Decimal, Decimal)> {
        let prices: Vec<String> = serde_json::from_str(&self.outcome_prices).ok()?;
        if prices.len() >= 2 {
            let up_price = prices[0].parse::<Decimal>().ok()?;
            let down_price = prices[1].parse::<Decimal>().ok()?;
            Some((up_price, down_price))
        } else {
            None
        }
    }

    /// Parses CLOB token IDs from JSON string array.
    #[must_use]
    pub fn parse_clob_token_ids(&self) -> Option<(String, String)> {
        let ids: Vec<String> = serde_json::from_str(&self.clob_token_ids).ok()?;
        if ids.len() >= 2 {
            Some((ids[0].clone(), ids[1].clone()))
        } else {
            None
        }
    }

    /// Parses liquidity from string.
    #[must_use]
    pub fn parse_liquidity(&self) -> Option<Decimal> {
        self.liquidity.as_ref()?.parse::<Decimal>().ok()
    }

    /// Converts to internal Market type.
    #[must_use]
    pub fn to_market(&self) -> Option<Market> {
        let (up_price, down_price) = self.parse_outcome_prices()?;
        let (up_token_id, down_token_id) = self.parse_clob_token_ids()?;

        let end_date = self.end_date.as_ref().and_then(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        });

        Some(Market {
            condition_id: self.condition_id.clone(),
            question: self
                .question
                .clone()
                .unwrap_or_else(|| format!("15-min Up/Down {}", self.condition_id)),
            description: None,
            end_date,
            tokens: vec![
                Token {
                    token_id: up_token_id,
                    outcome: "Up".to_string(),
                    price: up_price,
                    winner: None,
                },
                Token {
                    token_id: down_token_id,
                    outcome: "Down".to_string(),
                    price: down_price,
                    winner: None,
                },
            ],
            active: self.active.unwrap_or(true),
            tags: Some(vec!["15min".to_string(), "updown".to_string()]),
            volume_24h: None,
            liquidity: self.parse_liquidity(),
        })
    }
}

impl From<RawMarket> for Market {
    fn from(raw: RawMarket) -> Self {
        let end_date = raw.end_date_iso.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        });

        let tokens = raw.tokens.into_iter().map(Token::from).collect();

        Self {
            condition_id: raw.condition_id,
            question: raw.question,
            description: raw.description,
            end_date,
            tokens,
            active: raw.active,
            tags: raw.tags,
            volume_24h: raw
                .volume_num_24hr
                .map(Decimal::try_from)
                .and_then(Result::ok),
            liquidity: raw
                .liquidity_num
                .map(Decimal::try_from)
                .and_then(Result::ok),
        }
    }
}

impl From<RawToken> for Token {
    fn from(raw: RawToken) -> Self {
        Self {
            token_id: raw.token_id,
            outcome: raw.outcome,
            price: raw
                .price
                .map(Decimal::try_from)
                .and_then(Result::ok)
                .unwrap_or(Decimal::ZERO),
            winner: raw.winner,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn sample_yes_token() -> Token {
        Token {
            token_id: "yes-token-123".to_string(),
            outcome: "Yes".to_string(),
            price: dec!(0.65),
            winner: None,
        }
    }

    fn sample_no_token() -> Token {
        Token {
            token_id: "no-token-456".to_string(),
            outcome: "No".to_string(),
            price: dec!(0.35),
            winner: None,
        }
    }

    fn sample_market() -> Market {
        Market {
            condition_id: "btc-100k-2025".to_string(),
            question: "Will Bitcoin exceed $100k by end of 2025?".to_string(),
            description: Some("BTC price prediction market".to_string()),
            end_date: None,
            tokens: vec![sample_yes_token(), sample_no_token()],
            active: true,
            tags: Some(vec!["crypto".to_string(), "bitcoin".to_string()]),
            volume_24h: Some(dec!(50000)),
            liquidity: Some(dec!(100000)),
        }
    }

    // ========== Market Tests ==========

    #[test]
    fn test_market_yes_token() {
        let market = sample_market();
        let yes = market.yes_token().unwrap();
        assert_eq!(yes.outcome, "Yes");
        assert_eq!(yes.price, dec!(0.65));
    }

    #[test]
    fn test_market_no_token() {
        let market = sample_market();
        let no = market.no_token().unwrap();
        assert_eq!(no.outcome, "No");
        assert_eq!(no.price, dec!(0.35));
    }

    #[test]
    fn test_market_yes_price() {
        let market = sample_market();
        assert_eq!(market.yes_price(), Some(dec!(0.65)));
    }

    #[test]
    fn test_market_no_price() {
        let market = sample_market();
        assert_eq!(market.no_price(), Some(dec!(0.35)));
    }

    #[test]
    fn test_market_is_btc_related_bitcoin() {
        let market = sample_market();
        assert!(market.is_btc_related());
    }

    #[test]
    fn test_market_is_btc_related_btc() {
        let mut market = sample_market();
        market.question = "Will BTC hit $100k?".to_string();
        assert!(market.is_btc_related());
    }

    #[test]
    fn test_market_is_btc_related_case_insensitive() {
        let mut market = sample_market();
        market.question = "BITCOIN price prediction".to_string();
        assert!(market.is_btc_related());
    }

    #[test]
    fn test_market_is_not_btc_related() {
        let mut market = sample_market();
        market.question = "Will ETH exceed $10k?".to_string();
        assert!(!market.is_btc_related());
    }

    #[test]
    fn test_market_has_sufficient_liquidity() {
        let market = sample_market();
        assert!(market.has_sufficient_liquidity(dec!(50000)));
        assert!(market.has_sufficient_liquidity(dec!(100000)));
        assert!(!market.has_sufficient_liquidity(dec!(200000)));
    }

    #[test]
    fn test_market_has_sufficient_liquidity_none() {
        let mut market = sample_market();
        market.liquidity = None;
        assert!(!market.has_sufficient_liquidity(dec!(1000)));
    }

    #[test]
    fn test_market_is_tradeable() {
        let market = sample_market();
        assert!(market.is_tradeable());
    }

    #[test]
    fn test_market_is_not_tradeable_inactive() {
        let mut market = sample_market();
        market.active = false;
        assert!(!market.is_tradeable());
    }

    #[test]
    fn test_market_is_not_tradeable_no_tokens() {
        let mut market = sample_market();
        market.tokens.clear();
        assert!(!market.is_tradeable());
    }

    // ========== Token Tests ==========

    #[test]
    fn test_token_creation() {
        let token = sample_yes_token();
        assert_eq!(token.token_id, "yes-token-123");
        assert_eq!(token.outcome, "Yes");
        assert_eq!(token.price, dec!(0.65));
        assert!(token.winner.is_none());
    }

    #[test]
    fn test_token_with_winner() {
        let mut token = sample_yes_token();
        token.winner = Some(true);
        assert_eq!(token.winner, Some(true));
    }

    // ========== MarketFilter Tests ==========

    #[test]
    fn test_market_filter_btc_markets() {
        let filter = MarketFilter::btc_markets();
        assert!(filter.query.unwrap().contains("bitcoin"));
        assert!(filter.active_only);
    }

    #[test]
    fn test_market_filter_with_liquidity() {
        let filter = MarketFilter::btc_markets().with_min_liquidity(dec!(10000));
        assert_eq!(filter.min_liquidity, Some(dec!(10000)));
    }

    // ========== Price Tests ==========

    #[test]
    fn test_price_mid_price_with_bid_ask() {
        let price = Price {
            token_id: "test".to_string(),
            bid: Some(dec!(0.60)),
            ask: Some(dec!(0.64)),
            last: Some(dec!(0.62)),
            spread: Some(dec!(0.04)),
        };

        assert_eq!(price.mid_price(), Some(dec!(0.62)));
    }

    #[test]
    fn test_price_mid_price_bid_only() {
        let price = Price {
            token_id: "test".to_string(),
            bid: Some(dec!(0.60)),
            ask: None,
            last: Some(dec!(0.55)),
            spread: None,
        };

        assert_eq!(price.mid_price(), Some(dec!(0.60)));
    }

    #[test]
    fn test_price_mid_price_ask_only() {
        let price = Price {
            token_id: "test".to_string(),
            bid: None,
            ask: Some(dec!(0.64)),
            last: Some(dec!(0.55)),
            spread: None,
        };

        assert_eq!(price.mid_price(), Some(dec!(0.64)));
    }

    #[test]
    fn test_price_mid_price_last_fallback() {
        let price = Price {
            token_id: "test".to_string(),
            bid: None,
            ask: None,
            last: Some(dec!(0.62)),
            spread: None,
        };

        assert_eq!(price.mid_price(), Some(dec!(0.62)));
    }

    #[test]
    fn test_price_mid_price_none() {
        let price = Price {
            token_id: "test".to_string(),
            bid: None,
            ask: None,
            last: None,
            spread: None,
        };

        assert_eq!(price.mid_price(), None);
    }

    #[test]
    fn test_price_spread_bps() {
        let price = Price {
            token_id: "test".to_string(),
            bid: Some(dec!(0.60)),
            ask: Some(dec!(0.64)),
            last: None,
            spread: Some(dec!(0.04)),
        };

        // Mid = 0.62, spread = 0.04
        // spread_bps = 0.04 / 0.62 * 10000 = ~645 bps
        let spread_bps = price.spread_bps().unwrap();
        assert!(spread_bps > dec!(640) && spread_bps < dec!(650));
    }

    // ========== Raw API Response Parsing Tests ==========

    #[test]
    fn test_parse_raw_market_json() {
        let json = r#"{
            "condition_id": "0x123abc",
            "question": "Will Bitcoin reach $100k?",
            "description": "Price prediction",
            "end_date_iso": "2025-12-31T23:59:59Z",
            "tokens": [
                {"token_id": "yes-123", "outcome": "Yes", "price": 0.65, "winner": null},
                {"token_id": "no-456", "outcome": "No", "price": 0.35, "winner": null}
            ],
            "active": true,
            "tags": ["crypto", "bitcoin"],
            "volume_num_24hr": 50000.0,
            "liquidity_num": 100000.0
        }"#;

        let raw: RawMarket = serde_json::from_str(json).unwrap();
        let market: Market = raw.into();

        assert_eq!(market.condition_id, "0x123abc");
        assert_eq!(market.question, "Will Bitcoin reach $100k?");
        assert!(market.is_btc_related());
        assert!(market.active);
        assert_eq!(market.tokens.len(), 2);
        assert_eq!(market.yes_price(), Some(dec!(0.65)));
        assert_eq!(market.no_price(), Some(dec!(0.35)));
    }

    #[test]
    fn test_parse_raw_token_json() {
        let json = r#"{
            "token_id": "yes-123",
            "outcome": "Yes",
            "price": 0.65,
            "winner": null
        }"#;

        let raw: RawToken = serde_json::from_str(json).unwrap();
        let token: Token = raw.into();

        assert_eq!(token.token_id, "yes-123");
        assert_eq!(token.outcome, "Yes");
        assert_eq!(token.price, dec!(0.65));
        assert!(token.winner.is_none());
    }

    #[test]
    fn test_parse_raw_token_with_winner() {
        let json = r#"{
            "token_id": "yes-123",
            "outcome": "Yes",
            "price": 1.0,
            "winner": true
        }"#;

        let raw: RawToken = serde_json::from_str(json).unwrap();
        let token: Token = raw.into();

        assert_eq!(token.winner, Some(true));
    }

    #[test]
    fn test_parse_markets_response() {
        let json = r#"{
            "data": [
                {
                    "condition_id": "0x123",
                    "question": "Test market",
                    "description": null,
                    "end_date_iso": null,
                    "tokens": [],
                    "active": true,
                    "tags": null
                }
            ],
            "next_cursor": "abc123"
        }"#;

        let response: MarketsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.as_ref().unwrap().len(), 1);
        assert_eq!(response.next_cursor, Some("abc123".to_string()));
    }

    #[test]
    fn test_parse_raw_market_missing_optional_fields() {
        let json = r#"{
            "condition_id": "0x123",
            "question": "Minimal market",
            "tokens": [],
            "active": false
        }"#;

        let raw: RawMarket = serde_json::from_str(json).unwrap();
        let market: Market = raw.into();

        assert_eq!(market.condition_id, "0x123");
        assert!(market.description.is_none());
        assert!(market.end_date.is_none());
        assert!(market.volume_24h.is_none());
        assert!(market.liquidity.is_none());
    }

    // ========== Coin Tests ==========

    #[test]
    fn test_coin_slug_prefix() {
        assert_eq!(Coin::Btc.slug_prefix(), "btc");
        assert_eq!(Coin::Eth.slug_prefix(), "eth");
        assert_eq!(Coin::Sol.slug_prefix(), "sol");
        assert_eq!(Coin::Xrp.slug_prefix(), "xrp");
    }

    #[test]
    fn test_coin_all() {
        let all = Coin::all();
        assert_eq!(all.len(), 4);
        assert!(all.contains(&Coin::Btc));
        assert!(all.contains(&Coin::Eth));
        assert!(all.contains(&Coin::Sol));
        assert!(all.contains(&Coin::Xrp));
    }

    // ========== GammaMarket Tests ==========

    #[test]
    fn test_gamma_market_parse_outcome_prices() {
        let market = GammaMarket {
            condition_id: "0xabc".to_string(),
            outcome_prices: "[\"0.53\", \"0.47\"]".to_string(),
            clob_token_ids: "[\"t1\", \"t2\"]".to_string(),
            liquidity: Some("10000".to_string()),
            end_date: None,
            start_date: None,
            question: None,
            active: Some(true),
        };

        let (up, down) = market.parse_outcome_prices().unwrap();
        assert_eq!(up, dec!(0.53));
        assert_eq!(down, dec!(0.47));
    }

    #[test]
    fn test_gamma_market_parse_outcome_prices_invalid() {
        let market = GammaMarket {
            condition_id: "0xabc".to_string(),
            outcome_prices: "invalid".to_string(),
            clob_token_ids: "[\"t1\", \"t2\"]".to_string(),
            liquidity: None,
            end_date: None,
            start_date: None,
            question: None,
            active: None,
        };

        assert!(market.parse_outcome_prices().is_none());
    }

    #[test]
    fn test_gamma_market_parse_clob_token_ids() {
        let market = GammaMarket {
            condition_id: "0xabc".to_string(),
            outcome_prices: "[\"0.5\", \"0.5\"]".to_string(),
            clob_token_ids: "[\"111529abc\", \"33881def\"]".to_string(),
            liquidity: None,
            end_date: None,
            start_date: None,
            question: None,
            active: None,
        };

        let (up_id, down_id) = market.parse_clob_token_ids().unwrap();
        assert_eq!(up_id, "111529abc");
        assert_eq!(down_id, "33881def");
    }

    #[test]
    fn test_gamma_market_to_market() {
        let gamma_market = GammaMarket {
            condition_id: "0xabc123".to_string(),
            outcome_prices: "[\"0.60\", \"0.40\"]".to_string(),
            clob_token_ids: "[\"up-token\", \"down-token\"]".to_string(),
            liquidity: Some("15000.50".to_string()),
            end_date: Some("2026-01-31T12:15:00Z".to_string()),
            start_date: Some("2026-01-31T12:00:00Z".to_string()),
            question: Some("Will BTC go up?".to_string()),
            active: Some(true),
        };

        let market = gamma_market.to_market().unwrap();

        assert_eq!(market.condition_id, "0xabc123");
        assert_eq!(market.question, "Will BTC go up?");
        assert!(market.is_15min_market());
        assert_eq!(market.up_price(), Some(dec!(0.60)));
        assert_eq!(market.down_price(), Some(dec!(0.40)));
        assert_eq!(market.liquidity, Some(dec!(15000.50)));
        assert!(market.active);
    }

    // ========== Market Up/Down Tests ==========

    #[test]
    fn test_market_up_down_tokens() {
        let market = Market {
            condition_id: "15min-btc".to_string(),
            question: "15-min BTC Up/Down".to_string(),
            description: None,
            end_date: None,
            tokens: vec![
                Token {
                    token_id: "up-1".to_string(),
                    outcome: "Up".to_string(),
                    price: dec!(0.55),
                    winner: None,
                },
                Token {
                    token_id: "down-1".to_string(),
                    outcome: "Down".to_string(),
                    price: dec!(0.45),
                    winner: None,
                },
            ],
            active: true,
            tags: None,
            volume_24h: None,
            liquidity: Some(dec!(10000)),
        };

        assert!(market.is_15min_market());
        assert_eq!(market.up_price(), Some(dec!(0.55)));
        assert_eq!(market.down_price(), Some(dec!(0.45)));
        assert!(market.up_token().is_some());
        assert!(market.down_token().is_some());
    }

    #[test]
    fn test_market_is_not_15min_market() {
        let market = sample_market(); // Uses Yes/No tokens

        assert!(!market.is_15min_market());
        assert!(market.up_token().is_none());
        assert!(market.down_token().is_none());
    }
}
