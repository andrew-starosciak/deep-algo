# Context Report: Interactive TUI for Multi-Token Parameter Sweep Backtesting

**Date**: 2025-10-03
**Agent**: Context Gatherer
**Status**: ✅ Complete
**TaskMaster Handoff**: ✅ Ready

---

## Section 1: Request Analysis

### User Request (Verbatim)

```
"I want an interactive TUI that lets me:
1. Select a strategy (MA Crossover or Quad MA)
2. Select tokens from a list of all available Hyperliquid tokens (multiple selection, or select all)
3. Input strategy parameters with defaults shown, but allow adding multiple parameter configurations
4. Backtest the selected strategy against EVERY token × EVERY parameter configuration
5. Display results in a comparison view"
```

### Explicit Requirements

1. **Interactive TUI** - Terminal user interface with keyboard navigation
2. **Strategy Selection** - Choose between MA Crossover or Quad MA
3. **Token Selection** - Multi-select from Hyperliquid's available tokens
4. **Parameter Configuration** - Multiple parameter sets with default values shown
5. **Matrix Execution** - Run M tokens × N configs backtests
6. **Results Comparison** - Sortable table showing all results

### Implicit Requirements

1. **Fetch Hyperliquid Token List** - Need API endpoint to get all tradeable symbols
2. **Data Caching** - Downloaded OHLCV data should be reused across backtests
3. **Progress Tracking** - Visual feedback during long-running batch backtests
4. **Result Persistence** - Save results for later analysis (CSV/JSON)
5. **Error Handling** - Graceful failures for missing data, invalid params, network errors
6. **Parameter Validation** - Ensure MA1 < MA2 < MA3 < MA4, periods < data length
7. **UX Flow** - Multi-step wizard: Strategy → Tokens → Params → Run → Results
8. **Keyboard Navigation** - Arrow keys, Tab, Enter, Esc for all interactions
9. **Terminal Resize Handling** - TUI adapts to terminal size changes
10. **Async Execution** - Don't block TUI while backtests run

### Open Questions

1. **Token List Source** - Hyperliquid API endpoint or hardcoded list?
2. **Data Fetching Strategy** - Pre-fetch all tokens upfront or lazy fetch?
3. **Execution Strategy** - Sequential (simple) or parallel (fast)?
4. **Result Display** - Live updates or final table?
5. **Default Date Range** - What period to backtest (last 30 days? 90 days?)?
6. **Maximum Backtests** - Should there be a limit (e.g., max 100 backtests)?
7. **Data Interval** - 1h candles? 1d? Configurable?

### Success Criteria

- [ ] User can launch TUI with `cargo run -p algo-trade-cli -- tui-backtest`
- [ ] Strategy selection screen shows 2 strategies with descriptions
- [ ] Token selection screen shows all Hyperliquid perpetuals (fetched from API)
- [ ] Parameter screen allows adding/removing multiple configurations
- [ ] Backtest execution shows progress bar with current token/config
- [ ] Results table is sortable by Return, Sharpe, Win Rate, etc.
- [ ] User can export results to CSV
- [ ] All interactions work via keyboard (no mouse required)
- [ ] Existing CLI backtest command remains unchanged (backward compatibility)

---

## Section 2: Codebase Context

### Existing Architecture

**Current Backtest Flow** (`crates/cli/src/main.rs:103-156`):
```rust
async fn run_backtest(data_path: &str, strategy_name: &str) -> anyhow::Result<()> {
    // 1. Load CSV data from file
    let data_provider = HistoricalDataProvider::from_csv(data_path)?;

    // 2. Create strategy based on name
    let strategies: Vec<Arc<Mutex<dyn Strategy>>> = match strategy_name {
        "ma_crossover" => vec![Arc::new(Mutex::new(MaCrossoverStrategy::new(symbol, 10, 30)))],
        "quad_ma" => vec![Arc::new(Mutex::new(QuadMaStrategy::new(symbol)))],
        _ => anyhow::bail!("Unknown strategy"),
    };

    // 3. Create trading system
    let mut system = TradingSystem::new(
        data_provider,
        SimulatedExecutionHandler::new(0.001, 5.0),
        strategies,
        Arc::new(SimpleRiskManager::new(1000.0, 0.1)),
    );

    // 4. Run and display metrics
    let metrics = system.run().await?;
    println!("{}", MetricsFormatter::format(&metrics));
    Ok(())
}
```

**Key Integration Points**:
1. `/home/a/Work/algo-trade/crates/cli/src/main.rs:11-52` - CLI command definitions
   - Need to add new `TuiBacktest` command variant
2. `/home/a/Work/algo-trade/crates/cli/src/main.rs:123-135` - Strategy factory
   - Can reuse this pattern in TUI
3. `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs:73-137` - `fetch_candles()` method
   - Supports automatic pagination for large date ranges
4. `/home/a/Work/algo-trade/crates/core/src/engine.rs:24-296` - TradingSystem
   - Returns `PerformanceMetrics` struct (line 8-22)
5. `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs:43-56` - `QuadMaStrategy::with_periods()`
   - Already supports custom periods for parameter sweep
6. `/home/a/Work/algo-trade/crates/strategy/src/ma_crossover.rs:19-31` - `MaCrossoverStrategy::new()`
   - Takes fast/slow periods as constructor params

### Existing Patterns

1. **Strategy Construction**:
   - Factory pattern: String name → Strategy instance
   - QuadMA has default (5/8/13/21) and custom periods
   - MACrossover requires fast/slow periods (e.g., 10/30)

2. **Data Loading**:
   - `HistoricalDataProvider::from_csv(path)` loads all data into memory
   - Sorted by timestamp (line 50-55 in `backtest/src/data_provider.rs`)
   - CSV format: `timestamp,symbol,open,high,low,close,volume`

3. **Configuration**:
   - Uses `figment` for multi-source config (TOML + env vars)
   - `/home/a/Work/algo-trade/crates/core/src/config.rs` has `AppConfig` struct
   - Serde `#[serde(default)]` for optional fields

4. **Error Handling**:
   - `anyhow::Result<T>` throughout
   - `.context("Additional info")` for error chaining

5. **Async Pattern**:
   - Tokio runtime via `#[tokio::main]`
   - `Arc<Mutex<dyn Strategy>>` for concurrent access

### Current Constraints

**MUST Preserve**:
- [ ] Existing CLI commands (`run`, `backtest`, `server`, `fetch-data`)
- [ ] Current backtest signature: `run_backtest(data_path, strategy_name)`
- [ ] `PerformanceMetrics` struct fields (used by formatter)
- [ ] Strategy trait (`on_market_event`, `name`)
- [ ] Financial precision: `rust_decimal::Decimal` for all money values

**CANNOT Break**:
- [ ] CSV format for historical data
- [ ] Config.toml schema
- [ ] Public API of `TradingSystem`

**File Locations**:
- CLI entry: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
- Strategies: `/home/a/Work/algo-trade/crates/strategy/src/{ma_crossover,quad_ma}.rs`
- Backtest engine: `/home/a/Work/algo-trade/crates/core/src/engine.rs`
- Hyperliquid client: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
- Config: `/home/a/Work/algo-trade/crates/core/src/config.rs`

