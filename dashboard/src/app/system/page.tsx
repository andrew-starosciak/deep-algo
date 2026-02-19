"use client";

import { useWorkflows, useSystemStatus } from "@/lib/hooks";
import WorkflowPipeline from "@/components/system/WorkflowPipeline";
import ScheduleTimeline from "@/components/system/ScheduleTimeline";
import WorkflowRuns from "@/components/system/WorkflowRuns";
import RulesCards from "@/components/system/RulesCards";
import ServiceStatus from "@/components/system/ServiceStatus";

export default function SystemPage() {
  const { data: workflows, isLoading, error } = useWorkflows();
  const { data: status } = useSystemStatus();

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-2">
        <span className="text-loss">Failed to load system data</span>
        <span className="text-text-secondary text-sm">{error.message}</span>
      </div>
    );
  }

  if (isLoading || !workflows) {
    return (
      <div className="flex items-center justify-center h-64 text-text-secondary">
        Loading system...
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <WorkflowPipeline />
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <div className="lg:col-span-2">
          <WorkflowRuns runs={workflows.runs} stats={workflows.stats} />
        </div>
        <ScheduleTimeline />
      </div>
      <RulesCards />
      <ServiceStatus
        dbConnected={status?.db_connected ?? false}
        lastEquityTick={workflows.last_equity_tick}
        latestRun={workflows.runs[0] ?? null}
      />
    </div>
  );
}
