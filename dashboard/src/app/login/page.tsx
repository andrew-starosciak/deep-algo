"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { setToken } from "@/lib/api";
import Card from "@/components/ui/Card";

export default function LoginPage() {
  const [token, setTokenInput] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const router = useRouter();

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");
    setLoading(true);

    try {
      const apiUrl = process.env.NEXT_PUBLIC_API_URL || "";
      const res = await fetch(`${apiUrl}/api/status`, {
        headers: { Authorization: `Bearer ${token}` },
      });

      if (res.status === 401) {
        setError("Invalid token");
        return;
      }

      if (!res.ok) {
        setError("Connection failed");
        return;
      }

      setToken(token);
      router.replace("/");
    } catch {
      setError("Cannot connect to API");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-bg-primary">
      <Card className="w-full max-w-sm">
        <h1 className="text-xl font-bold mb-1 text-center">OpenClaw</h1>
        <p className="text-text-secondary text-sm text-center mb-6">
          Trading Dashboard
        </p>
        <form onSubmit={handleSubmit}>
          <input
            type="password"
            value={token}
            onChange={(e) => setTokenInput(e.target.value)}
            placeholder="Enter dashboard token"
            className="w-full px-3 py-2 rounded bg-bg-hover border border-border text-text-primary placeholder:text-text-secondary focus:outline-none focus:border-accent mb-3"
          />
          {error && (
            <p className="text-loss text-xs mb-3">{error}</p>
          )}
          <button
            type="submit"
            disabled={loading || !token}
            className="w-full py-2 rounded bg-accent text-white font-medium hover:bg-accent/80 disabled:opacity-50"
          >
            {loading ? "Connecting..." : "Login"}
          </button>
        </form>
      </Card>
    </div>
  );
}
