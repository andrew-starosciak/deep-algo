# Security Guidelines

## Mandatory Security Checks

Before ANY commit involving sensitive code:
- [ ] No hardcoded API keys or secrets
- [ ] No private keys in code or config
- [ ] Wallet credentials use environment variables
- [ ] Rate limiting prevents abuse
- [ ] Error messages don't leak sensitive data

## Secret Management

```rust
// NEVER: Hardcoded secrets
let api_key = "binance-api-key-12345";

// ALWAYS: Environment variables
let api_key = std::env::var("BINANCE_API_KEY")
    .expect("BINANCE_API_KEY must be set");

// BETTER: With proper error handling
let api_key = std::env::var("BINANCE_API_KEY")
    .map_err(|_| ConfigError::MissingApiKey("BINANCE_API_KEY"))?;
```

## Wallet Security

### Private Key Handling
- NEVER log private keys
- NEVER include in error messages
- Use secure memory (zeroize on drop)
- Separate hot/cold wallet concerns

```rust
use zeroize::Zeroize;

#[derive(Zeroize)]
#[zeroize(drop)]
pub struct WalletCredentials {
    private_key: String,
}
```

### Transaction Signing
- Validate all transaction parameters
- Implement maximum transaction limits
- Require confirmation for large amounts

## API Security

### Rate Limiting
```rust
use governor::{Quota, RateLimiter};

let limiter = RateLimiter::direct(Quota::per_minute(nonzero!(1200u32)));
```

### Request Signing
- Use HMAC-SHA256 for Binance
- Use EIP-712 for Polymarket/Ethereum
- Validate all responses

## Environment Files

```bash
# .env.example (commit this)
BINANCE_API_KEY=your-api-key-here
BINANCE_SECRET_KEY=your-secret-key-here
POLYMARKET_PRIVATE_KEY=your-private-key-here
DATABASE_URL=postgres://user:pass@localhost/db

# .env (NEVER commit)
# Add to .gitignore
```

## Security Response Protocol

If security issue found:
1. **STOP** - Do not deploy
2. **Rotate** - Any exposed credentials immediately
3. **Audit** - Check for similar issues
4. **Document** - Record incident
5. **Fix** - Apply security patch

## Audit Checklist

### Before Production
- [ ] All secrets in environment variables
- [ ] Rate limiting on all external APIs
- [ ] Maximum bet size limits enforced
- [ ] Wallet balance checks before transactions
- [ ] Transaction confirmations required
- [ ] Error handling doesn't expose internals
- [ ] Logging doesn't include sensitive data
