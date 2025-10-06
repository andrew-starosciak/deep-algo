# Hyperliquid SDK Integration - Context Report

**Date**: 2025-10-05
**Focus**: Wallet Integration for Authenticated Order Execution (Tasks 19-25)
**Status**: Research Complete

---

## 1. Executive Summary

This report provides focused research on integrating Hyperliquid order signing using either the official `hyperliquid_rust_sdk` or manual EIP-712 implementation with `ethers-rs`. The research reveals:

1. **Official SDK Status**: `hyperliquid_rust_sdk` v0.6.0 exists but has limited documentation and is "less maintained" than Python SDK
2. **Alternative Approach**: Manual EIP-712 signing with existing `ethers` v2.0.14 dependency is viable and well-documented
3. **Recommendation**: Use `ethers-rs` manual approach for better control, documentation, and compatibility with existing codebase

---

## 2. SDK Research Findings

### 2.1 Official hyperliquid_rust_sdk

**Version**: 0.6.0 (crates.io)
**Status**: Available but less maintained than Python SDK
**GitHub**: https://github.com/hyperliquid-dex/hyperliquid-rust-sdk

#### Key Components Found:

```rust
// From docs.rs source analysis
pub struct ExchangeClient {
    pub http_client: HttpClient,
    pub wallet: LocalWallet,
    pub meta: Meta,
    pub vault_address: Option<H160>,
    pub coin_to_asset: HashMap<String, u32>
}
```

#### Available Methods:
- `market_open()` - Place market order with slippage
- `market_close()` - Close position
- `order()` - Single order placement
- `bulk_order()` - Multiple orders
- `cancel()` / `bulk_cancel()` - Order cancellation
- `modify()` - Modify existing order

#### Example Usage (from GitHub PR #55):
```rust
let market_open_params = MarketOrderParams {
    asset: "ETH",
    is_buy: true,
    sz: 0.01,
    px: None,
    slippage: Some(0.01),
    cloid: None,
    wallet: None,
};
let response = exchange_client.market_open(market_open_params).await?;
```

#### Problems with Official SDK:
1. **No published crates.io documentation** - 404 on crates.io/crates/hyperliquid_rust_sdk
2. **Sparse GitHub examples** - Examples in `src/bin` but repository structure unclear
3. **Maintenance concerns** - Documented as "less maintained" than Python SDK
4. **API uncertainty** - Internal API structure unclear without source access

### 2.2 Alternative: ethers-rs Manual Implementation

**Version**: ethers v2.0.14 (already in dependencies)
**Status**: Mature, well-documented, production-ready
**Approach**: Manual EIP-712 signing + Hyperliquid API integration

#### Already Available:
```toml
# crates/exchange-hyperliquid/Cargo.toml
ethers = "2.0"
```

#### Complete Implementation Pattern:

**Step 1: Create Wallet from Private Key**
```rust
use ethers::signers::{LocalWallet, Signer};

// Parse hex string (with or without 0x prefix)
let wallet = "dcf2cbdd171a21c480aa7f53d77f31bb102282b3ff099c78e3118b37348c72f7"
    .parse::<LocalWallet>()?;

// Or from environment
let wallet = std::env::var("HYPERLIQUID_PRIVATE_KEY")?
    .parse::<LocalWallet>()?;
```

**Step 2: Define EIP-712 Typed Data Structures**
```rust
use ethers_contract::EthAbiType;
use ethers_derive_eip712::Eip712;

#[derive(Debug, Clone, Eip712, EthAbiType)]
#[eip712(
    name = "Exchange",
    version = "1",
    chain_id = 1337,  // Hyperliquid uses 1337, NOT 42161
    verifying_contract = "0x0000000000000000000000000000000000000000"
)]
pub struct HyperliquidAction {
    pub action: String,      // JSON-encoded action (msgpack alternative)
    pub nonce: u64,          // Timestamp in milliseconds
    pub phantom_agent: [u8; 32],  // Derived from action hash
}
```

