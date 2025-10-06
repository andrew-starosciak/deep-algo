# Phase 2: Wallet Integration Tasks (19-30)

**Generated**: 2025-10-05
**Context Report**: `/home/a/Work/algo-trade/.claude/context/2025-10-05_hyperliquid-sdk-integration.md`
**Phase Objective**: Implement EIP-712 wallet signing for authenticated Hyperliquid order execution

---

## Task Dependency Graph

```
Task 19 (deps) → Task 20 (signing) → Task 21 (lib.rs) ──┐
                                                          │
Task 22 (wallet utils) ───────────────────────────────────┤
                                                          ▼
                                           Task 23 (client fields) → Task 24 (with_wallet) ──┐
                                                                                              │
Task 25 (asset_map) ──────────────────────────────────────────────────────────────────────┬─┤
                                                                                           │ │
Task 26 (post_signed) ────────────────────────────────────────────────────────────────────┤ │
                                                                                           ▼ ▼
Task 27 (handler struct) → Task 28 (execute_order) ←─────────────────────────────────────┘ │
                                                                                             │
Task 29 (bot init) ← Task 30 (env validation) ←──────────────────────────────────────────────┘
```

---

## Atomic Tasks

### Task 19: Add EIP-712 dependencies to Cargo.toml
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml`
**Location**: After line 22 (after `ethers = "2.0"`)
**Action**:
1. Add three new dependencies:
   ```toml
   ethers-contract = "2.0"         # For EthAbiType derive macro
   ethers-derive-eip712 = "2.0"    # For Eip712 derive macro
   hex = "0.4"                     # For signature hex encoding
   ```
2. Maintain alphabetical ordering within dependency section
**Code Reference**: Section 7.4 in context report
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 2 minutes
**Estimated LOC**: 3 lines
**Dependencies**: None

---

### Task 20: Create signing module with EIP-712 struct
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/signing.rs` (NEW FILE)
**Location**: New file
**Action**:
1. Create new file `signing.rs` with exact content from Section 7.4:
   ```rust
   use ethers_contract::EthAbiType;
   use ethers_derive_eip712::Eip712;

   /// EIP-712 typed data structure for Hyperliquid L1 action signing
   ///
   /// # Important
   /// - chain_id = 1337 (Hyperliquid-specific, NOT Arbitrum's 42161)
   /// - verifying_contract = zero address (Hyperliquid convention)
   /// - phantom_agent is keccak256 hash of serialized action
   #[derive(Debug, Clone, Eip712, EthAbiType)]
   #[eip712(
       name = "Exchange",
       version = "1",
       chain_id = 1337,
       verifying_contract = "0x0000000000000000000000000000000000000000"
   )]
   pub struct L1Action {
       /// Keccak256 hash of serialized action (JSON or msgpack)
       pub phantom_agent: [u8; 32],
       /// true for mainnet, false for testnet
       pub is_mainnet: bool,
   }
   ```
**Code Reference**: Section 7.4
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 3 minutes
**Estimated LOC**: 15 lines
**Dependencies**: Task 19

---

### Task 21: Add signing module to lib.rs
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs`
**Location**: After existing `pub mod` declarations (find with `pub mod client;`)
**Action**:
1. Add module declaration: `pub mod signing;`
2. Place after `pub mod client;` line
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 1 minute
**Estimated LOC**: 1 line
**Dependencies**: Task 20

---

### Task 22: Create wallet utility module
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/wallet.rs` (NEW FILE)
**Location**: New file
**Action**:
1. Create new file `wallet.rs` with exact content from Section 7.1:
   ```rust
   use ethers::signers::{LocalWallet, Signer};
   use anyhow::{Context, Result};

   /// Load wallet from HYPERLIQUID_PRIVATE_KEY environment variable
   ///
   /// # Errors
   /// - Returns error if env var not set
   /// - Returns error if private key format invalid (must be 64 hex chars, with or without 0x prefix)
   pub fn wallet_from_env() -> Result<LocalWallet> {
       let private_key = std::env::var("HYPERLIQUID_PRIVATE_KEY")
           .context("HYPERLIQUID_PRIVATE_KEY not set")?;

       let wallet = private_key
           .trim()
           .trim_start_matches("0x")
           .parse::<LocalWallet>()
           .context("Invalid private key format (expected 64 hex chars)")?;

       Ok(wallet)
   }

   /// Validate Ethereum address format
   ///
   /// # Errors
   /// - Returns error if address format invalid
   pub fn validate_address(address: &str) -> Result<()> {
       use ethers::types::Address;
       address.parse::<Address>()
           .context("Invalid Ethereum address format")?;
       Ok(())
   }
   ```
