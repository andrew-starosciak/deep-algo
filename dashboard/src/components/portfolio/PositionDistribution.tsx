"use client";

import Card from "@/components/ui/Card";

interface Props {
  calls: number;
  puts: number;
}

export default function PositionDistribution({ calls, puts }: Props) {
  const total = calls + puts;
  const callPct = total > 0 ? (calls / total) * 100 : 50;

  return (
    <Card>
      <div className="text-xs text-text-secondary mb-2">Direction Bias</div>
      <div className="flex items-center gap-2 text-sm mb-2">
        <span className="text-profit">{calls} Calls</span>
        <span className="text-text-secondary">|</span>
        <span className="text-loss">{puts} Puts</span>
      </div>
      <div className="w-full h-2 rounded-full bg-bg-hover flex overflow-hidden">
        <div
          className="bg-profit h-full rounded-l-full"
          style={{ width: `${callPct}%` }}
        />
        <div
          className="bg-loss h-full rounded-r-full"
          style={{ width: `${100 - callPct}%` }}
        />
      </div>
    </Card>
  );
}