**Step 3: Sign Typed Data**
```rust
async fn sign_hyperliquid_order(
    wallet: &LocalWallet,
    action: serde_json::Value,
    nonce: u64,
) -> Result<Signature> {
    let typed_data = HyperliquidAction {
        action: serde_json::to_string(&action)?,
        nonce,
        phantom_agent: compute_phantom_agent(&action),
    };

    wallet.sign_typed_data(&typed_data).await
}
```

---

## 3. Hyperliquid API Specifications

### 3.1 Order Payload Structure (from official docs)

**Endpoint**: `POST https://api.hyperliquid.xyz/exchange`

**Complete Request Format**:
```json
{
  "action": {
    "type": "order",
    "orders": [{
      "a": 0,              // Asset index (number)
      "b": true,           // Is Buy (boolean)
      "p": "42750.5",      // Price (string)
      "s": "0.01",         // Size (string)
      "r": false,          // Reduce Only (boolean)
      "t": {
        "limit": { "tif": "Alo" }  // Alo|Ioc|Gtc
      },
      "c": "deadbeef..."   // Optional: client order ID (128-bit hex)
    }],
    "grouping": "na"       // na|normalTpsl|positionTpsl
  },
  "nonce": 1696348800000,  // Timestamp in milliseconds
  "signature": {
    "r": "0x...",
    "s": "0x...",
    "v": 27
  },
  "vaultAddress": null     // Optional: vault/subaccount
}
```

### 3.2 EIP-712 Signing Mechanism (Phantom Agent)

Hyperliquid uses a unique "phantom agent" construction:

1. **Serialize action** - Use msgpack binary format (Python) OR JSON (acceptable alternative)
2. **Generate timestamp** - Current time in milliseconds
3. **Compute phantom agent** - `keccak256(msgpack(action))` (or JSON hash)
4. **Sign with EIP-712** - Domain separator chain_id = 1337

