export interface Position {
  id: number;
  recommendation_id: number | null;
  ticker: string;
  right: "call" | "put";
  strike: string;
  expiry: string;
  quantity: number;
  avg_fill_price: string;
  current_price: string;
  cost_basis: string;
  unrealized_pnl: string;
  realized_pnl: string;
  status: string;
  ib_con_id: number | null;
  opened_at: string;
  closed_at: string | null;
  close_reason: string | null;
}

export interface PortfolioOverview {
  open_positions: number;
  closed_trades: number;
  total_unrealized_pnl: string;
  total_realized_pnl: string;
  total_options_exposure: string;
  total_cost_basis: string;
  calls_count: number;
  puts_count: number;
  positions: Position[];
}

export interface EquitySnapshot {
  timestamp: string;
  net_liquidation: string;
  total_unrealized_pnl: string;
  total_realized_pnl: string;
  open_positions_count: number;
  total_options_exposure: string;
}

export interface PortfolioHistory {
  days: number;
  snapshots: EquitySnapshot[];
}

export interface Thesis {
  id: number;
  run_id: number;
  ticker: string;
  direction: string;
  thesis_text: string;
  catalyst: unknown;
  scores: Record<string, number> | string;
  supporting_evidence: unknown;
  risks: unknown;
  overall_score: number;
  created_at: string;
}

export interface Recommendation {
  id: number;
  thesis_id: number;
  run_id: number;
  ticker: string;
  right: string;
  strike: number;
  expiry: string;
  entry_price_low: number;
  entry_price_high: number;
  position_size_pct: number;
  position_size_usd: number;
  exit_targets: string[] | string;
  stop_loss: string;
  max_hold_days: number;
  status: string;
  created_at: string;
  approved_at: string | null;
}

export interface WatchlistItem {
  ticker: string;
  sector: string;
  notes: string | null;
  added_at: string;
}

export interface WorkflowRun {
  id: number;
  workflow_id: string;
  trigger: string;
  status: string;
  started_at: string;
  completed_at: string | null;
}

export interface SystemStatus {
  db_connected: boolean;
  recent_workflows: WorkflowRun[];
}

export interface WorkflowStep {
  step_id: string;
  agent: string;
  passed_gate: boolean;
  duration_ms: number;
  attempt: number;
}

export interface WorkflowRunWithSteps extends WorkflowRun {
  duration_ms: number | null;
  steps: WorkflowStep[];
}

export interface WorkflowStats {
  total_runs: number;
  completed: number;
  failed: number;
  avg_duration_ms: number;
  runs_today: number;
}

export interface WorkflowsResponse {
  runs: WorkflowRunWithSteps[];
  stats: WorkflowStats;
  last_equity_tick: string | null;
}

export interface ResearchMemoryStats {
  total_research: number;
  total_theses: number;
  theses_with_outcome: number;
  winning_theses: number;
  losing_theses: number;
  total_outcome_pnl: string;
  tickers_analyzed: number;
  total_recommendations: number;
  approved_recommendations: number;
  filled_recommendations: number;
}