2. Add module declaration to lib.rs: `pub mod wallet;`
**Code Reference**: Section 7.1
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 4 minutes
**Estimated LOC**: 25 lines
**Dependencies**: Task 19 (for ethers imports)

---

### Task 23: Add wallet and asset_map fields to HyperliquidClient
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: Lines 15-19 (struct definition)
**Action**:
1. Add imports at top of file:
   ```rust
   use ethers::signers::LocalWallet;
   use std::collections::HashMap;
   use std::sync::Arc;
   ```
2. Add two new fields to `HyperliquidClient` struct (after `rate_limiter` field):
   ```rust
   wallet: Option<LocalWallet>,
   asset_map: Option<Arc<HashMap<String, u32>>>,
   ```
3. Update `new()` constructor to initialize new fields:
   ```rust
   wallet: None,
   asset_map: None,
   ```
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 3 minutes
**Estimated LOC**: 5 lines (3 imports + 2 fields)
**Dependencies**: Task 22 (for LocalWallet type)

---

### Task 24: Add HyperliquidClient::with_wallet() constructor
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After line 37 (after existing `new()` method)
**Action**:
1. Add new constructor method:
   ```rust
   /// Create client with authenticated wallet for order signing
   ///
   /// Automatically builds asset index mapping from /info endpoint
   ///
   /// # Errors
   /// - Returns error if asset map fetch fails
   pub async fn with_wallet(
       base_url: String,
       wallet: LocalWallet,
   ) -> Result<Self> {
       let quota = Quota::per_second(NonZeroU32::new(20).unwrap());
       let rate_limiter = Arc::new(RateLimiter::direct(quota));

       let mut client = Self {
           http_client: Client::new(),
           base_url: base_url.clone(),
           rate_limiter,
           wallet: Some(wallet),
           asset_map: None,
       };

       // Build asset map immediately
       let asset_map = client.build_asset_map().await?;
       client.asset_map = Some(asset_map);

       Ok(client)
   }
   ```
**Code Reference**: Section 8.1 Task 24
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 5 minutes
**Estimated LOC**: 20 lines
**Dependencies**: Task 23 (for wallet/asset_map fields), Task 25 (for build_asset_map method)

---

### Task 25: Add build_asset_map() method to HyperliquidClient
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After `with_wallet()` method (insert after Task 24 code)
**Action**:
1. Add method from Section 7.2:
   ```rust
   /// Fetch asset index mapping from Hyperliquid /info endpoint
   ///
   /// Returns HashMap of symbol → asset_index (e.g., "BTC" → 0, "ETH" → 1)
   ///
   /// # Errors
   /// - Returns error if /info request fails
   /// - Returns error if universe array missing from response
   async fn build_asset_map(&self) -> Result<Arc<HashMap<String, u32>>> {
       let request_body = serde_json::json!({"type": "meta"});
       let response = self.post("/info", request_body).await?;

       let universe = response
           .get("universe")
           .and_then(|u| u.as_array())
           .context("Missing universe in meta response")?;

       let mut map = HashMap::new();
       for (index, item) in universe.iter().enumerate() {
           if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
               map.insert(name.to_string(), index as u32);
           }
       }

       Ok(Arc::new(map))
   }
   ```
**Code Reference**: Section 7.2
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 5 minutes
**Estimated LOC**: 20 lines
**Dependencies**: Task 23 (for HashMap/Arc imports)

---

### Task 26: Add post_signed() method to HyperliquidClient
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After `build_asset_map()` method (insert after Task 25 code)
**Action**:
1. Add signed POST method from Section 8.1 Task 26:
   ```rust
   /// Execute signed POST request to Hyperliquid exchange endpoint
   ///
   /// Signs action with EIP-712 using wallet's private key
   ///
   /// # Errors
   /// - Returns error if wallet not configured (use with_wallet())
   /// - Returns error if signing fails
   /// - Returns error if HTTP request fails
   pub async fn post_signed(
       &self,
       endpoint: &str,
       action: serde_json::Value,
       nonce: u64,
   ) -> Result<serde_json::Value> {
       use ethers::signers::Signer;
       use crate::signing::L1Action;

       let wallet = self.wallet.as_ref()
           .context("Wallet not configured - use with_wallet()")?;

       // Compute phantom agent (keccak256 of serialized action)
       let action_bytes = serde_json::to_vec(&action)?;
       let phantom_agent = ethers::utils::keccak256(&action_bytes);

       // Sign typed data with EIP-712
       let typed_data = L1Action {
           phantom_agent,
           is_mainnet: true,  // TODO: Make configurable via constructor
       };

       let signature = wallet.sign_typed_data(&typed_data).await
           .context("Failed to sign typed data")?;

       // Build signed payload with signature object
       let signed_payload = serde_json::json!({
           "action": action,
           "nonce": nonce,
           "signature": {
               "r": format!("0x{}", hex::encode(signature.r.to_fixed_bytes())),
               "s": format!("0x{}", hex::encode(signature.s.to_fixed_bytes())),
               "v": signature.v
           }
       });

       // Send request with rate limiting
       self.rate_limiter.until_ready().await;
       let url = format!("{}{}", self.base_url, endpoint);
       let response = self.http_client.post(&url).json(&signed_payload).send().await?;
       let json = response.json().await?;
       Ok(json)
   }
   ```
