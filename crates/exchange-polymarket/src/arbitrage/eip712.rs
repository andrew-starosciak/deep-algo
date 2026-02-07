//! EIP-712 typed data signing for Polymarket CLOB orders.
//!
//! Implements the EIP-712 standard for signing Polymarket CTF Exchange orders
//! using k256 (secp256k1) ECDSA. No external SDK dependencies required.
//!
//! # References
//!
//! - [EIP-712](https://eips.ethereum.org/EIPS/eip-712)
//! - [Polymarket CTF Exchange](https://github.com/Polymarket/ctf-exchange)

use rust_decimal::Decimal;
use sha3::{Digest, Keccak256};
use thiserror::Error;

use super::signer::WalletError;

// =============================================================================
// Constants
// =============================================================================

/// EIP-712 domain name for the Polymarket CTF Exchange.
const DOMAIN_NAME: &str = "Polymarket CTF Exchange";

/// EIP-712 domain version.
const DOMAIN_VERSION: &str = "1";

/// Polygon mainnet chain ID.
pub const POLYGON_CHAIN_ID: u64 = 137;

/// Standard CTF Exchange contract on Polygon.
pub const STANDARD_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

/// Neg Risk CTF Exchange contract on Polygon.
pub const NEG_RISK_EXCHANGE: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";

/// EIP-712 domain name for ClobAuth messages.
pub const CLOB_AUTH_DOMAIN_NAME: &str = "ClobAuthDomain";

/// ClobAuth attestation message.
pub const CLOB_AUTH_MESSAGE: &str = "This message attests that I control the given wallet";

/// USDC uses 6 decimal places.
const USDC_DECIMALS: u32 = 6;

/// Side: BUY = 0.
pub const SIDE_BUY: u8 = 0;

/// Side: SELL = 1.
pub const SIDE_SELL: u8 = 1;

/// SignatureType: EOA = 0.
pub const SIGNATURE_TYPE_EOA: u8 = 0;

/// Zero address (taker default).
const ZERO_ADDRESS: [u8; 20] = [0u8; 20];

// =============================================================================
// Errors
// =============================================================================

/// Errors from EIP-712 operations.
#[derive(Debug, Error)]
pub enum Eip712Error {
    /// Invalid address format.
    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    /// Invalid private key.
    #[error("Invalid private key: {0}")]
    InvalidKey(String),

    /// Signing failed.
    #[error("Signing failed: {0}")]
    SigningFailed(String),

    /// Amount calculation error.
    #[error("Amount calculation error: {0}")]
    AmountError(String),
}

