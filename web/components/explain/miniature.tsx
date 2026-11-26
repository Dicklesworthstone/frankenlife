"use client";

import {
  ArrowRight,
  Brush as BrushIcon,
  MousePointer2,
  Pause,
  Play,
  RotateCcw,
  ShieldCheck,
  SkipForward,
} from "lucide-react";
import { type KeyboardEvent, type MouseEvent, useCallback, useEffect, useRef, useState } from "react";

import { DemoButton, DemoFrame, Segmented, Slider } from "@/components/explain/demo";
import {
  type Activity,
  type Brains,
  type Children,
  HIDDEN,
  INPUTS,
  INPUT_NAMES,
  LINEAGES,
  type Ledger,
  OUTPUT_NAMES,
  PARAMETERS,
  RULES,
  SIZE,
  OUTPUTS,
  World,
  sigmoidToy,
  tanhToy,
} from "@/components/explain/miniature-engine";
import { isSkipped, useInView, useReducedMotion } from "@/lib/hooks";
import { LINEAGE, TONE } from "@/lib/palette";
import { cn, grouped } from "@/lib/utils";

/** The seed the page opens on: one whose lineages are still contesting the sources after minutes. */
const FIRST_SEED = 3;
/** Ticks a second at 1×, kept by the clock rather than the display's frame rate (60 Hz and 120 Hz alike). */
const TICKS_PER_SECOND = 30;

const SPEEDS = [
  { value: "1", label: "1×" },
  { value: "3", label: "3×" },
  { value: "10", label: "10×" },
] as const;
type Speed = (typeof SPEEDS)[number]["value"];
type Pointer = "select" | "brush";

/** Lineage shares, one column every SAMPLE_EVERY ticks, the last HISTORY columns. */
const SAMPLE_EVERY = 8;
const HISTORY = 240;

type Snapshot = {
  tick: number;
  alive: number;
  lineages: number;
  counts: number[];
  ledger: Ledger;
  births: number;
  deaths: number;
  refused: number;
  crowded: number;
  lastIn: number;
  lastOut: number;
  digest: string;
};

type Selection = {
  identity: number;
  lineage: number;
  generation: number;
  energy: number;
  age: number;
  /** The tick it died, if it has. */
  diedAt: number | null;
  activity: Activity | null;
  weights: number[];
  /** Children born while it was watched, and the newest of them still alive. */
  born: number;
  heir: number | null;
  /** When reached by following a parent: that parent, and the largest change from its weights. */
  parent: { identity: number; gap: number; diedAt: number | null } | null;
};

type ReplayState =
  | { state: "idle" }
  | { state: "running"; done: number; target: number }
  | { state: "done"; tick: number; same: boolean; digest: string; live: string };

function snapshot(world: World): Snapshot {
  const counts = Array.from(world.lineageCounts());
  return {
    tick: world.tick,
    alive: world.count,
    lineages: counts.filter((n) => n > 0).length,
    counts,
    ledger: world.ledger(),
    births: world.births,
    deaths: world.deaths,
    refused: world.refused,
    crowded: world.crowded,
    lastIn: world.lastIn,
    lastOut: world.lastOut,
    digest: world.digest(),
  };
}

// ------------------------------------------------------------------ drawing