**Code Reference**: Section 8.1 Task 26
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 6 minutes
**Estimated LOC**: 40 lines
**Dependencies**: Task 21 (for signing::L1Action), Task 19 (for hex crate)

---

### Task 27: Update LiveExecutionHandler struct with nonce counter
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
**Location**: Lines 11-20 (replace entire struct and impl block)
**Action**:
1. Add imports at top of file:
   ```rust
   use std::sync::{atomic::{AtomicU64, Ordering}, Arc};
   use std::collections::HashMap;
   ```
2. Replace struct definition (lines 11-20):
   ```rust
   pub struct LiveExecutionHandler {
       client: HyperliquidClient,
       asset_map: Arc<HashMap<String, u32>>,
       nonce_counter: Arc<AtomicU64>,
   }

   impl LiveExecutionHandler {
       /// Create handler with client and asset mapping
       ///
       /// Initializes nonce counter to current timestamp in milliseconds
       pub fn new(client: HyperliquidClient, asset_map: Arc<HashMap<String, u32>>) -> Self {
           let nonce_counter = Arc::new(AtomicU64::new(
               chrono::Utc::now().timestamp_millis() as u64
           ));
           Self { client, asset_map, nonce_counter }
       }
   }
   ```
**Code Reference**: Section 8.1 Task 27
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 4 minutes
**Estimated LOC**: 15 lines
**Dependencies**: Task 23 (for asset_map type)

---

### Task 28: Rewrite execute_order() with signed requests
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
**Location**: Lines 25-52 (replace entire execute_order method)
**Action**:
1. Replace execute_order() implementation with signed version from Section 7.3:
   ```rust
   async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
       use serde_json::json;

       // 1. Get asset index from symbol
       let asset_index = self.asset_map.get(&order.symbol)
           .context(format!("Symbol {} not found in asset map", order.symbol))?;

       // 2. Build Hyperliquid order payload (compact format)
       let hl_order = json!({
           "a": asset_index,                                      // asset index
           "b": matches!(order.direction, OrderDirection::Buy),   // is_buy
           "p": order.price.map(|p| p.to_string()).unwrap_or("0".to_string()), // price
           "s": order.quantity.to_string(),                       // size
           "r": false,                                            // reduce_only
           "t": {
               "limit": { "tif": "Gtc" }                          // time_in_force: Good-til-cancel
           }
       });

       let action = json!({
           "type": "order",
           "orders": [hl_order],
           "grouping": "na"
       });

       // 3. Get unique nonce (atomic increment)
       let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);

       // 4. Send signed request
       let response = self.client.post_signed("/exchange", action, nonce).await?;

       // 5. Parse response
       let status = response.get("status")
           .and_then(|s| s.as_str())
           .context("Missing status in response")?;

       if status != "ok" {
           anyhow::bail!("Order failed: {:?}", response);
       }

       let response_data = response.get("response")
           .and_then(|r| r.get("data"))
           .context("Missing response data")?;

       // 6. Build FillEvent from response
       let fill = FillEvent {
           order_id: response_data.get("statuses")
               .and_then(|s| s.get(0))
               .and_then(|s| s.get("resting"))
               .and_then(|r| r.get("oid"))
               .and_then(|o| o.as_u64())
               .map(|o| o.to_string())
               .unwrap_or_default(),
           symbol: order.symbol,
           direction: order.direction,
           quantity: order.quantity,
           price: order.price.unwrap_or(Decimal::ZERO),
           commission: Decimal::ZERO,  // TODO: Parse from response
           timestamp: chrono::Utc::now(),
       };

       Ok(fill)
   }
   ```