impl From<Eip712Error> for WalletError {
    fn from(e: Eip712Error) -> Self {
        WalletError::SigningFailed(e.to_string())
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// EIP-712 signing configuration.
#[derive(Debug, Clone)]
pub struct Eip712Config {
    /// Blockchain chain ID.
    pub chain_id: u64,
    /// Whether this is a neg-risk market.
    pub neg_risk: bool,
}

impl Default for Eip712Config {
    fn default() -> Self {
        Self {
            chain_id: POLYGON_CHAIN_ID,
            neg_risk: false,
        }
    }
}

impl Eip712Config {
    /// Returns the exchange contract address for this config.
    #[must_use]
    pub fn exchange_address(&self) -> &str {
        if self.neg_risk {
            NEG_RISK_EXCHANGE
        } else {
            STANDARD_EXCHANGE
        }
    }
}

// =============================================================================
// Order struct
// =============================================================================

/// A Polymarket CLOB order for EIP-712 signing.
///
/// Maps to the Solidity `Order` struct in the CTF Exchange contract.
#[derive(Debug, Clone)]
pub struct Eip712Order {
    /// Random salt for uniqueness (small random number matching SDK convention).
    pub salt: u64,
    /// Maker (funder) address.
    pub maker: [u8; 20],
    /// Signer address.
    pub signer: [u8; 20],
    /// Taker address (usually zero).
    pub taker: [u8; 20],
    /// ERC1155 conditional token ID (numeric string).
    pub token_id: String,
    /// Maximum amount maker spends (USDC, 6 decimals).
    pub maker_amount: u64,
    /// Minimum amount taker pays (USDC, 6 decimals).
    pub taker_amount: u64,
    /// Unix expiration timestamp (0 = no expiration).
    pub expiration: u64,
    /// Nonce for cancellation.
    pub nonce: u64,
    /// Fee rate in basis points.
    pub fee_rate_bps: u16,
    /// Order side: 0 = BUY, 1 = SELL.
    pub side: u8,
    /// Signature type: 0 = EOA.
    pub signature_type: u8,
}

// =============================================================================
// Hashing functions
// =============================================================================

/// Computes keccak256 hash.
fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Computes keccak256 of a UTF-8 string.
fn keccak256_str(s: &str) -> [u8; 32] {
    keccak256(s.as_bytes())
}

/// EIP-712 domain type hash.
fn domain_type_hash() -> [u8; 32] {
    keccak256_str(
        "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    )
}

/// Order type hash matching the Polymarket CTF Exchange Solidity contract.
fn order_type_hash() -> [u8; 32] {
    keccak256_str(
        "Order(uint256 salt,address maker,address signer,address taker,\
         uint256 tokenId,uint256 makerAmount,uint256 takerAmount,\
         uint256 expiration,uint256 nonce,uint256 feeRateBps,\
         uint8 side,uint8 signatureType)",
    )
}

/// ClobAuth type hash for authentication messages.
pub fn clob_auth_type_hash() -> [u8; 32] {
    keccak256_str("ClobAuth(address address,string timestamp,uint256 nonce,string message)")
}

/// Computes the EIP-712 domain separator.
///
/// `hash(domainTypeHash || hash(name) || hash(version) || chainId || verifyingContract)`
pub fn compute_domain_separator(
    chain_id: u64,
    exchange_address: &str,
) -> Result<[u8; 32], Eip712Error> {
    let contract_bytes = parse_address(exchange_address)?;

    let mut encoded = Vec::with_capacity(5 * 32);
    encoded.extend_from_slice(&domain_type_hash());
    encoded.extend_from_slice(&keccak256_str(DOMAIN_NAME));
    encoded.extend_from_slice(&keccak256_str(DOMAIN_VERSION));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(chain_id));
    encoded.extend_from_slice(&abi_encode_address(&contract_bytes));

    Ok(keccak256(&encoded))
}

/// EIP-712 domain type hash for ClobAuth (no verifyingContract field).
fn clob_auth_domain_type_hash() -> [u8; 32] {
    keccak256_str("EIP712Domain(string name,string version,uint256 chainId)")
}

/// Computes the ClobAuth domain separator (no verifyingContract).
pub fn compute_clob_auth_domain_separator(chain_id: u64) -> [u8; 32] {
    let mut encoded = Vec::with_capacity(4 * 32);
    encoded.extend_from_slice(&clob_auth_domain_type_hash());
    encoded.extend_from_slice(&keccak256_str(CLOB_AUTH_DOMAIN_NAME));
    encoded.extend_from_slice(&keccak256_str(DOMAIN_VERSION));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(chain_id));

    keccak256(&encoded)
}

/// Computes the struct hash for an Order.
///
/// `hash(ORDER_TYPEHASH || abi_encode(field1, field2, ...))`
pub fn compute_order_struct_hash(order: &Eip712Order) -> [u8; 32] {
    let token_id_u256 = token_id_to_u256(&order.token_id);

    let mut encoded = Vec::with_capacity(13 * 32);
    encoded.extend_from_slice(&order_type_hash());
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.salt));
    encoded.extend_from_slice(&abi_encode_address(&order.maker));
    encoded.extend_from_slice(&abi_encode_address(&order.signer));
    encoded.extend_from_slice(&abi_encode_address(&order.taker));
    encoded.extend_from_slice(&token_id_u256);
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.maker_amount));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.taker_amount));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.expiration));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.nonce));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.fee_rate_bps as u64));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.side as u64));
    encoded.extend_from_slice(&abi_encode_u256_from_u64(order.signature_type as u64));

    keccak256(&encoded)
}

