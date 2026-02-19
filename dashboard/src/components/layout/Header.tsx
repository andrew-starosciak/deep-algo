"use client";

import { useSystemStatus } from "@/lib/hooks";
import { clearToken } from "@/lib/api";
import { useRouter } from "next/navigation";

export default function Header() {
  const { data } = useSystemStatus();
  const router = useRouter();

  const handleLogout = () => {
    clearToken();
    router.push("/login");
  };

  return (
    <header className="h-12 bg-bg-card border-b border-border flex items-center justify-between px-4">
      <div className="flex items-center gap-3">
        <span className="text-sm text-text-secondary">System:</span>
        <span className="flex items-center gap-1.5 text-sm">
          <span
            className={`w-2 h-2 rounded-full ${
              data?.db_connected ? "bg-profit" : "bg-loss"
            }`}
          />
          <span className="text-text-primary">
            {data?.db_connected ? "Connected" : "Disconnected"}
          </span>
        </span>
      </div>
      <button
        onClick={handleLogout}
        className="text-xs text-text-secondary hover:text-text-primary"
      >
        Logout
      </button>
    </header>
  );
}
