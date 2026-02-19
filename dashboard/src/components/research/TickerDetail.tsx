"use client";

import Link from "next/link";
import { useTickerTheses, useRecommendations } from "@/lib/hooks";
import ThesisCard from "./ThesisCard";
import Card from "@/components/ui/Card";
import Badge from "@/components/ui/Badge";
import { formatCurrency, formatDate } from "@/lib/utils";

interface Props {
  ticker: string;
}

export default function TickerDetail({ ticker }: Props) {
  const { data: thesesData, isLoading } = useTickerTheses(ticker);
  const { data: recsData } = useRecommendations();

  const tickerRecs =
    recsData?.recommendations.filter((r) => r.ticker === ticker) || [];

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64 text-text-secondary">
        Loading {ticker}...
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <Link
          href="/research"
          className="text-text-secondary hover:text-text-primary text-sm"
        >
          Research
        </Link>
        <span className="text-text-secondary">/</span>
        <h1 className="text-xl font-bold">{ticker}</h1>
      </div>

      {tickerRecs.length > 0 && (
        <div>
          <h2 className="text-sm font-medium text-text-secondary mb-2">
            Recommendations
          </h2>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            {tickerRecs.map((r) => {
              const statusVariant =
                r.status === "executed" || r.status === "approved"
                  ? ("profit" as const)
                  : r.status === "rejected" || r.status === "failed"
                    ? ("loss" as const)
                    : ("accent" as const);

              return (
                <Card key={r.id}>
                  <div className="flex items-center justify-between mb-1">
                    <div className="flex items-center gap-2">
                      <Badge
                        label={r.right.toUpperCase()}
                        variant={r.right === "call" ? "profit" : "loss"}
                      />
                      <span className="text-sm">
                        {formatCurrency(r.strike)} {formatDate(r.expiry)}
                      </span>
                    </div>
                    <Badge label={r.status} variant={statusVariant} />
                  </div>
                  <div className="text-xs text-text-secondary">
                    Size: {formatCurrency(r.position_size_usd)} | Created:{" "}
                    {formatDate(r.created_at)}
                  </div>
                </Card>
              );
            })}
          </div>
        </div>
      )}

      <div>
        <h2 className="text-sm font-medium text-text-secondary mb-2">
          Thesis History ({thesesData?.count || 0})
        </h2>
        {thesesData?.theses?.length ? (
          <div className="flex flex-col gap-3">
            {thesesData.theses.map((t, i) => (
              <ThesisCard key={t.id} thesis={t} expanded={i === 0} />
            ))}
          </div>
        ) : (
          <div className="text-text-secondary text-sm">
            No theses for {ticker}
          </div>
        )}
      </div>
    </div>
  );
}