/// Computes the final EIP-712 signing hash.
///
/// `keccak256("\x19\x01" || domainSeparator || structHash)`
pub fn compute_signing_hash(domain_separator: &[u8; 32], struct_hash: &[u8; 32]) -> [u8; 32] {
    let mut data = Vec::with_capacity(2 + 32 + 32);
    data.push(0x19);
    data.push(0x01);
    data.extend_from_slice(domain_separator);
    data.extend_from_slice(struct_hash);
    keccak256(&data)
}

// =============================================================================
// ECDSA Signing
// =============================================================================

/// Signs an order with the given private key.
///
/// Returns the hex-encoded signature string `0x{r}{s}{v}` (65 bytes = 130 hex + 2 prefix).
pub fn sign_order(
    order: &Eip712Order,
    config: &Eip712Config,
    private_key_hex: &str,
) -> Result<String, Eip712Error> {
    let domain_separator = compute_domain_separator(config.chain_id, config.exchange_address())?;
    let struct_hash = compute_order_struct_hash(order);
    let signing_hash = compute_signing_hash(&domain_separator, &struct_hash);

    sign_hash(&signing_hash, private_key_hex)
}

/// Signs a raw 32-byte hash with ECDSA using k256.
///
/// Returns `0x{r}{s}{v}` where v is 27 or 28.
pub fn sign_hash(hash: &[u8; 32], private_key_hex: &str) -> Result<String, Eip712Error> {
    use k256::ecdsa::SigningKey;

    let key_hex = private_key_hex
        .strip_prefix("0x")
        .unwrap_or(private_key_hex);
    let key_bytes =
        hex::decode(key_hex).map_err(|e| Eip712Error::InvalidKey(format!("Invalid hex: {}", e)))?;

    let signing_key = SigningKey::from_slice(&key_bytes)
        .map_err(|e| Eip712Error::InvalidKey(format!("Invalid key: {}", e)))?;

    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(hash)
        .map_err(|e| Eip712Error::SigningFailed(format!("ECDSA sign failed: {}", e)))?;

    let r_bytes = signature.r().to_bytes();
    let s_bytes = signature.s().to_bytes();
    let v = recovery_id.to_byte() + 27; // Ethereum convention

    let mut sig_bytes = Vec::with_capacity(65);
    sig_bytes.extend_from_slice(&r_bytes);
    sig_bytes.extend_from_slice(&s_bytes);
    sig_bytes.push(v);

    Ok(format!("0x{}", hex::encode(sig_bytes)))
}

// =============================================================================
// Amount calculations
// =============================================================================

/// Calculates maker and taker amounts from price and size.
///
/// Amounts are in USDC raw units (6 decimals, so $1.00 = 1_000_000).
///
/// Matches the Polymarket Python SDK's `get_order_amounts()`:
/// 1. Round size down to 2 decimal places
/// 2. Compute amounts in natural units (USDC / shares)
/// 3. Round to meet API precision requirements
/// 4. Convert to raw units (* 10^6)
///
/// - BUY: taker_amount = size (shares we receive), maker_amount = size * price (USDC we pay)
/// - SELL: maker_amount = size (shares we give), taker_amount = size * price (USDC we receive)
///
/// Matches Python SDK's ROUNDING_CONFIG for $0.01 tick markets:
///   price=2dp (round_normal), size=2dp (round_down), amount=4dp
/// The product of 2dp × 2dp has at most 4dp, so no further rounding is needed.
pub fn calculate_amounts(
    side: u8,
    price: Decimal,
    size: Decimal,
) -> Result<(u64, u64), Eip712Error> {
    let scale = Decimal::from(10u64.pow(USDC_DECIMALS));

    if price <= Decimal::ZERO || price >= Decimal::ONE {
        return Err(Eip712Error::AmountError(format!(
            "Price must be in (0, 1), got {}",
            price
        )));
    }
    if size <= Decimal::ZERO {
        return Err(Eip712Error::AmountError(
            "Size must be positive".to_string(),
        ));
    }

    // Match Python SDK: round price to tick (2dp for $0.01 markets)
    let price_tick = round_normal(price, 2);
    // Round size down to 2 decimal places (matching Python SDK's round_config.size=2)
    let size_rounded = round_down(size, 2);

    let (maker_amount, taker_amount) = if side == SIDE_BUY {
        // BUY: taker = shares we receive, maker = USDC we pay
        let taker_natural = size_rounded;
        // Full precision: 2dp × 2dp = 4dp max (matches round_config.amount=4)
        let maker_natural = taker_natural * price_tick;
        let taker_raw = (taker_natural * scale).floor();
        let maker_raw = (maker_natural * scale).floor();
        (maker_raw, taker_raw)
    } else {
        // SELL: maker = shares we give, taker = USDC we receive
        let maker_natural = size_rounded;
        // Full precision: 2dp × 2dp = 4dp max (matches round_config.amount=4)
        let taker_natural = maker_natural * price_tick;
        let maker_raw = (maker_natural * scale).floor();
        let taker_raw = (taker_natural * scale).floor();
        (maker_raw, taker_raw)
    };

    let maker_u64 = decimal_to_u64(maker_amount)?;
    let taker_u64 = decimal_to_u64(taker_amount)?;

    Ok((maker_u64, taker_u64))
}

