"use client";

import Card from "@/components/ui/Card";
import Badge from "@/components/ui/Badge";
import ScoreGauge from "./ScoreGauge";
import { formatDateTime } from "@/lib/utils";
import type { Thesis } from "@/lib/types";

interface Props {
  thesis: Thesis;
}

export default function ThesisCard({ thesis }: Props) {
  const dirVariant =
    thesis.direction === "bullish"
      ? ("profit" as const)
      : thesis.direction === "bearish"
        ? ("loss" as const)
        : ("neutral" as const);

  const scores =
    typeof thesis.scores === "string"
      ? JSON.parse(thesis.scores)
      : thesis.scores;

  return (
    <Card>
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-2">
          <span className="font-medium">{thesis.ticker}</span>
          <Badge label={thesis.direction} variant={dirVariant} />
        </div>
        <span className="text-xs text-text-secondary">
          {formatDateTime(thesis.created_at)}
        </span>
      </div>

      <ScoreGauge score={thesis.overall_score} label="Overall" />

      {scores && typeof scores === "object" && (
        <div className="mt-2 grid grid-cols-2 gap-1">
          {Object.entries(scores)
            .filter(([k]) => k !== "overall")
            .map(([k, v]) => (
              <ScoreGauge key={k} score={v as number} label={k} />
            ))}
        </div>
      )}

      <p className="mt-3 text-sm text-text-secondary line-clamp-3">
        {thesis.thesis_text}
      </p>
    </Card>
  );
}