**Key Differences from Standard EIP-712**:
- Chain ID is **1337** (not Arbitrum's 42161)
- Uses "phantom agent" derived from action hash
- Action data is serialized (msgpack preferred, JSON acceptable)

### 3.3 Asset Index Mapping

Orders use asset **index** (number), not symbol (string):

```rust
// Need to fetch from /info endpoint
{
  "type": "meta"
}
// Returns:
{
  "universe": [
    { "name": "BTC", ... },  // index 0
    { "name": "ETH", ... },  // index 1
    { "name": "SOL", ... }   // index 2
  ]
}
```

**Current Implementation**:
```rust
// crates/exchange-hyperliquid/src/client.rs:286
pub async fn fetch_available_symbols(&self) -> Result<Vec<String>>
```

**Needed Enhancement**: Build `symbol → asset_index` mapping

---

## 4. Current Codebase Integration Points

### 4.1 HyperliquidClient (client.rs)

**Current State**:
```rust
// Lines 15-19
pub struct HyperliquidClient {
    http_client: Client,
    base_url: String,
    rate_limiter: Arc<RateLimiter<...>>,
}

// Lines 55-61 - Unsigned POST
pub async fn post(&self, endpoint: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    self.rate_limiter.until_ready().await;
    let url = format!("{}{}", self.base_url, endpoint);
    let response = self.http_client.post(&url).json(&body).send().await?;
    let json = response.json().await?;
    Ok(json)
}
```

**Required Changes**:
1. Add wallet field: `wallet: Option<LocalWallet>`
2. Add asset mapping: `asset_index_map: Arc<HashMap<String, u32>>`
3. Add `with_wallet()` constructor
4. Add `post_signed()` method

### 4.2 LiveExecutionHandler (execution.rs)

**Current State**:
```rust
// Lines 11-20
pub struct LiveExecutionHandler {
    client: HyperliquidClient,
}

impl LiveExecutionHandler {
    pub const fn new(client: HyperliquidClient) -> Self {
        Self { client }
    }
}

// Lines 25-52 - Current execute_order (UNSIGNED)
async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
    let order_payload = json!({
        "type": match order.order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
        },
        "coin": order.symbol,
        "is_buy": matches!(order.direction, OrderDirection::Buy),
        "sz": order.quantity.to_string(),
        "limit_px": order.price.map(|p| p.to_string()),
    });

    let response = self.client.post("/exchange", order_payload).await?;
    // ... parse response ...
}
```

**Problems**:
1. ❌ Payload format is WRONG - missing `action` wrapper, asset index, nonce, signature
2. ❌ No nonce management
3. ❌ No signing logic
4. ❌ Response parsing incomplete

**Required Changes**:
1. Add nonce counter: `nonce_counter: Arc<AtomicU64>`
2. Use correct payload format (see Section 3.1)
3. Call `post_signed()` instead of `post()`
4. Map symbol to asset index

### 4.3 OrderEvent Structure (core/src/events.rs)

**Current Definition**:
```rust
pub struct OrderEvent {
    pub symbol: String,           // ✅ Maps to asset index
    pub order_type: OrderType,    // ✅ Market | Limit
    pub direction: OrderDirection, // ✅ Buy | Sell
    pub quantity: Decimal,        // ✅ Maps to "s"
    pub price: Option<Decimal>,   // ✅ Maps to "p"
    pub timestamp: DateTime<Utc>, // ✅ Can use for nonce
}
```

**Mapping to Hyperliquid Format**:
```rust
// OrderEvent → Hyperliquid order
{
    "a": asset_index_from_symbol(order.symbol),
    "b": matches!(order.direction, OrderDirection::Buy),
    "p": order.price.map(|p| p.to_string()).unwrap_or("0"),
    "s": order.quantity.to_string(),
    "r": false,  // reduce_only (could add to OrderEvent)
    "t": {
        "limit": { "tif": "Gtc" }  // time_in_force (could parameterize)
    }
}
```

### 4.4 Dependencies (Cargo.toml)

**Current**:
```toml
[dependencies]
algo-trade-core = { path = "../core" }
algo-trade-data = { path = "../data" }
tokio = { workspace = true, features = ["full"] }
serde = { workspace = true }
serde_json = "1.0"
reqwest = { version = "0.12", features = ["json"] }
tokio-tungstenite = "0.24"
futures-util = "0.3"
governor = "0.6"
ethers = "2.0"  # ✅ Already present!
anyhow = { workspace = true }
tracing = { workspace = true }
url = "2.5"
async-trait = "0.1.89"
chrono = "0.4.42"
rust_decimal = "1.38.0"
```

**Required Additions**:
```toml
ethers-contract = "2.0"         # For EthAbiType derive
ethers-derive-eip712 = "2.0"    # For Eip712 derive
# OR
hyperliquid_rust_sdk = "0.6.0"  # If using official SDK
```

**Recommendation**: Add ethers-contract and ethers-derive-eip712 (minimal additions to existing ethers dep)

---

## 5. Implementation Approaches

### Approach A: Manual EIP-712 with ethers-rs ✅ RECOMMENDED

**Pros**:
- ✅ Uses existing `ethers = "2.0"` dependency (already in Cargo.toml)
- ✅ Well-documented with extensive examples
- ✅ Full control over signing process
- ✅ Mature, production-tested library
- ✅ Better error handling and debugging

**Cons**:
- ⚠️ Need to implement phantom agent logic
- ⚠️ Manually construct Hyperliquid payload format

**Dependencies to Add**:
```toml
ethers-contract = "2.0"
ethers-derive-eip712 = "2.0"
```

**Estimated Code**: ~150 LOC total
- EIP-712 struct definitions: ~40 LOC
- Signing logic: ~60 LOC
- Payload construction: ~50 LOC

### Approach B: Official hyperliquid_rust_sdk ⚠️ RISKY

**Pros**:
- ✅ Official SDK with signing built-in
- ✅ Handles phantom agent automatically
- ✅ Example code in repository

**Cons**:
- ❌ Sparse documentation (crates.io 404)
- ❌ "Less maintained" warning in docs
- ❌ Unclear API surface
- ❌ Additional dependency weight
- ❌ Less control for debugging

**Dependencies to Add**:
```toml
hyperliquid_rust_sdk = "0.6.0"
```

**Estimated Code**: ~80 LOC (if SDK works as expected)

---

## 6. Phantom Agent Implementation

Hyperliquid's unique "phantom agent" construction (from Chainstack docs):

### 6.1 Python Reference Implementation
```python
from hyperliquid.utils.signing import sign_l1_action, get_timestamp_ms

timestamp = get_timestamp_ms()
action = {"type": "order", "orders": [...]}

signature = sign_l1_action(
    wallet,
    action,
    None,  # vault_address
    timestamp,
    None,  # expires_after
    True   # is_mainnet
)
```

### 6.2 Rust Implementation Strategy

**Step 1: Serialize Action**
```rust
use serde_json;

let action = json!({
    "type": "order",
    "orders": [...],
    "grouping": "na"
});

// Convert to bytes (msgpack preferred, JSON acceptable)
let action_bytes = serde_json::to_vec(&action)?;
```

**Step 2: Compute Phantom Agent**
```rust
use ethers::utils::keccak256;

let action_hash = keccak256(&action_bytes);
// action_hash is [u8; 32] - this IS the phantom agent
```

**Step 3: Construct EIP-712 Message**
```rust
#[derive(Debug, Clone, Eip712, EthAbiType)]
#[eip712(
    name = "Exchange",
    version = "1",
    chain_id = 1337,
    verifying_contract = "0x0000000000000000000000000000000000000000"
)]
pub struct L1Action {
    pub phantom_agent: [u8; 32],  // From action hash
    pub is_mainnet: bool,
}
```

**Step 4: Sign and Encode**
```rust
let message = L1Action {
    phantom_agent: action_hash,
    is_mainnet: true,
};

let signature = wallet.sign_typed_data(&message).await?;

// Convert to JSON signature object
let sig_object = json!({
    "r": format!("0x{}", hex::encode(signature.r)),
    "s": format!("0x{}", hex::encode(signature.s)),
    "v": signature.v
});
```

### 6.3 Alternative: Simplified Approach

If phantom agent proves complex, Hyperliquid MAY accept standard EIP-712 with action as string field:

```rust
#[derive(Debug, Clone, Eip712, EthAbiType)]
#[eip712(
    name = "Exchange",
    version = "1",
    chain_id = 1337,
    verifying_contract = "0x0000000000000000000000000000000000000000"
)]
pub struct HyperliquidOrder {
    pub action: String,  // JSON-encoded action
    pub nonce: u64,
}
```

**Testing Required**: Verify which format Hyperliquid actually accepts.

---

## 7. Code Examples - Production Ready

### 7.1 Wallet Creation from Environment

```rust
// crates/exchange-hyperliquid/src/wallet.rs (NEW FILE)
use ethers::signers::{LocalWallet, Signer};
use anyhow::{Context, Result};

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

pub fn validate_address(address: &str) -> Result<()> {
    use ethers::types::Address;
    address.parse::<Address>()
        .context("Invalid Ethereum address format")?;
    Ok(())
}
```

### 7.2 Asset Index Mapping

```rust
// crates/exchange-hyperliquid/src/client.rs
use std::collections::HashMap;
use std::sync::Arc;

impl HyperliquidClient {
    pub async fn build_asset_map(&self) -> Result<Arc<HashMap<String, u32>>> {
        let request_body = json!({"type": "meta"});
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
}
```

### 7.3 Signed Order Execution

```rust
// crates/exchange-hyperliquid/src/execution.rs
use std::sync::atomic::{AtomicU64, Ordering};
use ethers::signers::{LocalWallet, Signer};

pub struct LiveExecutionHandler {
    client: HyperliquidClient,
    wallet: LocalWallet,
    asset_map: Arc<HashMap<String, u32>>,
    nonce_counter: Arc<AtomicU64>,
}

impl LiveExecutionHandler {
    pub fn new(
        client: HyperliquidClient,
        wallet: LocalWallet,
        asset_map: Arc<HashMap<String, u32>>,
    ) -> Self {
        let nonce_counter = Arc::new(AtomicU64::new(
            chrono::Utc::now().timestamp_millis() as u64
        ));

        Self { client, wallet, asset_map, nonce_counter }
    }
}

#[async_trait]
impl ExecutionHandler for LiveExecutionHandler {
    async fn execute_order(&mut self, order: OrderEvent) -> Result<FillEvent> {
        // 1. Get asset index
        let asset_index = self.asset_map.get(&order.symbol)
            .context(format!("Symbol {} not found in asset map", order.symbol))?;

        // 2. Build Hyperliquid order payload
        let hl_order = json!({
            "a": asset_index,
            "b": matches!(order.direction, OrderDirection::Buy),
            "p": order.price.map(|p| p.to_string()).unwrap_or("0".to_string()),
            "s": order.quantity.to_string(),
            "r": false,
            "t": {
                "limit": { "tif": "Gtc" }
            }
        });

        let action = json!({
            "type": "order",
            "orders": [hl_order],
            "grouping": "na"
        });

        // 3. Get nonce
        let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);

        // 4. Sign action (phantom agent approach)
        let action_bytes = serde_json::to_vec(&action)?;
        let phantom_agent = ethers::utils::keccak256(&action_bytes);

        let typed_data = L1Action {
            phantom_agent,
            is_mainnet: true,
        };

        let signature = self.wallet.sign_typed_data(&typed_data).await?;

        // 5. Build signed request
        let signed_payload = json!({
            "action": action,
            "nonce": nonce,
            "signature": {
                "r": format!("0x{}", hex::encode(signature.r.to_fixed_bytes())),
                "s": format!("0x{}", hex::encode(signature.s.to_fixed_bytes())),
                "v": signature.v
            }
        });

        // 6. Send request
        let response = self.client.post("/exchange", signed_payload).await?;

        // 7. Parse response
        let status = response.get("status")
            .and_then(|s| s.as_str())
            .context("Missing status in response")?;

        if status != "ok" {
            anyhow::bail!("Order failed: {:?}", response);
        }

        let response_data = response.get("response")
            .and_then(|r| r.get("data"))
            .context("Missing response data")?;

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
}
```

### 7.4 EIP-712 Struct Definition

```rust
// crates/exchange-hyperliquid/src/signing.rs (NEW FILE)
use ethers_contract::EthAbiType;
use ethers_derive_eip712::Eip712;

#[derive(Debug, Clone, Eip712, EthAbiType)]
#[eip712(
    name = "Exchange",
    version = "1",
    chain_id = 1337,
    verifying_contract = "0x0000000000000000000000000000000000000000"
)]
pub struct L1Action {
    pub phantom_agent: [u8; 32],
    pub is_mainnet: bool,
}
```

---

## 8. TaskMaster Handoff Package

### 8.1 MUST DO - Atomic Tasks

#### Task 19: Add EIP-712 dependencies
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/Cargo.toml`
**Action**: Add after line 22 (after existing ethers):
```toml
ethers-contract = "2.0"
ethers-derive-eip712 = "2.0"
hex = "0.4"  # For signature encoding
```
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 3

#### Task 20: Create signing module with EIP-712 struct
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/signing.rs` (NEW)
**Action**: Create file with L1Action struct (see 7.4)
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 15

#### Task 21: Add signing module to lib.rs
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/lib.rs`
**Action**: Add line: `pub mod signing;`
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 1

#### Task 22: Create wallet utility module
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/wallet.rs` (NEW)
**Action**: Add wallet_from_env() and validate_address() (see 7.1)
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 25

#### Task 23: Add wallet and asset_map fields to HyperliquidClient
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: Lines 15-19 (struct definition)
**Action**: Add fields:
```rust
wallet: Option<LocalWallet>,
asset_map: Option<Arc<HashMap<String, u32>>>,
```
Add import: `use ethers::signers::LocalWallet;`
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 3

#### Task 24: Add HyperliquidClient::with_wallet() constructor
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After line 37 (after new() method)
**Action**: Add method:
```rust
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

    // Build asset map
    let asset_map = client.build_asset_map().await?;
    client.asset_map = Some(asset_map);

    Ok(client)
}
```
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 20

#### Task 25: Add build_asset_map() method
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After with_wallet() method
**Action**: Add method from Section 7.2
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 20

#### Task 26: Add post_signed() method
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
**Location**: After build_asset_map() method
**Action**: Add method:
```rust
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

    // Compute phantom agent
    let action_bytes = serde_json::to_vec(&action)?;
    let phantom_agent = ethers::utils::keccak256(&action_bytes);

    // Sign typed data
    let typed_data = L1Action {
        phantom_agent,
        is_mainnet: true,  // TODO: Make configurable
    };

    let signature = wallet.sign_typed_data(&typed_data).await
        .context("Failed to sign typed data")?;

    // Build signed payload
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
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 40

#### Task 27: Update LiveExecutionHandler struct
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
**Location**: Lines 11-20
**Action**: Replace with:
```rust
pub struct LiveExecutionHandler {
    client: HyperliquidClient,
    asset_map: Arc<HashMap<String, u32>>,
    nonce_counter: Arc<AtomicU64>,
}

impl LiveExecutionHandler {
    pub fn new(client: HyperliquidClient, asset_map: Arc<HashMap<String, u32>>) -> Self {
        let nonce_counter = Arc::new(AtomicU64::new(
            chrono::Utc::now().timestamp_millis() as u64
        ));
        Self { client, asset_map, nonce_counter }
    }
}
```
Add imports: `use std::sync::{atomic::{AtomicU64, Ordering}, Arc}; use std::collections::HashMap;`
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 15

#### Task 28: Rewrite execute_order() with signed requests
**File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/execution.rs`
**Location**: Lines 25-52 (entire method)
**Action**: Replace with implementation from Section 7.3
**Verification**: `cargo check -p algo-trade-hyperliquid`
**LOC**: 80

#### Task 29: Update BotActor initialization for wallet
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: initialize_system() method
**Action**: Modify client creation to:
```rust
let wallet = crate::wallet::wallet_from_env()?;
let client = HyperliquidClient::with_wallet(
    self.config.api_url.clone(),
    wallet,
).await?;
let asset_map = client.asset_map.clone().unwrap();
let execution_handler = LiveExecutionHandler::new(client.clone(), asset_map);
```
**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**LOC**: 10

#### Task 30: Add environment variable validation
**File**: `/home/a/Work/algo-trade/crates/bot-orchestrator/src/bot_actor.rs`
**Location**: Top of initialize_system()
**Action**: Add check:
```rust
if std::env::var("HYPERLIQUID_PRIVATE_KEY").is_err() {
    anyhow::bail!("HYPERLIQUID_PRIVATE_KEY environment variable not set");
}
```
**Verification**: `cargo check -p algo-trade-bot-orchestrator`
**LOC**: 5

### 8.2 MUST NOT DO

1. ❌ **Do NOT add hyperliquid_rust_sdk dependency** - Use ethers-rs manual approach
2. ❌ **Do NOT hardcode private keys** - Always use environment variables
3. ❌ **Do NOT use f64 for prices** - All financial values use Decimal
4. ❌ **Do NOT skip asset index mapping** - Orders will fail with wrong indices
5. ❌ **Do NOT use chain_id 42161** - Hyperliquid uses 1337, not Arbitrum
6. ❌ **Do NOT implement msgpack** - JSON serialization is acceptable for phantom agent
7. ❌ **Do NOT skip signature object formatting** - Must be {"r": "0x...", "s": "0x...", "v": 27}

### 8.3 Edge Cases & Constraints

1. **Nonce Collisions**: Use `AtomicU64` with millisecond timestamps, increment for each order
2. **Asset Not Found**: Return clear error if symbol not in asset_map
3. **Signature Failures**: Wrap in context explaining private key format requirements
4. **Rate Limiting**: post_signed() already applies rate limiting via client.rate_limiter
5. **Response Parsing**: Handle both success and error response formats from Hyperliquid
6. **Mainnet vs Testnet**: L1Action.is_mainnet should be configurable (currently hardcoded true)

### 8.4 Verification Checklist

After completing all tasks:
- [ ] `cargo build -p algo-trade-hyperliquid` succeeds
- [ ] `cargo test -p algo-trade-hyperliquid` passes
- [ ] `cargo clippy -p algo-trade-hyperliquid -- -D warnings` clean
- [ ] Environment variable HYPERLIQUID_PRIVATE_KEY is documented
- [ ] Asset index mapping tested with /info endpoint
- [ ] Signature format matches Hyperliquid expectations (test with API)
- [ ] Error messages provide actionable guidance

### 8.5 Testing Strategy

**Unit Tests** (execution.rs):
```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_order_payload_format() {
        // Verify Hyperliquid payload structure
    }

    #[tokio::test]
    async fn test_asset_index_mapping() {
        // Verify symbol -> index conversion
    }
}
```

**Integration Test** (requires testnet):
```rust
// tests/integration_test.rs
#[tokio::test]
#[ignore]  // Requires HYPERLIQUID_PRIVATE_KEY
async fn test_signed_order_execution() {
    let wallet = wallet_from_env().unwrap();
    let client = HyperliquidClient::with_wallet(
        "https://api.hyperliquid-testnet.xyz".to_string(),
        wallet
    ).await.unwrap();

    // Place small test order
    // Verify response format
}
```

---

## 9. References

### Documentation
- **Hyperliquid Exchange API**: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint
- **EIP-712 Specification**: https://eips.ethereum.org/EIPS/eip-712
- **ethers-rs Signing**: https://docs.rs/ethers/latest/ethers/signers/trait.Signer.html
- **Chainstack Signing Guide**: https://docs.chainstack.com/docs/hyperliquid-signing-overview

### Repositories
- **Official Rust SDK**: https://github.com/hyperliquid-dex/hyperliquid-rust-sdk
- **Alternative SDK (dennohpeter)**: https://github.com/dennohpeter/hyperliquid
- **ethers-rs**: https://github.com/gakonst/ethers-rs

### Crates
- **ethers**: https://crates.io/crates/ethers (v2.0.14 - already in deps)
- **ethers-contract**: https://crates.io/crates/ethers-contract (NEED TO ADD)
- **ethers-derive-eip712**: https://crates.io/crates/ethers-derive-eip712 (NEED TO ADD)
- **hyperliquid_rust_sdk**: https://crates.io/crates/hyperliquid_rust_sdk (NOT RECOMMENDED)

---

## 10. Summary & Recommendation

**RECOMMENDED APPROACH**: Manual EIP-712 implementation with ethers-rs

**Rationale**:
1. ✅ Leverages existing `ethers = "2.0"` dependency (minimal additions)
2. ✅ Well-documented, mature library with extensive community support
3. ✅ Full control over signing process for debugging
4. ✅ Better long-term maintainability vs underdocumented SDK
5. ✅ Only need to add 2 small crates (ethers-contract, ethers-derive-eip712)

**Total Implementation**: 10 atomic tasks, ~250 LOC, 2-3 hours

**Critical Success Factors**:
- Environment variable `HYPERLIQUID_PRIVATE_KEY` must be 64 hex chars (no 0x prefix)
- Chain ID must be 1337 (Hyperliquid-specific, not Arbitrum's 42161)
- Asset indices must be fetched from /info endpoint and cached
- Signature object must have exact format: `{"r": "0x...", "s": "0x...", "v": 27|28}`
- Nonce must be unique per request (use atomic counter initialized to current timestamp)

**Next Steps**: Feed this report to TaskMaster for atomic task breakdown.
