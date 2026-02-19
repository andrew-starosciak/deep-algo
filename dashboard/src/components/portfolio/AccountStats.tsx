"use client";

import Card from "@/components/ui/Card";
import { formatCurrency, formatPnL, getPnLColor } from "@/lib/utils";
import type { PortfolioOverview } from "@/lib/types";

interface Props {
  data: PortfolioOverview;
}

export default function AccountStats({ data }: Props) {
  const costBasis = parseFloat(data.total_cost_basis);
  const unrealized = parseFloat(data.total_unrealized_pnl);
  const roe = costBasis > 0 ? (unrealized / costBasis) * 100 : 0;

  const stats = [
    { label: "Options Exposure", value: formatCurrency(data.total_options_exposure) },
    { label: "Cost Basis", value: formatCurrency(data.total_cost_basis) },
    {
      label: "Unrealized P&L",
      value: formatPnL(data.total_unrealized_pnl),
      color: getPnLColor(data.total_unrealized_pnl),
      sub: `(${roe >= 0 ? "+" : ""}${roe.toFixed(1)}%)`,
    },
    {
      label: "Realized P&L",
      value: formatPnL(data.total_realized_pnl),
      color: getPnLColor(data.total_realized_pnl),
    },
    { label: "Open Positions", value: String(data.open_positions) },
    { label: "Closed Trades", value: String(data.closed_trades) },
  ];

  return (
    <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
      {stats.map((s) => (
        <Card key={s.label} className="text-center">
          <div className="text-xs text-text-secondary mb-1">{s.label}</div>
          <div className={`text-lg font-semibold ${s.color || "text-text-primary"}`}>
            {s.value}
            {s.sub && (
              <span className="text-xs ml-1 text-text-secondary">{s.sub}</span>
            )}
          </div>
        </Card>
      ))}
    </div>
  );
}
