---
name: security-review
description: Security review for trading systems. Wallet security, API credentials, rate limiting, and transaction safety.
---

# Security Review

Security patterns for cryptocurrency trading systems.

## When to Activate

- Implementing wallet integration
- Handling API credentials
- Processing transactions
- Setting up authentication
- Before production deployment

## Critical Security Areas

### 1. Private Key Management

**NEVER:**
- Hardcode private keys
- Log private keys
- Include in error messages
- Store in version control
- Transmit unencrypted

**ALWAYS:**
```rust
use zeroize::Zeroize;

#[derive(Zeroize)]
#[zeroize(drop)]
pub struct WalletCredentials {
    private_key: String,
}

impl WalletCredentials {
    pub fn from_env() -> Result<Self, ConfigError> {
        let key = std::env::var("WALLET_PRIVATE_KEY")
            .map_err(|_| ConfigError::MissingCredential("WALLET_PRIVATE_KEY"))?;

        Ok(Self { private_key: key })
    }

    // Sign without exposing key
    pub fn sign(&self, message: &[u8]) -> Signature {
        // Use key internally, never return it
    }
}
```

### 2. API Credential Security

```rust
// Environment-based configuration
pub struct ExchangeConfig {
    api_key: String,
    secret_key: String,
}

impl ExchangeConfig {
    pub fn from_env(prefix: &str) -> Result<Self> {
        Ok(Self {
            api_key: std::env::var(format!("{}_API_KEY", prefix))?,
            secret_key: std::env::var(format!("{}_SECRET_KEY", prefix))?,
        })
    }
}

// .env file (NEVER commit)
// BINANCE_API_KEY=xxx
// BINANCE_SECRET_KEY=xxx
// POLYMARKET_PRIVATE_KEY=xxx
```

### 3. Request Signing

```rust
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn sign_request(secret: &str, payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

// For Polymarket/Ethereum: EIP-712 typed data signing
pub fn sign_typed_data(
    wallet: &WalletCredentials,
    domain: &EIP712Domain,
    message: &TypedMessage,
) -> Signature {
    // Proper EIP-712 implementation
}
```

### 4. Rate Limiting

```rust
use governor::{Quota, RateLimiter, clock::DefaultClock};
use nonzero_ext::nonzero;

pub struct RateLimitedClient {
    limiter: RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
    client: reqwest::Client,
}

impl RateLimitedClient {
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            limiter: RateLimiter::direct(
                Quota::per_minute(nonzero!(requests_per_minute))
            ),
            client: reqwest::Client::new(),
        }
    }

    pub async fn request(&self, req: Request) -> Result<Response> {
        self.limiter.until_ready().await;
        self.client.execute(req).await.map_err(Into::into)
    }
}
```

### 5. Transaction Safety

```rust
pub struct TransactionLimits {
    max_single_bet: Decimal,
    max_daily_volume: Decimal,
    max_position_size: Decimal,
}

impl TransactionLimits {
    pub fn validate_bet(&self, bet: &Bet, daily_total: Decimal) -> Result<()> {
        if bet.stake > self.max_single_bet {
            return Err(SecurityError::ExceedsMaxBet {
                requested: bet.stake,
                limit: self.max_single_bet,
            });
        }

        if daily_total + bet.stake > self.max_daily_volume {
            return Err(SecurityError::ExceedsDailyLimit {
                current: daily_total,
                requested: bet.stake,
                limit: self.max_daily_volume,
            });
        }

        Ok(())
    }
}
```

### 6. Input Validation

```rust
pub fn validate_order(order: &OrderRequest) -> Result<()> {
    // Validate market ID format
    if !order.market_id.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err(ValidationError::InvalidMarketId);
    }

    // Validate price bounds
    if order.price <= Decimal::ZERO || order.price > Decimal::ONE {
        return Err(ValidationError::InvalidPrice);
    }

    // Validate stake
    if order.stake <= Decimal::ZERO {
        return Err(ValidationError::InvalidStake);
    }

    Ok(())
}
```

## Security Checklist

### Before Production Deployment

**Credentials:**
- [ ] All secrets in environment variables
- [ ] No hardcoded keys in code
- [ ] .env files in .gitignore
- [ ] Secrets use secure memory (zeroize)

**Network:**
- [ ] All API calls use HTTPS
- [ ] Certificate validation enabled
- [ ] Rate limiting implemented
- [ ] Timeout handling in place

**Transactions:**
- [ ] Maximum bet limits enforced
- [ ] Daily volume limits enforced
- [ ] Balance checked before transactions
- [ ] Transaction confirmation required

**Error Handling:**
- [ ] No sensitive data in error messages
- [ ] No stack traces exposed to users
- [ ] Proper logging (no secrets logged)
- [ ] Graceful degradation on failures

**Authentication:**
- [ ] Request signing implemented correctly
- [ ] Nonces used for replay protection
- [ ] Timestamp validation in requests

## Logging Security

```rust
use tracing::{info, error};

// WRONG: Logs sensitive data
error!("Auth failed for key: {}", api_key);

// CORRECT: Logs safely
error!(
    key_prefix = %&api_key[..8],
    "Authentication failed"
);

// WRONG: Logs transaction details
info!("Sending {} to {}", amount, wallet_address);

// CORRECT: Minimal logging
info!(
    tx_type = "withdrawal",
    amount_usd = %amount,
    "Transaction initiated"
);
```

## Incident Response

### If Credentials Exposed:

1. **Immediate:**
   - Revoke exposed credentials
   - Generate new credentials
   - Update environment variables

2. **Investigation:**
   - Check for unauthorized transactions
   - Review access logs
   - Identify exposure source

3. **Remediation:**
   - Fix root cause
   - Add detection for similar issues
   - Document incident

### If Unusual Activity Detected:

1. **Pause trading** - Stop all bots immediately
2. **Check balances** - Verify all funds
3. **Review logs** - Look for unauthorized actions
4. **Rotate credentials** - If any doubt

## Environment Setup

```bash
# .env.example (commit this)
# Copy to .env and fill in values

# Binance API
BINANCE_API_KEY=your-api-key
BINANCE_SECRET_KEY=your-secret-key

# Polymarket / Polygon
POLYMARKET_PRIVATE_KEY=your-private-key

# Database
DATABASE_URL=postgres://user:pass@localhost/trading

# Optional: Coinglass, CryptoPanic, etc.
COINGLASS_API_KEY=your-api-key
```

```gitignore
# .gitignore
.env
.env.local
*.pem
*.key
secrets/
```
