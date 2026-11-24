"use client";

import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { Film } from "@/components/film";
import { WATCH_EVENT } from "@/components/watch-film";
import { filmNotes } from "@/lib/content";
import { type FilmManifest, films } from "@/lib/media";
import { cn } from "@/lib/utils";

/**
 * The page's films behind one set of tabs: one player at a time, so only the
 * chosen film's segments load. A film is the player's for its lifetime
 * (`key`), so switching starts the new film at its first frame.
 */
export function FilmReel({
  className,
  feather = false,
  idPrefix,
  initial,
}: {
  className?: string;
  feather?: boolean;
  /** Prefix for the tabs' element ids, unique on the page. */
  idPrefix: string;
  /** The film shown first; the first film lib/films/index.ts lists by default. */
  initial?: string;
}) {
  const [chosen, setChosen] = useState<string>(initial ?? films[0]?.id ?? "");
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const shown: FilmManifest = films.find((entry) => entry.id === chosen) ?? films[0]!;

  // A "watch" button elsewhere on the page (components/watch-film.tsx) chooses a film here.
  useEffect(() => {
    const onWatch = (event: Event) => {
      const id = (event as CustomEvent<string>).detail;
      if (films.some((entry) => entry.id === id)) setChosen(id);
    };
    window.addEventListener(WATCH_EVENT, onWatch);
    return () => window.removeEventListener(WATCH_EVENT, onWatch);
  }, []);

  // Arrow keys move along the tabs and choose (WAI-ARIA tabs with automatic activation).
  const onKey = (event: KeyboardEvent<HTMLButtonElement>, at: number) => {
    const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    const to =
      event.key === "Home" ? 0 : event.key === "End" ? films.length - 1 : step ? (at + step + films.length) % films.length : -1;
    if (to < 0) return;
    event.preventDefault();
    const next = films[to];
    if (!next) return;
    setChosen(next.id);
    tabs.current[to]?.focus();
  };

  const note = filmNotes[shown.id];

  return (
    <div className={className}>
      {films.length > 1 ? (
        <div
          role="tablist"
          aria-label="Choose a film"
          className="mb-3 flex gap-1.5 overflow-x-auto px-1 pb-1 [scrollbar-width:none] lg:mb-4"
        >
          {films.map((entry, at) => {
            const on = entry.id === shown.id;
            const tab = filmNotes[entry.id];
            return (
              <button
                key={entry.id}
                ref={(element) => {
                  tabs.current[at] = element;
                }}
                id={`${idPrefix}-tab-${entry.id}`}
                type="button"
                role="tab"
                aria-selected={on}
                aria-controls={`${idPrefix}-panel`}
                tabIndex={on ? 0 : -1}
                onClick={() => setChosen(entry.id)}
                onKeyDown={(event) => onKey(event, at)}
                className={cn(
                  "group flex min-h-11 shrink-0 flex-col items-start justify-center rounded-xl border px-3 py-1.5 text-left transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                  on
                    ? "border-green-400/50 bg-green-500/10 text-white"
                    : "border-white/10 bg-black/40 text-slate-400 hover:border-white/25 hover:text-white",
                )}
              >
                <span className="text-[13px] leading-tight font-bold">{tab?.tab ?? entry.world.name}</span>
                <span className={cn("font-mono text-[9.5px] tracking-[0.12em] uppercase", on ? "text-green-300" : "text-slate-500")}>
                  {tab?.chip ?? entry.world.model ?? ""}
                </span>
              </button>
            );
          })}
        </div>
      ) : null}
      <div id={`${idPrefix}-panel`} role="tabpanel" aria-labelledby={`${idPrefix}-tab-${shown.id}`}>
        <Film key={shown.id} manifest={shown} feather={feather} />
        {note ? (
          <p className="mt-3 px-1 text-[13px] leading-relaxed text-slate-400 lg:px-2">
            <span className="font-semibold text-slate-200">{note.title}.</span> {note.line}
          </p>
        ) : null}
      </div>
    </div>
  );
}
