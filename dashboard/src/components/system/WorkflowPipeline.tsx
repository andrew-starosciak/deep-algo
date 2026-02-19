"use client";

import Card from "@/components/ui/Card";

const steps = [
  { label: "Watchlist", agent: null, gate: "tickers" },
  { label: "Research Agent", agent: "researcher", gate: "score \u2265 3" },
  { label: "Analyst Agent", agent: "analyst", gate: "score \u2265 6.0", feedback: true },
  { label: "Risk Checker", agent: "risk_checker", gate: "size \u2264 2%" },
  { label: "Contract Select", agent: null, gate: "IB live data" },
  { label: "Recommendation", agent: null, gate: "pending" },
  { label: "Position Manager", agent: null, gate: "execute" },
  { label: "IB Execution", agent: null, gate: "filled" },
];

export default function WorkflowPipeline() {
  return (
    <Card>
      <h2 className="text-sm font-semibold text-text-primary mb-4">
        Trade Thesis Pipeline
      </h2>
      <div className="relative">
        <div className="flex items-center gap-0 overflow-x-auto pb-2">
          {steps.map((step, i) => (
            <div key={step.label} className="flex items-center shrink-0">
              <div className="flex flex-col items-center relative">
                <div
                  className={`border rounded-lg px-3 py-2 text-center min-w-[100px] ${
                    step.feedback
                      ? "bg-accent/5 border-accent/30"
                      : "bg-bg-hover border-border"
                  }`}
                >
                  <div className="text-xs font-medium text-text-primary whitespace-nowrap">
                    {step.label}
                  </div>
                  <div className="text-[10px] text-accent mt-0.5 whitespace-nowrap">
                    {step.gate}
                  </div>
                </div>
                {step.feedback && (
                  <div className="absolute -bottom-5 text-[9px] text-accent whitespace-nowrap">
                    + history context
                  </div>
                )}
              </div>
              {i < steps.length - 1 && (
                <div className="text-text-secondary text-xs px-1 shrink-0">
                  &rarr;
                </div>
              )}
            </div>
          ))}
        </div>

        {/* Feedback loop annotation */}
        <div className="mt-6 flex items-center gap-2 text-[10px] text-text-secondary border-t border-border/50 pt-2">
          <span className="text-accent">&#x21ba;</span>
          <span>
            When positions close, outcomes (P&L, reason) are saved on the thesis
            and fed back to the Analyst on the next research cycle
          </span>
        </div>
      </div>
    </Card>
  );
}
