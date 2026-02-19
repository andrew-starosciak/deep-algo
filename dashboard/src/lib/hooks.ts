import useSWR, { SWRConfiguration } from "swr";
import { fetcher } from "./api";
import type {
  PortfolioOverview,
  PortfolioHistory,
  SystemStatus,
  Thesis,
  Recommendation,
  WatchlistItem,
  Position,
  WorkflowsResponse,
  ResearchMemoryStats,
} from "./types";

const REFRESH_MS = 30_000;

// Stop retrying after 3 attempts, back off exponentially
const defaults: SWRConfiguration = {
  errorRetryCount: 3,
  onErrorRetry: (error, _key, _config, revalidate, { retryCount }) => {
    if (error?.message === "Unauthorized") return;
    if (retryCount >= 3) return;
    setTimeout(() => revalidate({ retryCount }), 5000 * 2 ** retryCount);
  },
};

export function usePortfolio() {
  return useSWR<PortfolioOverview>("/api/portfolio", fetcher, {
    ...defaults,
    refreshInterval: REFRESH_MS,
  });
}

export function usePortfolioHistory(days = 30) {
  return useSWR<PortfolioHistory>(
    `/api/portfolio/history?days=${days}`,
    fetcher,
    { ...defaults, refreshInterval: 60_000 }
  );
}

export function usePositions(status = "open") {
  return useSWR<{ count: number; positions: Position[] }>(
    `/api/positions?status=${status}`,
    fetcher,
    { ...defaults, refreshInterval: REFRESH_MS }
  );
}

export function useTheses(ticker?: string) {
  const url = ticker ? `/api/theses?ticker=${ticker}` : "/api/theses";
  return useSWR<{ count: number; theses: Thesis[] }>(url, fetcher, {
    ...defaults,
    refreshInterval: 60_000,
  });
}

export function useTickerTheses(ticker: string) {
  return useSWR<{ ticker: string; count: number; theses: Thesis[] }>(
    `/api/theses/${ticker}`,
    fetcher,
    { ...defaults, refreshInterval: 60_000 }
  );
}

export function useRecommendations(status = "all") {
  return useSWR<{ count: number; recommendations: Recommendation[] }>(
    `/api/recommendations?status=${status}`,
    fetcher,
    { ...defaults, refreshInterval: REFRESH_MS }
  );
}

export function useWatchlist() {
  return useSWR<{ count: number; watchlist: WatchlistItem[] }>(
    "/api/watchlist",
    fetcher,
    { ...defaults, refreshInterval: 60_000 }
  );
}

export function useSystemStatus() {
  return useSWR<SystemStatus>("/api/status", fetcher, {
    ...defaults,
    refreshInterval: REFRESH_MS,
  });
}

export function useWorkflows() {
  return useSWR<WorkflowsResponse>("/api/workflows", fetcher, {
    ...defaults,
    refreshInterval: 60_000,
  });
}

export function useResearchMemory() {
  return useSWR<ResearchMemoryStats>("/api/research-memory", fetcher, {
    ...defaults,
    refreshInterval: 60_000,
  });
}