/// Rounds a Decimal down (floor) to the given number of decimal places.
fn round_down(value: Decimal, dp: u32) -> Decimal {
    let factor = Decimal::from(10u64.pow(dp));
    (value * factor).floor() / factor
}

/// Rounds a Decimal to nearest (half-up) to the given number of decimal places.
/// Matches Python SDK's `round_normal`.
fn round_normal(value: Decimal, dp: u32) -> Decimal {
    let factor = Decimal::from(10u64.pow(dp));
    (value * factor).round() / factor
}

fn decimal_to_u64(d: Decimal) -> Result<u64, Eip712Error> {
    d.to_string()
        .split('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| Eip712Error::AmountError(format!("Cannot convert {} to u64", d)))
}

// =============================================================================
// Salt generation
// =============================================================================

/// Generates a random salt for order uniqueness.
///
/// Matches the Polymarket Python SDK convention: `round(timestamp * random())`.
/// This produces a small number that fits in a JSON integer.
pub fn generate_salt() -> u64 {
    use rand::Rng;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let random: f64 = rand::thread_rng().gen();
    ((now as f64) * random) as u64
}

// =============================================================================
// ABI encoding helpers
// =============================================================================

/// Parses a hex address string (with or without 0x prefix) to 20 bytes.
pub fn parse_address(addr: &str) -> Result<[u8; 20], Eip712Error> {
    let hex_str = addr.strip_prefix("0x").unwrap_or(addr);
    let bytes = hex::decode(hex_str)
        .map_err(|e| Eip712Error::InvalidAddress(format!("Invalid hex: {}", e)))?;
    if bytes.len() != 20 {
        return Err(Eip712Error::InvalidAddress(format!(
            "Address must be 20 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// ABI-encodes an address as a 32-byte left-padded value.
fn abi_encode_address(addr: &[u8; 20]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..32].copy_from_slice(addr);
    out
}

/// ABI-encodes a u64 as a 32-byte big-endian value.
fn abi_encode_u256_from_u64(value: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..32].copy_from_slice(&value.to_be_bytes());
    out
}

/// ABI-encodes a bytes32 value (already 32 bytes).
fn abi_encode_bytes32(value: &[u8; 32]) -> [u8; 32] {
    *value
}

/// Converts a token ID string to a 32-byte big-endian uint256.
///
/// Token IDs are large numeric strings. We parse them as a decimal number
/// and convert to big-endian bytes.
fn token_id_to_u256(token_id: &str) -> [u8; 32] {
    // Token IDs can be very large (up to 2^256 - 1), so we do manual base-10 to bytes
    let mut result = [0u8; 32];

    // Parse the decimal string into a big-endian byte array
    let mut digits: Vec<u8> = token_id
        .bytes()
        .filter_map(|b| {
            if b.is_ascii_digit() {
                Some(b - b'0')
            } else {
                None
            }
        })
        .collect();

    if digits.is_empty() {
        return result;
    }

    // Convert decimal digits to binary using repeated division by 256
    let mut byte_vec = Vec::new();
    while !(digits.is_empty() || digits.len() == 1 && digits[0] == 0) {
        let mut remainder = 0u16;
        let mut new_digits = Vec::new();
        for &digit in &digits {
            let current = remainder * 10 + digit as u16;
            let quotient = current / 256;
            remainder = current % 256;
            if !new_digits.is_empty() || quotient > 0 {
                new_digits.push(quotient as u8);
            }
        }
        byte_vec.push(remainder as u8);
        digits = new_digits;
    }

    // byte_vec is in little-endian order, reverse into result (big-endian, right-aligned)
    let start = 32 - byte_vec.len().min(32);
    for (i, &b) in byte_vec.iter().rev().enumerate() {
        if start + i < 32 {
            result[start + i] = b;
        }
    }

    result
}

/// Parameters for building an EIP-712 order.
pub struct BuildOrderParams<'a> {
    /// Maker (wallet) address as hex string.
    pub maker_address: &'a str,
    /// Token ID for the conditional token.
    pub token_id: &'a str,
    /// Side: `SIDE_BUY` (0) or `SIDE_SELL` (1).
    pub side: u8,
    /// Price per share as a Decimal (0..1).
    pub price: Decimal,
    /// Number of shares (in whole units, scaled to 10^6 internally).
    pub size: Decimal,
    /// Order expiration in seconds since epoch.
    pub expiration_secs: u64,
    /// Nonce for order uniqueness.
    pub nonce: u64,
    /// Fee rate in basis points (e.g., 100 = 1%).
    pub fee_rate_bps: u16,
}

/// Creates a new order with common defaults filled in.
pub fn build_order(params: &BuildOrderParams<'_>) -> Result<Eip712Order, Eip712Error> {
    let maker = parse_address(params.maker_address)?;
    let (maker_amount, taker_amount) = calculate_amounts(params.side, params.price, params.size)?;

    Ok(Eip712Order {
        salt: generate_salt(),
        maker,
        signer: maker, // Same as maker for EOA
        taker: ZERO_ADDRESS,
        token_id: params.token_id.to_string(),
        maker_amount,
        taker_amount,
        expiration: params.expiration_secs,
        nonce: params.nonce,
        fee_rate_bps: params.fee_rate_bps,
        side: params.side,
        signature_type: SIGNATURE_TYPE_EOA,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // -------------------------------------------------------------------------
    // Hashing tests
    // -------------------------------------------------------------------------

    #[test]
    fn domain_separator_standard_is_deterministic() {
        let ds1 = compute_domain_separator(137, STANDARD_EXCHANGE).unwrap();
        let ds2 = compute_domain_separator(137, STANDARD_EXCHANGE).unwrap();
        assert_eq!(ds1, ds2);
        assert_ne!(ds1, [0u8; 32]);
    }

    #[test]
    fn domain_separator_differs_for_neg_risk() {
        let standard = compute_domain_separator(137, STANDARD_EXCHANGE).unwrap();
        let neg_risk = compute_domain_separator(137, NEG_RISK_EXCHANGE).unwrap();
        assert_ne!(standard, neg_risk);
    }

    #[test]
    fn domain_separator_differs_for_chain_id() {
        let mainnet = compute_domain_separator(137, STANDARD_EXCHANGE).unwrap();
        let amoy = compute_domain_separator(80002, STANDARD_EXCHANGE).unwrap();
        assert_ne!(mainnet, amoy);
    }

    #[test]
    fn order_type_hash_is_not_zero() {
        let hash = order_type_hash();
        assert_ne!(hash, [0u8; 32]);
    }

    #[test]
    fn order_struct_hash_deterministic() {
        let order = make_test_order();
        let h1 = compute_order_struct_hash(&order);
        let h2 = compute_order_struct_hash(&order);
        assert_eq!(h1, h2);
        assert_ne!(h1, [0u8; 32]);
    }

    #[test]
    fn order_struct_hash_changes_with_price() {
        let mut o1 = make_test_order();
        let mut o2 = make_test_order();
        o1.maker_amount = 500_000;
        o2.maker_amount = 600_000;
        assert_ne!(
            compute_order_struct_hash(&o1),
            compute_order_struct_hash(&o2)
        );
    }

    #[test]
    fn signing_hash_includes_domain_prefix() {
        let domain = [1u8; 32];
        let struct_hash = [2u8; 32];
        let hash = compute_signing_hash(&domain, &struct_hash);
        assert_ne!(hash, [0u8; 32]);
    }

    // -------------------------------------------------------------------------
    // Amount calculation tests
    // -------------------------------------------------------------------------

    #[test]
    fn calculate_amounts_buy_side() {
        // BUY: price=0.50, size=100 shares
        // taker_amount = 100 * 10^6 = 100_000_000
        // maker_amount = 100_000_000 * 0.50 = 50_000_000
        let (maker, taker) = calculate_amounts(SIDE_BUY, dec!(0.50), dec!(100)).unwrap();
        assert_eq!(taker, 100_000_000);
        assert_eq!(maker, 50_000_000);
    }

    #[test]
    fn calculate_amounts_sell_side() {
        // SELL: price=0.60, size=50 shares
        // maker_amount = 50 * 10^6 = 50_000_000
        // taker_amount = 50_000_000 * 0.60 = 30_000_000
        let (maker, taker) = calculate_amounts(SIDE_SELL, dec!(0.60), dec!(50)).unwrap();
        assert_eq!(maker, 50_000_000);
        assert_eq!(taker, 30_000_000);
    }

    #[test]
    fn calculate_amounts_rounds_correctly() {
        // BUY: price=0.19, size=10.752688 (from real trading scenario)
        // size_rounded = 10.75, taker = 10.75 * 10^6 = 10_750_000
        // price_tick = round_normal(0.19, 2) = 0.19
        // maker = 10.75 * 0.19 = 2.0425 (full 4dp precision, matching Python SDK amount=4)
        // maker_raw = 2.0425 * 10^6 = 2_042_500
        let (maker, taker) =
            calculate_amounts(SIDE_BUY, dec!(0.19), dec!(10.752688)).unwrap();
        assert_eq!(taker, 10_750_000); // Rounded down to 2 dp
        assert_eq!(maker, 2_042_500); // Full precision: 10.75 * 0.19 = 2.0425
    }

    #[test]
    fn calculate_amounts_sell_subcent_price_snaps_to_tick() {
        // Regression test: escape sell at sub-cent price 0.1995 was rejected by API.
        // round_normal(0.1995, 2) = 0.20, so amounts use price=0.20.
        // SELL: price=0.20, size=5.8823529... → size_rounded=5.88
        // maker = 5.88 * 10^6 = 5_880_000
        // taker = 5.88 * 0.20 = 1.176 * 10^6 = 1_176_000
        let (maker, taker) =
            calculate_amounts(SIDE_SELL, dec!(0.1995), dec!(5.8823529411764705)).unwrap();
        assert_eq!(maker, 5_880_000);
        assert_eq!(taker, 1_176_000); // 5.88 * 0.20 = 1.176 (not truncated to 1.17)
    }

    #[test]
    fn calculate_amounts_sell_exact_cent_price() {
        // SELL at exact cent price: no rounding needed
        // size=5.88, price=0.19
        // taker = 5.88 * 0.19 = 1.1172 * 10^6 = 1_117_200
        let (maker, taker) =
            calculate_amounts(SIDE_SELL, dec!(0.19), dec!(5.88)).unwrap();
        assert_eq!(maker, 5_880_000);
        assert_eq!(taker, 1_117_200); // Full 4dp precision (was 1_110_000 with old 2dp rounding)
    }

    #[test]
    fn calculate_amounts_rejects_invalid_price() {
        assert!(calculate_amounts(SIDE_BUY, dec!(0.00), dec!(100)).is_err());
        assert!(calculate_amounts(SIDE_BUY, dec!(1.00), dec!(100)).is_err());
        assert!(calculate_amounts(SIDE_BUY, dec!(-0.50), dec!(100)).is_err());
    }

    #[test]
    fn calculate_amounts_rejects_zero_size() {
        assert!(calculate_amounts(SIDE_BUY, dec!(0.50), dec!(0)).is_err());
    }

    // -------------------------------------------------------------------------
    // Salt generation
    // -------------------------------------------------------------------------

    #[test]
    fn generate_salt_not_zero() {
        let salt = generate_salt();
        assert_ne!(salt, 0);
    }

    #[test]
    fn generate_salt_unique() {
        let s1 = generate_salt();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let s2 = generate_salt();
        assert_ne!(s1, s2);
    }

    // -------------------------------------------------------------------------
    // Address parsing
    // -------------------------------------------------------------------------

    #[test]
    fn parse_address_with_prefix() {
        let addr = parse_address("0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E").unwrap();
        assert_eq!(addr.len(), 20);
        assert_eq!(addr[0], 0x4b);
    }

    #[test]
    fn parse_address_without_prefix() {
        let addr = parse_address("4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E").unwrap();
        assert_eq!(addr[0], 0x4b);
    }

    #[test]
    fn parse_address_rejects_wrong_length() {
        assert!(parse_address("0x1234").is_err());
    }

    // -------------------------------------------------------------------------
    // Token ID conversion
    // -------------------------------------------------------------------------

    #[test]
    fn token_id_to_u256_small_number() {
        let result = token_id_to_u256("256");
        // 256 = 0x0100
        assert_eq!(result[30], 1);
        assert_eq!(result[31], 0);
    }

    #[test]
    fn token_id_to_u256_zero() {
        let result = token_id_to_u256("0");
        assert_eq!(result, [0u8; 32]);
    }

    #[test]
    fn token_id_to_u256_large_number() {
        // Test a typical Polymarket token ID (large number)
        let result = token_id_to_u256("1000000");
        assert_ne!(result, [0u8; 32]);
        // 1_000_000 = 0x0F4240
        assert_eq!(result[29], 0x0F);
        assert_eq!(result[30], 0x42);
        assert_eq!(result[31], 0x40);
    }

    // -------------------------------------------------------------------------
    // Signing tests
    // -------------------------------------------------------------------------

    #[test]
    fn sign_order_produces_valid_length() {
        // Use a test private key (DO NOT use in production)
        let test_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let order = make_test_order();
        let config = Eip712Config::default();

        let sig = sign_order(&order, &config, test_key).unwrap();

        // Should be "0x" + 130 hex chars = 132 chars total (65 bytes)
        assert!(sig.starts_with("0x"));
        assert_eq!(sig.len(), 132);
    }

    #[test]
    fn sign_order_deterministic_with_same_salt() {
        let test_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let order = make_test_order();
        let config = Eip712Config::default();

        let sig1 = sign_order(&order, &config, test_key).unwrap();
        let sig2 = sign_order(&order, &config, test_key).unwrap();
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn sign_order_differs_for_different_orders() {
        let test_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let config = Eip712Config::default();

        let o1 = make_test_order();
        let mut o2 = make_test_order();
        o2.maker_amount = 999_999;

        let sig1 = sign_order(&o1, &config, test_key).unwrap();
        let sig2 = sign_order(&o2, &config, test_key).unwrap();
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn sign_order_handles_0x_prefix() {
        let test_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let order = make_test_order();
        let config = Eip712Config::default();

        let sig = sign_order(&order, &config, test_key).unwrap();
        assert!(sig.starts_with("0x"));
        assert_eq!(sig.len(), 132);
    }

    #[test]
    fn sign_order_rejects_invalid_key() {
        let result = sign_order(&make_test_order(), &Eip712Config::default(), "deadbeef");
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // Build order tests
    // -------------------------------------------------------------------------

    #[test]
    fn build_order_success() {
        let order = build_order(&BuildOrderParams {
            maker_address: "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E",
            token_id: "12345",
            side: SIDE_BUY,
            price: dec!(0.50),
            size: dec!(100),
            expiration_secs: 1700000000,
            nonce: 0,
            fee_rate_bps: 0,
        })
        .unwrap();

        assert_eq!(order.side, SIDE_BUY);
        assert_eq!(order.signature_type, SIGNATURE_TYPE_EOA);
        assert_eq!(order.taker, ZERO_ADDRESS);
        assert_eq!(order.maker, order.signer);
        assert_ne!(order.salt, 0); // Random salt
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn make_test_order() -> Eip712Order {
        Eip712Order {
            salt: 42,
            maker: [1u8; 20],
            signer: [1u8; 20],
            taker: ZERO_ADDRESS,
            token_id: "12345".to_string(),
            maker_amount: 500_000,
            taker_amount: 1_000_000,
            expiration: 1700000000,
            nonce: 0,
            fee_rate_bps: 0,
            side: SIDE_BUY,
            signature_type: SIGNATURE_TYPE_EOA,
        }
    }

    // -------------------------------------------------------------------------
    // Python SDK test vector compatibility
    // -------------------------------------------------------------------------

    /// Test vector from Polymarket/python-order-utils test_order_builder.py.
    /// Uses Amoy testnet (chain_id=80002) with known key and order params.
    #[test]
    fn test_vector_standard_exchange_signing_hash_and_signature() {
        let private_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let maker = parse_address("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
        let exchange = "0xdFE02Eb6733538f8Ea35D585af8DE5958AD99E40";
        let chain_id: u64 = 80002;

        let order = Eip712Order {
            salt: 479249096354,
            maker,
            signer: maker,
            taker: ZERO_ADDRESS,
            token_id: "1234".to_string(),
            maker_amount: 100_000_000,
            taker_amount: 50_000_000,
            expiration: 0,
            nonce: 0,
            fee_rate_bps: 100,
            side: SIDE_BUY,
            signature_type: SIGNATURE_TYPE_EOA,
        };

        // Compute intermediate hashes for debugging
        let type_hash = order_type_hash();
        eprintln!("Order type hash: 0x{}", hex::encode(type_hash));

        let domain_sep = compute_domain_separator(chain_id, exchange).unwrap();
        eprintln!("Domain separator: 0x{}", hex::encode(domain_sep));

        let struct_hash = compute_order_struct_hash(&order);
        eprintln!("Struct hash: 0x{}", hex::encode(struct_hash));

        let signing_hash = compute_signing_hash(&domain_sep, &struct_hash);
        eprintln!("Signing hash: 0x{}", hex::encode(signing_hash));

        // Python SDK _create_struct_hash returns the FINAL signing hash
        // (confusing name - it's keccak256(\x19\x01 || domainSep || structHash))
        let expected_signing_hash =
            "02ca1d1aa31103804173ad1acd70066cb6c1258a4be6dada055111f9a7ea4e55";
        assert_eq!(
            hex::encode(signing_hash),
            expected_signing_hash,
            "Signing hash mismatch with Python SDK test vector"
        );

        // Verify signature matches
        let signature = sign_hash(&signing_hash, private_key).unwrap();
        let expected_signature = "0x302cd9abd0b5fcaa202a344437ec0b6660da984e24ae9ad915a592a90facf5a51bb8a873cd8d270f070217fea1986531d5eec66f1162a81f66e026db653bf7ce1c";
        assert_eq!(
            signature, expected_signature,
            "Signature mismatch with Python SDK test vector"
        );
    }

    /// Test vector for neg-risk exchange (same order, different domain).
    #[test]
    fn test_vector_neg_risk_exchange_signing_hash_and_signature() {
        let private_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let maker = parse_address("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
        let exchange = "0xC5d563A36AE78145C45a50134d48A1215220f80a"; // neg-risk
        let chain_id: u64 = 80002;

        let order = Eip712Order {
            salt: 479249096354,
            maker,
            signer: maker,
            taker: ZERO_ADDRESS,
            token_id: "1234".to_string(),
            maker_amount: 100_000_000,
            taker_amount: 50_000_000,
            expiration: 0,
            nonce: 0,
            fee_rate_bps: 100,
            side: SIDE_BUY,
            signature_type: SIGNATURE_TYPE_EOA,
        };

        let domain_sep = compute_domain_separator(chain_id, exchange).unwrap();
        let struct_hash = compute_order_struct_hash(&order);
        let signing_hash = compute_signing_hash(&domain_sep, &struct_hash);

        let expected_signing_hash =
            "f15790d3edc4b5aed427b0b543a9206fcf4b1a13dfed016d33bfb313076263b8";
        assert_eq!(
            hex::encode(signing_hash),
            expected_signing_hash,
            "Neg-risk signing hash mismatch"
        );

        let signature = sign_hash(&signing_hash, private_key).unwrap();
        let expected_signature = "0x1b3646ef347e5bd144c65bd3357ba19c12c12abaeedae733cf8579bc51a2752c0454c3bc6b236957e393637982c769b8dc0706c0f5c399983d933850afd1cbcd1c";
        assert_eq!(
            signature, expected_signature,
            "Neg-risk signature mismatch"
        );
    }
}
