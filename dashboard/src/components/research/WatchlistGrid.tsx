"use client";

import Link from "next/link";
import Card from "@/components/ui/Card";
import Badge from "@/components/ui/Badge";
import { useWatchlist, useTheses } from "@/lib/hooks";
import type { Thesis } from "@/lib/types";

export default function WatchlistGrid() {
  const { data: wlData } = useWatchlist();
  const { data: thesesData } = useTheses();

  if (!wlData?.watchlist?.length) {
    return (
      <div className="text-center text-text-secondary py-12">
        No tickers on watchlist
      </div>
    );
  }

  // Build a map of ticker -> latest thesis
  const latestThesis = new Map<string, Thesis>();
  for (const t of thesesData?.theses || []) {
    if (!latestThesis.has(t.ticker)) {
      latestThesis.set(t.ticker, t);
    }
  }

  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      {wlData.watchlist.map((item) => {
        const thesis = latestThesis.get(item.ticker);
        const dirVariant =
          thesis?.direction === "bullish"
            ? ("profit" as const)
            : thesis?.direction === "bearish"
              ? ("loss" as const)
              : ("neutral" as const);

        return (
          <Link key={item.ticker} href={`/research?ticker=${item.ticker}`}>
            <Card className="hover:bg-bg-hover cursor-pointer transition-colors">
              <div className="flex items-center justify-between mb-2">
                <span className="text-lg font-bold">{item.ticker}</span>
                <Badge label={item.sector} variant="accent" />
              </div>
              {thesis ? (
                <>
                  <div className="flex items-center gap-2 mb-1">
                    <Badge label={thesis.direction} variant={dirVariant} />
                    <span className="text-sm text-text-secondary">
                      Score: {thesis.overall_score.toFixed(1)}
                    </span>
                  </div>
                  <p className="text-xs text-text-secondary line-clamp-2">
                    {thesis.thesis_text}
                  </p>
                </>
              ) : (
                <span className="text-xs text-text-secondary">
                  No thesis yet
                </span>
              )}
            </Card>
          </Link>
        );
      })}
    </div>
  );
}
