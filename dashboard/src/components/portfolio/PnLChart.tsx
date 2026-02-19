"use client";

import { useEffect, useRef, useState } from "react";
import { usePortfolioHistory } from "@/lib/hooks";
import Card from "@/components/ui/Card";

const TIMEFRAMES = [
  { label: "24H", days: 1 },
  { label: "1W", days: 7 },
  { label: "1M", days: 30 },
  { label: "All", days: 365 },
];

export default function PnLChart() {
  const [days, setDays] = useState(30);
  const { data } = usePortfolioHistory(days);
  const chartRef = useRef<HTMLDivElement>(null);
  const chartInstance = useRef<unknown>(null);

  useEffect(() => {
    if (!chartRef.current || !data?.snapshots?.length) return;

    let cancelled = false;

    import("lightweight-charts").then(({ createChart, ColorType }) => {
      if (cancelled || !chartRef.current) return;

      // Clear previous chart
      if (chartInstance.current) {
        (chartInstance.current as { remove: () => void }).remove();
      }

      const chart = createChart(chartRef.current, {
        width: chartRef.current.clientWidth,
        height: 300,
        layout: {
          background: { type: ColorType.Solid, color: "#141824" },
          textColor: "#8b95a5",
        },
        grid: {
          vertLines: { color: "#1c2130" },
          horzLines: { color: "#1c2130" },
        },
        rightPriceScale: { borderColor: "#2a3142" },
        timeScale: { borderColor: "#2a3142" },
      });

      const series = chart.addAreaSeries({
        lineColor: "#448aff",
        topColor: "rgba(68, 138, 255, 0.3)",
        bottomColor: "rgba(68, 138, 255, 0.02)",
        lineWidth: 2,
      });

      const points = data.snapshots.map((s) => ({
        time: s.timestamp.slice(0, 10) as string,
        value: parseFloat(s.net_liquidation),
      }));

      // Deduplicate by date (keep last value per day)
      const byDate = new Map<string, number>();
      for (const p of points) {
        byDate.set(p.time, p.value);
      }
      const deduped = Array.from(byDate.entries())
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([time, value]) => ({ time, value }));

      if (deduped.length > 0) {
        series.setData(deduped as Parameters<typeof series.setData>[0]);
      }

      chart.timeScale().fitContent();
      chartInstance.current = chart;

      const handleResize = () => {
        if (chartRef.current) {
          chart.applyOptions({ width: chartRef.current.clientWidth });
        }
      };
      window.addEventListener("resize", handleResize);

      return () => {
        window.removeEventListener("resize", handleResize);
      };
    });

    return () => {
      cancelled = true;
    };
  }, [data]);

  return (
    <Card className="col-span-full">
      <div className="flex items-center justify-between mb-3">
        <span className="text-sm text-text-secondary">Portfolio Value</span>
        <div className="flex gap-1">
          {TIMEFRAMES.map((tf) => (
            <button
              key={tf.label}
              onClick={() => setDays(tf.days)}
              className={`px-2 py-0.5 rounded text-xs ${
                days === tf.days
                  ? "bg-accent/20 text-accent"
                  : "text-text-secondary hover:bg-bg-hover"
              }`}
            >
              {tf.label}
            </button>
          ))}
        </div>
      </div>
      <div ref={chartRef} className="w-full" />
      {!data?.snapshots?.length && (
        <div className="h-[300px] flex items-center justify-center text-text-secondary text-sm">
          No equity data yet
        </div>
      )}
    </Card>
  );
}
