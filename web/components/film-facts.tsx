// What the films show, as four tiles (lib/content.ts `filmFacts`), above each film's own record.

import { filmFacts } from "@/lib/content";

export function FilmFacts() {
  return (
    <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
      {filmFacts.tiles.map((tile) => (
        <div key={tile.label} className="panel flex flex-col rounded-[1.5rem] p-5">
          {/* Label first for a screen reader (a group is dt then dd); the figure first on screen. */}
          <dt className="order-2 mt-1 font-mono text-[10px] tracking-[0.16em] text-[color:var(--chapter)] uppercase">
            {tile.label}
          </dt>
          <dd className="order-1 text-[1.6rem] leading-tight font-extrabold tracking-[-0.03em] text-white tabular-nums">
            {tile.value}
          </dd>
          <dd className="order-3 mt-2.5 text-[13px] leading-snug text-slate-400">{tile.body}</dd>
        </div>
      ))}
    </dl>
  );
}
