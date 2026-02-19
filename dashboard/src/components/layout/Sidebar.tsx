"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

const nav = [
  { href: "/", label: "Portfolio" },
  { href: "/research", label: "Research" },
  { href: "/system", label: "System" },
];

export default function Sidebar() {
  const pathname = usePathname();

  return (
    <aside className="w-48 bg-bg-card border-r border-border min-h-screen p-4 flex flex-col">
      <div className="text-lg font-bold text-text-primary mb-6">OpenClaw</div>
      <nav className="flex flex-col gap-1">
        {nav.map((item) => {
          const active =
            item.href === "/"
              ? pathname === "/"
              : pathname.startsWith(item.href);
          return (
            <Link
              key={item.href}
              href={item.href}
              className={`px-3 py-2 rounded text-sm ${
                active
                  ? "bg-accent/20 text-accent"
                  : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
              }`}
            >
              {item.label}
            </Link>
          );
        })}
      </nav>
    </aside>
  );
}