---

## Section 3: External Research

### Hyperliquid API - Token List Endpoint

**Endpoint**: `POST https://api.hyperliquid.xyz/info`

**Request Body**:
```json
{
  "type": "meta"
}
```

**Response** (perpetuals metadata):
```json
{
  "universe": [
    {
      "name": "BTC",
      "szDecimals": 5,
      "maxLeverage": 50,
      "onlyIsolated": false
    },
    {
      "name": "ETH",
      "szDecimals": 4,
      "maxLeverage": 50,
      "onlyIsolated": false
    },
    // ... more symbols
  ]
}
```

**Implementation Pattern**:
```rust
pub async fn fetch_available_symbols(&self) -> Result<Vec<String>> {
    let request_body = serde_json::json!({ "type": "meta" });
    let response = self.post("/info", request_body).await?;

    let universe = response["universe"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing universe in meta response"))?;

    let symbols: Vec<String> = universe
        .iter()
        .filter_map(|item| item["name"].as_str().map(String::from))
        .collect();

    Ok(symbols)
}
```

**Rate Limit**: Shared 1200 req/min (already implemented in `HyperliquidClient`)

---

### Ratatui TUI Framework

**Version**: `0.28.1` (latest stable as of 2025)

**Core Dependencies**:
```toml
ratatui = "0.28"
crossterm = "0.28"
```

**Event Loop Pattern**:
```rust
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};

fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    // App state
    let mut app = App::new();

    // Event loop
    loop {
        terminal.draw(|frame| ui(frame, &app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Down => app.next(),
                        KeyCode::Up => app.previous(),
                        _ => {}
                    }
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
```

**Multi-Step Wizard Pattern**:
```rust
enum AppState {
    StrategySelection,
    TokenSelection,
    ParameterConfig,
    Running,
    Results,
}

struct App {
    state: AppState,
    // ... state for each screen
}

impl App {
    fn handle_key(&mut self, key: KeyCode) {
        match self.state {
            AppState::StrategySelection => { /* ... */ },
            AppState::TokenSelection => { /* ... */ },
            // ...
        }
    }
}
```

**Table Widget** (for results display):
```rust
use ratatui::widgets::{Table, Row, TableState};

let rows = results.iter().map(|r| {
    Row::new(vec![
        r.symbol.clone(),
        r.config_name.clone(),
        format!("{:.2}%", r.return * 100.0),
        format!("{:.2}", r.sharpe),
    ])
});

let table = Table::new(rows, widths)
    .header(Row::new(vec!["Token", "Config", "Return", "Sharpe"]))
    .highlight_style(Style::default().bg(Color::DarkGray));

frame.render_stateful_widget(table, area, &mut table_state);
```

**List Widget** (for token selection):
```rust
use ratatui::widgets::{List, ListState, ListItem};

let items: Vec<ListItem> = tokens.iter().enumerate().map(|(i, token)| {
    let prefix = if selected.contains(&i) { "[x] " } else { "[ ] " };
    ListItem::new(format!("{}{}", prefix, token))
}).collect();

let list = List::new(items).highlight_style(Style::default().bg(Color::Blue));
frame.render_stateful_widget(list, area, &mut list_state);
```

---

### Form Input Libraries

**tui-input** (`0.10.1`):
- Single-line text input
- Cursor navigation, insert/delete
- Returns `String` value

**tui-textarea** (`0.6.1`):
- Multi-line editor (overkill for param input)
- Built-in validation support

**Recommendation**: Use `tui-input` for parameter fields (simpler, lighter)

**Example**:
```rust
use tui_input::Input;

let mut ma1_input = Input::default().with_value("5");

// In event handler
match key.code {
    KeyCode::Char(c) => { ma1_input.handle(Event::Key(key)); }
    KeyCode::Backspace => { ma1_input.handle(Event::Key(key)); }
    _ => {}
}

// Validate
let ma1: usize = ma1_input.value().parse()
    .context("MA1 must be a positive integer")?;
```

---

### Parallel Execution with Tokio

**Pattern**: `tokio::spawn` with `JoinSet` for progress tracking

```rust
use tokio::task::JoinSet;

let mut set = JoinSet::new();

for (token, config) in backtest_matrix {
    set.spawn(async move {
        run_single_backtest(token, config).await
    });
}

// Collect results
let mut results = Vec::new();
while let Some(res) = set.join_next().await {
    results.push(res??);
}
```

**Challenge**: Cannot update TUI progress from spawned tasks (need channel)

**Solution**: Use `mpsc::channel` to send progress updates to main loop

```rust
let (tx, mut rx) = mpsc::channel(100);

// Spawn backtest tasks
for (token, config) in matrix {
    let tx = tx.clone();
    set.spawn(async move {
        tx.send(ProgressUpdate::Started(token.clone())).await.ok();
        let result = run_single_backtest(token, config).await?;
        tx.send(ProgressUpdate::Completed(result)).await.ok();
        Ok(result)
    });
}

// Main loop
loop {
    terminal.draw(|f| ui(f, &app))?;

    // Check for progress updates (non-blocking)
    if let Ok(update) = rx.try_recv() {
        match update {
            ProgressUpdate::Started(token) => app.current = token,
            ProgressUpdate::Completed(result) => app.results.push(result),
        }
    }

    // Handle keyboard events...
}
```

---

### Progress Bar Libraries

**indicatif** (`0.17.8`):
- CLI progress bars
- **Problem**: Conflicts with ratatui (both control terminal)
- **Not suitable** for TUI integration

**Recommendation**: Implement progress display using ratatui's `Gauge` widget

```rust
use ratatui::widgets::Gauge;

let progress = app.completed as f64 / app.total as f64;
let gauge = Gauge::default()
    .block(Block::default().title("Progress"))
    .gauge_style(Style::default().fg(Color::Green))
    .percent((progress * 100.0) as u16);

frame.render_widget(gauge, area);
```

---

### Parameter Sweep Pattern

**Grid Search** (Cartesian Product):

**itertools** crate:
```rust
use itertools::iproduct;

// For 2 parameters
let combinations: Vec<(usize, usize)> = iproduct!(vec![5, 10, 20], vec![30, 50, 100])
    .collect();
// Results: [(5, 30), (5, 50), (5, 100), (10, 30), ...]

// For 4 parameters (QuadMA)
let configs: Vec<(usize, usize, usize, usize)> = iproduct!(
    vec![3, 5, 10],
    vec![8, 13, 20],
    vec![13, 21, 34],
    vec![21, 34, 55]
).collect();
```

**Alternative**: Manual nested loops (simpler for 2-3 configs)

