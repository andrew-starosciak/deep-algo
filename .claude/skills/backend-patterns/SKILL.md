---
name: backend-patterns
description: Rust backend patterns for trading systems. Async services, WebSocket handling, API design, and data pipelines.
---

# Backend Patterns (Rust)

Patterns for building robust trading system backends.

## When to Activate

- Building data collection services
- Implementing WebSocket connections
- Designing REST APIs
- Creating data pipelines

## WebSocket Patterns

### Reconnecting WebSocket Client

```rust
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures::{StreamExt, SinkExt};

pub struct ReconnectingWebSocket {
    url: String,
    reconnect_delay: Duration,
    max_retries: u32,
}

impl ReconnectingWebSocket {
    pub async fn connect_and_stream<F, Fut>(
        &self,
        mut handler: F,
    ) -> Result<()>
    where
        F: FnMut(Message) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let mut retries = 0;

        loop {
            match connect_async(&self.url).await {
                Ok((ws_stream, _)) => {
                    retries = 0;
                    let (_, mut read) = ws_stream.split();

                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(m) => {
                                if let Err(e) = handler(m).await {
                                    tracing::error!("Handler error: {e}");
                                }
                            }
                            Err(e) => {
                                tracing::warn!("WebSocket error: {e}");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Connection failed: {e}");
                }
            }

            retries += 1;
            if retries > self.max_retries {
                return Err(anyhow::anyhow!("Max retries exceeded"));
            }

            let delay = self.reconnect_delay * retries;
            tracing::info!("Reconnecting in {:?}", delay);
            tokio::time::sleep(delay).await;
        }
    }
}
```

### Multi-Stream Aggregator

```rust
use futures::stream::SelectAll;

pub struct StreamAggregator {
    streams: SelectAll<BoxStream<'static, MarketEvent>>,
}

impl StreamAggregator {
    pub fn new() -> Self {
        Self { streams: SelectAll::new() }
    }

    pub fn add_stream(&mut self, stream: BoxStream<'static, MarketEvent>) {
        self.streams.push(stream);
    }

    pub async fn next(&mut self) -> Option<MarketEvent> {
        self.streams.next().await
    }
}

// Usage:
let mut aggregator = StreamAggregator::new();
aggregator.add_stream(binance_stream);
aggregator.add_stream(coinbase_stream);

while let Some(event) = aggregator.next().await {
    process_event(event).await?;
}
```

## Data Collection Service

### Collector Architecture

```rust
pub struct DataCollector {
    db: DatabaseClient,
    buffer: Vec<OrderBookSnapshot>,
    flush_interval: Duration,
    buffer_size: usize,
}

impl DataCollector {
    pub async fn run(&mut self, mut stream: impl Stream<Item = OrderBookSnapshot>) {
        let mut flush_timer = tokio::time::interval(self.flush_interval);

        loop {
            tokio::select! {
                Some(snapshot) = stream.next() => {
                    self.buffer.push(snapshot);
                    if self.buffer.len() >= self.buffer_size {
                        self.flush().await;
                    }
                }
                _ = flush_timer.tick() => {
                    if !self.buffer.is_empty() {
                        self.flush().await;
                    }
                }
            }
        }
    }

    async fn flush(&mut self) {
        let snapshots = std::mem::take(&mut self.buffer);
        if let Err(e) = self.db.insert_batch(&snapshots).await {
            tracing::error!("Flush failed: {e}");
            // Re-add failed items (with limit)
            self.buffer.extend(snapshots.into_iter().take(self.buffer_size));
        }
    }
}
```

## REST API with Axum

### Handler Pattern

```rust
use axum::{
    extract::{State, Query},
    response::Json,
    http::StatusCode,
};

#[derive(Deserialize)]
pub struct SignalQuery {
    signal_name: Option<String>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

pub async fn get_signals(
    State(db): State<DatabaseClient>,
    Query(query): Query<SignalQuery>,
) -> Result<Json<Vec<SignalSnapshot>>, AppError> {
    let signals = db.query_signals(
        query.signal_name.as_deref(),
        query.start,
        query.end,
        query.limit.unwrap_or(100),
    ).await?;

    Ok(Json(signals))
}

// Error handling
pub struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": self.0.to_string() }))
        ).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
```

