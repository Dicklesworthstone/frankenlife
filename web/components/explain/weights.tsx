"use client";

import { type KeyboardEvent, useMemo, useState } from "react";

import { CATALOG, DEFAULT_TOPOLOGY, parameterCount, seeded, shape, widths } from "@/components/explain/brain-math";
import { BasisBadge, DemoFrame, Segmented } from "@/components/explain/demo";
import { LINEAGE } from "@/lib/palette";
import { cn, grouped } from "@/lib/utils";

type Mode = "own" | "shared" | "batched";

/** The drawn thumbnail's rows and columns for each catalog member: bigger networks, bigger pictures. */
const THUMBNAIL: readonly (readonly [number, number])[] = [
  [3, 5],
  [4, 6],
  [6, 6],
  [7, 7],
  [8, 8],
];

/** Eight lineages of six; each lineage keeps its founder's topology here (a picture, not a run). */
const LINEAGE_TOPOLOGY = [2, 2, 1, 2, 3, 2, 0, 4] as const;
const MEMBERS = 6;

type Organism = {
  id: number;
  lineage: number;
  topology: number;
  rows: number;
  cols: number;
  weights: number[];
  /** Cells that differ from the lineage founder's. */
  mutated: boolean[];
};

/** A weight in [−1, 1], roughly bell-shaped. */
function draw(random: () => number): number {
  return (random() + random() + random() - 1.5) / 1.5;
}

function matrix(random: () => number, size: number): number[] {
  return Array.from({ length: size }, () => draw(random));
}

/**
 * The organisms: each lineage's founder has its own random matrix, and each
 * descendant is that matrix with one to three cells mutated. Kin look alike,
 * never identical; unrelated organisms look unrelated. Seeded, so the server
 * and every browser draw the same picture.
 */
function population(): { organisms: Organism[]; policy: number[] } {
  const organisms: Organism[] = [];
  let nextId = LINEAGE_TOPOLOGY.length + 1;
  LINEAGE_TOPOLOGY.forEach((topology, lineage) => {
    const [rows, cols] = THUMBNAIL[topology] ?? [6, 6];
    const size = rows * cols;
    const founder = matrix(seeded(31 + lineage * 977), size);
    for (let member = 0; member < MEMBERS; member++) {
      const random = seeded(7919 * (lineage + 1) + member * 104_729);
      const weights = founder.slice();
      const mutated = Array.from({ length: size }, () => false);
      const changes = member === 0 ? 0 : 1 + Math.floor(random() * 3);
      for (let change = 0; change < changes; change++) {
        // A fresh cell each time: a second, opposite change to one cell could undo the first.
        let cell = Math.floor(random() * size);
        while (mutated[cell]) cell = (cell + 1) % size;
        weights[cell] = Math.max(-1, Math.min(1, (weights[cell] ?? 0) + (random() < 0.5 ? -0.7 : 0.7)));
        mutated[cell] = true;
      }
      organisms.push({
        id: member === 0 ? lineage + 1 : nextId++,
        lineage,
        topology,
        rows,
        cols,
        weights,
        mutated,
      });
    }
  });
  // Mixed together, as lineages are in a world: a fixed shuffle.
  const random = seeded(48);
  for (let index = organisms.length - 1; index > 0; index--) {
    const other = Math.floor(random() * (index + 1));
    const a = organisms[index]!;
    organisms[index] = organisms[other]!;
    organisms[other] = a;
  }
  const [rows, cols] = THUMBNAIL[DEFAULT_TOPOLOGY] ?? [6, 6];
  return { organisms, policy: matrix(seeded(2026), rows * cols) };
}

/** A weight's colour: cyan for negative, amber for positive, brighter for larger. */
function weightFill(value: number): { fill: string; opacity: number } {
  return {
    fill: value < 0 ? "#67e8f9" : "#fbbf24",
    opacity: 0.08 + 0.88 * Math.abs(value) ** 0.8,
  };
}

function Thumbnail({
  rows,
  cols,
  weights,
  mutated,
  outline,
  size = 40,
  label,
}: {
  rows: number;
  cols: number;
  weights: readonly number[];
  mutated?: readonly boolean[];
  outline?: boolean;
  size?: number;
  label?: string;
}) {
  const cell = Math.min(size / cols, size / rows);
  const width = cell * cols;
  const height = cell * rows;
  return (
    <svg
      viewBox={`0 0 ${size} ${size}`}
      className="diagram h-auto w-full"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <rect x={0} y={0} width={size} height={size} rx={4} fill="#000" fillOpacity={0.35} />
      <g transform={`translate(${(size - width) / 2} ${(size - height) / 2})`}>
        {weights.map((value, index) => {
          const { fill, opacity } = weightFill(value);
          const x = (index % cols) * cell;
          const y = Math.floor(index / cols) * cell;
          const changed = outline && mutated?.[index];
          return (
            <rect
              key={index}
              x={x + 0.4}
              y={y + 0.4}
              width={cell - 0.8}
              height={cell - 0.8}
              rx={0.8}
              fill={fill}
              fillOpacity={opacity}
              stroke={changed ? "#ffffff" : undefined}
              strokeWidth={changed ? 1.1 : undefined}
            />
          );
        })}
      </g>
    </svg>
  );
}

