"use client";

interface CardProps {
  children: React.ReactNode;
  className?: string;
}

export default function Card({ children, className = "" }: CardProps) {
  return (
    <div
      className={`bg-bg-card border border-border rounded-lg p-4 ${className}`}
    >
      {children}
    </div>
  );
}
