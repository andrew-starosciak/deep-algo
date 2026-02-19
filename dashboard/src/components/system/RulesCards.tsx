"use client";

import Card from "@/components/ui/Card";

const rules = [
  {
    label: "Hard Stop",
    trigger: "-50%",
    action: "Close All",
    color: "text-loss",
    borderColor: "border-loss/30",
  },
  {
    label: "Profit Target 1",
    trigger: "+50%",
    action: "Sell Half",
    color: "text-profit",
    borderColor: "border-profit/30",
  },
  {
    label: "Profit Target 2",
    trigger: "+100%",
    action: "Close All",
    color: "text-profit",
    borderColor: "border-profit/30",
  },
  {
    label: "Time Stop",
    trigger: "7 DTE + losing",
    action: "Close All",
    color: "text-loss",
    borderColor: "border-loss/30",
  },
  {
    label: "Max Allocation",
    trigger: "10% of account",
    action: "Block New",
    color: "text-accent",
    borderColor: "border-accent/30",
  },
  {
    label: "Tick Interval",
    trigger: "Every 30s",
    action: "Market Hours",
    color: "text-accent",
    borderColor: "border-accent/30",
  },
];

export default function RulesCards() {
  return (
    <Card>
      <h2 className="text-sm font-semibold text-text-primary mb-4">
        Position Manager Rules
      </h2>
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
        {rules.map((rule) => (
          <div
            key={rule.label}
            className={`border ${rule.borderColor} rounded-lg p-3 text-center bg-bg-hover/50`}
          >
            <div className="text-[10px] text-text-secondary mb-1">
              {rule.label}
            </div>
            <div className={`text-sm font-semibold ${rule.color}`}>
              {rule.trigger}
            </div>
            <div className="text-[10px] text-text-secondary mt-1">
              {rule.action}
            </div>
          </div>
        ))}
      </div>
    </Card>
  );
}
