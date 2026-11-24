"use client";

// The films' records, one at a time: a tab per film, in the hero's order, and
// the chosen film's FilmProvenance beneath.

import { type KeyboardEvent, useRef, useState } from "react";

import { FilmProvenance } from "@/components/film-provenance";
import { filmNotes } from "@/lib/content";
import { films } from "@/lib/media";
import { cn } from "@/lib/utils";

export function FilmRecords() {
  const [chosen, setChosen] = useState(films[0]?.id ?? "");
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const shown = films.find((film) => film.id === chosen) ?? films[0];

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

  if (!shown) return null;
  return (
    <div>
      <div role="tablist" aria-label="Which film's record" className="flex flex-wrap gap-1.5">
        {films.map((film, at) => {
          const on = film.id === shown.id;
          const note = filmNotes[film.id];
          return (
            <button
              key={film.id}
              ref={(element) => {
                tabs.current[at] = element;
              }}
              id={`record-tab-${film.id}`}
              type="button"
              role="tab"
              aria-selected={on}
              aria-controls="record-panel"
              tabIndex={on ? 0 : -1}
              onClick={() => setChosen(film.id)}
              onKeyDown={(event) => onKey(event, at)}
              className={cn(
                "flex min-h-11 flex-col items-start justify-center rounded-xl border px-3.5 py-1.5 text-left transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                on ? "border-green-400/50 bg-green-500/10 text-white" : "border-white/10 text-slate-400 hover:border-white/25 hover:text-white",
              )}
            >
              <span className="text-[13.5px] font-semibold">{note?.tab ?? film.id}</span>
              {note ? <span className={cn("font-mono text-[10px]", on ? "text-green-300" : "text-slate-500")}>{note.chip}</span> : null}
            </button>
          );
        })}
      </div>
      <div id="record-panel" role="tabpanel" aria-labelledby={`record-tab-${shown.id}`} className="mt-4">
        <FilmProvenance film={shown} />
      </div>
    </div>
  );
}
