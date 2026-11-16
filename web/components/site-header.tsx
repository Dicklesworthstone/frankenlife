"use client";

import { ExternalLink } from "lucide-react";
import { useEffect, useState } from "react";

import { navItems, siteConfig } from "@/lib/content";
import { cn } from "@/lib/utils";

export function SiteHeader() {
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 24);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  return (
    <header className="fixed inset-x-0 top-0 z-50 px-3 pt-3 sm:px-6 sm:pt-4">
      <div
        className={cn(
          "mx-auto max-w-7xl rounded-2xl border px-3 py-2 transition-[background-color,border-color,box-shadow] duration-500 sm:px-4",
          scrolled ? "glass-modern" : "border-transparent bg-transparent",
        )}
      >
        <div className="flex items-center gap-3">
          <a
            href="#top"
            className="flex shrink-0 items-center gap-2 rounded-lg focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
          >
            <span
              aria-hidden="true"
              className="h-6 w-6 rounded-full bg-[radial-gradient(circle_at_35%_35%,#bef264,#22c55e_45%,#052e16_75%)] shadow-[0_0_18px_rgba(34,197,94,0.55)]"
            />
            <span className="text-sm font-black tracking-tight text-white">{siteConfig.name}</span>
          </a>
          {/* On a phone the links scroll sideways; the fade at the edge says so. */}
          <nav
            aria-label="Sections"
            className="min-w-0 flex-1 [mask-image:linear-gradient(to_right,black_80%,transparent)] sm:[mask-image:none]"
          >
            <ul className="flex items-center gap-1 overflow-x-auto [scrollbar-width:none] sm:justify-center [&::-webkit-scrollbar]:hidden">
              {navItems.map((item) => (
                <li key={item.href} className="shrink-0">
                  <a
                    href={item.href}
                    className="block rounded-lg px-2.5 py-1.5 text-[11px] font-bold tracking-wide whitespace-nowrap text-slate-300 uppercase transition hover:bg-green-500/10 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
                  >
                    {item.label}
                  </a>
                </li>
              ))}
            </ul>
          </nav>
          <a
            href={siteConfig.github}
            className="hidden shrink-0 items-center gap-1.5 rounded-xl bg-green-500 px-3.5 py-2 text-xs font-black text-black transition hover:bg-green-400 focus-visible:ring-2 focus-visible:ring-green-300 focus-visible:outline-none sm:inline-flex"
          >
            GitHub <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
          </a>
        </div>
      </div>
    </header>
  );
}