function hexRgb(hex: string): [number, number, number] {
  const n = Number.parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** The field's colour ramp: black-green, through green, to the page's energy yellow. */
const FIELD_LUT = (() => {
  const stops: [number, [number, number, number]][] = [
    [0, [2, 10, 2]],
    [0.22, [5, 38, 16]],
    [0.55, [21, 110, 52]],
    [0.82, [110, 200, 90]],
    [1, hexRgb(TONE.energy)],
  ];
  const lut = new Uint8ClampedArray(256 * 3);
  for (let i = 0; i < 256; i++) {
    const v = i / 255;
    let k = 0;
    while (k < stops.length - 2 && v > (stops[k + 1]?.[0] ?? 1)) k++;
    const [v0, c0] = stops[k] ?? stops[0]!;
    const [v1, c1] = stops[k + 1] ?? stops[0]!;
    const f = v1 > v0 ? (v - v0) / (v1 - v0) : 0;
    for (let ch = 0; ch < 3; ch++) lut[i * 3 + ch] = (c0[ch] ?? 0) + ((c1[ch] ?? 0) - (c0[ch] ?? 0)) * f;
  }
  return lut;
})();

/** A lit sphere in a lineage's colour, drawn once per lineage. */
function sphereSprite(color: string, size: number): CanvasImageSource {
  // An immutable bitmap where the browser has one: a canvas is checked for changes at each of the thousand draws a frame.
  const sprite: HTMLCanvasElement | OffscreenCanvas =
    typeof OffscreenCanvas === "function" ? new OffscreenCanvas(size, size) : document.createElement("canvas");
  sprite.width = size;
  sprite.height = size;
  const g = sprite.getContext("2d") as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null;
  if (!g) return sprite;
  const r = size / 2;
  const glow = g.createRadialGradient(r, r, r * 0.35, r, r, r);
  glow.addColorStop(0, `${color}55`);
  glow.addColorStop(1, `${color}00`);
  g.fillStyle = glow;
  g.fillRect(0, 0, size, size);
  const body = g.createRadialGradient(r * 0.78, r * 0.72, r * 0.05, r, r, r * 0.56);
  body.addColorStop(0, "#ffffff");
  body.addColorStop(0.28, color);
  body.addColorStop(1, `${color}aa`);
  g.fillStyle = body;
  g.beginPath();
  g.arc(r, r, r * 0.56, 0, Math.PI * 2);
  g.fill();
  return "transferToImageBitmap" in sprite ? sprite.transferToImageBitmap() : sprite;
}

type Ripple = { x: number; y: number; at: number };

/** What the field's picture was last made from: a frame between two ticks reuses it. */
type FieldPicture = { world: World | null; tick: number };

function drawWorld(
  canvas: HTMLCanvasElement,
  fieldCanvas: HTMLCanvasElement,
  image: ImageData,
  picture: FieldPicture,
  sprites: CanvasImageSource[],
  world: World,
  selected: number | null,
  cursor: { x: number; y: number } | null,
  ripples: Ripple[],
  now: number,
  /** 1 repaints from scratch; below 1 lays the new frame over the last, so motion leaves a short trail. */
  fade: number,
): void {
  const g = canvas.getContext("2d");
  const fg = fieldCanvas.getContext("2d");
  if (!g || !fg) return;
  const w = canvas.width;
  const cell = w / SIZE;

  // The field, one pixel a cell, smoothed as it is scaled up. It changes only when the world steps.
  if (picture.world !== world || picture.tick !== world.tick) {
    const data = image.data;
    const field = world.field;
    for (let c = 0; c < field.length; c++) {
      const v = Math.min(1, Math.sqrt((field[c] ?? 0) / 40));
      const k = Math.round(v * 255) * 3;
      data[c * 4] = FIELD_LUT[k] ?? 0;
      data[c * 4 + 1] = FIELD_LUT[k + 1] ?? 0;
      data[c * 4 + 2] = FIELD_LUT[k + 2] ?? 0;
      data[c * 4 + 3] = 255;
    }
    fg.putImageData(image, 0, 0);
    picture.world = world;
    picture.tick = world.tick;
  }
  g.imageSmoothingEnabled = true;
  g.imageSmoothingQuality = "high";
  g.globalAlpha = fade;
  g.drawImage(fieldCanvas, 0, 0, w, w);
  g.globalAlpha = 1;

  // Where food enters: a faint warm light at each source.
  for (let s = 0; s < RULES.sources; s++) {
    const [sx, sy] = world.sourceAt(s);
    const px = sx * cell;
    const py = sy * cell;
    const radius = RULES.sourceRadius * cell;
    const light = g.createRadialGradient(px, py, 0, px, py, radius);
    light.addColorStop(0, `rgba(253,230,138,${(0.13 * fade).toFixed(3)})`);
    light.addColorStop(1, "rgba(253,230,138,0)");
    g.fillStyle = light;
    g.fillRect(px - radius, py - radius, radius * 2, radius * 2);
  }

  // The organisms: a lit sphere each, in its lineage's colour, larger as it holds more energy.
  const focusLineage = selected !== null ? (world.lineage[selected] ?? -1) : -1;
  for (let i = 0; i < world.count; i++) {
    const lineage = world.lineage[i] ?? 0;
    const sprite = sprites[lineage % sprites.length];
    if (!sprite) continue;
    const e = world.energy[i] ?? 0;
    const d = cell * (1.25 + 0.9 * Math.min(1.5, Math.sqrt(e / RULES.divideAt)));
    const alpha = focusLineage < 0 || lineage === focusLineage ? 1 : 0.28;
    if (g.globalAlpha !== alpha) g.globalAlpha = alpha;
    g.drawImage(sprite, (world.x[i] ?? 0) * cell - d / 2, (world.y[i] ?? 0) * cell - d / 2, d, d);
  }
  g.globalAlpha = 1;

  if (selected !== null) {
    const px = (world.x[selected] ?? 0) * cell;
    const py = (world.y[selected] ?? 0) * cell;
    g.strokeStyle = "#ffffff";
    g.lineWidth = Math.max(1.5, cell * 0.25);
    g.beginPath();
    g.arc(px, py, cell * 2.6, 0, Math.PI * 2);
    g.stroke();
    g.strokeStyle = "rgba(255,255,255,0.45)";
    g.lineWidth = Math.max(1, cell * 0.12);
    for (const [dx, dy] of [
      [1, 0],
      [-1, 0],
      [0, 1],
      [0, -1],
    ] as const) {
      g.beginPath();
      g.moveTo(px + dx * cell * 3.3, py + dy * cell * 3.3);
      g.lineTo(px + dx * cell * 5, py + dy * cell * 5);
      g.stroke();
    }
  }

  for (const ripple of ripples) {
    const t = (now - ripple.at) / 900;
    if (t < 0 || t > 1) continue;
    g.strokeStyle = `rgba(253,230,138,${0.9 * (1 - t)})`;
    g.lineWidth = Math.max(1.5, cell * 0.3);
    g.beginPath();
    g.arc((ripple.x + 0.5) * cell, (ripple.y + 0.5) * cell, cell * (1 + 7 * t), 0, Math.PI * 2);
    g.stroke();
  }

  if (cursor) {
    const px = (cursor.x + 0.5) * cell;
    const py = (cursor.y + 0.5) * cell;
    g.strokeStyle = "rgba(74,222,128,0.95)";
    g.lineWidth = Math.max(1.5, cell * 0.2);
    g.strokeRect(px - cell * 2, py - cell * 2, cell * 4, cell * 4);
  }
}

/** Lineage shares over time (a Muller plot): each column a sample, stacked in lineage order. */
function drawHistory(canvas: HTMLCanvasElement, history: Int32Array, filled: number, head: number): void {
  const g = canvas.getContext("2d");
  if (!g) return;
  const w = canvas.width;
  const h = canvas.height;
  g.fillStyle = "#020a02";
  g.fillRect(0, 0, w, h);
  // Until the window fills, the samples so far span the whole strip.
  const column = w / Math.max(filled, 24);
  for (let k = 0; k < filled; k++) {
    const sample = (head - filled + k + HISTORY) % HISTORY;
    let total = 0;
    for (let l = 0; l < LINEAGES; l++) total += history[sample * LINEAGES + l] ?? 0;
    if (total === 0) continue;
    let y = 0;
    const x = k * column;
    for (let l = 0; l < LINEAGES; l++) {
      const n = history[sample * LINEAGES + l] ?? 0;
      if (n === 0) continue;
      const height = (n / total) * h;
      g.fillStyle = LINEAGE[l % LINEAGE.length] ?? "#ffffff";
      g.fillRect(x, y, column + 0.6, height + 0.6);
      y += height;
    }
  }
}

// ---------------------------------------------------------------- component

export function Miniature() {
  const reduced = useReducedMotion();
  const frame = useRef<HTMLDivElement>(null);
  const inView = useInView(frame, "80px");

  const [seed, setSeed] = useState(FIRST_SEED);
  const [brains, setBrains] = useState<Brains>("own");
  const [children, setChildren] = useState<Children>("inherit");
  const [speed, setSpeed] = useState<Speed>("1");
  const [pointer, setPointer] = useState<Pointer>("select");
  const [playing, setPlaying] = useState(true);
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [replayState, setReplayState] = useState<ReplayState>({ state: "idle" });
  const [announcement, setAnnouncement] = useState("");

  const world = useRef<World | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const historyCanvas = useRef<HTMLCanvasElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const selectedIdentity = useRef<number | null>(null);
  /** The watched organism's children, in birth order, and the parent it was followed from. */
  const offspring = useRef<number[]>([]);
  const followedFrom = useRef<{ identity: number; child: number; weights: number[]; diedAt: number | null } | null>(null);
  /** The watched organism's weights when last seen alive, and the tick it died on. */
  const lastWeights = useRef<number[]>([]);
  const deathTick = useRef<number | null>(null);
  const cursor = useRef<{ x: number; y: number } | null>(null);
  const ripples = useRef<Ripple[]>([]);
  const history = useRef({ data: new Int32Array(HISTORY * LINEAGES), filled: 0, head: 0 });
  const drawing = useRef<{
    field: HTMLCanvasElement;
    image: ImageData;
    picture: FieldPicture;
    sprites: CanvasImageSource[];
  } | null>(null);
  /** Whether the history strip on the page is out of date: it changes with a sample or a resize, not with a frame. */
  const stripDue = useRef(true);
  const dirty = useRef(true);
  const replaying = useRef(false);

  // Readers who ask for less motion start paused, with Step.
  useEffect(() => {
    if (reduced) setPlaying(false);
  }, [reduced]);

  const sample = useCallback((w: World) => {
    const watched = selectedIdentity.current;
    if (watched !== null) {
      for (let k = 0; k < w.lastBirths.length; k += 2) {
        if (w.lastBirths[k] === watched) offspring.current.push(w.lastBirths[k + 1] ?? 0);
      }
      // The tick just stepped is w.tick − 1.
      if (deathTick.current === null && w.find(watched) < 0) deathTick.current = w.tick - 1;
    }
    if (w.tick % SAMPLE_EVERY !== 0) return;
    const h = history.current;
    const counts = w.lineageCounts();
    h.data.set(counts, h.head * LINEAGES);
    h.head = (h.head + 1) % HISTORY;
    h.filled = Math.min(HISTORY, h.filled + 1);
    stripDue.current = true;
  }, []);

  const select = useCallback((w: World, identity: number | null) => {
    let id = identity;
    for (;;) {
      selectedIdentity.current = id;
      if (id === null) {
        setSelection(null);
        return;
      }
      let heir: number | null = null;
      for (let k = offspring.current.length - 1; k >= 0 && heir === null; k--) {
        const child = offspring.current[k] ?? -1;
        if (w.find(child) >= 0) heir = child;
      }
      const born = offspring.current.length;
      const slot = w.find(id);
      if (slot < 0 && heir !== null) {
        // It died, leaving a living child: the watch passes down to the child.
        followedFrom.current = { identity: id, child: heir, weights: lastWeights.current, diedAt: deathTick.current };
        offspring.current = [];
        deathTick.current = null;
        id = heir;
        continue;
      }
      if (slot < 0) {
        const died = deathTick.current ?? w.tick;
        const gone = id;
        setSelection((previous) =>
          previous && previous.identity === gone ? { ...previous, diedAt: previous.diedAt ?? died, born, heir } : previous,
        );
        return;
      }
      const weights = Array.from(w.weightsOf(slot));
      lastWeights.current = weights;
      const from = followedFrom.current;
      let parent: Selection["parent"] = null;
      if (from && from.child === id) {
        let gap = 0;
        for (let p = 0; p < weights.length; p++) gap = Math.max(gap, Math.abs((weights[p] ?? 0) - (from.weights[p] ?? 0)));
        parent = { identity: from.identity, gap, diedAt: from.diedAt };
      }
      setSelection({
        identity: id,
        lineage: w.lineage[slot] ?? 0,
        generation: w.generation[slot] ?? 0,
        energy: w.energy[slot] ?? 0,
        age: w.age[slot] ?? 0,
        diedAt: null,
        activity: w.activity(slot),
        weights,
        born,
        heir,
        parent,
      });
      return;
    }
  }, []);

  /** Forget the watched organism's family: a new selection starts a new watch. */
  const unwatch = () => {
    offspring.current = [];
    followedFrom.current = null;
    deathTick.current = null;
  };

  const refresh = useCallback(() => {
    const w = world.current;
    if (!w) return;
    setSnap(snapshot(w));
    if (selectedIdentity.current !== null) select(w, selectedIdentity.current);
  }, [select]);

  // A new world whenever the seed or a switch changes: the same seed under one changed rule.
  useEffect(() => {
    const w = new World(seed, { brains, children });
    world.current = w;
    history.current = { data: new Int32Array(HISTORY * LINEAGES), filled: 0, head: 0 };
    sample(w);
    selectedIdentity.current = null;
    offspring.current = [];
    followedFrom.current = null;
    deathTick.current = null;
    setSelection(null);
    setReplayState({ state: "idle" });
    dirty.current = true;
    refresh();
  }, [seed, brains, children, sample, refresh]);

  // Canvas size: CSS pixels times the device's ratio, kept square.
  useEffect(() => {
    const element = box.current;
    const target = canvas.current;
    const strip = historyCanvas.current;
    if (!element || !target) return;
    const fit = () => {
      const ratio = Math.min(2, window.devicePixelRatio || 1);
      const width = Math.max(1, Math.round(element.clientWidth * ratio));
      target.width = width;
      target.height = width;
      if (strip) {
        strip.width = Math.max(1, Math.round(strip.clientWidth * ratio));
        strip.height = Math.max(1, Math.round(strip.clientHeight * ratio));
        stripDue.current = true;
      }
      // Resizing clears both canvases: paint them again now, or a paused world stays blank.
      repaint();
    };
    const observes = typeof ResizeObserver !== "undefined";
    if (!observes || !isSkipped(element)) fit();
    if (!observes) return;
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Drawing resources, made once in the browser.
  useEffect(() => {
    const field = document.createElement("canvas");
    field.width = SIZE;
    field.height = SIZE;
    const context = field.getContext("2d");
    if (!context) return;
    drawing.current = {
      field,
      image: context.createImageData(SIZE, SIZE),
      picture: { world: null, tick: -1 },
      sprites: LINEAGE.map((color) => sphereSprite(color, 48)),
    };
    dirty.current = true;
  }, []);

  const paint = useCallback((now: number, fade = 1) => {
    const w = world.current;
    const d = drawing.current;
    const target = canvas.current;
    if (!w || !d || !target) return;
    const slot = selectedIdentity.current !== null ? w.find(selectedIdentity.current) : -1;
    ripples.current = ripples.current.filter((ripple) => now - ripple.at < 900);
    drawWorld(target, d.field, d.image, d.picture, d.sprites, w, slot >= 0 ? slot : null, cursor.current, ripples.current, now, fade);
    const strip = historyCanvas.current;
    if (strip && stripDue.current) {
      stripDue.current = false;
      drawHistory(strip, history.current.data, history.current.filled, history.current.head);
    }
  }, []);

  // The loop: only while playing and on screen; readouts a few times a second, never per frame.
  const running = playing && inView && replayState.state !== "running";
  useEffect(() => {
    let handle = 0;
    let lastReadout = 0;
    let lastFrame = 0;
    let budget = 0;
    const loop = (now: number) => {
      const w = world.current;
      if (w && running && !replaying.current) {
        // A long gap (a background tab, a slow frame) counts as at most a tenth of a second.
        const elapsed = lastFrame === 0 ? 1000 / 60 : Math.min(100, now - lastFrame);
        lastFrame = now;
        budget += (elapsed / 1000) * TICKS_PER_SECOND * Number(speed);
        const ticks = Math.floor(budget);
        budget -= ticks;
        for (let k = 0; k < ticks; k++) {
          w.step();
          sample(w);
        }
        if (now - lastReadout > 250) {
          lastReadout = now;
          refresh();
        }
        paint(now, 0.42);
      } else if (dirty.current || ripples.current.length > 0) {
        dirty.current = false;
        paint(now);
      }
      if (running || ripples.current.length > 0 || dirty.current) handle = requestAnimationFrame(loop);
    };
    handle = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(handle);
  }, [running, speed, sample, refresh, paint]);

  // When paused, repaint on demand.
  const repaint = useCallback(() => {
    dirty.current = true;
    requestAnimationFrame((now) => {
      dirty.current = false;
      paint(now);
    });
  }, [paint]);

  useEffect(() => {
    if (!running) repaint();
  }, [running, selection, repaint, snap]);

  // Readouts are taken every 250 ms while running: take one when it stops, so they show the tick the world is on.
  useEffect(() => {
    if (!running) refresh();
  }, [running, refresh]);

  const step = () => {
    const w = world.current;
    if (!w) return;
    w.step();
    sample(w);
    refresh();
    repaint();
  };

  const reset = () => {
    const w = new World(seed, { brains, children });
    world.current = w;
    history.current = { data: new Int32Array(HISTORY * LINEAGES), filled: 0, head: 0 };
    sample(w);
    selectedIdentity.current = null;
    unwatch();
    setSelection(null);
    setReplayState({ state: "idle" });
    refresh();
    repaint();
    setAnnouncement(`The world is back at tick 0, seed ${seed}.`);
  };

  /** Act at a cell: select the organism there, or brush energy into it. */
  const actAt = (x: number, y: number) => {
    const w = world.current;
    if (!w) return;
    if (pointer === "brush") {
      const command = w.brush(x, y);
      ripples.current.push({ x: command.x, y: command.y, at: performance.now() });
      setReplayState({ state: "idle" });
      setAnnouncement(`Brushed ${command.amount} quanta into cell ${command.x}, ${command.y}, applied at tick ${grouped(command.tick)}.`);
      refresh();
      repaint();
      return;
    }
    const slot = w.nearest(x, y, 3);
    unwatch();
    if (slot < 0) {
      select(w, null);
      setAnnouncement("No organism there.");
    } else {
      select(w, w.identity[slot] ?? 0);
      setAnnouncement(`Selected organism ${w.identity[slot]}, lineage ${(w.lineage[slot] ?? 0) + 1}.`);
    }
    repaint();
  };

  const onClick = (event: MouseEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / rect.width) * SIZE;
    const y = ((event.clientY - rect.top) / rect.height) * SIZE;
    actAt(x, y);
  };

  const onKey = (event: KeyboardEvent<HTMLCanvasElement>) => {
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    const move = moves[event.key];
    if (move) {
      event.preventDefault();
      const stride = event.shiftKey ? 8 : 2;
      const at = cursor.current ?? { x: SIZE / 2, y: SIZE / 2 };
      cursor.current = {
        x: (at.x + move[0] * stride + SIZE) % SIZE,
        y: (at.y + move[1] * stride + SIZE) % SIZE,
      };
      repaint();
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const at = cursor.current ?? { x: SIZE / 2, y: SIZE / 2 };
      cursor.current = at;
      actAt(at.x + 0.5, at.y + 0.5);
    } else if (event.key === "Escape") {
      const w = world.current;
      unwatch();
      if (w) select(w, null);
      repaint();
    }
  };

  /** Hand the inspector to the watched organism's newest living child, keeping the parent's weights to compare. */
  const follow = () => {
    const w = world.current;
    if (!w || !selection || selection.heir === null) return;
    const child = selection.heir;
    followedFrom.current = { identity: selection.identity, child, weights: selection.weights, diedAt: selection.diedAt };
    offspring.current = [];
    deathTick.current = null;
    select(w, child);
    setAnnouncement(`Following organism ${grouped(child)}, a child of organism ${grouped(selection.identity)}.`);
    repaint();
  };

  /** Rebuild the world from its seed and brushes to the live tick, in slices, and compare digests. */
  const replay = () => {
    const live = world.current;
    if (!live || replaying.current) return;
    const target = live.tick;
    const liveDigest = live.digest();
    const copy = new World(live.seed, live.mode, live.commands);
    replaying.current = true;
    setReplayState({ state: "running", done: 0, target });
    const slice = () => {
      // A new seed, a switch or Reset replaced the world: drop this replay rather than report it on the new one.
      if (world.current !== live) {
        replaying.current = false;
        return;
      }
      const until = Math.min(target, copy.tick + 400);
      while (copy.tick < until) copy.step();
      if (copy.tick < target) {
        setReplayState({ state: "running", done: copy.tick, target });
        window.setTimeout(slice, 0);
        return;
      }
      const digest = copy.digest();
      replaying.current = false;
      const same = digest === liveDigest;
      setReplayState({ state: "done", tick: target, same, digest, live: liveDigest });
      setAnnouncement(
        same
          ? `Replayed from seed ${live.seed} to tick ${grouped(target)}: the same digest.`
          : `Replayed to tick ${grouped(target)}: the digests differ.`,
      );
    };
    window.setTimeout(slice, 0);
  };

  const ledger = snap?.ledger;
  const exact = ledger ? ledger.residual === 0 : true;

  return (
    <DemoFrame
      eyebrow={`A toy world · ${SIZE} × ${SIZE} cells · ${PARAMETERS} weights an organism`}
      title="Run a world in your browser."
      basis="toy"
      source={
        <>
          A toy written in TypeScript for this page. It is not FrankenLife&apos;s code, and nothing it does is a
          FrankenLife result. It borrows four rules in miniature: uptake in two passes, in whole quanta by largest
          remainder, ties to the lower identity (plan §5.2, fixture C.1); a ledger that closes exactly on every tick (INV-03); birth as
          a transaction, cost paid and the rest split (C.5); and each organism its own weights, changed only by
          inherited mutation (INV-13). It leaves out chemistry and its 32 channels, emission, the sensing pyramid,
          the 9,988-parameter networks, the GPU kernels and the ordered ten-phase tick with its candidate world.
          Founders start from one forager&apos;s genome with every weight moved at random. Its step uses only +, −,
          ×, ÷, rounding down and 32-bit integer arithmetic, which every browser computes to the same bit (never exp, tanh or sin, whose last
          bit can differ), so a seed reaches the same digest anywhere. The real model:{" "}
          <a className="text-green-300 underline-offset-4 hover:underline" href="#rules">
            its rules, checked by hand
          </a>
          ,{" "}
          <a className="text-green-300 underline-offset-4 hover:underline" href="#brains">
            its networks
          </a>
          ,{" "}
          <a className="text-green-300 underline-offset-4 hover:underline" href="#tick">
            one of its ticks
          </a>
          .
        </>
      }
    >
      <p className="max-w-3xl text-sm leading-relaxed text-slate-300 sm:text-base">
        Every dot has its own {PARAMETERS}-weight network. Food enters at four drifting sources and wherever you brush; a
        child inherits a mutated copy of its parent&apos;s weights.
      </p>

      <div
        ref={frame}
        className="mt-6 flex flex-col gap-5 lg:grid lg:grid-cols-[minmax(0,1.12fr)_minmax(0,1fr)] lg:gap-8"
      >
        {/* The world. Below lg both columns dissolve (display: contents), so the inspector sits under the world. */}
        <div className="contents lg:block lg:min-w-0">
          <div className="order-1 min-w-0">
            <div
              ref={box}
              className="relative aspect-square w-full overflow-hidden rounded-2xl border border-white/10 bg-[#020a02] shadow-[inset_0_0_60px_rgba(0,0,0,0.6)]"
            >
              <canvas
                ref={canvas}
                tabIndex={0}
                role="application"
                aria-roledescription="toy world"
                aria-label={`Toy world, tick ${grouped(snap?.tick ?? 0)}, ${grouped(snap?.alive ?? 0)} organisms. Arrow keys move a cursor (Shift for longer steps); Enter ${pointer === "brush" ? "brushes energy into the cell under it" : "selects the organism under it"}; Escape clears the selection.`}
                onClick={onClick}
                onKeyDown={onKey}
                onBlur={() => {
                  cursor.current = null;
                  repaint();
                }}
                className={cn(
                  // Its frame clips outside it, so the focus ring is drawn inside (focus-inset, app/globals.css).
                  "focus-inset absolute inset-0 h-full w-full touch-manipulation rounded-2xl",
                  pointer === "brush" ? "cursor-crosshair" : "cursor-pointer",
                )}
              />
              <div className="pointer-events-none absolute top-3 left-3 flex flex-wrap gap-1.5 font-mono text-[11px] text-slate-200 tabular-nums">
                <span className="rounded-full border border-white/10 bg-black/60 px-2.5 py-1 backdrop-blur">
                  tick {grouped(snap?.tick ?? 0)}
                </span>
                <span className="rounded-full border border-white/10 bg-black/60 px-2.5 py-1 backdrop-blur">
                  {grouped(snap?.alive ?? 0)} alive
                </span>
              </div>
              <span className="pointer-events-none absolute right-3 bottom-3 inline-flex items-center gap-1.5 rounded-full border border-white/10 bg-black/60 px-2.5 py-1 text-[11px] font-bold text-slate-200 backdrop-blur">
                {pointer === "brush" ? (
                  <>
                    <BrushIcon className="h-3 w-3 text-amber-200" aria-hidden="true" /> Tap to feed a cell
                  </>
                ) : (
                  <>
                    <MousePointer2 className="h-3 w-3 text-green-300" aria-hidden="true" /> Tap an organism
                  </>
                )}
              </span>
            </div>

            {/* Transport */}
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <DemoButton
                onClick={() => setPlaying((value) => !value)}
                label={playing ? "Pause the toy world" : "Run the toy world"}
                className="min-h-11 px-4"
              >
                {playing ? <Pause className="h-4 w-4" aria-hidden="true" /> : <Play className="h-4 w-4" aria-hidden="true" />}
                {playing ? "Pause" : "Run"}
              </DemoButton>
              <DemoButton onClick={step} disabled={playing} className="min-h-11" label="Step: advance one tick">
                <SkipForward className="h-4 w-4" aria-hidden="true" /> Step
              </DemoButton>
              <Segmented
                label="Speed"
                options={SPEEDS}
                value={speed}
                onChange={setSpeed}
                className="[&>button]:min-h-9"
              />
              <DemoButton onClick={reset} className="min-h-11" label="Reset the world to tick 0">
                <RotateCcw className="h-4 w-4" aria-hidden="true" /> Reset
              </DemoButton>
            </div>
          </div>

          {/* Who is winning */}
          <div className="order-6 min-w-0 lg:mt-4">
            <div className="flex items-baseline justify-between gap-3">
              <p className="text-xs text-slate-400">
                <span className="font-bold text-slate-200">Lineages over time.</span> Each column, the living by founder
                lineage; {snap?.lineages ?? LINEAGES} of {LINEAGES} lineages left.
              </p>
              <p className="shrink-0 font-mono text-[10px] text-slate-500">last {grouped(HISTORY * SAMPLE_EVERY)} ticks</p>
            </div>
            <canvas
              ref={historyCanvas}
              aria-hidden="true"
              className="mt-2 block h-14 w-full rounded-lg border border-white/10 bg-[#020a02]"
            />
          </div>

          {/* Counts, the digest, replay */}
          <div className="order-7 min-w-0 rounded-2xl border border-white/10 bg-black/30 p-4 lg:mt-5">
            {/* slop-ok(S12-three-cards): three counters (births, deaths, blocked divisions), not three cards. */}
            <dl className="grid grid-cols-3 items-end gap-3 font-mono text-xs tabular-nums">
              <Count label="births" value={snap?.births} />
              <Count label="deaths" value={snap?.deaths} />
              {/* Events, like births: an organism hemmed in tries again next tick, and each try that finds no room counts. */}
              <Count label="divisions blocked" value={snap ? snap.crowded + snap.refused : undefined} />
            </dl>
            <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-white/5 pt-4">
              <div className="min-w-0">
                <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">World digest</p>
                <p className="mt-1 font-mono text-sm break-all text-white">{snap?.digest ?? "—"}</p>
              </div>
              <DemoButton onClick={replay} disabled={replayState.state === "running"} className="min-h-11">
                <RotateCcw className="h-4 w-4" aria-hidden="true" /> Replay from the seed
              </DemoButton>
            </div>
            <ReplayLine state={replayState} seed={seed} brushes={world.current?.commands.length ?? 0} />
            <div className="mt-4">
              <Slider label="Seed (a new world)" value={seed} min={1} max={64} onChange={setSeed} />
            </div>
          </div>
        </div>

        {/* Controls, the inspector and the accounts */}
        <div className="contents lg:block lg:min-w-0 lg:space-y-5">
          <div className="order-4 min-w-0 space-y-3">
            <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">Rules you can flip</p>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-1 xl:grid-cols-2">
              <div className="min-w-0">
                <p className="mb-1.5 text-xs text-slate-400">Brains</p>
                <Segmented
                  label="Brains"
                  options={[
                    { value: "own", label: "Each its own", tone: "rule" },
                    { value: "shared", label: "One shared", tone: "wrong" },
                  ]}
                  value={brains}
                  onChange={setBrains}
                  className="[&>button]:min-h-10"
                />
              </div>
              <div className="min-w-0">
                <p className="mb-1.5 text-xs text-slate-400">Children</p>
                <Segmented
                  label="Children"
                  options={[
                    { value: "inherit", label: "Inherit, mutated", tone: "rule" },
                    { value: "fresh", label: "Fresh weights", tone: "wrong" },
                  ]}
                  value={children}
                  onChange={setChildren}
                  className="[&>button]:min-h-10"
                />
              </div>
            </div>
            <p className="text-xs leading-relaxed text-slate-500">
              <span className="text-slate-300">One shared</span>: every body runs one network, the forbidden shortcut.{" "}
              <span className="text-slate-300">Fresh weights</span>: children don&apos;t inherit. Each restarts from the
              same seed.
            </p>
          </div>

          <div className="order-2 flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
            <Segmented
              label="What a tap on the world does"
              options={[
                {
                  value: "select",
                  label: (
                    <span className="inline-flex items-center gap-1.5">
                      <MousePointer2 className="h-3.5 w-3.5" aria-hidden="true" /> Select
                    </span>
                  ),
                },
                {
                  value: "brush",
                  label: (
                    <span className="inline-flex items-center gap-1.5">
                      <BrushIcon className="h-3.5 w-3.5" aria-hidden="true" /> Brush {RULES.brush} quanta
                    </span>
                  ),
                },
              ]}
              value={pointer}
              onChange={setPointer}
              className="[&>button]:min-h-10"
            />
          </div>

          <div className="order-3 min-w-0">
            <Inspector selection={selection} onFollow={follow} shared={brains === "shared"} />
          </div>

          {/* The ledger */}
          <div className="order-5 min-w-0 rounded-2xl border border-white/10 bg-black/30 p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">The ledger, this tick</p>
              <span
                className={cn(
                  "inline-flex items-center gap-1.5 text-xs font-bold",
                  exact ? "text-green-300" : "text-rose-300",
                )}
              >
                <ShieldCheck className="h-3.5 w-3.5" aria-hidden="true" />
                {exact ? "exact, every tick" : `off by ${ledger?.residual ?? 0}`}
              </span>
            </div>
            <dl className="mt-3 grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-1 font-mono text-xs tabular-nums">
              <LedgerRow sign="" swatch="transparent" label="injected, ever" value={ledger?.injected} strong />
              <LedgerRow sign="=" swatch={FIELD_SWATCH} label="in the field" value={ledger?.field} />
              <LedgerRow sign="+" swatch={TONE.energy} label="in bodies" value={ledger?.bodies} />
              <LedgerRow sign="+" swatch={TONE.dissipated} label="dissipated, ever" value={ledger?.dissipated} />
            </dl>
            <p className="mt-4 text-[10px] font-black tracking-[0.18em] text-slate-500 uppercase">The energy in the world now</p>
            <div
              className="mt-1.5 flex h-2.5 overflow-hidden rounded-full bg-white/5"
              role="img"
              aria-label={
                ledger
                  ? `${grouped(ledger.field)} quanta in the field and ${grouped(ledger.bodies)} in bodies.`
                  : "The energy in the world now"
              }
            >
              {ledger ? (
                <>
                  <span className="h-full" style={{ width: share(ledger.field, ledger), background: FIELD_SWATCH }} />
                  <span className="h-full" style={{ width: share(ledger.bodies, ledger), background: TONE.energy }} />
                </>
              ) : null}
            </div>
            <p className="mt-3 font-mono text-[11px] leading-relaxed text-slate-400 tabular-nums">
              last tick: <span className="text-green-300">+{grouped(snap?.lastIn ?? 0)}</span> in from the sources
              {(snap?.lastIn ?? 0) > RULES.sources * RULES.sourcePower ? " and a brush" : ""},{" "}
              <span className="text-slate-300">−{grouped(snap?.lastOut ?? 0)}</span> out as heat
            </p>
            <p className="mt-2 text-[11px] leading-snug text-slate-500">
              Upkeep, motion, births and the field&apos;s slow loss are dissipated; a death returns the body&apos;s energy
              to its cell. Only the sources and your brush add energy.
            </p>
          </div>

        </div>
      </div>
      <p className="sr-only" aria-live="polite">
        {announcement}
      </p>
    </DemoFrame>
  );
}

const FIELD_SWATCH = "#16a34a";

/** A stock's share of the energy now in the world (field + bodies). */
function share(n: number, ledger: Ledger): string {
  const now = Math.max(1, ledger.field + ledger.bodies);
  return `${((n / now) * 100).toFixed(2)}%`;
}

function LedgerRow({
  sign,
  swatch,
  label,
  value,
  strong,
}: {
  sign: string;
  swatch: string;
  label: string;
  value: number | undefined;
  strong?: boolean;
}) {
  return (
    <>
      <dt className="flex items-center gap-2 text-[11px] text-slate-500">
        <span aria-hidden="true" className="w-3 text-center text-slate-400">
          {sign}
        </span>
        <span aria-hidden="true" className="h-2 w-2 shrink-0 rounded-full" style={{ background: swatch }} />
        {label}
      </dt>
      <dd className={cn("text-right", strong ? "text-green-200" : "text-slate-200")}>
        {value === undefined ? "—" : grouped(value)}
      </dd>
    </>
  );
}

function Count({ label, value }: { label: string; value: number | undefined }) {
  return (
    <div className="min-w-0">
      <dt className="text-[10px] leading-tight text-slate-500">{label}</dt>
      <dd className="mt-1 text-sm text-white">{value === undefined ? "—" : grouped(value)}</dd>
    </div>
  );
}

function ReplayLine({ state, seed, brushes }: { state: ReplayState; seed: number; brushes: number }) {
  if (state.state === "idle") {
    return (
      <p className="mt-3 text-[11px] leading-snug text-slate-500">
        Rebuilds this world from seed {seed}
        {brushes > 0 ? ` and your ${brushes} brush${brushes === 1 ? "" : "es"}, each at its tick,` : ""} and runs it to the
        tick you are on, then compares digests.
      </p>
    );
  }
  if (state.state === "running") {
    return (
      <div className="mt-3">
        <div className="h-1.5 overflow-hidden rounded-full bg-white/5">
          <span
            className="block h-full bg-green-400/70"
            style={{ width: `${state.target > 0 ? (state.done / state.target) * 100 : 100}%` }}
          />
        </div>
        <p className="mt-1.5 font-mono text-[11px] text-slate-400 tabular-nums">
          replaying… tick {grouped(state.done)} of {grouped(state.target)}
        </p>
      </div>
    );
  }
  return (
    <p className={cn("mt-3 text-xs leading-snug", state.same ? "text-green-300" : "text-rose-300")}>
      {state.same ? "✓ " : "✗ "}
      Replayed to tick {grouped(state.tick)}:{" "}
      {state.same ? "the same digest, bit for bit." : `a different digest (${state.digest}).`}
    </p>
  );
}

// ---------------------------------------------------------------- inspector

const signed = (v: number) => `${v >= 0 ? "+" : "−"}${Math.abs(v).toFixed(2)}`;

function Bar({ value, range, color }: { value: number; range: number; color: string }) {
  const v = Math.max(-1, Math.min(1, value / range));
  return (
    <span className="relative block h-2 w-full overflow-hidden rounded-full bg-white/[0.06]">
      <span aria-hidden="true" className="absolute inset-y-0 left-1/2 w-px bg-white/20" />
      <span
        className="absolute inset-y-0 rounded-full"
        style={{
          left: v >= 0 ? "50%" : `${50 + v * 50}%`,
          width: `${Math.abs(v) * 50}%`,
          background: color,
        }}
      />
    </span>
  );
}

function Inspector({
  selection,
  onFollow,
  shared,
}: {
  selection: Selection | null;
  onFollow: () => void;
  /** One shared brain: every organism runs the same weights. */
  shared: boolean;
}) {
  if (!selection) {
    return (
      <div className="rounded-2xl border border-dashed border-white/15 bg-black/20 p-4 text-sm leading-relaxed text-slate-400">
        <span className="font-bold text-slate-200">Tap an organism</span> to read its own network: what it senses, its
        eight hidden units, and what it decides. Its kin stay lit; the rest of the world dims.
      </div>
    );
  }
  const color = LINEAGE[selection.lineage % LINEAGE.length] ?? "#ffffff";
  const activity = selection.activity;
  const outputs = activity?.outputs ?? [];
  const decisions = [
    { name: OUTPUT_NAMES[0], text: `${signed(RULES.speed * tanhToy(outputs[0] ?? 0))} / tick`, v: tanhToy(outputs[0] ?? 0) },
    { name: OUTPUT_NAMES[1], text: `${signed(RULES.speed * tanhToy(outputs[1] ?? 0))} / tick`, v: tanhToy(outputs[1] ?? 0) },
    {
      name: OUTPUT_NAMES[2],
      text: `asks ${Math.floor(RULES.bite * sigmoidToy(outputs[2] ?? 0))} of ${RULES.bite}`,
      v: 2 * sigmoidToy(outputs[2] ?? 0) - 1,
    },
    {
      name: OUTPUT_NAMES[3],
      text: (outputs[3] ?? 0) > 0 ? (selection.energy >= RULES.divideAt ? "yes" : `at ${RULES.divideAt}`) : "no",
      v: tanhToy(outputs[3] ?? 0),
    },
  ];
  return (
    <div className="rounded-2xl border bg-black/30 p-4" style={{ borderColor: `${color}55` }}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-sm text-white">
          <span aria-hidden="true" className="h-3 w-3 rounded-full" style={{ background: color, boxShadow: `0 0 10px ${color}` }} />
          <span className="font-bold whitespace-nowrap">Organism {grouped(selection.identity)}</span>
          <span className="whitespace-nowrap text-slate-400">
            lineage {selection.lineage + 1} · generation {selection.generation}
          </span>
        </p>
        <p className="font-mono text-[11px] text-slate-400 tabular-nums">
          {selection.diedAt !== null ? (
            <span className="text-rose-300">died at tick {grouped(selection.diedAt)}</span>
          ) : (
            <>
              energy {selection.energy} · age {selection.age}
            </>
          )}
        </p>
      </div>

      {selection.parent ? (
        <p className="mt-2 text-[11px] leading-snug text-slate-400">
          Child of organism {grouped(selection.parent.identity)}
          {selection.parent.diedAt !== null ? `, which died at tick ${grouped(selection.parent.diedAt)}` : ""}:{" "}
          {selection.parent.gap === 0 ? (
            <span className="text-slate-200">its parent&apos;s very weights, one brain shared.</span>
          ) : (
            <>
              no weight more than <span className="font-mono text-slate-200">{selection.parent.gap.toFixed(2)}</span> from
              its parent&apos;s.
            </>
          )}
        </p>
      ) : null}
      <div className="mt-3 flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <p className="font-mono text-[11px] text-slate-500 tabular-nums">
          {selection.born === 0
            ? selection.diedAt !== null
              ? "no children while you watched"
              : "no children yet while you watch"
            : `${grouped(selection.born)} ${selection.born === 1 ? "child" : "children"} while you watched`}
          {selection.diedAt === null && selection.born > 0 ? "; when it dies, the watch passes to the newest" : ""}
        </p>
        {selection.heir !== null ? (
          <DemoButton onClick={onFollow} className="min-h-10 text-xs" label={`Follow its child, organism ${grouped(selection.heir)}, the newest still alive`}>
            Follow its child <ArrowRight className="h-3.5 w-3.5" aria-hidden="true" />
          </DemoButton>
        ) : selection.diedAt !== null ? (
          <span className="text-[11px] text-slate-500">no living child to follow; tap another</span>
        ) : null}
      </div>

      <div className="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)] lg:grid-cols-1 xl:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)]">
        <div className="min-w-0 space-y-3">
          <div>
            <p className="text-[10px] font-black tracking-[0.18em] text-slate-500 uppercase">It senses</p>
            <ul className="mt-1.5 space-y-1">
              {INPUT_NAMES.slice(0, INPUTS - 1).map((name, k) => (
                <li key={name} className="grid grid-cols-[6.5rem_minmax(0,1fr)_2.6rem] items-center gap-2 text-[11px]">
                  <span className="truncate text-slate-400">{name}</span>
                  <Bar value={activity?.inputs[k] ?? 0} range={k === 0 || k === 5 ? 2 : 1} color={TONE.chemistry} />
                  <span className="text-right font-mono text-slate-300 tabular-nums">{(activity?.inputs[k] ?? 0).toFixed(2)}</span>
                </li>
              ))}
            </ul>
          </div>
          <div>
            <p className="text-[10px] font-black tracking-[0.18em] text-slate-500 uppercase">Its {HIDDEN} hidden units</p>
            <div className="mt-1.5 flex h-10 items-center gap-1" aria-hidden="true">
              {Array.from({ length: HIDDEN }, (_, h) => {
                const v = activity?.hidden[h] ?? 0;
                return (
                  <span key={h} className="relative h-full flex-1 overflow-hidden rounded-sm bg-white/[0.05]">
                    <span className="absolute inset-x-0 top-1/2 h-px bg-white/15" />
                    <span
                      className="absolute inset-x-0"
                      style={{
                        top: v >= 0 ? `${50 - v * 50}%` : "50%",
                        height: `${Math.abs(v) * 50}%`,
                        background: v >= 0 ? TONE.rule : TONE.wrong,
                      }}
                    />
                  </span>
                );
              })}
            </div>
          </div>
          <div>
            <p className="text-[10px] font-black tracking-[0.18em] text-slate-500 uppercase">It decides</p>
            <ul className="mt-1.5 space-y-1">
              {decisions.map((decision) => (
                <li key={decision.name} className="grid grid-cols-[6.5rem_minmax(0,1fr)] items-center gap-2 text-[11px]">
                  <span className="truncate text-slate-400">{decision.name}</span>
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="w-14 shrink-0">
                      <Bar value={decision.v} range={1} color={TONE.rule} />
                    </span>
                    <span className="truncate font-mono text-slate-200">{decision.text}</span>
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </div>
        <div className="min-w-0">
          <p className="text-[10px] font-black tracking-[0.18em] text-slate-500 uppercase">Its own {PARAMETERS} weights</p>
          <WeightGrid weights={selection.weights} />
          <p className="mt-2 text-[11px] leading-snug text-slate-500">
            In order: {INPUTS} inputs into {HIDDEN} hidden units, the hidden units into {OUTPUTS} decisions, then{" "}
            {OUTPUTS} biases. Green positive, rose negative.{" "}
            {shared ? "With one shared brain, every organism runs these same numbers." : "No other organism has these numbers."}
          </p>
        </div>
      </div>
    </div>
  );
}

/** The 100 weights as a 10 × 10 mosaic in genome order. */
function WeightGrid({ weights }: { weights: number[] }) {
  return (
    <div className="mt-1.5 grid aspect-square w-full max-w-[13rem] grid-cols-10 gap-[2px]" role="img" aria-label="Its 100 weights as a mosaic">
      {weights.map((w, p) => {
        const v = Math.max(-1, Math.min(1, w / 2.5));
        return (
          <span
            key={p}
            className="rounded-[2px]"
            style={{ background: v >= 0 ? TONE.rule : TONE.wrong, opacity: 0.12 + 0.88 * Math.abs(v) }}
          />
        );
      })}
    </div>
  );
}
