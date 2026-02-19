const API_URL = process.env.NEXT_PUBLIC_API_URL || "";

export function getToken(): string | null {
  if (typeof window === "undefined") return null;
  return localStorage.getItem("dashboard_token");
}

export function setToken(token: string) {
  localStorage.setItem("dashboard_token", token);
}

export function clearToken() {
  localStorage.removeItem("dashboard_token");
}

export async function apiFetch<T>(path: string): Promise<T> {
  const token = getToken();
  if (!token) throw new Error("Not authenticated");

  const res = await fetch(`${API_URL}${path}`, {
    headers: { Authorization: `Bearer ${token}` },
  });

  if (res.status === 401) {
    clearToken();
    window.location.href = "/login";
    throw new Error("Unauthorized");
  }

  if (!res.ok) {
    throw new Error(`API error: ${res.status}`);
  }

  return res.json();
}

export const fetcher = <T>(path: string) => apiFetch<T>(path);
