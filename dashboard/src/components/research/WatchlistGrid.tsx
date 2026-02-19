"use client";

import Link from "next/link";
import Card from "@/components/ui/Card";
import Badge from "@/components/ui/Badge";
import ScoreGauge from "./ScoreGauge";
import { useWatchlist, useTheses } from "@/lib/hooks";
import { formatDate } from "@/lib/utils";
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

  // Split into tickers with research vs without
  const withResearch = wlData.watchlist.filter((w) =>
    latestThesis.has(w.ticker)
  );
  const withoutResearch = wlData.watchlist.filter(
    (w) => !latestThesis.has(w.ticker)
  );

  return (
    <div className="flex flex-col gap-6">
      {/* Active research — full cards */}
      {withResearch.length > 0 && (
        <div className="flex flex-col gap-4">
          {withResearch.map((item) => {
            const thesis = latestThesis.get(item.ticker)!;
            const scores =
              typeof thesis.scores === "string"
                ? JSON.parse(thesis.scores)
                : thesis.scores;
            const catalyst = (
              typeof thesis.catalyst === "string"
                ? JSON.parse(thesis.catalyst)
                : thesis.catalyst
            ) as Record<string, string | number> | null;
            const risks = Array.isArray(thesis.risks)
              ? thesis.risks
              : typeof thesis.risks === "string"
                ? JSON.parse(thesis.risks)
                : [];
            const evidence = Array.isArray(thesis.supporting_evidence)
              ? thesis.supporting_evidence
              : typeof thesis.supporting_evidence === "string"
                ? JSON.parse(thesis.supporting_evidence)
                : [];

            const dirVariant =
              thesis.direction === "bullish"
                ? ("profit" as const)
                : thesis.direction === "bearish"
                  ? ("loss" as const)
                  : ("neutral" as const);

            return (
              <Link
                key={item.ticker}
                href={`/research?ticker=${item.ticker}`}
              >
                <Card className="hover:bg-bg-hover cursor-pointer transition-colors">
                  {/* Header */}
                  <div className="flex items-center justify-between mb-3">
                    <div className="flex items-center gap-3">
                      <span className="text-xl font-bold">{item.ticker}</span>
                      <Badge label={thesis.direction} variant={dirVariant} />
                      <Badge label={item.sector} variant="accent" />
                    </div>
                    <div className="text-xs text-text-secondary">
                      {formatDate(thesis.created_at)}
                    </div>
                  </div>

                  {/* Scores row */}
                  <div className="flex flex-wrap gap-4 mb-3">
                    <ScoreGauge
                      score={thesis.overall_score}
                      label="Overall"
                    />
                    {scores &&
                      typeof scores === "object" &&
                      Object.entries(scores)
                        .filter(([k]) => k !== "overall")
                        .map(([k, v]) => (
                          <ScoreGauge
                            key={k}
                            score={v as number}
                            label={k.replace(/_/g, " ")}
                          />
                        ))}
                  </div>

                  {/* Thesis text */}
                  <p className="text-sm text-text-secondary mb-3 line-clamp-3">
                    {thesis.thesis_text}
                  </p>

                  {/* Catalyst */}
                  {catalyst && typeof catalyst === "object" && (
                    <div className="bg-bg-primary rounded px-3 py-2 mb-3 border border-border/50">
                      <div className="flex items-center gap-2 text-xs mb-1">
                        <span className="text-accent font-medium uppercase">
                          {catalyst.type as string}
                        </span>
                        <span className="text-text-secondary">
                          {formatDate(
                            catalyst.date as string
                          )}
                        </span>
                        {catalyst.days_until && (
                          <span className="text-text-secondary">
                            ({catalyst.days_until as number}d away)
                          </span>
                        )}
                      </div>
                      <p className="text-xs text-text-secondary line-clamp-2">
                        {catalyst.description as string}
                      </p>
                    </div>
                  )}

                  {/* Key evidence + risks side by side */}
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                    {evidence.length > 0 && (
                      <div>
                        <div className="text-xs font-medium text-profit mb-1">
                          Bull Case
                        </div>
                        <ul className="space-y-1">
                          {(evidence as string[]).slice(0, 3).map((e, i) => (
                            <li
                              key={i}
                              className="text-xs text-text-secondary line-clamp-2 pl-2 border-l border-profit/30"
                            >
                              {e}
                            </li>
                          ))}
                        </ul>
                      </div>
                    )}
                    {risks.length > 0 && (
                      <div>
                        <div className="text-xs font-medium text-loss mb-1">
                          Risks
                        </div>
                        <ul className="space-y-1">
                          {(risks as string[]).slice(0, 3).map((r, i) => (
                            <li
                              key={i}
                              className="text-xs text-text-secondary line-clamp-2 pl-2 border-l border-loss/30"
                            >
                              {r}
                            </li>
                          ))}
                        </ul>
                      </div>
                    )}
                  </div>
                </Card>
              </Link>
            );
          })}
        </div>
      )}

      {/* Tickers without research — compact row */}
      {withoutResearch.length > 0 && (
        <div>
          <h2 className="text-sm font-medium text-text-secondary mb-2">
            Watchlist — Awaiting Research
          </h2>
          <div className="flex flex-wrap gap-2">
            {withoutResearch.map((item) => (
              <div
                key={item.ticker}
                className="flex items-center gap-2 bg-bg-card border border-border rounded px-3 py-2"
              >
                <span className="text-sm font-medium">{item.ticker}</span>
                <Badge label={item.sector} variant="accent" />
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
