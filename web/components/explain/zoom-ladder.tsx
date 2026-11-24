"use client";

import { ChevronLeft, ChevronRight, MousePointer2 } from "lucide-react";
import {
  type KeyboardEvent,
  memo,
  type PointerEvent,
  type ReactNode,
  type Ref,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import {
  CATALOG,
  DEFAULT_TOPOLOGY,
  INPUT_GROUPS,
  OUTPUT_GROUPS,
  parameterCount,
  parameterSum,
  seeded,
  widths,
} from "@/components/explain/brain-math";
import { type Basis, BasisBadge } from "@/components/explain/demo";
import {
  BODY,
  cardPixelToWorld,
  cardPlacement,
  FIELD_CELLS,
  inView,
  layerSizes,
  PICK,
  plateau,
  posterPlacement,
  ramp,
  roughly,
  scaleBar,
  smoothstep,
  stopViews,
  TORUS,
  viewAt,
} from "@/components/explain/zoom-ladder-geometry";
import { GreenTitle, MicroLabel } from "@/components/franken";
import { useInView, useReducedMotion } from "@/lib/hooks";
import { filmById } from "@/lib/media";

// The confluent film: the ladder is cut from its poster and share card.
const film = filmById("confluent");
import { LINEAGE } from "@/lib/palette";
import { cn, grouped } from "@/lib/utils";

// ─── The figures, all from lib/films/confluent.json, the preset and the plan ──

const SHAPE = widths(CATALOG[DEFAULT_TOPOLOGY] ?? { hidden: [64, 32] });
const PARAMETERS = parameterCount(SHAPE);
const LAYERS = layerSizes(SHAPE);
const POSTER = posterPlacement(film);
const CARD = cardPlacement(film);
const VIEWS = stopViews(film);
const [OX, OY] = cardPixelToWorld(CARD, PICK.x, PICK.y);
const LIVE_POSTER = film.world.live[film.poster.frame] ?? 0;
const LIVE_CARD = film.world.live[film.social.frame] ?? 0;
const seconds = (ticks: number) => (ticks * film.world.timestep[0]) / film.world.timestep[1];
const CARD_VIEW = VIEWS[1]?.span ?? 14;
const NEAR_VIEW = VIEWS[2]?.span ?? 3;
/** presets/confluent-world.toml [mutation] weight_probability: a child's expected changed weights. */
const WEIGHT_PROBABILITY = 0.002;
const CHANGED = Math.round(WEIGHT_PROBABILITY * PARAMETERS);
const fmt = (value: number, digits = 1) => value.toFixed(digits).replace(/\.0+$/, "");

/** The group colours brain.tsx uses: what an organism reads and writes in the same quantity share a colour. */
const COLOUR: Record<string, string> = {
  q: "#fde68a",
  z: "#67e8f9",
  dx: "#a78bfa",
  dy: "#f0abfc",
  self: "#4ade80",
  lambda: "#fb923c",
  beta: "#fbbf24",
  gate: "#f472b6",
  direction: "#67e8f9",
  spend: "#fde68a",
};

/** The picked organism's lineage colour in the film (lime), for the rings drawn around it. */
const PICKED = LINEAGE[5];

type Figure = { label: string; value: string; note: string };
type Stop = {
  rail: string;
  eyebrow: string;
  title: string;
  body: ReactNode;
  figures: Figure[];
  basis: Basis;
  /** The stage's caption once the picture is a diagram. */
  hud?: string;
};

const STOPS: Stop[] = [
  {
    rail: "World",
    eyebrow: "The whole torus",
    title: `[${grouped(LIVE_POSTER)}] organisms, one world.`,
    body: (
      <>
        The film&apos;s first frame: the whole {TORUS} × {TORUS}-unit torus at tick {grouped(POSTER.tick)}, after{" "}
        {fmt(seconds(POSTER.tick), 0)} simulated seconds of evolution. Every speck is one organism, with a body, a
        metabolism and its own neural network. <code className="text-lime-200">flife render</code> drew it from the
        committed world.
      </>
    ),
    figures: [
      { label: "Alive", value: grouped(LIVE_POSTER), note: `the render's count for frame ${film.poster.frame}` },
      { label: "View", value: `${TORUS} × ${TORUS} units`, note: "the whole torus; every edge wraps" },
      { label: "Founders", value: grouped(film.world.founders), note: `each lineage wears one of ${LINEAGE.length} colours` },
      { label: "Field", value: `${film.world.grid[0]} × ${film.world.grid[1]}`, note: "cells of energy and 32 chemical channels" },
    ],
    basis: "render",
  },
  {
    rail: "Tissue",
    eyebrow: "A neighbourhood",
    title: "Lineages become [tissue].",
    body: (
      <>
        A {fmt(CARD_VIEW)} × {fmt(CARD_VIEW)}-unit patch of the film&apos;s closest shot, at the same place on the
        torus {grouped(CARD.tick - POSTER.tick)} ticks ({fmt(seconds(CARD.tick - POSTER.tick), 0)} simulated seconds)
        later. Each founder&apos;s lineage wears one of {LINEAGE.length} colours, lighter where an organism holds more energy. The labyrinths,
        rims and foam come from what the networks choose to emit: in the record&apos;s control without emission, this
        world grows a uniform haze instead.
      </>
    ),
    figures: [
      {
        label: "In view",
        value: `≈ ${grouped(roughly(inView(LIVE_CARD, CARD_VIEW)))}`,
        note: `derived: ${grouped(LIVE_CARD)} alive over ${grouped(TORUS * TORUS)} square units, times ${fmt(CARD_VIEW * CARD_VIEW, 0)}`,
      },
      { label: "View", value: `${fmt(CARD_VIEW)} × ${fmt(CARD_VIEW)} units`, note: "the middle of the social card's crop" },
      { label: "Tick", value: grouped(CARD.tick), note: `frame ${film.social.frame} of 0–${film.video.frames - 1}` },
      { label: "Camera", value: `zoom ${fmt(CARD.zoom, 2)}`, note: `flife render's record for frame ${film.social.frame}` },
    ],
    basis: "render",
  },
  {
    rail: "Organism",
    eyebrow: "One organism",
    title: "One body, one [field cell] wide.",
    body: (
      <>
        Each sphere is one organism, drawn at its body&apos;s size: {BODY} units across, the width of one cell of the{" "}
        {FIELD_CELLS} × {FIELD_CELLS} field it feeds from and writes into. Overlapping bodies push apart, and near
        neighbours pull together as alike as their chemistry is; food and signals pass only through the field.
      </>
    ),
    figures: [
      { label: "Body", value: `${BODY} units`, note: "the preset's [contact] diameter" },
      { label: "Field cell", value: `${TORUS / FIELD_CELLS} units`, note: `${TORUS} units ÷ ${FIELD_CELLS} cells` },
      { label: "This one", value: `(${fmt(OX)}, ${fmt(OY)})`, note: "picked on the card: a bright body with space around it" },
      {
        label: "In view",
        value: `≈ ${grouped(roughly(inView(LIVE_CARD, NEAR_VIEW)))}`,
        note: `derived the same way, for ${fmt(NEAR_VIEW)} × ${fmt(NEAR_VIEW)} units`,
      },
    ],
    basis: "render",
  },
  {
    rail: "Network",
    eyebrow: "Its network",
    title: "Its own network, [every tick].",
    body: (
      <>
        The founders&apos; controller: {SHAPE[0]} numbers in, from the field around it and from itself; hidden layers of{" "}
        {SHAPE[1]} and {SHAPE[2]} tanh units; {SHAPE[3]} numbers out: when to try to split, the child&apos;s share,
        whether to emit, which chemistry and how much. A birth moves a child to another of the catalog&apos;s five
        shapes with probability 0.01 in this world, so this one&apos;s may differ; every shape is its own.
      </>
    ),
    figures: [
      { label: "In", value: String(SHAPE[0]), note: "energy, 32 chemistry channels, their x and y gradients, itself" },
      { label: "Out", value: String(SHAPE.at(-1)), note: "split rate, child share, emission gate, direction ×32, spending" },
      { label: "Evaluated", value: "every tick", note: "[controller] evaluation_interval_ticks = 1" },
      { label: "Stored as", value: "f16", note: "[controller] canonical_weight_dtype" },
    ],
    basis: "diagram",
    // No-break spaces keep the shape on one line, and the dot on the first, when a phone wraps the caption.
    hud: `one organism's controller\u00a0· ${SHAPE.join("\u00a0→\u00a0")}`,
  },
  {
    rail: "Numbers",
    eyebrow: "Its numbers",
    title: `[${grouped(PARAMETERS)}] numbers of its own.`,
    body: (
      <>
        In the founders&apos; shape, {parameterSum(SHAPE).replace(/ \+ /g, " + ")} = {grouped(PARAMETERS)} parameters. No other
        organism shares them, and nothing trains them: there is no gradient. A child inherits a copy in which each weight changes with
        probability {WEIGHT_PROBABILITY}, so about {CHANGED} of the {grouped(PARAMETERS)} differ, and the world keeps
        whichever copies survive.
      </>
    ),
    figures: [
      { label: "Parameters", value: grouped(PARAMETERS), note: `${LAYERS.map((size) => grouped(size)).join(" + ")}, layer by layer, in the founders' shape` },
      { label: "Shared with", value: "no one", note: "each organism carries its own (INV-13)" },
      { label: "A child differs in", value: `≈ ${CHANGED}`, note: `derived: ${WEIGHT_PROBABILITY} × ${grouped(PARAMETERS)} ([mutation])` },
      { label: "Training", value: "none", note: "[controller] autograd = false" },
    ],
    basis: "diagram",
    hud: `${grouped(PARAMETERS)} parameters · illustrative values · ≈ ${CHANGED} differ in a child`,
  },
];

const LAST = STOPS.length - 1;

// ─── Modes ────────────────────────────────────────────────────────────────────

/** Wide screens with a fine pointer that allow motion zoom as the page scrolls; everything else steps. */
const SCROLLY_QUERY = "(min-width: 1024px) and (pointer: fine) and (prefers-reduced-motion: no-preference)";

function useMedia(query: string): boolean {
  // Stable per query: a new `subscribe` on each render would have React unsubscribe and resubscribe every time.
  const subscribe = useCallback(
    (notify: () => void) => {
      const list = window.matchMedia(query);
      list.addEventListener("change", notify);
      return () => list.removeEventListener("change", notify);
    },
    [query],
  );
  const snapshot = useCallback(() => window.matchMedia(query).matches, [query]);
  return useSyncExternalStore(subscribe, snapshot, () => false);
}

// ─── Where each layer is at progress p ────────────────────────────────────────

/** A picture placed on the torus, as a transform of an element sized to the stage's width. */
function placement(
  picture: { x0: number; y0: number; width: number; height: number },
  view: { cx: number; cy: number; span: number },
): string {
  const left = view.cx - view.span / 2;
  const top = view.cy - view.span / 2;
  // The element is the stage's width; its height keeps the picture's aspect. Percent translations are of the element.
  const aspect = picture.height / picture.width;
  const x = ((picture.x0 - left) / view.span) * 100;
  const y = ((picture.y0 - top) / view.span / aspect) * 100;
  return `translate3d(${css(x)}%, ${css(y)}%, 0) scale(${css(picture.width / view.span)})`;
}

/** At most six significant digits, as a browser serializes a style: the server's HTML and the hydrated style then agree. */
function css(value: number): number {
  return Math.abs(value) < 1e-6 ? 0 : Number(value.toPrecision(6));
}

type Frame = {
  view: { cx: number; cy: number; span: number };
  poster: number;
  card: number;
  reticle: number;
  network: number;
  networkScale: number;
  mosaic: number;
  mosaicScale: number;
  changed: number;
  real: number;
  diagram: number;
};

function frameAt(p: number): Frame {
  const view = viewAt(VIEWS, Math.min(p, VIEWS.length - 1));
  const toCard = smoothstep(ramp(p, 0.55, 0.95));
  const leaveCard = smoothstep(ramp(p, 2.3, 2.75));
  return {
    view,
    poster: 1 - toCard,
    card: toCard * (1 - leaveCard),
    reticle: smoothstep(ramp(p, 1.35, 1.8)) * (1 - smoothstep(ramp(p, 2.15, 2.5))),
    network: smoothstep(ramp(p, 2.4, 2.85)) * (1 - smoothstep(ramp(p, 3.3, 3.65))),
    networkScale: 0.22 + 0.78 * smoothstep(ramp(p, 2.3, 3)) + 0.14 * smoothstep(ramp(p, 3.3, 3.8)),
    mosaic: smoothstep(ramp(p, 3.35, 3.8)),
    mosaicScale: 0.86 + 0.14 * smoothstep(ramp(p, 3.35, 4)),
    changed: smoothstep(ramp(p, 3.8, 4)),
    real: 1 - smoothstep(ramp(p, 2.3, 2.65)),
    diagram: smoothstep(ramp(p, 2.5, 2.85)),
  };
}

const INITIAL = frameAt(0);

// ─── The network, drawn once ─────────────────────────────────────────────────

type Dot = { x: number; y: number; colour: string; r: number };
type Edge = { x1: number; y1: number; x2: number; y2: number; w: number };
type Label = { x: number; y: number; text: string; colour: string; anchor: "start" | "end" };

const NET = { top: 66, bottom: 560, columns: [150, 280, 395, 470] } as const;

function column(
  x: number,
  groups: readonly { id: string; size: number; short: string }[] | null,
  size: number,
  r: number,
  gap: number,
): { dots: Dot[]; labels: { y: number; text: string; colour: string }[] } {
  const parts = groups ?? [{ id: "hidden", size, short: "" }];
  const span = NET.bottom - NET.top - gap * (parts.length - 1);
  const step = span / Math.max(1, size - 1);
  const dots: Dot[] = [];
  const labels: { y: number; text: string; colour: string }[] = [];
  let y = NET.top;
  parts.forEach((group, index) => {
    const start = y;
    for (let k = 0; k < group.size; k++) {
      dots.push({ x, y, r, colour: COLOUR[group.id] ?? "#cbd5e1" });
      if (k < group.size - 1) y += step;
    }
    labels.push({ y: (start + y) / 2, text: group.short, colour: COLOUR[group.id] ?? "#cbd5e1" });
    if (index < parts.length - 1) y += step + gap;
  });
  return { dots, labels };
}

function network(): { dots: Dot[]; edges: Edge[]; labels: Label[] } {
  const [x0, x1, x2, x3] = NET.columns;
  const inputs = column(x0, INPUT_GROUPS, SHAPE[0] ?? 104, 1.9, 16);
  const first = column(x1, null, SHAPE[1] ?? 64, 2.6, 0);
  const second = column(x2, null, SHAPE[2] ?? 32, 3.3, 0);
  const outputs = column(x3, OUTPUT_GROUPS, SHAPE[3] ?? 36, 3.3, 16);
  const layers = [inputs.dots, first.dots, second.dots, outputs.dots];
  const random = seeded(9988);
  const edges: Edge[] = [];
  const perLayer = [420, 320, 240];
  for (let layer = 0; layer < 3; layer++) {
    const from = layers[layer] ?? [];
    const to = layers[layer + 1] ?? [];
    for (let k = 0; k < (perLayer[layer] ?? 0); k++) {
      const a = from[Math.floor(random() * from.length)];
      const b = to[Math.floor(random() * to.length)];
      if (a && b) edges.push({ x1: a.x, y1: a.y, x2: b.x, y2: b.y, w: (random() + random() + random() - 1.5) / 1.5 });
    }
  }
  const labels: Label[] = [
    ...inputs.labels.map((label) => ({ x: x0 - 14, y: label.y, text: label.text, colour: label.colour, anchor: "end" as const })),
    ...outputs.labels.map((label) => ({ x: x3 + 14, y: label.y, text: label.text, colour: label.colour, anchor: "start" as const })),
  ];
  return { dots: layers.flat(), edges, labels };
}

// ─── The numbers, drawn once on a canvas ────────────────────────────────────

/** The mosaic's geometry: each layer's weight matrix as it is (rows: units it feeds; columns: its inputs) plus its bias column. */
const CELL = 8;
const MOSAIC_W = 860;
const MOSAIC_H = 880;
type Block = { x: number; y: number; rows: number; cols: number; label: string; seed: number };

function blocks(): Block[] {
  const [i, h1, h2, o] = [SHAPE[0] ?? 104, SHAPE[1] ?? 64, SHAPE[2] ?? 32, SHAPE[3] ?? 36];
  const top = 14;
  const lower = top + h1 * CELL + 30;
  const second = { x: 4, y: lower, rows: h2, cols: h1 + 1, label: `layer 2 · ${h2} × ${h1} + ${h2}`, seed: 2 };
  return [
    { x: 4, y: top, rows: h1, cols: i + 1, label: `layer 1 · ${h1} × ${i} + ${h1}`, seed: 1 },
    second,
    { x: 4 + (second.cols + 3) * CELL, y: lower, rows: o, cols: h2 + 1, label: `layer 3 · ${o} × ${h2} + ${o}`, seed: 3 },
  ];
}

/** A weight in [−1, 1], bell-shaped; a picture of a weight, never an organism's genome. */
function weightAt(random: () => number): number {
  return (random() + random() + random() - 1.5) / 1.5;
}

function drawMosaic(canvas: HTMLCanvasElement) {
  const context = canvas.getContext("2d");
  if (!context) return;
  context.clearRect(0, 0, MOSAIC_W, MOSAIC_H);
  // The input groups along layer 1's columns, the output groups along layer 3's rows.
  let x = 4;
  for (const group of INPUT_GROUPS) {
    context.fillStyle = COLOUR[group.id] ?? "#cbd5e1";
    context.globalAlpha = 0.85;
    context.fillRect(x, 2, group.size * CELL - 1, 5);
    x += group.size * CELL;
  }
  for (const block of blocks()) {
    const random = seeded(4242 + block.seed * 7919);
    for (let row = 0; row < block.rows; row++) {
      for (let col = 0; col < block.cols; col++) {
        const bias = col === block.cols - 1;
        const value = weightAt(random) * (bias ? 0.6 : 1);
        context.globalAlpha = 0.1 + 0.88 * Math.abs(value) ** 0.8;
        context.fillStyle = value < 0 ? "#67e8f9" : "#fbbf24";
        context.fillRect(block.x + col * CELL + (bias ? 2 : 0), block.y + row * CELL, CELL - 1, CELL - 1);
      }
    }
  }
  const third = blocks()[2];
  if (third) {
    let y = third.y;
    for (const group of OUTPUT_GROUPS) {
      context.fillStyle = COLOUR[group.id] ?? "#cbd5e1";
      context.globalAlpha = 0.85;
      context.fillRect(third.x + third.cols * CELL + 6, y, 5, group.size * CELL - 1);
      y += group.size * CELL;
    }
  }
  context.globalAlpha = 1;
}

/** About `CHANGED` cells a child's copy would change, spread over the parameters (seeded). */
function changedCells(): { x: number; y: number }[] {
  const all = blocks();
  const random = seeded(20_2611);
  const cells: { x: number; y: number }[] = [];
  const total = all.reduce((sum, block) => sum + block.rows * block.cols, 0);
  for (let k = 0; k < CHANGED; k++) {
    let index = Math.floor(random() * total);
    for (const block of all) {
      const size = block.rows * block.cols;
      if (index < size) {
        const col = index % block.cols;
        const row = Math.floor(index / block.cols);
        cells.push({ x: block.x + col * CELL + (col === block.cols - 1 ? 2 : 0), y: block.y + row * CELL });
        break;
      }
      index -= size;
    }
  }
  return cells;
}

/** The network of one organism: 1,239 elements, the same at every step, so it is drawn once and never reconciled. */
const Network = memo(function Network() {
  const net = useMemo(network, []);
  return (
    <svg viewBox="0 0 600 620" className="h-full w-full">
      <defs>
        <radialGradient id="zoom-ladder-body" cx="0.5" cy="0.5" r="0.5">
          <stop offset="0" stopColor={PICKED} stopOpacity="0.16" />
          <stop offset="0.7" stopColor={PICKED} stopOpacity="0.06" />
          <stop offset="1" stopColor={PICKED} stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle cx="300" cy="312" r="292" fill="url(#zoom-ladder-body)" />
      <circle cx="300" cy="312" r="288" fill="none" stroke={PICKED} strokeOpacity="0.35" strokeWidth="1.5" />
      <g strokeWidth="0.7">
        {net.edges.map((edge, index) => (
          <line
            key={index}
            x1={edge.x1}
            y1={edge.y1}
            x2={edge.x2}
            y2={edge.y2}
            stroke={edge.w < 0 ? "#67e8f9" : "#fbbf24"}
            strokeOpacity={0.06 + 0.3 * Math.abs(edge.w)}
          />
        ))}
      </g>
      {net.dots.map((dot, index) => (
        <circle key={index} cx={dot.x} cy={dot.y} r={dot.r} fill={dot.colour} />
      ))}
      {net.labels.map((label) => (
        <text
          key={label.text}
          x={label.x}
          y={label.y + 5}
          textAnchor={label.anchor}
          fontSize="17"
          fill={label.colour}
          className="font-mono @max-md:text-[22px]"
        >
          {label.text}
        </text>
      ))}
      {NET.columns.map((x, index) => (
        <text key={x} x={x} y="42" textAnchor="middle" fontSize="19" fill="#e2e8f0" className="font-mono @max-md:text-[23px]">
          {SHAPE[index]}
        </text>
      ))}
    </svg>
  );
});

/** The cells a child's copy would change; `groupRef` is the group the stage fades in. */
const Changed = memo(function Changed({ groupRef }: { groupRef: Ref<SVGGElement> }) {
  const changed = useMemo(changedCells, []);
  return (
    // One glow for them all: a filter on each cell is a blur on each cell, again at every step of the fade.
    <g ref={groupRef} style={{ opacity: 0, visibility: "hidden", filter: "drop-shadow(0 0 6px rgba(255,255,255,0.9))" }}>
      {changed.map((cell, index) => (
        <rect
          key={index}
          x={cell.x - 2}
          y={cell.y - 2}
          width={CELL + 3}
          height={CELL + 3}
          fill="#ffffff"
          stroke="#ffffff"
          strokeWidth="2"
        />
      ))}
    </g>
  );
});

// ─── The component ────────────────────────────────────────────────────────────

/** `index` is the section's number in its chapter ("01.1"), shown as every section shows its own. */
export function ZoomLadder({ index }: { index?: string }) {
  const reduced = useReducedMotion();
  const scrolly = useMedia(SCROLLY_QUERY);
  const touch = useMedia("(pointer: coarse)");
  const track = useRef<HTMLDivElement>(null);
  const near = useInView(track, "300px");
  const [stop, setStop] = useState(0);
  const progress = useRef(0);
  const stopRef = useRef(0);
  // The badge names the layer most of the picture is, which a crossfade does not switch where the stop does.
  const [shown, setShown] = useState<Basis>(STOPS[0]!.basis);
  const shownRef = useRef<Basis>(STOPS[0]!.basis);
  const tween = useRef(0);

  const posterRef = useRef<HTMLImageElement>(null);
  const cardRef = useRef<HTMLImageElement>(null);
  const reticleRef = useRef<SVGGElement>(null);
  const networkRef = useRef<HTMLDivElement>(null);
  const mosaicRef = useRef<HTMLDivElement>(null);
  const changedRef = useRef<SVGGElement>(null);
  const realRef = useRef<HTMLDivElement>(null);
  const realBottomRef = useRef<HTMLDivElement>(null);
  const diagramRef = useRef<HTMLDivElement>(null);
  const tickRef = useRef<HTMLSpanElement>(null);
  const zoomRef = useRef<HTMLSpanElement>(null);
  const spanRef = useRef<HTMLSpanElement>(null);
  const countRef = useRef<HTMLSpanElement>(null);
  const barRef = useRef<HTMLSpanElement>(null);
  const barLabelRef = useRef<HTMLSpanElement>(null);
  const railRef = useRef<HTMLSpanElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const drawn = useRef(false);

  const mosaicBlocks = useMemo(blocks, []);

  // The network starts hidden: it is mounted once the page has loaded and the browser is idle, or when the ladder comes
  // near, whichever is first.
  const [met, setMet] = useState(false);
  useEffect(() => {
    if (met) return;
    if (near || typeof requestIdleCallback !== "function") {
      setMet(true);
      return;
    }
    let idle = 0;
    const whenIdle = () => {
      idle = requestIdleCallback(() => setMet(true), { timeout: 3000 });
    };
    if (document.readyState === "complete") whenIdle();
    else window.addEventListener("load", whenIdle, { once: true });
    return () => {
      window.removeEventListener("load", whenIdle);
      if (idle) cancelIdleCallback(idle);
    };
  }, [near, met]);

  /** Writes every layer's transform and every readout for progress `p`, without a React render. */
  const apply = useCallback((p: number) => {
    progress.current = p;
    const frame = frameAt(p);
    const { view } = frame;
    /** Writes only a change: a text set again is laid out again. */
    const text = (element: HTMLElement | null, value: string) => {
      if (element && element.textContent !== value) element.textContent = value;
    };
    const set = (element: HTMLElement | SVGElement | null, opacity: number, transform?: string) => {
      if (!element) return;
      element.style.opacity = String(opacity);
      element.style.visibility = opacity <= 0.001 ? "hidden" : "visible";
      if (transform !== undefined) element.style.transform = transform;
    };
    set(posterRef.current, frame.poster, placement(POSTER, view));
    set(cardRef.current, frame.card, placement(CARD, view));
    set(networkRef.current, frame.network, `scale(${frame.networkScale})`);
    set(mosaicRef.current, frame.mosaic, `scale(${frame.mosaicScale})`);
    set(changedRef.current, frame.changed);
    set(realRef.current, frame.real);
    // Gone, the zoom readout gives its room back, so the basis badge beside it keeps to one line on a phone.
    if (realRef.current) realRef.current.style.display = frame.real <= 0.001 ? "none" : "";
    set(realBottomRef.current, frame.real);
    set(diagramRef.current, frame.diagram);

    const reticle = reticleRef.current;
    if (reticle && frame.reticle <= 0.001) {
      set(reticle, 0);
    } else if (reticle) {
      const x = ((OX - (view.cx - view.span / 2)) / view.span) * 1000;
      const y = ((OY - (view.cy - view.span / 2)) / view.span) * 1000;
      const r = Math.max(14, (BODY / 2 / view.span) * 1000 * 2.1);
      reticle.setAttribute("transform", `translate(${x} ${y})`);
      reticle.querySelector("circle")?.setAttribute("r", String(r));
      reticle.querySelectorAll("line").forEach((line, index) => {
        const angle = (index * Math.PI) / 2;
        line.setAttribute("x1", String(Math.cos(angle) * (r + 6)));
        line.setAttribute("y1", String(Math.sin(angle) * (r + 6)));
        line.setAttribute("x2", String(Math.cos(angle) * (r + 26)));
        line.setAttribute("y2", String(Math.sin(angle) * (r + 26)));
      });
      reticle.querySelector("text")?.setAttribute("x", String(r + 34));
      set(reticle, frame.reticle);
    }

    const first = p < 0.75;
    const live = first ? LIVE_POSTER : LIVE_CARD;
    text(
      tickRef.current,
      first
        ? `tick ${grouped(POSTER.tick)} · frame ${film.poster.frame}`
        : `tick ${grouped(CARD.tick)} · frame ${film.social.frame}`,
    );
    text(zoomRef.current, `×${fmt(TORUS / view.span, TORUS / view.span < 10 ? 1 : 0)}`);
    text(spanRef.current, `${fmt(view.span, view.span < 10 ? 1 : 0)} × ${fmt(view.span, view.span < 10 ? 1 : 0)} units`);
    text(
      countRef.current,
      view.span >= TORUS - 1e-6 ? `${grouped(live)} alive` : `≈ ${grouped(roughly(inView(live, view.span)))} in view`,
    );
    const length = scaleBar(view.span, 0.28);
    // In the stage's width (cqw; the stage is the container), which is what `view.span` spans.
    if (barRef.current) barRef.current.style.width = `${(length / view.span) * 100}cqw`;
    text(barLabelRef.current, length === BODY ? `${length} units · one body` : `${length} unit${length === 1 ? "" : "s"}`);
    if (railRef.current) railRef.current.style.transform = `scaleX(${p / LAST})`;

    const basis: Basis = frame.network + frame.mosaic > frame.poster + frame.card ? "diagram" : "render";
    if (basis !== shownRef.current) {
      shownRef.current = basis;
      setShown(basis);
    }

    const nearest = Math.round(p);
    if (nearest !== stopRef.current) {
      stopRef.current = nearest;
      setStop(nearest);
    }
  }, []);

  // Draw the numbers once, when the ladder comes near.
  useEffect(() => {
    if (!near || drawn.current || !canvasRef.current) return;
    drawn.current = true;
    drawMosaic(canvasRef.current);
  }, [near]);

  // Scrolling: the sticky stage zooms with the scroll position, holding at each stop.
  useEffect(() => {
    if (!scrolly || !near) return;
    window.cancelAnimationFrame(tween.current);
    let frame = 0;
    const update = () => {
      frame = 0;
      const element = track.current;
      if (!element) return;
      const rect = element.getBoundingClientRect();
      const total = rect.height - window.innerHeight;
      apply(plateau(total > 0 ? -rect.top / total : 0, STOPS.length));
    };
    const schedule = () => {
      if (!frame) frame = window.requestAnimationFrame(update);
    };
    update();
    window.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", schedule);
    return () => {
      window.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      window.cancelAnimationFrame(frame);
    };
  }, [scrolly, near, apply]);

  // Leaving scroll mode (a narrow window, reduced motion): settle on the nearest stop.
  useEffect(() => {
    if (!scrolly) apply(Math.round(progress.current));
  }, [scrolly, apply]);

  useEffect(() => () => window.cancelAnimationFrame(tween.current), []);

  /** Go to a stop: scroll there in scroll mode; otherwise ease the zoom there (or jump, for reduced motion). */
  const goTo = useCallback(
    (target: number) => {
      const next = Math.max(0, Math.min(LAST, target));
      if (scrolly && track.current) {
        const rect = track.current.getBoundingClientRect();
        const total = rect.height - window.innerHeight;
        window.scrollTo({ top: window.scrollY + rect.top + (next / LAST) * total, behavior: "smooth" });
        return;
      }
      window.cancelAnimationFrame(tween.current);
      const from = progress.current;
      if (reduced || from === next) {
        apply(next);
        return;
      }
      const duration = Math.min(1700, 700 + 380 * Math.abs(next - from));
      const start = performance.now();
      const step = (now: number) => {
        const t = Math.min(1, (now - start) / duration);
        const eased = t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
        apply(from + (next - from) * eased);
        if (t < 1) tween.current = window.requestAnimationFrame(step);
      };
      tween.current = window.requestAnimationFrame(step);
    },
    [scrolly, reduced, apply],
  );

  // The stop's words arrive with a short rise (none for reduced motion).
  const words = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (reduced || !words.current?.animate) return;
    words.current.animate(
      [
        { opacity: 0, transform: "translateY(10px)" },
        { opacity: 1, transform: "none" },
      ],
      { duration: 420, easing: "cubic-bezier(0.19, 1, 0.22, 1)" },
    );
  }, [stop, reduced]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (scrolly) return;
    if (event.key === "ArrowRight") {
      event.preventDefault();
      goTo(stopRef.current + 1);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      goTo(stopRef.current - 1);
    }
  };

  // A horizontal swipe on the stage steps; vertical movement stays the page's (touch-action: pan-y).
  const swipe = useRef<{ x: number; y: number } | null>(null);
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.pointerType !== "mouse") swipe.current = { x: event.clientX, y: event.clientY };
  };
  const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
    const start = swipe.current;
    swipe.current = null;
    if (!start || scrolly) return;
    const dx = event.clientX - start.x;
    const dy = event.clientY - start.y;
    if (Math.abs(dx) > 40 && Math.abs(dx) > 1.5 * Math.abs(dy)) goTo(stopRef.current + (dx < 0 ? 1 : -1));
  };

  const current = STOPS[stop] ?? STOPS[0];
  if (!current) return null;

  return (
    <figure className="relative min-w-0">
      {/* Tall on wide screens with motion, where the scroll drives the zoom; as tall as its content elsewhere. */}
      <div
        ref={track}
        className="relative [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:h-[380vh]"
      >
        <div className="[@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:sticky [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:top-0 [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:flex [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:h-[100svh] [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:items-start [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:pt-[max(5.5rem,calc((100svh-46rem)/2))]">
          <div className="grid w-full grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,26rem)] lg:grid-rows-[auto_1fr] lg:gap-x-12 lg:gap-y-7">
            {/* The heading: first on a phone, beside the stage on wide screens. */}
            <header className="space-y-3 lg:col-start-2 lg:row-start-1 lg:self-start">
              <div className="inline-flex items-center gap-3">
                {index ? (
                  <span className="font-mono text-[11px] font-medium text-[color:var(--chapter,#4ade80)] tabular-nums">{index}</span>
                ) : null}
                <span className="h-px w-6 bg-[color:var(--chapter,#4ade80)]/50" />
                <MicroLabel className="text-slate-400">Powers of ten</MicroLabel>
              </div>
              <h3 className="display-3 text-white">
                <GreenTitle text="From the world to [one brain]." />
              </h3>
              <p className="max-w-md text-[15px] leading-relaxed text-slate-400">
                Five steps from the film&apos;s whole world to the {grouped(PARAMETERS)} numbers inside one of its
                organisms.{" "}
                <span className="text-slate-300">
                  {scrolly
                    ? "Scroll to zoom, or pick a step."
                    : touch
                      ? "Swipe the picture, or pick a step."
                      : "Pick a step, or focus the picture and use the arrow keys."}
                </span>
              </p>
            </header>

            {/* The stage. */}
            <div className="w-full min-w-0 lg:col-start-1 lg:row-span-2 lg:row-start-1 lg:self-start">
              <div
                role="group"
                aria-roledescription="zoom"
                aria-label={`From the world to one brain, step ${stop + 1} of ${STOPS.length}: ${current.eyebrow}. ${scrolly ? "" : "Left and right arrows step."}`}
                tabIndex={0}
                onKeyDown={onKeyDown}
                onPointerDown={onPointerDown}
                onPointerUp={onPointerUp}
                className="@container relative aspect-square w-full touch-pan-y overflow-hidden contain-strict rounded-[1.75rem] border border-white/10 bg-black shadow-[0_0_90px_-30px_rgba(74,222,128,0.35)] select-none focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none lg:mx-auto lg:w-[min(100%,calc(100svh-8.5rem),46rem)]"
              >
                {/* Real pixels: the poster, then the card. Each is the stage's width and placed on the torus by transform. */}
                {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
                <img
                  ref={posterRef}
                  src={film.poster.src}
                  alt={`The confluent film's first frame: the whole torus at tick ${grouped(POSTER.tick)}, ${grouped(LIVE_POSTER)} organisms coloured by founder lineage.`}
                  width={film.video.width}
                  height={film.video.height}
                  decoding="async"
                  draggable={false}
                  className="absolute top-0 left-0 w-full max-w-none origin-top-left will-change-transform"
                  style={{ opacity: INITIAL.poster, transform: placement(POSTER, INITIAL.view) }}
                />
                {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
                <img
                  ref={cardRef}
                  src={film.social.src}
                  alt={`A patch of the confluent film's closest shot at tick ${grouped(CARD.tick)}: single bodies, each a sphere, coloured by lineage.`}
                  width={film.social.width}
                  height={film.social.height}
                  loading="lazy"
                  decoding="async"
                  draggable={false}
                  className="absolute top-0 left-0 w-full max-w-none origin-top-left will-change-transform"
                  style={{ opacity: INITIAL.card, visibility: "hidden", transform: placement(CARD, INITIAL.view) }}
                />

                {/* The reticle on the picked organism. A phone's stage draws these pictures at a third of their size, so
                    their words are set larger there (CSS sizes SVG text in the drawing's own units). */}
                <svg aria-hidden="true" viewBox="0 0 1000 1000" className="pointer-events-none absolute inset-0 h-full w-full">
                  <g ref={reticleRef} style={{ opacity: 0, visibility: "hidden" }}>
                    <circle r="20" fill="none" stroke={PICKED} strokeWidth="2" vectorEffect="non-scaling-stroke" />
                    <line stroke={PICKED} strokeWidth="2" vectorEffect="non-scaling-stroke" />
                    <line stroke={PICKED} strokeWidth="2" vectorEffect="non-scaling-stroke" />
                    <line stroke={PICKED} strokeWidth="2" vectorEffect="non-scaling-stroke" />
                    <line stroke={PICKED} strokeWidth="2" vectorEffect="non-scaling-stroke" />
                    <text y="-8" fill="#ecfccb" fontSize="26" className="font-mono @max-md:text-[34px]" paintOrder="stroke" stroke="#000" strokeWidth="6">
                      one organism
                    </text>
                  </g>
                </svg>

                {/* Its network; on a phone's stage, set lower, so its column heads clear the basis badge. */}
                <div
                  ref={networkRef}
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-x-[5%] top-[7%] bottom-[11%] will-change-transform @max-md:top-[12%]"
                  style={{ opacity: 0, visibility: "hidden", transform: "scale(0.22)" }}
                >
                  {met ? <Network /> : null}
                </div>

                {/* Its numbers. */}
                <div
                  ref={mosaicRef}
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-x-[8%] top-[9%] bottom-[12%] will-change-transform"
                  style={{ opacity: 0, visibility: "hidden", transform: "scale(0.86)" }}
                >
                  <canvas ref={canvasRef} width={MOSAIC_W} height={MOSAIC_H} className="h-full w-full object-contain" />
                  <svg viewBox={`0 0 ${MOSAIC_W} ${MOSAIC_H}`} className="absolute inset-0 h-full w-full">
                    <Changed groupRef={changedRef} />
                    {mosaicBlocks.map((block) => (
                      <text
                        key={block.label}
                        x={block.x}
                        y={block.y + block.rows * CELL + 22}
                        fontSize="19"
                        fill="#94a3b8"
                        className="font-mono max-sm:hidden"
                      >
                        {block.label}
                      </text>
                    ))}
                  </svg>
                </div>

                {/* Vignette and instrument chrome. */}
                <div aria-hidden="true" className="pointer-events-none absolute inset-0 shadow-[inset_0_0_110px_rgba(0,0,0,0.7)]" />
                <Corners />

                <div className="pointer-events-none absolute inset-x-0 top-0 flex items-start justify-between gap-2 p-4 sm:p-6">
                  <BasisBadge basis={shown} className="bg-black/60 backdrop-blur" />
                  <div ref={realRef} className="rounded-lg bg-black/60 px-2 py-1 text-right font-mono text-[11px] leading-tight text-slate-200 backdrop-blur sm:text-xs">
                    <span ref={zoomRef} className="block text-base font-bold text-white sm:text-lg">
                      ×1
                    </span>
                    <span ref={spanRef} className="block whitespace-nowrap text-slate-300">
                      {TORUS} × {TORUS} units
                    </span>
                  </div>
                </div>

                <div className="pointer-events-none absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/85 via-black/40 to-transparent p-4 pt-10 sm:p-6 sm:pt-14">
                  <div className="relative">
                    <div ref={realBottomRef} style={{ opacity: INITIAL.real }} className="flex items-end justify-between gap-3">
                      <div className="min-w-0 flex-1">
                        <span className="block h-[3px] rounded-full bg-white shadow-[0_0_8px_rgba(255,255,255,0.6)]" ref={barRef} style={{ width: "25cqw" }} />
                        <span ref={barLabelRef} className="mt-1.5 block font-mono text-[11px] text-slate-200 sm:text-xs">
                          20 units
                        </span>
                      </div>
                      <div className="text-right font-mono text-[11px] leading-tight sm:text-xs">
                        <span ref={countRef} className="block text-sm font-bold text-white tabular-nums sm:text-base">
                          {grouped(LIVE_POSTER)} alive
                        </span>
                        <span ref={tickRef} className="block text-slate-300">
                          tick {grouped(POSTER.tick)} · frame {film.poster.frame}
                        </span>
                      </div>
                    </div>
                  </div>
                </div>

                <div
                  ref={diagramRef}
                  className="pointer-events-none absolute inset-x-0 bottom-0 p-4 sm:p-6"
                  style={{ opacity: 0, visibility: "hidden" }}
                >
                  <p className="rounded-lg bg-black/70 px-2.5 py-1.5 font-mono text-[11px] leading-snug text-balance text-slate-200 backdrop-blur sm:text-xs">
                    {current.hud ?? STOPS[3]?.hud}
                  </p>
                </div>
              </div>
            </div>

            {/* The steps and the stop's words. */}
            <div className="min-w-0 space-y-5 lg:col-start-2 lg:row-start-2 lg:self-start">
              <nav aria-label="Zoom steps" className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => goTo(stop - 1)}
                  aria-disabled={stop === 0}
                  aria-label="Zoom out a step"
                  className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-white/15 text-white transition-[border-color,color,opacity] hover:border-green-400/60 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none aria-disabled:pointer-events-none aria-disabled:opacity-30 [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:hidden"
                >
                  <ChevronLeft className="h-4 w-4" aria-hidden="true" />
                </button>
                <div className="relative min-w-0 flex-1">
                  <ol className="grid grid-cols-5 gap-1">
                    {STOPS.map((entry, index) => (
                      <li key={entry.rail}>
                        <button
                          type="button"
                          onClick={() => goTo(index)}
                          aria-current={index === stop ? "step" : undefined}
                          className={cn(
                            "flex h-11 w-full flex-col items-center justify-center rounded-lg text-[10px] font-bold tracking-wide transition-[background-color,color] focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none sm:text-[11px]",
                            index === stop ? "bg-white/[0.07] text-white" : "text-slate-400 hover:text-green-300",
                          )}
                        >
                          <span className={cn("font-mono", index === stop ? "text-green-300" : "text-slate-500")}>
                            {String(index + 1).padStart(2, "0")}
                          </span>
                          <span className="truncate">{entry.rail}</span>
                        </button>
                      </li>
                    ))}
                  </ol>
                  <span aria-hidden="true" className="absolute inset-x-0 -bottom-1 block h-0.5 overflow-hidden rounded-full bg-white/10">
                    <span
                      ref={railRef}
                      className="block h-full origin-left bg-gradient-to-r from-green-400 to-lime-300"
                      style={{ transform: "scaleX(0)" }}
                    />
                  </span>
                </div>
                <button
                  type="button"
                  onClick={() => goTo(stop + 1)}
                  aria-disabled={stop === LAST}
                  aria-label="Zoom in a step"
                  className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-white/15 text-white transition-[border-color,color,opacity] hover:border-green-400/60 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none aria-disabled:pointer-events-none aria-disabled:opacity-30 [@media(min-width:1024px)_and_(pointer:fine)_and_(prefers-reduced-motion:no-preference)]:hidden"
                >
                  <ChevronRight className="h-4 w-4" aria-hidden="true" />
                </button>
              </nav>

              <div ref={words} aria-live={scrolly ? undefined : "polite"} className="min-w-0">
                <p className="font-mono text-[11px] tracking-[0.2em] text-green-400/90 uppercase">
                  {String(stop + 1).padStart(2, "0")} / {String(STOPS.length).padStart(2, "0")} · {current.eyebrow}
                </p>
                <h4 className="mt-2 text-2xl text-white sm:text-[1.7rem]">
                  <GreenTitle text={current.title} />
                </h4>
                <p className="mt-3 text-[15px] leading-relaxed text-slate-300">{current.body}</p>
                <dl className="mt-5 grid grid-cols-2 gap-x-5 gap-y-4 border-t border-white/10 pt-4">
                  {current.figures.map((figure) => (
                    <div key={figure.label} className="min-w-0">
                      <dt className="text-[10px] font-black tracking-[0.18em] text-slate-500 uppercase">{figure.label}</dt>
                      <dd className="mt-1 font-mono text-base text-white tabular-nums">{figure.value}</dd>
                      <dd className="mt-0.5 text-[11px] leading-snug text-slate-500">{figure.note}</dd>
                    </div>
                  ))}
                </dl>
                {scrolly && stop < LAST ? (
                  <p className="mt-5 inline-flex items-center gap-2 text-xs text-slate-500">
                    <MousePointer2 className="h-3.5 w-3.5" aria-hidden="true" /> Keep scrolling to zoom in.
                  </p>
                ) : null}
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* Every step's words, for a screen reader reading the page in order. */}
      <ol className="sr-only">
        {STOPS.map((entry, index) => (
          <li key={entry.rail}>
            Step {index + 1}, {entry.eyebrow}: {entry.title.replace(/[[\]]/g, "")} {entry.figures.map((figure) => `${figure.label} ${figure.value}, ${figure.note}.`).join(" ")}
          </li>
        ))}
      </ol>

      <figcaption className="mt-8 border-t border-white/5 pt-4 font-mono text-[11px] leading-relaxed [overflow-wrap:anywhere] text-slate-500">
        Steps 1–3 are flife&apos;s pixels, from the confluent film&apos;s manifest (lib/films/confluent.json): the poster (frame {film.poster.frame}, tick{" "}
        {grouped(POSTER.tick)}, camera zoom 1, the whole torus) and the social card (frame {film.social.frame}, tick{" "}
        {grouped(CARD.tick)}, camera zoom {fmt(CARD.zoom, 2)} centred at ({fmt(CARD.center[0], 3)},{" "}
        {fmt(CARD.center[1], 3)}): the {film.video.width}² frame scaled to 1,200 wide and cropped to 1,200 × 630, whose
        middle {fmt(CARD_VIEW)} × {fmt(CARD_VIEW)} units are shown). Between steps 1 and 2 the picture changes frame as
        well as zoom. The organism at ({fmt(OX, 2)}, {fmt(OY, 2)}) was picked on the card as a bright body with space
        around it. Counts in view are the frame&apos;s live count spread evenly over the torus, an average, not a count.
        Steps 4–5 draw presets/{film.world.name}.toml&apos;s [controller], [mutation] and [architecture_mutation] and the
        sensor and action ABIs; their weights are illustrative, from a fixed seed, and are no organism&apos;s genome.
      </figcaption>
    </figure>
  );
}

/** Corner ticks: the stage is an instrument's field of view. */
function Corners() {
  const corner = "pointer-events-none absolute h-3.5 w-3.5 border-lime-300/70 sm:h-4 sm:w-4";
  return (
    <div aria-hidden="true">
      <span className={cn(corner, "top-2 left-2 rounded-tl-md border-t-2 border-l-2")} />
      <span className={cn(corner, "top-2 right-2 rounded-tr-md border-t-2 border-r-2")} />
      <span className={cn(corner, "bottom-2 left-2 rounded-bl-md border-b-2 border-l-2")} />
      <span className={cn(corner, "right-2 bottom-2 rounded-br-md border-r-2 border-b-2")} />
    </div>
  );
}