```rust
struct ParamConfig {
    name: String,
    ma1: usize,
    ma2: usize,
    ma3: usize,
    ma4: usize,
}

let configs = vec![
    ParamConfig { name: "Default".to_string(), ma1: 5, ma2: 8, ma3: 13, ma4: 21 },
    ParamConfig { name: "Fast".to_string(), ma1: 3, ma2: 5, ma3: 8, ma4: 13 },
    ParamConfig { name: "Slow".to_string(), ma1: 10, ma2: 20, ma3: 50, ma4: 100 },
];

let matrix: Vec<(String, ParamConfig)> = tokens.iter()
    .flat_map(|token| configs.iter().map(move |config| (token.clone(), config.clone())))
    .collect();
// Total backtests: tokens.len() * configs.len()
```

---

### Data Caching Strategy

**Problem**: Fetching OHLCV for 50 tokens × 3 configs = 50 unique fetches (not 150)

**Solution**: Cache downloaded data by symbol to disk

**Pattern**:
```rust
use std::path::Path;

async fn get_or_fetch_data(
    symbol: &str,
    interval: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    client: &HyperliquidClient,
) -> Result<String> {
    let cache_path = format!("cache/{}_{}_{}_{}.csv",
        symbol, interval, start.timestamp(), end.timestamp());

    if Path::new(&cache_path).exists() {
        Ok(cache_path)
    } else {
        let records = client.fetch_candles(symbol, interval, start, end).await?;
        CsvStorage::write_ohlcv(&cache_path, &records)?;
        Ok(cache_path)
    }
}
```

**Directory Structure**:
```
cache/
├── BTC_1h_1704067200_1711929600.csv
├── ETH_1h_1704067200_1711929600.csv
├── SOL_1h_1704067200_1711929600.csv
└── ...
```

---

### Crate Recommendations

| Crate | Version | Purpose | Notes |
|-------|---------|---------|-------|
| `ratatui` | 0.28.1 | TUI framework | Successor to tui-rs |
| `crossterm` | 0.28.1 | Terminal backend | Cross-platform, default for ratatui |
| `tui-input` | 0.10.1 | Text input widget | For parameter entry |
| `itertools` | 0.13.0 | Cartesian product | For parameter combinations |
| `tokio` | 1.40.0 | Async runtime | Already in use |
| `anyhow` | 1.0 | Error handling | Already in use |
| `chrono` | 0.4 | Date/time | Already in use |

**Already Available** (no new deps needed):
- `serde`, `serde_json` - Serialization
- `rust_decimal` - Financial precision
- `csv` - CSV reading/writing

---

## Section 4: Architectural Recommendations

### Proposed Design

**Multi-Screen Wizard Flow**:

```
┌────────────────────────────────────────────────┐
│ Screen 1: Strategy Selection                   │
│                                                 │
│ Select a strategy:                             │
│ > [•] MA Crossover (2 periods)                 │
│   [ ] Quad MA (4 periods - Fibonacci)          │
│                                                 │
│ [Enter] Continue  [Q] Quit                     │
└────────────────────────────────────────────────┘
          ↓ [Enter]
┌────────────────────────────────────────────────┐
│ Screen 2: Token Selection                      │
│                                                 │
│ Select tokens (Space to toggle, A for all):    │
│   [x] BTC     [x] ETH     [ ] SOL              │
│   [x] AVAX    [ ] MATIC   [x] DOGE             │
│   [ ] APT     [x] SUI     [ ] ARB              │
│                                                 │
│ Selected: 5 tokens                             │
│ [Enter] Continue  [Esc] Back  [Q] Quit         │
└────────────────────────────────────────────────┘
          ↓ [Enter]
┌────────────────────────────────────────────────┐
│ Screen 3: Parameter Configurations             │
│                                                 │
│ Config 1 (Default): [Edit] [Delete]            │
│   MA1: 5   MA2: 8   MA3: 13   MA4: 21          │
│                                                 │
│ Config 2 (Custom): [Edit] [Delete]             │
│   MA1: 10  MA2: 20  MA3: 50   MA4: 100         │
│                                                 │
│ [+] Add Config  [Enter] Run  [Esc] Back        │
└────────────────────────────────────────────────┘
          ↓ [Enter]
┌────────────────────────────────────────────────┐
│ Screen 4: Running Backtests                    │
│                                                 │
│ Running 5 tokens × 2 configs = 10 backtests    │
│                                                 │
│ ████████████████░░░░ 80% (8/10)                │
│ Current: ETH - Config 2                        │
│                                                 │
│ [Esc] Cancel                                   │
└────────────────────────────────────────────────┘
          ↓ Complete
┌────────────────────────────────────────────────┐
│ Screen 5: Results                              │
│                                                 │
│ Sort by: [Return▼] Sharpe Win% Trades          │
│ ┌─────┬─────────┬────────┬────────┬────────┐  │
│ │Token│Config   │Return  │Sharpe  │Win%    │  │
│ ├─────┼─────────┼────────┼────────┼────────┤  │
│ │ETH  │Config 2 │ 12.5%  │  2.34  │ 65%    │  │
│ │BTC  │Default  │  8.3%  │  1.89  │ 58%    │  │
│ │DOGE │Config 2 │  6.1%  │  1.45  │ 52%    │  │
│ │AVAX │Default  │  3.2%  │  0.87  │ 48%    │  │
│ │SUI  │Config 2 │ -1.5%  │ -0.34  │ 42%    │  │
│ └─────┴─────────┴────────┴────────┴────────┘  │
│                                                 │
│ [S] Save CSV  [R] Restart  [Q] Quit            │
└────────────────────────────────────────────────┘
```

**State Machine**:

```rust
enum AppState {
    StrategySelection {
        selected: StrategyType,
    },
    TokenSelection {
        strategy: StrategyType,
        tokens: Vec<String>,       // All available
        selected: HashSet<usize>,   // Indices of selected
        list_state: ListState,
    },
    ParameterConfig {
        strategy: StrategyType,
        tokens: Vec<String>,        // Selected tokens
        configs: Vec<ParamConfig>,
        editing_index: Option<usize>,
    },
    Running {
        matrix: Vec<BacktestJob>,
        completed: usize,
        total: usize,
        current_job: String,
        results: Vec<BacktestResult>,
    },
    Results {
        results: Vec<BacktestResult>,
        sort_by: SortColumn,
        table_state: TableState,
    },
}

enum StrategyType {
    MaCrossover,
    QuadMa,
}

struct ParamConfig {
    name: String,
    params: StrategyParams,
}

enum StrategyParams {
    MaCrossover { fast: usize, slow: usize },
    QuadMa { ma1: usize, ma2: usize, ma3: usize, ma4: usize },
}

struct BacktestJob {
    symbol: String,
    config_name: String,
    params: StrategyParams,
}

struct BacktestResult {
    symbol: String,
    config_name: String,
    metrics: PerformanceMetrics,
}
```

---

### Module Changes

**NEW FILE**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs` (~800 lines)

Main TUI application logic:

```rust
pub struct TuiApp {
    state: AppState,
    hyperliquid_client: HyperliquidClient,
}

