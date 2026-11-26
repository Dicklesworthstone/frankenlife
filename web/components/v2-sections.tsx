"use client";

// The Version 2 chapter's pieces besides the step-throughs: the two presets,
// each preset beside its controls with the population in every Stage-2
// condition, and what is not yet shown. Every figure is lib/content.ts's (from
// experiments/v2-emergence/README.md) or lib/stills.json's.

import { useState } from "react";

import { Reticle } from "@/components/franken";
import { Reveal } from "@/components/reveal";
import { WatchFilm } from "@/components/watch-film";
import { type AblationRow, filmNotes, v2Ablations, v2Bundles, v2Residuals } from "@/lib/content";
import { type V2Still, stills } from "@/lib/media";
import { cn, grouped } from "@/lib/utils";

const byId = (id: string): V2Still => {
  const found = stills.stills.find((still) => still.id === id);
  if (!found) throw new Error(`lib/stills.json has no still ${id}`);
  return found;
};

/**
 * The two presets, side by side, each led by its own still from a Stage-2
 * treatment run, and ending with a button for the hero's film of one of its
 * mechanisms alone in the confluent world.
 */
export function BundlePanels() {
  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-2">
      {v2Bundles.map((bundle, at) => {
        const still = byId(`${bundle.id}-treatment`);
        return (
          <Reveal key={bundle.id} delay={at * 0.08}>
            <article className="panel relative flex h-full flex-col overflow-hidden rounded-[1.75rem]">
              <div className="relative aspect-[16/10] overflow-hidden bg-black">
                {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
                <img
                  src={still.src}
                  width={still.width}
                  height={still.height}
                  loading="lazy"
                  decoding="async"
                  alt={`The ${bundle.preset} preset at tick ${grouped(still.tick)}, seed ${still.seed}: ${grouped(still.live)} organisms.`}
                  className="h-full w-full object-cover"
                />
                <Reticle inset={10} size={14} />
                <span className="absolute top-3 left-3 rounded-full border border-white/10 bg-black/60 px-2.5 py-1 font-mono text-[10px] font-bold tracking-[0.12em] text-white uppercase backdrop-blur-md">
                  {bundle.tag}
                </span>
                <span className="absolute bottom-3 left-3 rounded-full border border-white/10 bg-black/60 px-2.5 py-1 font-mono text-[10px] tracking-[0.12em] text-green-200 uppercase backdrop-blur-md">
                  tick {grouped(still.tick)} · {grouped(still.live)} alive
                </span>
              </div>
              <div className="flex flex-1 flex-col p-6 sm:p-7">
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <h3 className="text-2xl font-extrabold tracking-tight text-white">{bundle.name}</h3>
                  <code className="text-[11px] text-green-300/80">{bundle.preset}</code>
                </div>
                <p className="mt-3 text-[15px] leading-relaxed text-slate-300">{bundle.picture}</p>
                <ul className="mt-4 flex flex-wrap gap-1.5">
                  {bundle.facts.map((fact) => (
                    <li key={fact} className="rounded-full border border-white/10 bg-white/[0.03] px-2.5 py-1 text-[12px] text-slate-300">
                      {fact}
                    </li>
                  ))}
                </ul>
                <p className="mt-5 border-t border-white/[0.06] pt-5 font-mono text-[10px] tracking-[0.16em] text-slate-500 uppercase">
                  Measured in Stage-2 runs
                </p>
                <dl className="mt-3 grid grid-cols-3 gap-3">
                  {bundle.measured.map((stat) => (
                    <div key={stat.label} className="min-w-0">
                      <dt className="text-[11px] leading-tight text-slate-500">{stat.label}</dt>
                      {/* At 320 px with wider letter spacing (WCAG 1.4.12) "15,446" outgrew its third: it wraps. */}
                      <dd className="mt-1 text-2xl font-extrabold tracking-tight text-white tabular-nums [overflow-wrap:anywhere]">{stat.value}</dd>
                      <dd className="mt-0.5 text-[11px] leading-snug text-slate-500">{stat.note}</dd>
                    </div>
                  ))}
                </dl>
                <div className="mt-auto flex flex-wrap items-center gap-x-4 gap-y-2 pt-6">
                  <WatchFilm id={bundle.film} label={`Watch the ${(filmNotes[bundle.film]?.tab ?? bundle.film).toLowerCase()} film`} />
                  <p className="min-w-0 flex-1 basis-56 text-[12px] leading-snug text-slate-500">{bundle.filmLine}</p>
                </div>
              </div>
            </article>
          </Reveal>
        );
      })}
    </div>
  );
}

