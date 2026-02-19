"use client";

import Card from "@/components/ui/Card";

const steps = [
  { label: "Watchlist", agent: null, gate: "tickers" },
  { label: "Research Agent", agent: "researcher", gate: "score \u2265 3" },
  { label: "Analyst Agent", agent: "analyst", gate: "score \u2265 6.0" },
  { label: "Risk Checker", agent: "risk_checker", gate: "size \u2264 2%" },
  { label: "Recommendation", agent: null, gate: "pending_review" },
  { label: "Human Approve", agent: null, gate: "approved" },
  { label: "Position Manager", agent: null, gate: "execute order" },
  { label: "IB Execution", agent: null, gate: "filled" },
];

export default function WorkflowPipeline() {
  return (
    <Card>
      <h2 className="text-sm font-semibold text-text-primary mb-4">
        Trade Thesis Pipeline
      </h2>
      <div className="flex items-center gap-0 overflow-x-auto pb-2">
        {steps.map((step, i) => (
          <div key={step.label} className="flex items-center shrink-0">
            <div className="flex flex-col items-center">
              <div className="bg-bg-hover border border-border rounded-lg px-3 py-2 text-center min-w-[100px]">
                <div className="text-xs font-medium text-text-primary whitespace-nowrap">
                  {step.label}
                </div>
                <div className="text-[10px] text-accent mt-0.5 whitespace-nowrap">
                  {step.gate}
                </div>
              </div>
            </div>
            {i < steps.length - 1 && (
              <div className="text-text-secondary text-xs px-1 shrink-0">
                &rarr;
              </div>
            )}
          </div>
        ))}
      </div>
    </Card>
  );
}
