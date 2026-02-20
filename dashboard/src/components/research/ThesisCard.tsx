"use client";

import Card from "@/components/ui/Card";
import Badge from "@/components/ui/Badge";
import ScoreGauge from "./ScoreGauge";
import { formatDateTime, formatDate } from "@/lib/utils";
import type { Thesis } from "@/lib/types";

interface Props {
  thesis: Thesis;
  expanded?: boolean;
}

export default function ThesisCard({ thesis, expanded = false }: Props) {
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

  return (
    <Card>
      {/* Header */}
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-2">
          <span className="font-bold text-lg">{thesis.ticker}</span>
          <Badge label={thesis.direction} variant={dirVariant} />
        </div>
        <span className="text-xs text-text-secondary">
          {formatDateTime(thesis.created_at)}
        </span>
      </div>

      {/* Scores */}
      <div className="flex flex-wrap gap-4 mb-3">
        <ScoreGauge score={thesis.overall_score} label="Overall" />
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
      <p
        className={`text-sm text-text-secondary mb-3 ${expanded ? "" : "line-clamp-4"}`}
      >
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
                ({catalyst.days_until as number}d
                away)
              </span>
            )}
          </div>
          <p className="text-xs text-text-secondary">
            {catalyst.description as string}
          </p>
        </div>
      )}

      {/* Evidence + Risks */}
      {expanded && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
          {(evidence as string[]).length > 0 && (
            <div>
              <div className="text-xs font-medium text-profit mb-1">
                Bull Case
              </div>
              <ul className="space-y-1">
                {(evidence as string[]).map((e, i) => (
                  <li
                    key={i}
                    className="text-xs text-text-secondary pl-2 border-l border-profit/30"
                  >
                    {e}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {(risks as string[]).length > 0 && (
            <div>
              <div className="text-xs font-medium text-loss mb-1">Risks</div>
              <ul className="space-y-1">
                {(risks as string[]).map((r, i) => (
                  <li
                    key={i}
                    className="text-xs text-text-secondary pl-2 border-l border-loss/30"
                  >
                    {r}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {/* Analyst & Critic Reasoning */}
      {expanded && (thesis.analyst_reasoning || thesis.critic_reasoning) && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mt-3 pt-3 border-t border-border/50">
          {thesis.analyst_reasoning && (
            <div>
              <div className="text-xs font-medium text-accent mb-1">
                Analyst Reasoning
              </div>
              <p className="text-xs text-text-secondary">
                {thesis.analyst_reasoning}
              </p>
            </div>
          )}
          {thesis.critic_reasoning && (
            <div>
              <div className="text-xs font-medium text-yellow-500 mb-1">
                Critic Counter-Case
              </div>
              <p className="text-xs text-text-secondary">
                {thesis.critic_reasoning}
              </p>
            </div>
          )}
        </div>
      )}
    </Card>
  );
}