function Tile({
  organism,
  mode,
  policy,
  hovered,
  related,
  onHover,
}: {
  organism: Organism;
  mode: Mode;
  policy: readonly number[];
  hovered: boolean;
  related: boolean;
  onHover: (organism: Organism | null) => void;
}) {
  const shared = mode === "shared";
  const [rows, cols] = shared ? (THUMBNAIL[DEFAULT_TOPOLOGY] ?? [6, 6]) : [organism.rows, organism.cols];
  const colour = LINEAGE[organism.lineage % LINEAGE.length] ?? "#ffffff";
  return (
    <div
      onMouseEnter={() => onHover(organism)}
      onMouseLeave={() => onHover(null)}
      className={cn(
        "relative rounded-lg border p-1 transition-[border-color,opacity] duration-300",
        hovered ? "border-white/50" : related ? "border-white/25" : "border-white/[0.06]",
      )}
    >
      <Thumbnail
        rows={rows}
        cols={cols}
        weights={shared ? policy : organism.weights}
        mutated={organism.mutated}
        outline={hovered && mode === "own"}
      />
      <span
        aria-hidden="true"
        className="absolute -top-1 -left-1 h-2.5 w-2.5 rounded-full"
        style={{ background: colour, boxShadow: `0 0 8px ${colour}` }}
      />
    </div>
  );
}

const MODES = [
  { value: "own", label: "FrankenLife: each its own", tone: "rule" },
  { value: "shared", label: "Shared policy: one copied", tone: "wrong" },
  { value: "batched", label: "Batched by topology", tone: "rule" },
] as const satisfies readonly { value: Mode; label: string; tone: "rule" | "wrong" }[];

const EXPLAIN: Record<Mode, string> = {
  own: "Every organism carries its own parameters, inherited from its parent with a few mutations and shared with no one. Kin look alike but are never identical, and unrelated organisms look different. Selection acts on exactly those differences.",
  shared:
    "What FrankenLife refuses: one network copied into every body. It is cheap, since the weights are read once and reused for everyone, and it turns hundreds of thousands of brains into a single policy with nothing for selection to choose between. Plan §6.5: replacing each organism's W with one common W would fundamentally change the project.",
  batched:
    "How it actually runs. Organisms with the same topology share a kernel and a memory layout, so one dispatch evaluates a whole batch: Y_b = X_b W_b + b_b, where W_b changes with every organism b. The batch shares a code path but no parameters (INV-13), and a rare topology gets a correct fallback, never a different controller.",
};