### Router Setup

```rust
use axum::{Router, routing::get};

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/signals", get(get_signals))
        .route("/api/signals/:name/latest", get(get_latest_signal))
        .route("/api/backtest", post(run_backtest))
        .route("/api/trades", get(get_trades))
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK"
}
```

## Configuration

### Environment-Based Config

```rust
use figment::{Figment, providers::{Env, Toml, Format}};

#[derive(Deserialize)]
pub struct Config {
    pub database_url: String,
    pub binance_api_key: String,
    pub binance_secret_key: String,
    pub collection: CollectionConfig,
}

#[derive(Deserialize)]
pub struct CollectionConfig {
    pub buffer_size: usize,
    pub flush_interval_secs: u64,
    pub symbols: Vec<String>,
}

impl Config {
    pub fn load() -> Result<Self> {
        Figment::new()
            .merge(Toml::file("Config.toml"))
            .merge(Env::prefixed("TRADING_"))
            .extract()
            .map_err(Into::into)
    }
}
```

## Graceful Shutdown

```rust
use tokio::sync::broadcast;

pub struct ShutdownSignal {
    tx: broadcast::Sender<()>,
}

impl ShutdownSignal {
    pub fn new() -> (Self, ShutdownReceiver) {
        let (tx, rx) = broadcast::channel(1);
        (Self { tx }, ShutdownReceiver { rx })
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(());
    }
}

pub struct ShutdownReceiver {
    rx: broadcast::Receiver<()>,
}

impl ShutdownReceiver {
    pub async fn wait(&mut self) {
        let _ = self.rx.recv().await;
    }
}

// Usage in main:
#[tokio::main]
async fn main() -> Result<()> {
    let (shutdown, mut shutdown_rx) = ShutdownSignal::new();

    // Spawn signal handler
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown.shutdown();
    });

    // Run service until shutdown
    tokio::select! {
        result = run_service() => result,
        _ = shutdown_rx.wait() => {
            tracing::info!("Shutting down gracefully");
            Ok(())
        }
    }
}
```

## Metrics and Monitoring

```rust
use metrics::{counter, gauge, histogram};

pub fn record_signal_computation(signal_name: &str, duration: Duration, success: bool) {
    let labels = [("signal", signal_name.to_string())];

    histogram!("signal_computation_duration_seconds", &labels)
        .record(duration.as_secs_f64());

    if success {
        counter!("signal_computations_total", &labels).increment(1);
    } else {
        counter!("signal_computation_errors_total", &labels).increment(1);
    }
}

pub fn record_active_connections(count: usize) {
    gauge!("websocket_connections_active").set(count as f64);
}
```

## Rate Limiting

```rust
use governor::{Quota, RateLimiter, state::InMemoryState, clock::DefaultClock};
use nonzero_ext::nonzero;

pub struct RateLimitedClient<C> {
    client: C,
    limiter: RateLimiter<NotKeyed, InMemoryState, DefaultClock>,
}

impl<C> RateLimitedClient<C> {
    pub fn new(client: C, requests_per_second: u32) -> Self {
        Self {
            client,
            limiter: RateLimiter::direct(
                Quota::per_second(nonzero!(requests_per_second))
            ),
        }
    }

    pub async fn execute<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&C) -> T,
    {
        self.limiter.until_ready().await;
        f(&self.client)
    }
}
```

## Pipeline Pattern

```rust
pub trait Stage: Send + Sync {
    type Input;
    type Output;

    async fn process(&self, input: Self::Input) -> Result<Self::Output>;
}

pub struct Pipeline<A, B, C>
where
    A: Stage,
    B: Stage<Input = A::Output>,
    C: Stage<Input = B::Output>,
{
    stage_a: A,
    stage_b: B,
    stage_c: C,
}

impl<A, B, C> Pipeline<A, B, C>
where
    A: Stage,
    B: Stage<Input = A::Output>,
    C: Stage<Input = B::Output>,
{
    pub async fn run(&self, input: A::Input) -> Result<C::Output> {
        let a_out = self.stage_a.process(input).await?;
        let b_out = self.stage_b.process(a_out).await?;
        self.stage_c.process(b_out).await
    }
}
```
