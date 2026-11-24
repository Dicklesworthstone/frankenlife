// The Version 2 chapter's pieces: the two presets, the read-only views, the
// controls beside each preset, the ablation table and what is not yet shown.
// Every figure is lib/content.ts's (from experiments/v2-emergence/README.md)
// or a manifest's (lib/stills.json, lib/films/*.json).

import { MicroLabel, Reticle } from "@/components/franken";
import { Reveal } from "@/components/reveal";
import { WatchFilm } from "@/components/watch-film";
import { type AblationRow, v2Ablations, v2Bundles, v2Residuals } from "@/lib/content";
import { type V2Still, stills } from "@/lib/media";
import { cn, grouped } from "@/lib/utils";

const byId = (id: string): V2Still => {
  const found = stills.stills.find((still) => still.id === id);
  if (!found) throw new Error(`lib/stills.json has no still ${id}`);
  return found;
};

/** The two presets, side by side, each with a button that plays its film in the hero. */
export function BundlePanels() {
  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-2">
      {v2Bundles.map((bundle, at) => {
        const still = byId(`${bundle.id}-treatment`);
        return (
          <Reveal key={bundle.id} delay={at * 0.08}>
            <article className="panel relative flex h-full flex-col overflow-hidden rounded-[1.75rem]">
              <div className="relative aspect-[16/9] overflow-hidden bg-black">
                {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
                <img
                  src={still.src}
                  width={still.width}
                  height={still.height}
                  loading="lazy"
                  decoding="async"
                  alt={`The ${bundle.name} preset at tick ${grouped(still.tick)}, seed ${still.seed}: ${grouped(still.live)} organisms, the whole torus.`}
                  className="h-full w-full object-cover"
                />
                <Reticle inset={10} size={14} />
                <span className="absolute bottom-3 left-3 rounded-full border border-white/10 bg-black/60 px-2.5 py-1 font-mono text-[10px] tracking-[0.12em] text-green-200 uppercase backdrop-blur-md">
                  tick {grouped(still.tick)} · {grouped(still.live)} alive
                </span>
              </div>
              <div className="flex flex-1 flex-col p-6 sm:p-7">
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <h4 className="text-xl font-bold text-white">{bundle.name}</h4>
                  <code className="text-[11px] text-green-300/80">{bundle.preset}</code>
                </div>
                <p className="mt-3 text-[15px] leading-relaxed text-slate-300">{bundle.picture}</p>
                <dl className="mt-4 space-y-1.5 text-[13px] text-slate-400">
                  <div className="flex gap-2">
                    <dt className="w-20 shrink-0 text-slate-500">Founders</dt>
                    <dd>
                      {grouped(bundle.founders)}, {bundle.features} v2 mechanisms on
                    </dd>
                  </div>
                  <div className="flex gap-2">
                    <dt className="w-20 shrink-0 text-slate-500">Brain</dt>
                    <dd>{bundle.abi}</dd>
                  </div>
                  <div className="flex gap-2">
                    <dt className="w-20 shrink-0 text-slate-500">Bodies</dt>
                    <dd>{bundle.bodies}</dd>
                  </div>
                </dl>
                <dl className="mt-5 grid grid-cols-3 gap-3 border-t border-white/[0.06] pt-5">
                  {bundle.measured.map((stat) => (
                    <div key={stat.label} className="min-w-0">
                      <dt className="text-[11px] leading-tight text-slate-500">{stat.label}</dt>
                      <dd className="mt-1 text-xl font-extrabold tracking-tight text-white tabular-nums">{stat.value}</dd>
                      <dd className="mt-0.5 text-[11px] leading-snug text-slate-500">{stat.note}</dd>
                    </div>
                  ))}
                </dl>
                <div className="mt-6">
                  <WatchFilm id={bundle.film} label={`Watch the ${bundle.id} film`} />
                </div>
              </div>
            </article>
          </Reveal>
        );
      })}
    </div>
  );
}