/** Every organism has its own weights: the picture, with the one way it is not done and the way it is. */
export function IndependentWeights() {
  const [mode, setMode] = useState<Mode>("own");
  const [hovered, setHovered] = useState<Organism | null>(null);
  const { organisms, policy } = useMemo(population, []);

  const batches = useMemo(
    () =>
      CATALOG.map((topology, index) => ({
        index,
        topology,
        members: organisms.filter((organism) => organism.topology === index),
      })).filter((batch) => batch.members.length > 0),
    [organisms],
  );

  const tile = (organism: Organism) => (
    <Tile
      key={organism.id}
      organism={organism}
      mode={mode}
      policy={policy}
      hovered={hovered?.id === organism.id}
      related={mode === "own" && hovered !== null && hovered.lineage === organism.lineage}
      onHover={setHovered}
    />
  );

  // The parent, a child and an unrelated founder, for the heredity panel.
  const parent = organisms.find((organism) => organism.lineage === 0 && organism.id === 1)!;
  const child = organisms.find((organism) => organism.lineage === 0 && organism.id !== 1)!;
  const stranger = organisms.find((organism) => organism.lineage === 1 && organism.id === 2)!;

  const info = hovered
    ? `Organism ${hovered.id} · lineage ${hovered.lineage + 1} · ${
        mode === "shared" ? shape(CATALOG[DEFAULT_TOPOLOGY]!) : shape(CATALOG[hovered.topology]!)
      } · ${grouped(parameterCount(widths(CATALOG[mode === "shared" ? DEFAULT_TOPOLOGY : hovered.topology]!)))} parameters${
        mode === "own" && hovered.mutated.some(Boolean) ? " · outlined: where it differs from its lineage's founder" : ""
      }${mode === "own" && !hovered.mutated.some(Boolean) ? " · a founder" : ""}`
    : "Point at or tap an organism, or focus the grid and use the arrow keys, to see its lineage and network; its kin light up.";

  // The keyboard's way through the grid, in reading order (batch by batch when batched).
  const order = mode === "batched" ? batches.flatMap((batch) => batch.members) : organisms;
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = hovered ? order.findIndex((organism) => organism.id === hovered.id) : -1;
    const steps: Record<string, number> = {
      ArrowRight: (at + 1) % order.length,
      ArrowDown: (at + 1) % order.length,
      ArrowLeft: (at <= 0 ? order.length : at) - 1,
      ArrowUp: (at <= 0 ? order.length : at) - 1,
      Home: 0,
      End: order.length - 1,
    };
    const next = steps[event.key];
    if (next !== undefined) {
      event.preventDefault();
      setHovered(order[next] ?? null);
    } else if (event.key === "Escape" && hovered) {
      event.preventDefault();
      setHovered(null);
    }
  };

  return (
    <DemoFrame
      eyebrow="Independent controllers · plan §6.5, INV-13"
      title="Same shape, never the same weights."
      basis="diagram"
      source={
        <>
          The thumbnails are illustrative weights drawn from a fixed seed, not genomes from a run; each lineage keeps
          one topology here for clarity. The rule: plan §6.5 and INV-13; the batching: crates/flife-backend-metal and
          crates/flife-backend-cpu (per-topology parameter shards, FL-026). The heredity figures: README.md, the
          reference preset&apos;s 1,024-tick smoke run.
        </>
      }
    >
      <div className="space-y-5">
        <Segmented label="How the controllers are held" options={MODES} value={mode} onChange={setMode} />
        <p className="min-h-[4.5rem] text-sm leading-relaxed text-slate-300">{EXPLAIN[mode]}</p>

        {/* Focusable, so the keyboard reaches what pointing shows: the readout under it says each organism aloud. */}
        <div
          role="group"
          tabIndex={0}
          onKeyDown={onKeyDown}
          aria-label={`${
            mode === "shared"
              ? "48 organisms in eight lineage colours, every one drawn with the same weight matrix."
              : mode === "batched"
                ? `48 organisms grouped into ${batches.length} batches by network topology; within each batch every weight matrix differs.`
                : "48 organisms in eight lineage colours, each with its own weight matrix; organisms of one lineage have nearly the same matrix."
          } Arrow keys step from organism to organism; Escape clears.`}
          className="rounded-lg focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
        >
          {mode === "batched" ? (
            <div className="space-y-3">
              {batches.map((batch) => (
                <div key={batch.index} className="rounded-xl border border-green-500/20 bg-green-500/[0.03] p-2.5">
                  <p className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 px-1 pb-2">
                    <span className="font-mono text-xs text-green-200">{shape(batch.topology)}</span>
                    <span className="text-[11px] text-slate-400">
                      {batch.members.length} organisms · one kernel, one layout ·{" "}
                      {grouped(parameterCount(widths(batch.topology)))} parameters each, none shared
                    </span>
                  </p>
                  <div className="grid grid-cols-6 gap-1.5 sm:grid-cols-8 lg:grid-cols-12">
                    {batch.members.map(tile)}
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div className="grid grid-cols-6 gap-1.5 sm:grid-cols-8 lg:grid-cols-12">{organisms.map(tile)}</div>
          )}
        </div>
        <p className="min-h-[1.25rem] font-mono text-[11px] text-slate-500" aria-live="polite">
          {info}
        </p>

        <div className="rounded-2xl border border-white/10 bg-black/30 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">Inheritance</p>
            {/* The thumbnails are drawn, like the grid above; only the figures under them are the record's. */}
            <BasisBadge basis="diagram" />
          </div>
          <div className="mt-3 grid grid-cols-[repeat(3,minmax(0,5.5rem))] gap-4">
            {[
              { organism: parent, label: "Parent", outline: false },
              { organism: child, label: "Its child", outline: true },
              { organism: stranger, label: "Unrelated", outline: false },
            ].map(({ organism, label, outline }) => (
              <div key={label}>
                <Thumbnail
                  rows={organism.rows}
                  cols={organism.cols}
                  weights={organism.weights}
                  mutated={organism.mutated}
                  outline={outline}
                  size={48}
                  label={`${label}'s weights${outline ? ", with its mutated entries outlined" : ""}`}
                />
                <p className="mt-1.5 text-center text-[11px] text-slate-400">{label}</p>
              </div>
            ))}
          </div>
          <BasisBadge basis="record" className="mt-4" />
          <p className="mt-2 text-sm leading-relaxed text-slate-300">
            In the reference world&apos;s 1,024-tick smoke run, live children keep{" "}
            <span className="font-mono text-green-300">99.8%</span> of their parent&apos;s parameters bit for bit,
            while unrelated founders share <span className="font-mono text-slate-200">1.3%</span>. Grandchildren carry
            99.7% of their parent&apos;s mutations. Weights start random and change only by inherited mutation: nothing
            is trained by gradient.
          </p>
        </div>
      </div>
    </DemoFrame>
  );
}
