"use client";

import Card from "@/components/ui/Card";

const schedule = [
  { time: "8:00 AM", label: "Premarket Research", days: "Mon\u2013Fri", hour: 8 },
  { time: "9:30 AM", label: "Tick Loop Starts (30s)", days: "Mon\u2013Fri", hour: 9.5 },
  { time: "12:30 PM", label: "Midday Position Check", days: "Mon\u2013Fri", hour: 12.5 },
  { time: "4:00 PM", label: "Tick Loop Ends", days: "Mon\u2013Fri", hour: 16 },
  { time: "4:30 PM", label: "Post-market Check", days: "Mon\u2013Fri", hour: 16.5 },
  { time: "Sat 10 AM", label: "Weekly Deep Dive", days: "Saturday", hour: 10 },
];

function currentHour(): number {
  const now = new Date();
  return now.getHours() + now.getMinutes() / 60;
}

export default function ScheduleTimeline() {
  const now = currentHour();
  const isSaturday = new Date().getDay() === 6;

  // Find next upcoming job
  const weekdayJobs = schedule.filter((s) => s.days !== "Saturday");
  const nextIdx = weekdayJobs.findIndex((s) => s.hour > now);

  return (
    <Card>
      <h2 className="text-sm font-semibold text-text-primary mb-4">
        Daily Schedule
      </h2>
      <div className="space-y-2">
        {schedule.map((item, i) => {
          const isNext =
            (!isSaturday && item.days !== "Saturday" && weekdayJobs[nextIdx] === item) ||
            (isSaturday && item.days === "Saturday" && item.hour > now);
          const isPast =
            item.days !== "Saturday"
              ? !isSaturday && item.hour <= now
              : isSaturday && item.hour <= now;

          return (
            <div
              key={i}
              className={`flex items-center gap-3 px-3 py-1.5 rounded text-sm ${
                isNext
                  ? "bg-accent/10 border border-accent/30"
                  : "border border-transparent"
              }`}
            >
              <span
                className={`font-mono text-xs w-20 shrink-0 ${
                  isPast ? "text-text-secondary" : "text-text-primary"
                }`}
              >
                {item.time}
              </span>
              <div className="flex-1 flex items-center gap-2">
                <div
                  className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                    isNext
                      ? "bg-accent"
                      : isPast
                        ? "bg-text-secondary/40"
                        : "bg-text-secondary"
                  }`}
                />
                <span
                  className={
                    isPast ? "text-text-secondary" : "text-text-primary"
                  }
                >
                  {item.label}
                </span>
              </div>
              <span className="text-[10px] text-text-secondary shrink-0">
                {item.days}
              </span>
            </div>
          );
        })}
      </div>
    </Card>
  );
}