function StillFigure({ still, className, sizes }: { still: V2Still; className?: string; sizes?: string }) {
  return (
    <figure className={cn("panel relative overflow-hidden rounded-[1.5rem]", className)}>
      <a href={still.src} className="relative block aspect-square bg-black" aria-label={`${still.label}: open the ${still.width} × ${still.height} still`}>
        {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
        <img
          src={still.src}
          width={still.width}
          height={still.height}
          sizes={sizes}
          loading="lazy"
          decoding="async"
          alt={`${still.label}: ${still.caption}`}
          className="h-full w-full object-cover"
        />
        <Reticle inset={10} size={14} />
      </a>
      <figcaption className="p-5">
        <div className="flex flex-wrap items-baseline justify-between gap-2">
          <span className="text-base font-bold text-white">{still.label}</span>
          <span className="font-mono text-[10.5px] text-slate-400">
            {grouped(still.live)} alive{still.view.zoom > 1.01 ? ` · ×${still.view.zoom}` : ""}
          </span>
        </div>
        <p className="mt-2 text-[13.5px] leading-relaxed text-slate-400">{still.caption}</p>
      </figcaption>
    </figure>
  );
}

/** The four read-only v2 views. */
export function ViewStills() {
  const views = stills.stills.filter((still) => still.set === "v2-views");
  return (
    <div className="grid grid-cols-1 gap-5 sm:grid-cols-2">
      {views.map((still, at) => (
        <Reveal key={still.id} delay={(at % 2) * 0.06}>
          <StillFigure still={still} />
        </Reveal>
      ))}
    </div>
  );
}

/** Each preset beside its two controls at the same tick and seed. */
export function ControlStills() {
  return (
    <div className="space-y-8">
      {(["tissue", "supercolony"] as const).map((bundle) => {
        const row = stills.stills.filter((still) => still.set === "v2-controls" && still.bundle === bundle);
        return (
          <div key={bundle}>
            <MicroLabel className="text-slate-400">{bundle === "tissue" ? "Tissue (Bundle A)" : "Supercolony (Bundle B)"}</MicroLabel>
            <div className="mt-3 grid grid-cols-1 gap-4 md:grid-cols-3">
              {row.map((still, at) => (
                <Reveal key={still.id} delay={at * 0.06}>
                  <StillFigure still={still} sizes="(min-width: 768px) 33vw, 100vw" />
                </Reveal>
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}

const KIND: Record<AblationRow["kind"], string> = {
  treatment: "bg-green-400",
  control: "bg-slate-400/70",
  minus: "bg-cyan-400/70",
  plus: "bg-amber-300/80",
};

/** Mean live count per Stage-2 condition, on one linear axis per preset. */
export function AblationBars() {
  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-2">
      {v2Ablations.map((table) => {
        const max = Math.max(...table.rows.map((row) => row.live));
        return (
          <Reveal key={table.bundle}>
            <figure className="panel h-full rounded-[1.75rem] p-6 sm:p-7">
              <figcaption className="flex flex-wrap items-baseline justify-between gap-2">
                <span className="text-base font-bold text-white">{table.bundle}</span>
                <span className="font-mono text-[10.5px] text-slate-500">mean alive at tick 2,048 · 4 seeds</span>
              </figcaption>
              <ul className="mt-5 space-y-2.5">
                {table.rows.map((row) => (
                  <li key={row.condition} className="grid grid-cols-[minmax(0,10.5rem)_minmax(0,1fr)] items-center gap-3 sm:grid-cols-[12rem_minmax(0,1fr)]">
                    <span className={cn("text-[12.5px] leading-snug break-words", row.kind === "treatment" ? "font-bold text-white" : "text-slate-300")}>
                      {row.condition}
                    </span>
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="relative h-2.5 min-w-0 flex-1 rounded-full bg-white/[0.05]">
                        <span
                          className={cn("absolute inset-y-0 left-0 rounded-full", KIND[row.kind])}
                          style={{ width: `${Math.max(0.4, (row.live / max) * 100)}%` }}
                        />
                      </span>
                      <span className="w-14 shrink-0 text-right font-mono text-[11.5px] text-slate-200 tabular-nums">{grouped(row.live)}</span>
                    </span>
                    {row.note ? <span className="col-start-2 -mt-1.5 text-[11.5px] leading-snug text-slate-500">{row.note}</span> : null}
                  </li>
                ))}
              </ul>
              <div className="mt-5 flex flex-wrap gap-x-4 gap-y-1.5 border-t border-white/[0.06] pt-4 text-[11px] text-slate-500">
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-3 rounded-full bg-green-400" /> as committed
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-3 rounded-full bg-slate-400/70" /> control
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-3 rounded-full bg-cyan-400/70" /> one mechanism removed
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-3 rounded-full bg-amber-300/80" /> added
                </span>
              </div>
            </figure>
          </Reveal>
        );
      })}
    </div>
  );
}

/** What the v2 report says is not shown, or costs. */
export function Residuals() {
  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
      {v2Residuals.map((item, at) => (
        <Reveal key={item.title} delay={(at % 3) * 0.05}>
          <div className="h-full rounded-[1.5rem] border border-amber-300/15 bg-amber-300/[0.03] p-5 sm:p-6">
            <h4 className="text-base font-bold text-amber-100">{item.title}</h4>
            <p className="mt-2 text-[14px] leading-relaxed text-slate-400">{item.body}</p>
          </div>
        </Reveal>
      ))}
    </div>
  );
}