impl TuiApp {
    pub async fn new() -> Result<Self> {
        let api_url = "https://api.hyperliquid.xyz".to_string();
        let client = HyperliquidClient::new(api_url);

        Ok(Self {
            state: AppState::StrategySelection { selected: StrategyType::MaCrossover },
            hyperliquid_client: client,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout());
        let mut terminal = Terminal::new(backend)?;

        // Main event loop
        loop {
            terminal.draw(|f| self.ui(f))?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if !self.handle_key(key.code).await? {
                            break;
                        }
                    }
                }
            }

            // Check for backtest progress updates
            if let AppState::Running { .. } = &mut self.state {
                self.poll_backtest_progress().await?;
            }
        }

        // Restore terminal
        disable_raw_mode()?;
        stdout().execute(LeaveAlternateScreen)?;
        Ok(())
    }

    fn ui(&self, frame: &mut Frame) {
        match &self.state {
            AppState::StrategySelection { .. } => self.ui_strategy_selection(frame),
            AppState::TokenSelection { .. } => self.ui_token_selection(frame),
            AppState::ParameterConfig { .. } => self.ui_parameter_config(frame),
            AppState::Running { .. } => self.ui_running(frame),
            AppState::Results { .. } => self.ui_results(frame),
        }
    }

    async fn handle_key(&mut self, key: KeyCode) -> Result<bool> {
        match key {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(false),
            _ => {}
        }

        match &mut self.state {
            AppState::StrategySelection { selected } => {
                self.handle_strategy_selection_key(key, selected).await?;
            }
            AppState::TokenSelection { .. } => {
                self.handle_token_selection_key(key).await?;
            }
            AppState::ParameterConfig { .. } => {
                self.handle_parameter_config_key(key).await?;
            }
            AppState::Running { .. } => {
                if key == KeyCode::Esc {
                    // Cancel backtests
                    self.cancel_backtests();
                }
            }
            AppState::Results { .. } => {
                self.handle_results_key(key).await?;
            }
        }

        Ok(true)
    }
}
```

**NEW FILE**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs` (~400 lines)

Screen rendering functions:

```rust
impl TuiApp {
    pub fn ui_strategy_selection(&self, frame: &mut Frame) {
        let area = frame.area();

        let items = vec![
            ListItem::new("MA Crossover (2 moving averages)"),
            ListItem::new("Quad MA (4 moving averages - Fibonacci sequence)"),
        ];

        let list = List::new(items)
            .block(Block::default().title("Select Strategy").borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::Blue))
            .highlight_symbol("> ");

        frame.render_widget(list, area);
    }

    pub fn ui_token_selection(&self, frame: &mut Frame, tokens: &[String], selected: &HashSet<usize>) {
        // Grid layout for tokens with checkboxes
        // ...
    }

    pub fn ui_parameter_config(&self, frame: &mut Frame, configs: &[ParamConfig]) {
        // List of configurations with edit/delete buttons
        // ...
    }

    pub fn ui_running(&self, frame: &mut Frame, completed: usize, total: usize, current: &str) {
        let progress = completed as f64 / total as f64;
        let gauge = Gauge::default()
            .block(Block::default().title("Running Backtests").borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Green))
            .percent((progress * 100.0) as u16)
            .label(format!("{}/{} - {}", completed, total, current));

        frame.render_widget(gauge, area);
    }

    pub fn ui_results(&self, frame: &mut Frame, results: &[BacktestResult], sort_by: SortColumn) {
        let rows = results.iter().map(|r| {
            Row::new(vec![
                r.symbol.clone(),
                r.config_name.clone(),
                format!("{:.2}%", r.metrics.total_return.to_string().parse::<f64>().unwrap() * 100.0),
                format!("{:.2}", r.metrics.sharpe_ratio),
                format!("{:.1}%", r.metrics.win_rate * 100.0),
                r.metrics.num_trades.to_string(),
            ])
        });

        let table = Table::new(rows, widths)
            .header(Row::new(vec!["Token", "Config", "Return", "Sharpe", "Win%", "Trades"]))
            .block(Block::default().title("Results").borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::DarkGray));

        frame.render_stateful_widget(table, area, &mut table_state);
    }
}
```

**NEW FILE**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` (~300 lines)

Backtest execution engine:

```rust
pub struct BacktestRunner {
    hyperliquid_client: HyperliquidClient,
    progress_tx: mpsc::Sender<ProgressUpdate>,
}

pub enum ProgressUpdate {
    Started { symbol: String, config: String },
    Completed { result: BacktestResult },
    Failed { symbol: String, config: String, error: String },
}