**Code Reference**: Section 7.3
**Verification**: `cargo check -p algo-trade-hyperliquid`
**Estimated Time**: 8 minutes
**Estimated LOC**: 60 lines
**Dependencies**: Task 26 (for post_signed method), Task 27 (for nonce_counter)

---

### Task 29: Update BotActor initialization to use authenticated client
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: `initialize_system()` method (find HyperliquidClient::new call)
**Action**:
1. Find client initialization code (search for `HyperliquidClient::new`)
2. Replace with wallet-based initialization:
   ```rust
   // Load wallet from environment
   let wallet = algo_trade_hyperliquid::wallet::wallet_from_env()
       .context("Failed to load wallet - ensure HYPERLIQUID_PRIVATE_KEY is set")?;

   // Create authenticated client with asset mapping
   let client = HyperliquidClient::with_wallet(
       self.config.api_url.clone(),
       wallet,
   ).await
       .context("Failed to initialize authenticated Hyperliquid client")?;

   // Extract asset map for execution handler
   let asset_map = client.asset_map.clone()
       .context("Asset map not initialized")?;

   // Create execution handler with asset mapping
   let execution_handler = LiveExecutionHandler::new(client.clone(), asset_map);
   ```
**Code Reference**: Section 8.1 Task 29
**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated Time**: 5 minutes
**Estimated LOC**: 15 lines
**Dependencies**: Task 24 (for with_wallet), Task 27 (for new handler signature)

---

### Task 30: Add environment variable validation at startup
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Top of `initialize_system()` method (before any initialization)
**Action**:
1. Add environment check at start of initialize_system():
   ```rust
   // Validate required environment variables before initialization
   if std::env::var("HYPERLIQUID_PRIVATE_KEY").is_err() {
       anyhow::bail!(
           "HYPERLIQUID_PRIVATE_KEY environment variable not set. \
            Required for authenticated order execution. \
            Format: 64 hex characters (with or without 0x prefix)"
       );
   }
   ```
**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**Estimated Time**: 2 minutes
**Estimated LOC**: 8 lines
**Dependencies**: Task 29 (context for where this check is needed)

---

## Summary

### Total Estimates
- **Total LOC**: ~227 lines
- **Total Time**: 48 minutes (~0.8 hours)
- **Files Modified**: 5 files
- **Files Created**: 2 new files

### Execution Order (Sequential)
1. Task 19 (deps) - 2 min
2. Task 20 (signing) - 3 min
3. Task 21 (lib.rs) - 1 min
4. Task 22 (wallet) - 4 min
5. Task 23 (client fields) - 3 min
6. Task 25 (asset_map) - 5 min
7. Task 24 (with_wallet) - 5 min
8. Task 26 (post_signed) - 6 min
9. Task 27 (handler struct) - 4 min
10. Task 28 (execute_order) - 8 min
11. Task 30 (env validation) - 2 min
12. Task 29 (bot init) - 5 min

### Verification Commands (Run After Each Task)
```bash
# After Tasks 19-26
cargo check -p algo-trade-hyperliquid

# After Tasks 27-28
cargo check -p algo-trade-hyperliquid

# After Tasks 29-30
cargo check -p algo-trade-bot-orchestrator

# Final verification (all packages)
cargo build --workspace
cargo clippy --workspace -- -D warnings
```

---

## Critical Reminders

### MUST DO
- ✅ Use `ethers-rs` manual approach (NOT hyperliquid_rust_sdk)
- ✅ Chain ID = 1337 (Hyperliquid-specific)
- ✅ Nonce = atomic counter starting at current timestamp
- ✅ Asset indices from /info endpoint
- ✅ Signature format: `{"r": "0x...", "s": "0x...", "v": 27|28}`

### MUST NOT DO
- ❌ Do NOT hardcode private keys
- ❌ Do NOT use f64 for prices (use Decimal)
- ❌ Do NOT skip asset index mapping
- ❌ Do NOT use chain_id 42161 (Arbitrum)
- ❌ Do NOT implement msgpack (JSON is sufficient)

### Edge Cases Handled
1. **Nonce collisions**: AtomicU64 with millisecond timestamps + increment
2. **Asset not found**: Clear error with symbol name
3. **Signature failures**: Context with format requirements
4. **Missing wallet**: Error with setup instructions
5. **Response parsing**: Handle both success/error formats

---

## Next Step: Execute Tasks

After completing all tasks, invoke **Karen agent** for Phase 2 quality review.
