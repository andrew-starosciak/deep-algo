"use client";

import { useState } from "react";
import Card from "@/components/ui/Card";
import Badge from "@/components/ui/Badge";
import { usePositions, useRecommendations } from "@/lib/hooks";
import {
  formatCurrency,
  formatPnL,
  getPnLColor,
  formatDate,
  daysToExpiry,
} from "@/lib/utils";

const TABS = ["Open Positions", "Closed Trades", "Recommendations"] as const;

export default function PositionsTable() {
  const [tab, setTab] = useState<(typeof TABS)[number]>("Open Positions");
  const { data: openData } = usePositions("open");
  const { data: closedData } = usePositions("closed");
  const { data: recsData } = useRecommendations();

  return (
    <Card className="col-span-full">
      <div className="flex gap-4 border-b border-border mb-3 pb-2">
        {TABS.map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`text-sm pb-1 ${
              tab === t
                ? "text-accent border-b-2 border-accent"
                : "text-text-secondary hover:text-text-primary"
            }`}
          >
            {t}
          </button>
        ))}
      </div>

      {tab === "Open Positions" && (
        <PositionRows positions={openData?.positions || []} />
      )}
      {tab === "Closed Trades" && (
        <PositionRows positions={closedData?.positions || []} closed />
      )}
      {tab === "Recommendations" && (
        <RecommendationRows recommendations={recsData?.recommendations || []} />
      )}
    </Card>
  );
}

function PositionRows({
  positions,
  closed,
}: {
  positions: ReturnType<typeof usePositions>["data"] extends
    | { positions: infer P }
    | undefined
    ? P extends (infer I)[]
      ? I[]
      : never
    : never;
  closed?: boolean;
}) {
  if (!positions.length) {
    return (
      <div className="text-center text-text-secondary py-6 text-sm">
        No {closed ? "closed" : "open"} positions
      </div>
    );
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-text-secondary text-xs">
            <th className="text-left py-2 px-2">Ticker</th>
            <th className="text-left py-2 px-2">Type</th>
            <th className="text-right py-2 px-2">Strike</th>
            <th className="text-left py-2 px-2">Expiry</th>
            <th className="text-right py-2 px-2">Qty</th>
            <th className="text-right py-2 px-2">Cost</th>
            <th className="text-right py-2 px-2">Current</th>
            <th className="text-right py-2 px-2">P&L</th>
            <th className="text-right py-2 px-2">P&L %</th>
            {!closed && <th className="text-right py-2 px-2">DTE</th>}
          </tr>
        </thead>
        <tbody>
          {positions.map((p) => {
            const cost = parseFloat(p.cost_basis);
            const pnl = closed
              ? parseFloat(p.realized_pnl)
              : parseFloat(p.unrealized_pnl);
            const pnlPct = cost > 0 ? (pnl / cost) * 100 : 0;
            const dte = daysToExpiry(p.expiry);

            return (
              <tr
                key={p.id}
                className="border-t border-border/50 hover:bg-bg-hover"
              >
                <td className="py-2 px-2 font-medium">{p.ticker}</td>
                <td className="py-2 px-2">
                  <Badge
                    label={p.right.toUpperCase()}
                    variant={p.right === "call" ? "profit" : "loss"}
                  />
                </td>
                <td className="py-2 px-2 text-right">{formatCurrency(p.strike)}</td>
                <td className="py-2 px-2">{formatDate(p.expiry)}</td>
                <td className="py-2 px-2 text-right">{p.quantity}</td>
                <td className="py-2 px-2 text-right">
                  {formatCurrency(p.cost_basis)}
                </td>
                <td className="py-2 px-2 text-right">
                  {formatCurrency(p.current_price)}
                </td>
                <td className={`py-2 px-2 text-right ${getPnLColor(pnl)}`}>
                  {formatPnL(pnl)}
                </td>
                <td className={`py-2 px-2 text-right ${getPnLColor(pnlPct)}`}>
                  {pnlPct >= 0 ? "+" : ""}
                  {pnlPct.toFixed(1)}%
                </td>
                {!closed && (
                  <td
                    className={`py-2 px-2 text-right ${
                      dte <= 7 ? "text-loss" : "text-text-secondary"
                    }`}
                  >
                    {dte}d
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function RecommendationRows({
  recommendations,
}: {
  recommendations: ReturnType<typeof useRecommendations>["data"] extends
    | { recommendations: infer R }
    | undefined
    ? R extends (infer I)[]
      ? I[]
      : never
    : never;
}) {
  if (!recommendations.length) {
    return (
      <div className="text-center text-text-secondary py-6 text-sm">
        No recommendations
      </div>
    );
  }

  const statusVariant = (s: string) => {
    if (s === "executed" || s === "approved") return "profit" as const;
    if (s === "rejected" || s === "failed") return "loss" as const;
    if (s === "pending_review") return "accent" as const;
    return "neutral" as const;
  };

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-text-secondary text-xs">
            <th className="text-left py-2 px-2">Ticker</th>
            <th className="text-left py-2 px-2">Type</th>
            <th className="text-right py-2 px-2">Strike</th>
            <th className="text-left py-2 px-2">Expiry</th>
            <th className="text-right py-2 px-2">Size (USD)</th>
            <th className="text-left py-2 px-2">Status</th>
            <th className="text-left py-2 px-2">Created</th>
          </tr>
        </thead>
        <tbody>
          {recommendations.map((r) => (
            <tr
              key={r.id}
              className="border-t border-border/50 hover:bg-bg-hover"
            >
              <td className="py-2 px-2 font-medium">{r.ticker}</td>
              <td className="py-2 px-2">
                <Badge
                  label={r.right.toUpperCase()}
                  variant={r.right === "call" ? "profit" : "loss"}
                />
              </td>
              <td className="py-2 px-2 text-right">{formatCurrency(r.strike)}</td>
              <td className="py-2 px-2">{formatDate(r.expiry)}</td>
              <td className="py-2 px-2 text-right">
                {formatCurrency(r.position_size_usd)}
              </td>
              <td className="py-2 px-2">
                <Badge label={r.status} variant={statusVariant(r.status)} />
              </td>
              <td className="py-2 px-2 text-text-secondary">
                {formatDate(r.created_at)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