function StillFigure({ still }: { still: V2Still }) {
  return (
    <figure className="panel relative h-full overflow-hidden rounded-[1.5rem]">
      {/* The name holds the badge's visible "N alive", so a voice command that reads it finds the link. Its focus ring
          is drawn inside: outside, the figure's overflow-hidden cut it off entirely. */}
      <a
        href={still.src}
        className="focus-inset relative block aspect-square rounded-t-[1.5rem] bg-black"
        aria-label={`${still.label}, ${grouped(still.live)} alive: open the ${still.width} × ${still.height} still`}
      >
        {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
        <img
          src={still.src}
          width={still.width}
          height={still.height}
          sizes="(min-width: 768px) 33vw, 100vw"
          loading="lazy"
          decoding="async"
          alt={`${still.label}: ${still.caption}`}
          className="h-full w-full object-cover"
        />
        <Reticle inset={10} size={14} />
        <span className="absolute right-3 bottom-3 rounded-full border border-white/10 bg-black/65 px-2.5 py-1 font-mono text-[11px] font-bold text-white tabular-nums backdrop-blur-md">
          {grouped(still.live)} alive
        </span>
      </a>
      <figcaption className="p-5">
        <span className="text-base font-bold text-white">{still.label}</span>
        <p className="mt-1.5 text-[13.5px] leading-relaxed text-slate-400">{still.caption}</p>
      </figcaption>
    </figure>
  );
}

const KIND: Record<AblationRow["kind"], string> = {
  treatment: "bg-green-400",
  control: "bg-slate-400/70",
  minus: "bg-cyan-400/70",
  plus: "bg-amber-300/80",
};

function BarList({ rows, max }: { rows: AblationRow[]; max: number }) {
  return (
    <ul className="space-y-2.5">
      {rows.map((row) => (
        <li key={row.condition} className="grid grid-cols-[minmax(0,10.5rem)_minmax(0,1fr)] items-center gap-3 sm:grid-cols-[12rem_minmax(0,1fr)]">
          <span className={cn("text-[12.5px] leading-snug break-words", row.kind === "treatment" ? "font-bold text-white" : "text-slate-300")}>{row.condition}</span>
          <span className="flex min-w-0 items-center gap-2">
            <span className="relative h-2.5 min-w-0 flex-1 rounded-full bg-white/[0.05]">
              <span className={cn("absolute inset-y-0 left-0 rounded-full", KIND[row.kind])} style={{ width: `${Math.max(0.4, (row.live / max) * 100)}%` }} />
            </span>
            <span className="w-14 shrink-0 text-right font-mono text-[11.5px] text-slate-200 tabular-nums">{grouped(row.live)}</span>
          </span>
          {row.note ? <span className="col-start-2 -mt-1.5 text-[11.5px] leading-snug text-slate-500">{row.note}</span> : null}
        </li>
      ))}
    </ul>
  );
}

function AblationBars({ table }: { table: (typeof v2Ablations)[number] }) {
  const max = Math.max(...table.rows.map((row) => row.live));
  const controls = table.rows.filter((row) => row.kind === "treatment" || row.kind === "control");
  const changed = table.rows.filter((row) => row.kind === "minus" || row.kind === "plus");
  const head = "font-mono text-[10px] tracking-[0.16em] text-slate-500 uppercase";
  return (
    <figure className="panel rounded-[1.75rem] p-6 sm:p-7">
      <figcaption className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="text-base font-bold text-white">Alive in every condition</span>
        <span className="font-mono text-[10.5px] text-slate-500">mean at tick 2,048 · 4 seeds · {table.preset} · one scale</span>
      </figcaption>
      <div className="mt-5 grid grid-cols-1 gap-x-10 gap-y-6 xl:grid-cols-2">
        <div>
          <p className={head}>As committed, and its controls</p>
          <div className="mt-3">
            <BarList rows={controls} max={max} />
          </div>
        </div>
        <div>
          <p className={head}>One switch changed</p>
          <div className="mt-3">
            <BarList rows={changed} max={max} />
          </div>
        </div>
      </div>
      <div className="mt-5 flex flex-wrap gap-x-4 gap-y-1.5 border-t border-white/[0.06] pt-4 text-[11px] text-slate-500">
        {(
          [
            ["bg-green-400", "as committed"],
            ["bg-slate-400/70", "control"],
            ["bg-cyan-400/70", "one mechanism removed"],
            ["bg-amber-300/80", "added"],
          ] as const
        ).map(([swatch, label]) => (
          <span key={label} className="flex items-center gap-1.5">
            <span className={cn("h-2 w-3 rounded-full", swatch)} /> {label}
          </span>
        ))}
      </div>
    </figure>
  );
}

const WORLDS = [
  { id: "tissue", label: "Tissue", preset: "morphogenesis-tissue-v2" },
  { id: "supercolony", label: "Supercolony", preset: "supercolony-v2" },
] as const;

/** One preset at a time: beside its two controls at the same tick and seed, then its population in every condition. */
export function TakeOneAway() {
  const [world, setWorld] = useState<(typeof WORLDS)[number]["id"]>("tissue");
  const shown = WORLDS.find((each) => each.id === world) ?? WORLDS[0];
  const row = stills.stills.filter((still) => still.set === "v2-controls" && still.bundle === world);
  const table = v2Ablations.find((each) => each.preset === shown.preset);
  return (
    <div>
      <div role="group" aria-label="Preset" className="inline-flex rounded-xl border border-white/10 bg-black/40 p-1">
        {WORLDS.map((each) => (
          <button
            key={each.id}
            type="button"
            aria-pressed={world === each.id}
            onClick={() => setWorld(each.id)}
            className={cn(
              "min-h-11 rounded-lg px-4 text-[13.5px] font-bold transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
              world === each.id ? "bg-green-500 text-black" : "text-slate-300 hover:bg-white/5 hover:text-white",
            )}
          >
            {each.label}
          </button>
        ))}
      </div>
      <div className="mt-5 grid grid-cols-1 gap-4 md:grid-cols-3">
        {row.map((still) => (
          <StillFigure key={still.id} still={still} />
        ))}
      </div>
      <p className="mt-4 text-xs leading-relaxed text-slate-500">
        Each still here and in the step-throughs above is the world&apos;s frame at tick {grouped(stills.tick)} of{" "}
        <code className="text-slate-300">flife render</code> from tick 0, seed {row[0]?.seed ?? 42}, 2160 × 2160.{" "}
        {stills.stage2
          ? "Each world's digest and live count at that tick equal those the Stage-2 campaign recorded for the same preset, condition and seed."
          : null}{" "}
        Rendered at revision <code className="text-slate-300">{stills.revision.slice(0, 12)}</code>.
      </p>
      {table ? (
        <div className="mt-8">
          <AblationBars table={table} />
        </div>
      ) : null}
    </div>
  );
}

/** What the v2 report says is not shown, or costs. */
export function Residuals() {
  return (
    <ul className="grid grid-cols-1 gap-x-10 gap-y-6 rounded-[1.75rem] border border-amber-300/15 bg-amber-300/[0.03] p-6 sm:p-8 md:grid-cols-2">
      {v2Residuals.map((item) => (
        <li key={item.title} className="flex gap-3">
          <span aria-hidden="true" className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-amber-300/80" />
          <div>
            <h3 className="text-[15px] font-bold text-amber-100">{item.title}</h3>
            <p className="mt-1 text-[14px] leading-relaxed text-slate-400">{item.body}</p>
          </div>
        </li>
      ))}
    </ul>
  );
}