impl BacktestRunner {
    pub async fn run_batch(
        &self,
        jobs: Vec<BacktestJob>,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<BacktestResult>> {
        let mut results = Vec::new();

        for job in jobs {
            // Send progress update
            self.progress_tx.send(ProgressUpdate::Started {
                symbol: job.symbol.clone(),
                config: job.config_name.clone(),
            }).await?;

            // Get or fetch data
            let csv_path = self.get_or_fetch_data(&job.symbol, interval, start, end).await?;

            // Run backtest
            match self.run_single_backtest(&job, &csv_path).await {
                Ok(result) => {
                    self.progress_tx.send(ProgressUpdate::Completed {
                        result: result.clone(),
                    }).await?;
                    results.push(result);
                }
                Err(e) => {
                    self.progress_tx.send(ProgressUpdate::Failed {
                        symbol: job.symbol.clone(),
                        config: job.config_name.clone(),
                        error: e.to_string(),
                    }).await?;
                }
            }
        }

        Ok(results)
    }

    async fn get_or_fetch_data(
        &self,
        symbol: &str,
        interval: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<String> {
        let cache_dir = "cache";
        std::fs::create_dir_all(cache_dir)?;

        let cache_path = format!("{}/{}_{}_{}_{}.csv",
            cache_dir, symbol, interval, start.timestamp(), end.timestamp());

        if Path::new(&cache_path).exists() {
            tracing::info!("Using cached data for {}", symbol);
            Ok(cache_path)
        } else {
            tracing::info!("Fetching data for {}", symbol);
            let records = self.hyperliquid_client
                .fetch_candles(symbol, interval, start, end)
                .await?;

            CsvStorage::write_ohlcv(&cache_path, &records)?;
            Ok(cache_path)
        }
    }

    async fn run_single_backtest(
        &self,
        job: &BacktestJob,
        csv_path: &str,
    ) -> Result<BacktestResult> {
        // Load data
        let data_provider = HistoricalDataProvider::from_csv(csv_path)?;

        // Create strategy
        let strategy: Arc<Mutex<dyn Strategy>> = match &job.params {
            StrategyParams::MaCrossover { fast, slow } => {
                Arc::new(Mutex::new(MaCrossoverStrategy::new(
                    job.symbol.clone(), *fast, *slow
                )))
            }
            StrategyParams::QuadMa { ma1, ma2, ma3, ma4 } => {
                Arc::new(Mutex::new(QuadMaStrategy::with_periods(
                    job.symbol.clone(), *ma1, *ma2, *ma3, *ma4
                )))
            }
        };

        // Create trading system
        let mut system = TradingSystem::new(
            data_provider,
            SimulatedExecutionHandler::new(0.001, 5.0),
            vec![strategy],
            Arc::new(SimpleRiskManager::new(1000.0, 0.1)),
        );

        // Run backtest
        let metrics = system.run().await?;

        Ok(BacktestResult {
            symbol: job.symbol.clone(),
            config_name: job.config_name.clone(),
            metrics,
        })
    }
}
```

**MODIFY**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`

Add TUI command:

```rust
// Line 11: Add to Commands enum
enum Commands {
    // ... existing commands

    /// Interactive TUI for multi-token parameter sweep backtesting
    TuiBacktest {
        /// Candle interval (1h, 1d, etc.)
        #[arg(short, long, default_value = "1h")]
        interval: String,

        /// Start time in ISO 8601 format
        #[arg(long, default_value = "2025-01-01T00:00:00Z")]
        start: String,

        /// End time in ISO 8601 format
        #[arg(long, default_value = "2025-03-01T00:00:00Z")]
        end: String,
    },
}

// Line 66: Add to match statement
match cli.command {
    // ... existing commands

    Commands::TuiBacktest { interval, start, end } => {
        run_tui_backtest(&interval, &start, &end).await?;
    }
}

// New function
async fn run_tui_backtest(
    interval: &str,
    start_str: &str,
    end_str: &str,
) -> anyhow::Result<()> {
    use tui_backtest::TuiApp;

    let start: DateTime<Utc> = start_str.parse()?;
    let end: DateTime<Utc> = end_str.parse()?;

    let mut app = TuiApp::new(interval, start, end).await?;
    app.run().await?;

    Ok(())
}
```

**NEW METHOD**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`

Add to `HyperliquidClient`:

```rust
// Line 217: Add new method
/// Fetches list of all tradeable perpetual symbols
///
/// # Errors
/// Returns error if API request fails or response parsing fails
pub async fn fetch_available_symbols(&self) -> Result<Vec<String>> {
    let request_body = serde_json::json!({ "type": "meta" });
    let response = self.post("/info", request_body).await?;

    let universe = response["universe"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing universe in meta response"))?;

    let symbols: Vec<String> = universe
        .iter()
        .filter_map(|item| item["name"].as_str().map(String::from))
        .collect();

    tracing::info!("Fetched {} available symbols from Hyperliquid", symbols.len());

    Ok(symbols)
}
```

---

### Critical Decisions

**Decision 1: Sequential vs Parallel Execution**

- **Choice**: **Sequential** (at least for MVP)
- **Rationale**:
  - Simpler progress tracking (1 backtest at a time)
  - TUI updates easier to implement (no complex channel orchestration)
  - Network I/O (data fetching) is bottleneck, not CPU
  - Can add parallel later as optimization
- **Alternative**: Parallel with `JoinSet` (rejected for MVP complexity)
- **Trade-off**: Slower total time, but simpler codebase

**Decision 2: Data Caching Strategy**

- **Choice**: **Disk cache** in `cache/` directory
- **Rationale**:
  - Avoids re-downloading same symbol multiple times
  - Persists across TUI sessions
  - Simple key: `symbol_interval_start_end.csv`
- **Alternative**: In-memory cache (rejected - lost on restart)
- **Trade-off**: Disk space usage, but negligible for ~50 symbols

**Decision 3: TUI Framework**

- **Choice**: **ratatui + crossterm**
- **Rationale**:
  - Modern, actively maintained (tui-rs is deprecated)
  - Excellent docs and examples
  - Crossterm is cross-platform
  - Already used in Rust ecosystem (e.g., `gitui`)
- **Alternative**: cursive (rejected - heavier, different paradigm)
- **Trade-off**: None - clear winner

**Decision 4: Progress Display**

- **Choice**: **Ratatui Gauge widget** (not indicatif)
- **Rationale**:
  - indicatif conflicts with ratatui terminal control
  - Gauge integrates seamlessly with TUI
  - Supports percentage + text label
- **Alternative**: indicatif MultiProgress (rejected - incompatible)
- **Trade-off**: None

**Decision 5: Parameter Input**

- **Choice**: **tui-input** for single-line fields
- **Rationale**:
  - Lightweight (vs tui-textarea)
  - Perfect for numeric inputs
  - Built-in cursor navigation
- **Alternative**: Custom input widget (rejected - reinventing wheel)
- **Trade-off**: Extra dependency, but small and focused

**Decision 6: Result Persistence**

- **Choice**: **CSV export** (optional)
- **Rationale**:
  - Easy to analyze in Excel/Python
  - Human-readable
  - Standardized format
- **Alternative**: JSON (also viable, can add both)
- **Trade-off**: CSV lacks nested structures, but metrics are flat

**Decision 7: Default Date Range**

- **Choice**: **Last 60 days** (CLI arg overridable)
- **Rationale**:
  - Recent market conditions
  - ~1440 candles at 1h (good statistical sample)
  - Fast to fetch (<5000 candle limit)
- **Alternative**: Last 365 days (rejected - too slow for default)
- **Trade-off**: May not capture full market cycle

---

### Risk Assessment

**Breaking Changes**:
- ❌ NONE - New command, no modifications to existing commands

**Performance Implications**:
- ⚠️  Fetching 50 symbols × 1h data for 60 days = ~50 API calls (~5 seconds with rate limiting)
- ⚠️  Running 50 backtests sequentially = ~30-60 seconds (depends on data size)
- ✅ Caching reduces subsequent runs to <5 seconds

**Security Implications**:
- ✅ No API keys needed (public data only)
- ✅ Cache directory isolated (`cache/`)
- ⚠️  Cache directory not auto-cleaned (could grow large over time)

**Usability Implications**:
- ✅ Keyboard-only navigation (accessible)
- ⚠️  Terminal must be ≥80×24 (standard, but could be issue)
- ⚠️  Color scheme assumes dark background (most terminals)

---

## Section 5: Edge Cases & Constraints

### Edge Cases

**EC1: Hyperliquid API Failure**
- **Scenario**: `/info` endpoint returns 500 error
- **Expected Behavior**: Show error message in TUI, allow retry or quit
- **TaskMaster TODO**: Add retry logic with exponential backoff in `fetch_available_symbols()`

**EC2: Token Has Insufficient Data**
- **Scenario**: Token launched 10 days ago, backtest requests 60 days
- **Expected Behavior**: Skip token with warning, continue with others
- **TaskMaster TODO**: Check CSV length before backtest, show "Insufficient data (N candles)" in results

**EC3: Invalid Parameter Combination**
- **Scenario**: User enters MA1=50, MA2=30 (MA1 > MA2)
- **Expected Behavior**: Validation error on parameter config screen, red text
- **TaskMaster TODO**: Add validation in parameter input handler: `ma1 < ma2 < ma3 < ma4`

**EC4: Terminal Resize During Backtest**
- **Scenario**: User resizes terminal while backtests running
- **Expected Behavior**: TUI adapts layout gracefully
- **TaskMaster TODO**: Ratatui handles this automatically, no action needed

**EC5: User Cancels Mid-Backtest**
- **Scenario**: User presses Esc during running screen
- **Expected Behavior**: Stop current backtest, return to results screen with partial results
- **TaskMaster TODO**: Add cancellation flag, check between backtests, save partial results

**EC6: All Tokens Deselected**
- **Scenario**: User advances from token screen with 0 selected
- **Expected Behavior**: Validation error: "Select at least 1 token"
- **TaskMaster TODO**: Check `selected.len() > 0` on Enter key in token selection

**EC7: 100+ Backtests Selected**
- **Scenario**: User selects 50 tokens × 5 configs = 250 backtests
- **Expected Behavior**: Confirmation dialog: "This will take ~15 minutes. Continue?"
- **TaskMaster TODO**: Add threshold check (e.g., >100 jobs) and show estimate

**EC8: Cache Directory Permissions**
- **Scenario**: `cache/` directory not writable
- **Expected Behavior**: Error message, fall back to temp directory or fail gracefully
- **TaskMaster TODO**: Check directory creation with `.context("Failed to create cache dir")`

**EC9: Network Disconnection During Data Fetch**
- **Scenario**: WiFi drops while fetching BTC candles
- **Expected Behavior**: Retry with backoff, then skip token with error message
- **TaskMaster TODO**: Wrap `fetch_candles()` in retry logic (3 attempts)

**EC10: Duplicate Config Names**
- **Scenario**: User names two configs "Fast"
- **Expected Behavior**: Auto-append number: "Fast", "Fast (2)"
- **TaskMaster TODO**: Validate uniqueness in `add_config()`, auto-rename if duplicate

### Constraints

**C1: Terminal Size**
- **Minimum**: 80 columns × 24 rows
- **Reason**: Results table needs ~70 chars width
- **TaskMaster TODO**: Check terminal size on startup, error if too small

**C2: Maximum Tokens**
- **Limit**: 100 tokens
- **Reason**: Hyperliquid may have ~50-200 perpetuals, but UI becomes unwieldy
- **TaskMaster TODO**: No hard limit needed (Hyperliquid universe is ~50 currently)

**C3: Maximum Configs**
- **Limit**: 10 parameter configurations
- **Reason**: UI space limited, >10 is excessive for manual analysis
- **TaskMaster TODO**: Enforce in "Add Config" handler with error message

**C4: Date Range**
- **Minimum**: 7 days
- **Maximum**: 365 days
- **Reason**: <7 days insufficient for strategy evaluation, >365 days slow to fetch
- **TaskMaster TODO**: Validate CLI args, show error if out of range

**C5: Candle Interval**
- **Supported**: 1m, 5m, 15m, 1h, 4h, 1d
- **Reason**: Hyperliquid API interval whitelist
- **TaskMaster TODO**: Use existing `interval_to_millis()` validation

**C6: Strategy Periods vs Data Length**
- **Constraint**: Slowest MA period must be < data length
- **Example**: MA4=100 requires ≥100 candles
- **TaskMaster TODO**: Calculate required candles, error if insufficient: "QuadMA (21) requires ≥21 candles, got 15"

**C7: Memory Usage**
- **Estimate**: 50 tokens × 1440 candles × ~200 bytes = ~14 MB
- **Concern**: Minimal for modern systems
- **TaskMaster TODO**: No action needed (data loaded one backtest at a time)

**C8: CSV Format**
- **Constraint**: Must match `timestamp,symbol,open,high,low,close,volume`
- **Reason**: `HistoricalDataProvider::from_csv()` expects this format
- **TaskMaster TODO**: Ensure `CsvStorage::write_ohlcv()` uses correct format (already implemented)

### Testing Requirements

**Unit Tests**:
- [ ] `StrategyParams` validation (MA1 < MA2 < MA3 < MA4)
- [ ] `BacktestJob` matrix generation (tokens × configs)
- [ ] Cache path generation (symbol, interval, timestamps)
- [ ] Config name uniqueness enforcement
- [ ] Sort column comparison functions

**Integration Tests**:
- [ ] Fetch Hyperliquid token list (live API call)
- [ ] Run single backtest with QuadMA strategy
- [ ] Cache hit/miss logic (fetch once, reuse second time)
- [ ] CSV result export format

**Manual Tests** (TUI requires interactive):
- [ ] Navigate through all 5 screens with keyboard
- [ ] Select multiple tokens with Space, deselect with Space
- [ ] Add 3 parameter configs, delete middle one
- [ ] Cancel backtest mid-run, verify partial results
- [ ] Sort results by each column (Return, Sharpe, Win%)
- [ ] Export results to CSV, verify format
- [ ] Terminal resize during each screen
- [ ] Quit from each screen (Q key)

---

## Section 6: TaskMaster Handoff Package

### MUST DO

1. ✅ **Add `ratatui`, `crossterm`, `tui-input` dependencies** to `crates/cli/Cargo.toml`
2. ✅ **Create `crates/cli/src/tui_backtest.rs`** - Main TUI application (~800 lines)
3. ✅ **Create `crates/cli/src/tui_backtest/screens.rs`** - Screen rendering (~400 lines)
4. ✅ **Create `crates/cli/src/tui_backtest/runner.rs`** - Backtest execution (~300 lines)
5. ✅ **Add `TuiBacktest` command** to `crates/cli/src/main.rs:11` (Commands enum)
6. ✅ **Add `run_tui_backtest()` handler** to `crates/cli/src/main.rs:66` (match statement)
7. ✅ **Add `fetch_available_symbols()` method** to `crates/exchange-hyperliquid/src/client.rs:217`
8. ✅ **Implement 5-screen state machine**: Strategy → Tokens → Params → Running → Results
9. ✅ **Implement data caching** in `cache/` directory with format `symbol_interval_start_end.csv`
10. ✅ **Implement progress tracking** with ratatui Gauge widget
11. ✅ **Implement results table** with sorting by Return, Sharpe, Win%, Trades
12. ✅ **Implement CSV export** in results screen (S key)
13. ✅ **Add parameter validation**: MA1 < MA2 < MA3 < MA4, periods > 0
14. ✅ **Add edge case handling**: Empty selection, network errors, insufficient data
15. ✅ **Preserve backward compatibility**: Existing `backtest` command unchanged

### MUST NOT DO

1. ❌ DO NOT modify existing `run_backtest()` function (line 103-156 in `cli/src/main.rs`)
2. ❌ DO NOT change `PerformanceMetrics` struct (used by existing code)
3. ❌ DO NOT use `f64` for financial values (use `rust_decimal::Decimal`)
4. ❌ DO NOT add parallel execution in MVP (keep sequential for simplicity)
5. ❌ DO NOT use `indicatif` for progress (conflicts with ratatui)
6. ❌ DO NOT hardcode token list (must fetch from Hyperliquid API)
7. ❌ DO NOT break existing CLI commands (`run`, `server`, `fetch-data`)
8. ❌ DO NOT modify `TradingSystem` or `Strategy` traits (public API)
9. ❌ DO NOT add authentication (TUI uses public data only)
10. ❌ DO NOT auto-clean cache directory (let user manage disk space)

### Exact File Modifications

**Task 1: Add dependencies**
- **File**: `/home/a/Work/algo-trade/crates/cli/Cargo.toml`
- **Line**: ~15 (in `[dependencies]` section)
- **Complexity**: LOW
- **Add**:
  ```toml
  ratatui = "0.28"
  crossterm = "0.28"
  tui-input = "0.10"
  ```

**Task 2: Add TUI command**
- **File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
- **Line**: 11 (Commands enum)
- **Complexity**: LOW
- **Add**:
  ```rust
  /// Interactive TUI for multi-token parameter sweep backtesting
  TuiBacktest {
      /// Candle interval (1h, 1d, etc.)
      #[arg(short, long, default_value = "1h")]
      interval: String,

      /// Start time in ISO 8601 format
      #[arg(long, default_value = "2025-01-01T00:00:00Z")]
      start: String,

      /// End time in ISO 8601 format
      #[arg(long, default_value = "2025-03-01T00:00:00Z")]
      end: String,
  },
  ```

**Task 3: Add command handler**
- **File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
- **Line**: 66 (match cli.command)
- **Complexity**: LOW
- **Add**:
  ```rust
  Commands::TuiBacktest { interval, start, end } => {
      run_tui_backtest(&interval, &start, &end).await?;
  }
  ```

**Task 4: Add handler function**
- **File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
- **Line**: 234 (after `run_fetch_data()`)
- **Complexity**: MEDIUM
- **Add**:
  ```rust
  async fn run_tui_backtest(
      interval: &str,
      start_str: &str,
      end_str: &str,
  ) -> anyhow::Result<()> {
      use tui_backtest::TuiApp;

      let start: DateTime<Utc> = start_str.parse()
          .context("Invalid start time. Use ISO 8601 format")?;
      let end: DateTime<Utc> = end_str.parse()
          .context("Invalid end time. Use ISO 8601 format")?;

      if start >= end {
          anyhow::bail!("Start time must be before end time");
      }

      let mut app = TuiApp::new(interval.to_string(), start, end).await?;
      app.run().await?;

      Ok(())
  }
  ```

**Task 5: Create TUI module**
- **File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest.rs` (NEW)
- **Lines**: ~800
- **Complexity**: HIGH
- **Content**: Main TuiApp struct, event loop, state machine, UI rendering
- **Dependencies**: Requires Task 1 (Cargo.toml) complete

**Task 6: Create screens module**
- **File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/screens.rs` (NEW)
- **Lines**: ~400
- **Complexity**: HIGH
- **Content**: 5 screen rendering functions (strategy, tokens, params, running, results)
- **Dependencies**: Requires Task 5 (tui_backtest.rs) complete

**Task 7: Create runner module**
- **File**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs` (NEW)
- **Lines**: ~300
- **Complexity**: HIGH
- **Content**: BacktestRunner struct, batch execution, data caching, progress updates
- **Dependencies**: Requires Task 5 (tui_backtest.rs) complete

**Task 8: Add Hyperliquid API method**
- **File**: `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs`
- **Line**: 217 (after `fetch_candles_chunk()`)
- **Complexity**: MEDIUM
- **Add**:
  ```rust
  /// Fetches list of all tradeable perpetual symbols
  ///
  /// # Errors
  /// Returns error if API request fails or response parsing fails
  pub async fn fetch_available_symbols(&self) -> Result<Vec<String>> {
      let request_body = serde_json::json!({ "type": "meta" });
      let response = self.post("/info", request_body).await?;

      let universe = response["universe"]
          .as_array()
          .ok_or_else(|| anyhow::anyhow!("Missing universe in meta response"))?;

      let symbols: Vec<String> = universe
          .iter()
          .filter_map(|item| item["name"].as_str().map(String::from))
          .collect();

      tracing::info!("Fetched {} available symbols from Hyperliquid", symbols.len());

      Ok(symbols)
  }
  ```

**Task 9: Add module declaration**
- **File**: `/home/a/Work/algo-trade/crates/cli/src/main.rs`
- **Line**: 1 (top of file)
- **Complexity**: LOW
- **Add**: `mod tui_backtest;`

**Task 10: Create cache directory**
- **File**: N/A (directory creation in code)
- **Location**: `/home/a/Work/algo-trade/crates/cli/src/tui_backtest/runner.rs`
- **Complexity**: LOW
- **Add**: `std::fs::create_dir_all("cache")?;` in `BacktestRunner::new()`

### Task Dependencies

```
Task 1 (Cargo.toml deps) ──┬─→ Task 5 (tui_backtest.rs)
                           └─→ Task 8 (Hyperliquid API)

Task 2 (TUI command) ──┬─→ Task 3 (command handler)
Task 3 ────────────────┴─→ Task 4 (handler function)

Task 4 ──→ Task 9 (module declaration)

Task 5 ──┬─→ Task 6 (screens.rs)
         └─→ Task 7 (runner.rs)

Task 6 ──┬─→ Task 7
Task 7 ──┴─→ Task 10 (cache directory)

All tasks ──→ Integration test
```

### Estimated Complexity

| Task | LOC | Time | Risk | Reason |
|------|-----|------|------|--------|
| 1 | 3 | 5m | LOW | Simple dependency add |
| 2 | 12 | 10m | LOW | Enum variant with args |
| 3 | 3 | 5m | LOW | Match arm addition |
| 4 | 20 | 15m | MEDIUM | Date parsing, validation |
| 5 | 800 | 6h | HIGH | Core TUI logic, state machine, event loop |
| 6 | 400 | 4h | HIGH | 5 screen layouts with ratatui widgets |
| 7 | 300 | 3h | HIGH | Backtest execution, caching, progress |
| 8 | 20 | 30m | MEDIUM | API integration, JSON parsing |
| 9 | 1 | 2m | LOW | Module declaration |
| 10 | 5 | 10m | LOW | Directory creation |

**Total**: ~1564 LOC, ~14 hours

### Verification Criteria

**Per-Task Verification**:
- [ ] Task 1: `cargo tree -p algo-trade-cli | grep ratatui` shows dependency
- [ ] Task 2: `cargo check -p algo-trade-cli` succeeds
- [ ] Task 3: `cargo check -p algo-trade-cli` succeeds
- [ ] Task 4: `cargo build -p algo-trade-cli` succeeds
- [ ] Task 5: `cargo check -p algo-trade-cli` succeeds (no errors in tui_backtest.rs)
- [ ] Task 6: `cargo check -p algo-trade-cli` succeeds (screens.rs compiles)
- [ ] Task 7: `cargo check -p algo-trade-cli` succeeds (runner.rs compiles)
- [ ] Task 8: `cargo test -p algo-trade-hyperliquid fetch_symbols` passes
- [ ] Task 9: `cargo check -p algo-trade-cli` succeeds
- [ ] Task 10: TUI run creates `cache/` directory

**Integration Verification**:
- [ ] `cargo run -p algo-trade-cli -- tui-backtest` launches TUI
- [ ] Strategy selection screen displays with keyboard navigation
- [ ] Token selection fetches symbols from Hyperliquid API
- [ ] Parameter config allows adding/editing configs
- [ ] Backtest runs and shows progress
- [ ] Results table displays and sorts correctly
- [ ] CSV export creates valid file
- [ ] Existing commands still work: `cargo run -p algo-trade-cli -- backtest --data tests/data/sample.csv --strategy quad_ma`

**Karen Quality Gates**:
- [ ] Phase 0: `cargo build --package algo-trade-cli --lib` succeeds
- [ ] Phase 1: Zero clippy warnings (default + pedantic + nursery)
- [ ] Phase 2: Zero rust-analyzer diagnostics
- [ ] Phase 3: No broken cross-file references
- [ ] Phase 6: `cargo build --release -p algo-trade-cli` succeeds

---

## Appendices

### Appendix A: Commands Executed

**Codebase Reconnaissance**:
```bash
Read: /home/a/Work/algo-trade/crates/cli/src/main.rs
Read: /home/a/Work/algo-trade/crates/backtest/src/lib.rs
Read: /home/a/Work/algo-trade/crates/backtest/src/metrics.rs
Read: /home/a/Work/algo-trade/crates/backtest/src/data_provider.rs
Read: /home/a/Work/algo-trade/crates/core/src/traits.rs
Read: /home/a/Work/algo-trade/crates/core/src/engine.rs
Read: /home/a/Work/algo-trade/crates/strategy/src/ma_crossover.rs
Read: /home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs
Read: /home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs
Read: /home/a/Work/algo-trade/crates/core/src/config.rs
Read: /home/a/Work/algo-trade/config/Config.toml

Grep: "struct PerformanceMetrics" (found in backtest/src/metrics.rs)
Grep: "impl.*Strategy.*for" (found strategies)
Grep: "trait Strategy" (found in core/src/traits.rs)
Grep: "pub struct.*Config" (found config files)

Glob: **/strategy*.rs (no matches - strategies in crates/strategy/src/*.rs)
Glob: **/Config.toml (found config/Config.toml)

Bash: ls -la /home/a/Work/algo-trade/crates/ (verified structure)
Bash: ls -la /home/a/Work/algo-trade/.claude/context/ (checked existing reports)
```

**External Research**:
```bash
WebSearch: "ratatui rust TUI multi-step wizard form example 2025"
WebSearch: "Hyperliquid API list all tradeable symbols coins endpoint"
WebSearch: "rust ratatui table pagination sorting interactive example"
WebSearch: "tokio spawn parallel backtest execution progress tracking"
WebSearch: "rust parameter sweep grid search backtest optimization pattern"
WebSearch: "rust indicatif progress bar async tokio multi progress"
WebSearch: "ratatui crossterm event loop keyboard navigation multi screen tabs example"
WebSearch: "rust tui form input validation multiple fields widget"
WebSearch: "rust cartesian product combinations multiple vectors iterator"
WebSearch: "rust serde serialize deserialize struct with default values toml"

WebFetch: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint
WebFetch: https://docs.rs/ratatui/latest/ratatui/
```

### Appendix B: Files Examined

| File | Lines Read | Purpose |
|------|-----------|---------|
| `/home/a/Work/algo-trade/crates/cli/src/main.rs` | 1-234 | Current CLI structure, backtest function |
| `/home/a/Work/algo-trade/crates/core/src/engine.rs` | 1-296 | TradingSystem, PerformanceMetrics |
| `/home/a/Work/algo-trade/crates/strategy/src/quad_ma.rs` | 1-151 | QuadMA strategy, `with_periods()` constructor |
| `/home/a/Work/algo-trade/crates/strategy/src/ma_crossover.rs` | 1-95 | MACrossover strategy, constructor |
| `/home/a/Work/algo-trade/crates/backtest/src/data_provider.rs` | 1-76 | HistoricalDataProvider, CSV loading |
| `/home/a/Work/algo-trade/crates/exchange-hyperliquid/src/client.rs` | 1-217 | HyperliquidClient, fetch_candles |
| `/home/a/Work/algo-trade/crates/core/src/traits.rs` | 1-25 | Strategy trait definition |
| `/home/a/Work/algo-trade/crates/core/src/config.rs` | 1-46 | AppConfig structure |
| `/home/a/Work/algo-trade/crates/backtest/src/metrics.rs` | 1-118 | PerformanceMetrics struct, MetricsCalculator |

### Appendix C: External References

1. **Ratatui Official Docs**: https://docs.rs/ratatui/latest/ratatui/
   - Summary: Core TUI framework, widgets (Table, List, Gauge), event loop patterns

2. **Ratatui GitHub Examples**: https://github.com/ratatui/ratatui/tree/main/examples
   - Summary: table.rs example shows interactive table with TableState

3. **Hyperliquid API Info Endpoint**: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint
   - Summary: `{"type": "meta"}` returns universe of tradeable symbols

4. **Crossterm Event Handling**: https://ratatui.rs/concepts/event-handling/
   - Summary: `event::read()` with `KeyCode` matching for keyboard navigation

5. **tui-input Crate**: https://crates.io/crates/tui-input
   - Summary: Single-line text input widget for ratatui

6. **itertools Cartesian Product**: https://docs.rs/itertools/latest/itertools/trait.Itertools.html#method.cartesian_product
   - Summary: `iproduct!()` macro for generating parameter combinations

7. **Tokio Spawning**: https://tokio.rs/tokio/tutorial/spawning
   - Summary: `tokio::spawn()` for async task execution, JoinHandle for results

8. **Serde Default Values**: https://serde.rs/attr-default.html
   - Summary: `#[serde(default)]` for optional fields with defaults

---

## Context Gatherer Sign-off

**Report Status**: ✅ Complete (All 7 phases executed)

**Ready for TaskMaster**: ✅ Yes
- Total tasks identified: 10 atomic tasks
- Estimated lines of code: ~1564 LOC
- Estimated time: ~14 hours
- Complexity: HIGH (new crate, TUI framework, multi-screen state machine)
- Critical decisions documented: 7 architectural choices
- Edge cases identified: 10 scenarios
- Constraints defined: 8 boundaries

**Key Findings**:
1. Hyperliquid provides `/info` endpoint with `{"type": "meta"}` for symbol list
2. Ratatui 0.28 is best-in-class TUI framework with excellent examples
3. Sequential execution recommended for MVP (simpler than parallel)
4. Data caching is essential (50 symbols = 50 API calls, reusable across configs)
5. Parameter validation critical (MA1 < MA2 < MA3 < MA4)
6. Backward compatibility preserved (new command, no existing code changes)

**Handoff to TaskMaster**:
Section 6 (TaskMaster Handoff Package) contains complete specification with:
- Exact file paths and line numbers
- MUST DO / MUST NOT DO boundaries
- Task dependencies mapped
- Verification criteria per task
- Estimated complexity and LOC

**Next Step**: Invoke TaskMaster to generate atomic playbook from Section 6.

---

**Date Generated**: 2025-10-03
**Context Gatherer**: Claude (Sonnet 4.5)
