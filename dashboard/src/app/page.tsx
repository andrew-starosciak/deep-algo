"use client";

import { usePortfolio } from "@/lib/hooks";
import AccountStats from "@/components/portfolio/AccountStats";
import PositionDistribution from "@/components/portfolio/PositionDistribution";
import PnLChart from "@/components/portfolio/PnLChart";
import PositionsTable from "@/components/portfolio/PositionsTable";

export default function HomePage() {
  const { data, isLoading, error } = usePortfolio();

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-2">
        <span className="text-loss">Failed to load portfolio</span>
        <span className="text-text-secondary text-sm">{error.message}</span>
      </div>
    );
  }

  if (isLoading || !data) {
    return (
      <div className="flex items-center justify-center h-64 text-text-secondary">
        Loading portfolio...
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <AccountStats data={data} />
      <div className="grid grid-cols-1 lg:grid-cols-4 gap-4">
        <div className="lg:col-span-3">
          <PnLChart />
        </div>
        <div>
          <PositionDistribution
            calls={data.calls_count}
            puts={data.puts_count}
          />
        </div>
      </div>
      <PositionsTable />
    </div>
  );
}
