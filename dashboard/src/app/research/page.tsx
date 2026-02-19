"use client";

import { useSearchParams } from "next/navigation";
import { Suspense } from "react";
import WatchlistGrid from "@/components/research/WatchlistGrid";
import TickerDetail from "@/components/research/TickerDetail";

function ResearchContent() {
  const searchParams = useSearchParams();
  const ticker = searchParams.get("ticker");

  if (ticker) {
    return <TickerDetail ticker={ticker.toUpperCase()} />;
  }

  return (
    <div>
      <h1 className="text-xl font-bold mb-4">Research</h1>
      <WatchlistGrid />
    </div>
  );
}

export default function ResearchPage() {
  return (
    <Suspense
      fallback={
        <div className="text-text-secondary text-center py-12">Loading...</div>
      }
    >
      <ResearchContent />
    </Suspense>
  );
}
