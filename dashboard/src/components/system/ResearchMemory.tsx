"use client";

import Card from "@/components/ui/Card";
import type { ResearchMemoryStats } from "@/lib/types";

interface Props {
  stats: ResearchMemoryStats;
}

export default function ResearchMemory({ stats }: Props) {
  const winRate =
    stats.theses_with_outcome > 0
      ? Math.round(
          (stats.winning_theses / stats.theses_with_outcome) * 100
        )
      : null;

  const pnl = parseFloat(stats.total_outcome_pnl);

  return (
    <Card>
      <h2 className="text-sm font-semibold text-text-primary mb-4">
        Research Memory & Feedback Loop
      </h2>

      {/* Feedback loop visualization */}
      <div className="flex items-center gap-0 overflow-x-auto pb-3 mb-4">
        {[
          { label: "Research", value: stats.total_research, sub: "summaries" },
          { label: "Theses", value: stats.total_theses, sub: `${stats.tickers_analyzed} tickers` },
          { label: "Recommendations", value: stats.total_recommendations, sub: `${stats.filled_recommendations} filled` },
          { label: "Outcomes", value: stats.theses_with_outcome, sub: winRate !== null ? `${winRate}% win` : "none yet" },
        ].map((step, i, arr) => (
          <div key={step.label} className="flex items-center shrink-0">
            <div className="flex flex-col items-center">
              <div className="bg-bg-hover border border-border rounded-lg px-4 py-2 text-center min-w-[110px]">
                <div className="text-lg font-semibold text-text-primary">
                  {step.value}
                </div>
                <div className="text-xs font-medium text-text-primary">
                  {step.label}
                </div>
                <div className="text-[10px] text-text-secondary mt-0.5">
                  {step.sub}
                </div>
              </div>
            </div>
            {i < arr.length - 1 && (
              <div className="text-text-secondary text-xs px-1.5 shrink-0">
                &rarr;
              </div>
            )}
          </div>
        ))}

        {/* Feedback arrow */}
        <div className="text-accent text-xs px-2 shrink-0 flex items-center gap-1">
          <span className="text-accent/60">&larr;</span>
          <span className="text-[10px] text-accent whitespace-nowrap">
            feeds context
          </span>
        </div>
      </div>

      {/* Stats row */}
      <div className="grid grid-cols-3 gap-3">
        <div className="bg-bg-hover/50 rounded-lg p-3 text-center border border-border">
          <div className="text-[10px] text-text-secondary mb-1">
            Outcome P&L
          </div>
          <div
            className={`text-sm font-semibold ${
              pnl > 0
                ? "text-profit"
                : pnl < 0
                  ? "text-loss"
                  : "text-text-secondary"
            }`}
          >
            {pnl !== 0 ? `$${pnl.toFixed(2)}` : "--"}
          </div>
        </div>
        <div className="bg-bg-hover/50 rounded-lg p-3 text-center border border-border">
          <div className="text-[10px] text-text-secondary mb-1">
            Win Rate
          </div>
          <div
            className={`text-sm font-semibold ${
              winRate !== null && winRate >= 50
                ? "text-profit"
                : winRate !== null
                  ? "text-loss"
                  : "text-text-secondary"
            }`}
          >
            {winRate !== null ? `${winRate}%` : "--"}
          </div>
          <div className="text-[10px] text-text-secondary mt-0.5">
            {stats.winning_theses}W / {stats.losing_theses}L
          </div>
        </div>
        <div className="bg-bg-hover/50 rounded-lg p-3 text-center border border-border">
          <div className="text-[10px] text-text-secondary mb-1">
            Fill Rate
          </div>
          <div className="text-sm font-semibold text-text-primary">
            {stats.total_recommendations > 0
              ? `${Math.round((stats.filled_recommendations / stats.total_recommendations) * 100)}%`
              : "--"}
          </div>
          <div className="text-[10px] text-text-secondary mt-0.5">
            {stats.filled_recommendations} / {stats.total_recommendations} recs
          </div>
        </div>
      </div>
    </Card>
  );
}
