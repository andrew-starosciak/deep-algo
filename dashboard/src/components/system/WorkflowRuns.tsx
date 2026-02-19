"use client";

import { useState } from "react";
import Card from "@/components/ui/Card";
import { formatDateTime } from "@/lib/utils";
import type { WorkflowRunWithSteps, WorkflowStats } from "@/lib/types";

function StatusBadge({ status }: { status: string }) {
  const colors: Record<string, string> = {
    completed: "bg-profit/20 text-profit",
    running: "bg-accent/20 text-accent",
    failed: "bg-loss/20 text-loss",
  };
  return (
    <span
      className={`text-[10px] px-1.5 py-0.5 rounded font-medium ${colors[status] || "bg-bg-hover text-text-secondary"}`}
    >
      {status}
    </span>
  );
}

function formatDuration(ms: number | null): string {
  if (ms === null) return "\u2014";
  if (ms < 1000) return `${ms}ms`;
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m ${s % 60}s`;
}

function StepBar({ step }: { step: { step_id: string; agent: string; passed_gate: boolean; duration_ms: number; attempt: number } }) {
  return (
    <div className="flex items-center gap-2 text-xs py-1">
      <span className={`w-3 h-3 rounded-full shrink-0 ${step.passed_gate ? "bg-profit" : "bg-loss"}`} />
      <span className="text-text-primary font-medium w-24 shrink-0">{step.step_id}</span>
      <span className="text-text-secondary w-24 shrink-0">{step.agent}</span>
      <div className="flex-1 h-1.5 bg-bg-hover rounded-full overflow-hidden">
        <div
          className={`h-full rounded-full ${step.passed_gate ? "bg-profit/60" : "bg-loss/60"}`}
          style={{ width: `${Math.min((step.duration_ms / 30000) * 100, 100)}%` }}
        />
      </div>
      <span className="text-text-secondary w-12 text-right shrink-0">
        {formatDuration(step.duration_ms)}
      </span>
      {step.attempt > 0 && (
        <span className="text-[10px] text-loss">retry {step.attempt}</span>
      )}
    </div>
  );
}

interface Props {
  runs: WorkflowRunWithSteps[];
  stats: WorkflowStats;
}

export default function WorkflowRuns({ runs, stats }: Props) {
  const [expanded, setExpanded] = useState<number | null>(null);

  return (
    <Card>
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-sm font-semibold text-text-primary">
          Recent Workflow Runs
        </h2>
        <div className="flex gap-3 text-xs text-text-secondary">
          <span>{stats.total_runs} total</span>
          <span className="text-profit">{stats.completed} ok</span>
          <span className="text-loss">{stats.failed} failed</span>
          <span>avg {formatDuration(stats.avg_duration_ms)}</span>
          <span>{stats.runs_today} today</span>
        </div>
      </div>
      <div className="space-y-1">
        {runs.length === 0 && (
          <div className="text-text-secondary text-sm text-center py-4">
            No workflow runs yet
          </div>
        )}
        {runs.map((run) => (
          <div key={run.id}>
            <button
              onClick={() => setExpanded(expanded === run.id ? null : run.id)}
              className="w-full flex items-center gap-3 px-3 py-2 rounded hover:bg-bg-hover text-sm text-left"
            >
              <span className="text-text-primary font-medium w-32 shrink-0 truncate">
                {run.workflow_id}
              </span>
              <span className="text-text-secondary text-xs w-16 shrink-0">
                {run.trigger}
              </span>
              <StatusBadge status={run.status} />
              <span className="text-text-secondary text-xs flex-1 text-right">
                {formatDuration(run.duration_ms)}
              </span>
              <span className="text-text-secondary text-xs w-32 text-right shrink-0">
                {run.started_at ? formatDateTime(run.started_at) : "\u2014"}
              </span>
              <span className="text-text-secondary text-xs w-4">
                {expanded === run.id ? "\u25B4" : "\u25BE"}
              </span>
            </button>
            {expanded === run.id && run.steps.length > 0 && (
              <div className="ml-6 mr-3 mb-2 pl-3 border-l border-border">
                {run.steps.map((step, i) => (
                  <StepBar key={i} step={step} />
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
    </Card>
  );
}
