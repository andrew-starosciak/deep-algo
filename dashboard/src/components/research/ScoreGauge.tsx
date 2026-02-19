"use client";

interface Props {
  score: number;
  label?: string;
}

export default function ScoreGauge({ score, label }: Props) {
  const color =
    score >= 7
      ? "text-profit"
      : score >= 4
        ? "text-accent"
        : "text-loss";

  const bgColor =
    score >= 7
      ? "bg-profit"
      : score >= 4
        ? "bg-accent"
        : "bg-loss";

  return (
    <div className="flex items-center gap-2">
      {label && <span className="text-xs text-text-secondary">{label}</span>}
      <div className="w-16 h-1.5 rounded-full bg-bg-hover">
        <div
          className={`h-full rounded-full ${bgColor}`}
          style={{ width: `${(score / 10) * 100}%` }}
        />
      </div>
      <span className={`text-sm font-medium ${color}`}>{score.toFixed(1)}</span>
    </div>
  );
}
