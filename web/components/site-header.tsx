"use client";

import { Cog, ExternalLink, FlaskConical, type LucideIcon, Microscope, ShieldCheck, Terminal } from "lucide-react";
import { type CSSProperties, useEffect, useRef, useState } from "react";

import { chapters, siteConfig } from "@/lib/content";
import { LINEAGE } from "@/lib/palette";
import { cn } from "@/lib/utils";

const ICONS: Record<string, LucideIcon> = {
  world: Microscope,
  different: ShieldCheck,
  machinery: Cog,
  evidence: FlaskConical,
  lab: Terminal,
};

const chapterColour = (index: number) => LINEAGE[index % LINEAGE.length] ?? "#4ade80";

/**
 * The page's chrome. Wide screens: a floating pill with the five chapters, the one in view marked, and a
 * hairline that fills as the page is read. Phones: the name and the code at the top, and the chapters in a
 * dock at the bottom, where a thumb reaches them.
 */
export function SiteHeader() {
  const [scrolled, setScrolled] = useState(false);
  const [active, setActive] = useState<string | null>(null);
  const progress = useRef<HTMLSpanElement>(null);

  // Scrolled state and reading progress: the progress bar is written directly, never through React.
  useEffect(() => {
    let frame = 0;
    const update = () => {
      frame = 0;
      const doc = document.documentElement;
      const max = doc.scrollHeight - window.innerHeight;
      const done = max > 0 ? Math.min(1, Math.max(0, window.scrollY / max)) : 0;
      if (progress.current) progress.current.style.transform = `scaleX(${done})`;
      setScrolled(window.scrollY > 24);
    };
    const onScroll = () => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };
    update();
    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll, { passive: true });
    return () => {
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onScroll);
    };
  }, []);

  // The chapter crossing the middle of the screen is the one in view.
  useEffect(() => {
    if (typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) setActive(entry.target.getAttribute("data-chapter"));
        }
      },
      { rootMargin: "-45% 0px -54% 0px" },
    );
    const outside = [...document.querySelectorAll("[data-hero], [data-outside]")];
    const watched = [...document.querySelectorAll("[data-chapter]")];
    for (const element of watched) observer.observe(element);
    // Above the first chapter and below the last (the hero, the footer), no chapter is in view.
    const heroObserver = new IntersectionObserver(
      ([entry]) => {
        if (entry?.isIntersecting) setActive(null);
      },
      { rootMargin: "-45% 0px -54% 0px" },
    );
    for (const element of outside) heroObserver.observe(element);
    return () => {
      observer.disconnect();
      heroObserver.disconnect();
    };
  }, []);

  return (
    <>
      <a
        href="#main"
        className="sr-only z-[70] rounded-xl bg-green-500 px-4 py-2 text-sm font-bold text-black focus:not-sr-only focus:fixed focus:top-3 focus:left-3"
      >
        Skip to the page
      </a>
      <header className="fixed inset-x-0 top-0 z-50 px-3 pt-3 sm:px-6 sm:pt-4">
        <div
          className={cn(
            "relative mx-auto max-w-7xl overflow-hidden rounded-2xl border px-3 py-2 transition-[background-color,border-color,box-shadow] duration-500 sm:px-4",
            scrolled ? "glass-modern" : "border-transparent bg-transparent",
          )}
        >
          <div className="flex h-10 items-center gap-3">
            <a href="#top" className="flex shrink-0 items-center gap-2.5 rounded-lg">
              <span
                aria-hidden="true"
                className="h-6 w-6 rounded-full bg-[radial-gradient(circle_at_35%_35%,#bef264,#22c55e_45%,#052e16_75%)] shadow-[0_0_18px_rgba(34,197,94,0.55)]"
              />
              <span className="text-[15px] font-extrabold tracking-tight text-white">{siteConfig.name}</span>
            </a>
            <nav aria-label="Chapters" className="hidden min-w-0 flex-1 justify-center md:flex">
              <ul className="flex items-center gap-0.5 lg:gap-1">
                {chapters.map((chapter, index) => {
                  const on = active === chapter.id;
                  return (
                    <li key={chapter.id}>
                      <a
                        href={`#ch-${chapter.id}`}
                        aria-current={on ? "location" : undefined}
                        style={{ "--sphere": chapterColour(chapter.colour) } as CSSProperties}
                        className={cn(
                          "group flex items-center gap-2 rounded-xl px-2.5 py-2 text-[13px] font-semibold whitespace-nowrap transition-colors lg:px-3",
                          on ? "bg-white/[0.06] text-white" : "text-slate-400 hover:bg-white/[0.04] hover:text-white",
                        )}
                      >
                        <span
                          aria-hidden="true"
                          className={cn(
                            "sphere h-2 w-2 transition-[opacity,transform] duration-300",
                            on ? "scale-125 opacity-100" : "opacity-40 group-hover:opacity-80",
                          )}
                        />
                        <span className="hidden font-mono text-[11px] text-slate-500 xl:inline">{String(index + 1).padStart(2, "0")}</span>
                        {chapter.label}
                      </a>
                    </li>
                  );
                })}
              </ul>
            </nav>
            <a
              href={siteConfig.github}
              className="ml-auto inline-flex h-10 shrink-0 items-center gap-1.5 rounded-xl bg-green-500 px-3.5 text-[13px] font-extrabold text-black transition-colors hover:bg-green-400 md:ml-0"
            >
              GitHub <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
            </a>
          </div>
          {/* Reading progress, as a hairline along the pill's foot. */}
          <span
            aria-hidden="true"
            className={cn(
              "absolute inset-x-4 bottom-0 h-px bg-white/[0.04] transition-opacity duration-500",
              scrolled ? "opacity-100" : "opacity-0",
            )}
          >
            <span
              ref={progress}
              className="block h-full w-full origin-left bg-gradient-to-r from-green-400/90 via-green-300/80 to-teal-300/80"
              style={{ transform: "scaleX(0)" }}
            />
          </span>
        </div>
      </header>

      {/* Phones: the chapters in a dock at the bottom of the screen. */}
      <nav
        aria-label="Chapters"
        className="fixed inset-x-0 bottom-0 z-50 px-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] md:hidden"
      >
        <ul className="glass-modern mx-auto grid max-w-md grid-cols-5 rounded-2xl p-1">
          {chapters.map((chapter) => {
            const Icon = ICONS[chapter.id] ?? Microscope;
            const on = active === chapter.id;
            return (
              <li key={chapter.id}>
                <a
                  href={`#ch-${chapter.id}`}
                  aria-current={on ? "location" : undefined}
                  style={{ "--sphere": chapterColour(chapter.colour) } as CSSProperties}
                  className={cn(
                    "flex min-h-[3.25rem] flex-col items-center justify-center gap-1 rounded-xl text-[10px] font-bold tracking-wide transition-colors",
                    on ? "bg-white/[0.07] text-white" : "text-slate-400 active:bg-white/[0.06]",
                  )}
                >
                  <span className="relative">
                    <Icon className="h-[18px] w-[18px]" aria-hidden="true" />
                    {on ? <span aria-hidden="true" className="sphere absolute -top-1 -right-1.5 h-1.5 w-1.5" /> : null}
                  </span>
                  {chapter.short}
                </a>
              </li>
            );
          })}
        </ul>
      </nav>
    </>
  );
}
