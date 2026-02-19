import { format, parseISO, differenceInDays } from "date-fns";

export function formatCurrency(value: string | number): string {
  const num = typeof value === "string" ? parseFloat(value) : value;
  if (isNaN(num)) return "$0.00";
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
  }).format(num);
}

export function formatPnL(value: string | number): string {
  const num = typeof value === "string" ? parseFloat(value) : value;
  if (isNaN(num)) return "$0.00";
  const sign = num >= 0 ? "+" : "";
  return `${sign}${formatCurrency(num)}`;
}

export function formatPercent(value: number): string {
  const sign = value >= 0 ? "+" : "";
  return `${sign}${value.toFixed(2)}%`;
}

export function getPnLColor(value: string | number): string {
  const num = typeof value === "string" ? parseFloat(value) : value;
  if (num > 0) return "text-profit";
  if (num < 0) return "text-loss";
  return "text-text-secondary";
}

export function formatDate(iso: string): string {
  try {
    return format(parseISO(iso), "MMM d, yyyy");
  } catch {
    return iso;
  }
}

export function formatDateTime(iso: string): string {
  try {
    return format(parseISO(iso), "MMM d, h:mm a");
  } catch {
    return iso;
  }
}

export function daysToExpiry(expiry: string): number {
  try {
    return differenceInDays(parseISO(expiry), new Date());
  } catch {
    return 0;
  }
}
