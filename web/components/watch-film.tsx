"use client";

import { Play } from "lucide-react";

/** The event the hero's film reel listens for: play the film with this id. */
export const WATCH_EVENT = "frankenlife:watch-film";

/** A button that plays a film in the hero and brings the hero into view. */
export function WatchFilm({ id, label }: { id: string; label: string }) {
  return (
    <button
      type="button"
      onClick={() => {
        window.dispatchEvent(new CustomEvent(WATCH_EVENT, { detail: id }));
        const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
        document.getElementById("top")?.scrollIntoView({ behavior: still ? "auto" : "smooth" });
      }}
      className="inline-flex min-h-11 items-center gap-2 rounded-xl border border-white/15 bg-white/[0.03] px-4 text-[14px] font-bold text-white transition-colors hover:border-green-400/50 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
    >
      <Play className="h-4 w-4" aria-hidden="true" /> {label}
    </button>
  );
}
