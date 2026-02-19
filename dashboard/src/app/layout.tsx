"use client";

import { useEffect, useState } from "react";
import { usePathname, useRouter } from "next/navigation";
import { getToken } from "@/lib/api";
import Sidebar from "@/components/layout/Sidebar";
import Header from "@/components/layout/Header";
import "@/styles/globals.css";

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  const pathname = usePathname();
  const router = useRouter();
  const [ready, setReady] = useState(false);
  const isLogin = pathname === "/login" || pathname === "/login/";

  useEffect(() => {
    if (!isLogin && !getToken()) {
      router.replace("/login");
    } else {
      setReady(true);
    }
  }, [pathname, isLogin, router]);

  if (isLogin) {
    return (
      <html lang="en">
        <body className="bg-bg-primary text-text-primary">{children}</body>
      </html>
    );
  }

  if (!ready) {
    return (
      <html lang="en">
        <body className="bg-bg-primary text-text-primary" />
      </html>
    );
  }

  return (
    <html lang="en">
      <body className="bg-bg-primary text-text-primary">
        <div className="flex min-h-screen">
          <Sidebar />
          <div className="flex-1 flex flex-col">
            <Header />
            <main className="flex-1 p-4 overflow-auto">{children}</main>
          </div>
        </div>
      </body>
    </html>
  );
}
