"use client";

import Card from "@/components/ui/Card";
import { formatDateTime } from "@/lib/utils";
import type { WorkflowRunWithSteps } from "@/lib/types";

interface Service {
  name: string;
  status: "ok" | "warning" | "error" | "unknown";
  detail: string;
}

function StatusDot({ status }: { status: Service["status"] }) {
  const colors = {
    ok: "bg-profit",
    warning: "bg-yellow-500",
    error: "bg-loss",
    unknown: "bg-text-secondary",
  };
  return <span className={`w-2 h-2 rounded-full ${colors[status]}`} />;
}

interface Props {
  dbConnected: boolean;
  lastEquityTick: string | null;
  latestRun: WorkflowRunWithSteps | null;
}

export default function ServiceStatus({
  dbConnected,
  lastEquityTick,
  latestRun,
}: Props) {
  const fiveMinAgo = Date.now() - 5 * 60 * 1000;
  const oneHourAgo = Date.now() - 60 * 60 * 1000;

  const tickTime = lastEquityTick ? new Date(lastEquityTick).getTime() : 0;
  const tickOk = tickTime > fiveMinAgo;

  const runTime = latestRun?.started_at
    ? new Date(latestRun.started_at).getTime()
    : 0;
  const schedulerOk = runTime > oneHourAgo;

  const services: Service[] = [
    {
      name: "Database",
      status: dbConnected ? "ok" : "error",
      detail: dbConnected ? "Connected" : "Disconnected",
    },
    {
      name: "Dashboard API",
      status: "ok",
      detail: "Serving",
    },
    {
      name: "Position Manager",
      status: lastEquityTick ? (tickOk ? "ok" : "warning") : "unknown",
      detail: lastEquityTick ? formatDateTime(lastEquityTick) : "No data",
    },
    {
      name: "Workflow Scheduler",
      status: latestRun ? (schedulerOk ? "ok" : "warning") : "unknown",
      detail: latestRun?.started_at
        ? formatDateTime(latestRun.started_at)
        : "No runs",
    },
  ];

  return (
    <Card>
      <h2 className="text-sm font-semibold text-text-primary mb-4">
        Service Status
      </h2>
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        {services.map((svc) => (
          <div
            key={svc.name}
            className="flex items-center gap-2 px-3 py-2 bg-bg-hover/50 rounded-lg border border-border"
          >
            <StatusDot status={svc.status} />
            <div>
              <div className="text-xs font-medium text-text-primary">
                {svc.name}
              </div>
              <div className="text-[10px] text-text-secondary">{svc.detail}</div>
            </div>
          </div>
        ))}
      </div>
    </Card>
  );
}
