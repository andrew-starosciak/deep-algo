"use client";

interface BadgeProps {
  label: string;
  variant?: "profit" | "loss" | "neutral" | "accent";
}

const variants = {
  profit: "bg-profit/20 text-profit",
  loss: "bg-loss/20 text-loss",
  neutral: "bg-text-secondary/20 text-text-secondary",
  accent: "bg-accent/20 text-accent",
};

export default function Badge({ label, variant = "neutral" }: BadgeProps) {
  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium ${variants[variant]}`}
    >
      {label}
    </span>
  );
}
