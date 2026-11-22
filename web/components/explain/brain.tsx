"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import {
  CATALOG,
  DEFAULT_TOPOLOGY,
  type Group,
  INPUT_GROUPS,
  OUTPUT_GROUPS,
  parameterCount,
  parameterSum,
  seeded,
  shape,
  widths,
} from "@/components/explain/brain-math";
import { DemoFrame, Segmented } from "@/components/explain/demo";
import { useFigureWidth, useInView, useReducedMotion } from "@/lib/hooks";
import { cn, grouped } from "@/lib/utils";

/**
 * Each group's colour. Reading and writing the same quantity share a colour:
 * the composition it senses and the composition it emits are both chemistry
 * (cyan); the energy it senses and the energy it spends are both energy (gold).
 */
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

// The drawing's geometry, in viewBox units. Its width is 640 where the page is wide; a phone draws it in as few as
// 400 units, the columns closer together, so the labels (14 units) stay near 10 px instead of shrinking to 6.
const VIEW_W = 640;
const VIEW_W_LEAST = 400;
const VIEW_H = 356;
const TOP = 12;
const SPAN = 300;
const LEFT = 86;
/** The output column's distance from the right edge. */
const RIGHT_MARGIN = 98;
const NODE_W = 10;
/** The least vertical distance between two group labels. */
const LABEL_GAP = 16;

// The flow: dashes of 7 units and gaps of 9 that travel at 17.5 units a second, each line a twelfth of a second or
// so ahead of the last. A cycle is 32 units, two whole dash patterns, so it repeats without a jump.
const DASH: [number, number] = [7, 9];
const FLOW_SPEED = 17.5;
const FLOW_CYCLE = 32;
const FLOW_LEAD = 0.12;
const FLOW_WIDTH = 0.7;
const FLOW_OPACITY = 0.3;

type Span = { from: number; to: number };
type Line = { x1: number; y1: number; x2: number; y2: number };

/**
 * The flowing connections, as a canvas laid exactly over the drawing's lines. 138 dash animations in the drawing
 * itself restyle and repaint all of it on every frame; here a frame is 138 strokes.
 */
function Flow({ lines, viewW, colour }: { lines: Line[]; viewW: number; colour: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    const box = canvas.parentElement;
    if (!box) return;
    let scale = 1;
    const paint = (now: number) => {
      // Timed from the clock, not from this effect, so a change of colour or size never moves the dashes.
      const seconds = now / 1000;
      context.setTransform(1, 0, 0, 1, 0, 0);
      context.clearRect(0, 0, canvas.width, canvas.height);
      context.setTransform(scale, 0, 0, scale, 0, 0);
      context.setLineDash(DASH);
      context.lineWidth = FLOW_WIDTH;
      context.globalAlpha = FLOW_OPACITY;
      context.strokeStyle = colour;
      for (let index = 0; index < lines.length; index++) {
        const line = lines[index]!;
        context.lineDashOffset = -((FLOW_SPEED * (seconds + (index % 13) * FLOW_LEAD)) % FLOW_CYCLE);
        context.beginPath();
        context.moveTo(line.x1, line.y1);
        context.lineTo(line.x2, line.y2);
        context.stroke();
      }
    };
    /** Whole device pixels, shown at exactly their size, so the lines are never resampled. */
    const fit = () => {
      const ratio = window.devicePixelRatio || 1;
      const { width, height } = box.getBoundingClientRect();
      canvas.width = Math.max(1, Math.ceil(width * ratio));
      canvas.height = Math.max(1, Math.ceil(height * ratio));
      canvas.style.width = `${canvas.width / ratio}px`;
      canvas.style.height = `${canvas.height / ratio}px`;
      scale = (width * ratio) / viewW;
      // Resizing clears it: paint now, not on the next frame.
      paint(performance.now());
    };
    fit();
    let handle = 0;
    const loop = (now: number) => {
      paint(now);
      handle = requestAnimationFrame(loop);
    };
    handle = requestAnimationFrame(loop);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(fit);
    observer?.observe(box);
    return () => {
      cancelAnimationFrame(handle);
      observer?.disconnect();
    };
  }, [lines, viewW, colour]);
  return <canvas ref={ref} aria-hidden="true" className="pointer-events-none absolute top-0 left-0 max-w-none" />;
}

/** A layer of `width` nodes, centred: each node's pitch, at most 12 units. */
function layerSpan(width: number): Span {
  const pitch = Math.min(12, SPAN / width);
  const height = pitch * width;
  const from = TOP + (SPAN - height) / 2;
  return { from, to: from + height };
}

function nodeY(span: Span, width: number, index: number): number {
  const pitch = (span.to - span.from) / width;
  return span.from + pitch * (index + 0.5);
}

/** Where each group sits within its layer. */
function groupSpans(groups: readonly Group[], span: Span, width: number): (Span & { group: Group; first: number })[] {
  const pitch = (span.to - span.from) / width;
  let index = 0;
  return groups.map((group) => {
    const from = span.from + pitch * index;
    const first = index;
    index += group.size;
    return { group, first, from, to: from + pitch * group.size };
  });
}

/** Each label's y: its group's centre, pushed down where it would crowd the label above. */
function labelYs(spans: readonly Span[]): number[] {
  const ys: number[] = [];
  for (const span of spans) {
    const centre = (span.from + span.to) / 2;
    const previous = ys.at(-1);
    ys.push(previous === undefined ? centre : Math.max(centre, previous + LABEL_GAP));
  }
  return ys;
}

type Active = { side: "in" | "out"; id: string } | null;

/**
 * The controller, intuitively: 104 senses in, 36 decisions out, through one
 * of the five catalog networks. Every organism has its own copy of these
 * weights; nothing here is shared.
 */
export function BrainAnatomy() {
  const [topologyIndex, setTopologyIndex] = useState(DEFAULT_TOPOLOGY);
  const [hovered, setHovered] = useState<Active>(null);
  const [pinned, setPinned] = useState<Active>(null);
  const active = hovered ?? pinned;
  const figure = useRef<HTMLDivElement>(null);
  const inView = useInView(figure);
  const reduced = useReducedMotion();
  const flowing = inView && !reduced;

  const topology = CATALOG[topologyIndex] ?? CATALOG[DEFAULT_TOPOLOGY]!;
  const layers = widths(topology);
  const parameters = parameterCount(layers);
  const { ref: box, width: drawn } = useFigureWidth(VIEW_W);
  const viewW = Math.round(Math.min(VIEW_W, Math.max(VIEW_W_LEAST, drawn * 1.4)));
  const right = viewW - RIGHT_MARGIN;
  // The footnote: 9.5 px at the least where a phone shrinks the drawing, and never under its 11 units where a wide
  // page enlarges it.
  const footnote = Math.max(11, (9.5 * viewW) / drawn);
  const columns = layers.map((width, index) => ({
    width,
    x: LEFT + ((right - LEFT) * index) / (layers.length - 1),
    span: layerSpan(width),
  }));
  const input = columns[0]!;
  const output = columns[columns.length - 1]!;
  const inputs = groupSpans(INPUT_GROUPS, input.span, input.width);
  const outputs = groupSpans(OUTPUT_GROUPS, output.span, output.width);

  // A few dozen connections per layer pair, for texture: the same on the server and in every browser.
  const lines = useMemo(() => {
    const random = seeded(9988 + topologyIndex);
    const result: { x1: number; y1: number; x2: number; y2: number; band: number }[] = [];
    for (let band = 0; band < columns.length - 1; band++) {
      const from = columns[band]!;
      const to = columns[band + 1]!;
      for (let line = 0; line < 46; line++) {
        const a = Math.floor(random() * from.width);
        const b = Math.floor(random() * to.width);
        result.push({
          x1: from.x + NODE_W / 2,
          y1: nodeY(from.span, from.width, a),
          x2: to.x - NODE_W / 2,
          y2: nodeY(to.span, to.width, b),
          band,
        });
      }
    }
    return result;
    // `columns` follows from the topology and the drawing's width.
  }, [topologyIndex, viewW]);

  const activeGroup =
    active === null
      ? null
      : (active.side === "in" ? inputs : outputs).find((entry) => entry.group.id === active.id) ?? null;
  const activeColour = active ? (COLOUR[active.id] ?? "#ffffff") : "#ffffff";

  /** The band between columns `band` and `band + 1`, narrowed to the active group where it touches it. */
  function bandPath(band: number): string {
    const from = columns[band]!;
    const to = columns[band + 1]!;
    let a = from.span;
    let b = to.span;
    if (activeGroup && active?.side === "in" && band === 0) a = activeGroup;
    if (activeGroup && active?.side === "out" && band === columns.length - 2) b = activeGroup;
    const x1 = from.x + NODE_W / 2;
    const x2 = to.x - NODE_W / 2;
    const mid = (x1 + x2) / 2;
    return `M${x1},${a.from} C${mid},${a.from} ${mid},${b.from} ${x2},${b.from} L${x2},${b.to} C${mid},${b.to} ${mid},${a.to} ${x1},${a.to} Z`;
  }

  const hiddenLabel = (index: number, width: number) =>
    index === 0 ? "104 in" : index === layers.length - 1 ? "36 out" : `${width} tanh`;

  const describe = (side: "in" | "out", group: Group) => {
    const list = side === "in" ? inputs : outputs;
    const entry = list.find((item) => item.group.id === group.id);
    const first = entry?.first ?? 0;
    const last = first + group.size - 1;
    return group.size === 1 ? `${side === "in" ? "input" : "output"} ${first}` : `${side === "in" ? "inputs" : "outputs"} ${first}–${last}`;
  };

  const chip = (side: "in" | "out", group: Group) => {
    const on = active?.side === side && active.id === group.id;
    const isPinned = pinned?.side === side && pinned.id === group.id;
    return (
      <li key={group.id}>
        <button
          type="button"
          aria-pressed={isPinned}
          onMouseEnter={() => setHovered({ side, id: group.id })}
          onMouseLeave={() => setHovered(null)}
          onFocus={() => setHovered({ side, id: group.id })}
          onBlur={() => setHovered(null)}
          onClick={() => setPinned(isPinned ? null : { side, id: group.id })}
          className={cn(
            "flex items-center gap-2 rounded-lg border px-2.5 py-1.5 text-left text-xs transition focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
            on ? "border-white/30 bg-white/10 text-white" : "border-white/10 bg-black/30 text-slate-300 hover:border-white/25",
          )}
        >
          <span aria-hidden="true" className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ background: COLOUR[group.id] }} />
          <span className="font-bold">{group.name}</span>
          <span className="font-mono text-[11px] text-slate-500 tabular-nums">×{group.size}</span>
        </button>
      </li>
    );
  };

  return (
    <DemoFrame
      eyebrow="The controller · plan §6.1–§6.4"
      title="104 senses in, 36 decisions out."
      basis="diagram"
      source={
        <>
          Inputs and outputs in the code&apos;s order: crates/flife-neural/src/sensors.rs
          (energy-moment-gradient-104-v1) and actions.rs. The five networks: crates/flife-genome/src/catalog.rs
          (dense-104-36-catalog-v1, plan §6.4). Parameter counts include biases and are exact arithmetic; the drawn
          connections are a sample of the dense layers&apos; full set.
        </>
      }
    >
      <div ref={figure} className="space-y-5">
        <div className="flex flex-wrap items-center gap-3">
          <span className="text-xs text-slate-400">Hidden layers</span>
          <Segmented
            label="The catalog network to draw"
            options={CATALOG.map((member, index) => ({
              value: String(index),
              label: member.hidden.join("·"),
              tone: index === DEFAULT_TOPOLOGY ? ("rule" as const) : ("neutral" as const),
            }))}
            value={String(topologyIndex)}
            onChange={(value) => setTopologyIndex(Number(value))}
          />
        </div>

        <div ref={box} className="relative mx-auto w-full max-w-[52rem]">
          {/* Under the figure, and in its box: the bands, and the connections at rest. */}
          <svg aria-hidden="true" viewBox={`0 0 ${viewW} ${VIEW_H}`} className="absolute inset-0 h-full w-full">
            {/* The dense bands between layers. */}
            {columns.slice(0, -1).map((_, band) => (
              <path
                key={`band-${band}`}
                d={bandPath(band)}
                fill={active ? activeColour : "#ffffff"}
                fillOpacity={active ? (band === 0 || band === columns.length - 2 ? 0.16 : 0.08) : 0.035}
                className="transition-[fill-opacity] duration-300"
              />
            ))}

            {/* A sample of the connections, as they are drawn where nothing flows. */}
            <g
              stroke={active ? activeColour : "#4ade80"}
              strokeOpacity={FLOW_OPACITY}
              strokeWidth={FLOW_WIDTH}
              strokeDasharray={DASH.join(" ")}
              style={flowing ? { display: "none" } : undefined}
            >
              {lines.map((line, index) => (
                <line key={index} x1={line.x1} y1={line.y1} x2={line.x2} y2={line.y2} />
              ))}
            </g>
          </svg>
          {/* The same connections, flowing left to right while on screen. */}
          {flowing ? <Flow lines={lines} viewW={viewW} colour={active ? activeColour : "#4ade80"} /> : null}
          <svg
            viewBox={`0 0 ${viewW} ${VIEW_H}`}
            className="diagram relative block h-auto w-full"
            role="img"
            aria-label={`The controller network ${shape(topology)}, ${grouped(parameters)} parameters. Inputs: energy, 32 composition values, 33 x gradients, 33 y gradients and five inputs about the organism itself. Outputs: reproduction rate, child share, emission gate, 32-dimensional emission direction and emission spending.`}
          >
            {/* Hidden layers: tanh neurons. */}
            {columns.slice(1, -1).map((column, index) => (
              <g key={`hidden-${index}`}>
                {Array.from({ length: column.width }, (_, node) => {
                  const pitch = (column.span.to - column.span.from) / column.width;
                  return (
                    <rect
                      key={node}
                      x={column.x - NODE_W / 2}
                      y={column.span.from + pitch * node + Math.min(0.6, pitch * 0.15)}
                      width={NODE_W}
                      height={Math.max(0.8, pitch - Math.min(1.2, pitch * 0.3))}
                      rx={Math.min(2, pitch / 3)}
                      fill="#cbd5e1"
                      fillOpacity={0.55}
                    />
                  );
                })}
              </g>
            ))}

            {/* Inputs and outputs, coloured by group. */}
            {[
              { side: "in" as const, list: inputs, column: input },
              { side: "out" as const, list: outputs, column: output },
            ].map(({ side, list, column }) => {
              const ys = labelYs(list);
              return list.map((entry, entryIndex) => {
                const on = active?.side === side && active.id === entry.group.id;
                const dim = active !== null && !on;
                const pitch = (entry.to - entry.from) / entry.group.size;
                const colour = COLOUR[entry.group.id] ?? "#ffffff";
                const edge = side === "in" ? column.x - NODE_W / 2 - 2 : column.x + NODE_W / 2 + 2;
                const labelX = side === "in" ? edge - 12 : edge + 12;
                const centre = (entry.from + entry.to) / 2;
                const labelY = ys[entryIndex] ?? centre;
                return (
                  <g
                    key={`${side}-${entry.group.id}`}
                    onMouseEnter={() => setHovered({ side, id: entry.group.id })}
                    onMouseLeave={() => setHovered(null)}
                    onClick={() =>
                      setPinned(pinned?.side === side && pinned.id === entry.group.id ? null : { side, id: entry.group.id })
                    }
                    className="cursor-pointer"
                    opacity={dim ? 0.35 : 1}
                  >
                    {/* A generous hit area. */}
                    <rect
                      x={side === "in" ? 0 : column.x - NODE_W}
                      y={entry.from}
                      width={side === "in" ? column.x + NODE_W : viewW - column.x + NODE_W}
                      height={Math.max(6, entry.to - entry.from)}
                      fill="transparent"
                    />
                    {Array.from({ length: entry.group.size }, (_, node) => (
                      <rect
                        key={node}
                        x={column.x - NODE_W / 2}
                        y={entry.from + pitch * node + Math.min(0.4, pitch * 0.12)}
                        width={NODE_W}
                        height={Math.max(0.9, pitch - Math.min(0.8, pitch * 0.24))}
                        rx={Math.min(2, pitch / 3)}
                        fill={colour}
                        fillOpacity={on ? 1 : 0.85}
                      />
                    ))}
                    <path
                      d={`M${edge},${centre} L${side === "in" ? edge - 6 : edge + 6},${labelY} L${side === "in" ? labelX + 2 : labelX - 2},${labelY}`}
                      fill="none"
                      stroke={colour}
                      strokeOpacity={0.5}
                      strokeWidth={0.8}
                    />
                    <text
                      x={labelX}
                      y={labelY + 4}
                      textAnchor={side === "in" ? "end" : "start"}
                      fontSize={14}
                      fontWeight={700}
                      fill={colour}
                    >
                      {entry.group.short}
                    </text>
                  </g>
                );
              });
            })}

            {/* Layer captions, raised by as much as a phone enlarges the footnote under them. */}
            {columns.map((column, index) => (
              <text
                key={`caption-${index}`}
                x={column.x}
                y={VIEW_H - 7 - footnote}
                textAnchor="middle"
                fontSize={14}
                className="mono"
                fill={index === 0 || index === columns.length - 1 ? "#e2e8f0" : "#94a3b8"}
              >
                {hiddenLabel(index, column.width)}
              </text>
            ))}
            <text x={output.x} y={VIEW_H - 2} textAnchor="middle" fontSize={footnote} fill="#64748b">
              linear · σ once, in decoding
            </text>
          </svg>
        </div>

        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <div>
            <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">Senses · 104</p>
            <ul className="mt-2 flex flex-wrap gap-1.5">{INPUT_GROUPS.map((group) => chip("in", group))}</ul>
          </div>
          <div>
            <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">Decisions · 36</p>
            <ul className="mt-2 flex flex-wrap gap-1.5">{OUTPUT_GROUPS.map((group) => chip("out", group))}</ul>
          </div>
        </div>

        <div className="min-h-[5.5rem] rounded-2xl border border-white/10 bg-black/30 p-4" aria-live="polite">
          {activeGroup && active ? (
            <>
              <p className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <span className="font-bold text-white" style={{ color: activeColour }}>
                  {activeGroup.group.name}
                </span>
                <span className="font-mono text-xs text-slate-400">{activeGroup.group.symbol}</span>
                <span className="font-mono text-xs text-slate-500">{describe(active.side, activeGroup.group)}</span>
              </p>
              <p className="mt-1.5 text-sm leading-relaxed text-slate-300">{activeGroup.group.description}</p>
            </>
          ) : (
            <p className="text-sm leading-relaxed text-slate-300">
              Every input reaches every neuron of the first hidden layer, and every layer the next: a dense network
              with tanh hidden layers and a linear output. Point at a group, or tab to one, to see what it carries.
              The colours pair what it reads with what it writes: chemistry in, chemistry out; energy in, energy
              spent.
            </p>
          )}
        </div>

        <div className="grid grid-cols-1 gap-3 lg:grid-cols-[1.4fr_1fr]">
          <div className="rounded-2xl border border-white/10 bg-black/30 p-4">
            <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">Its own parameters</p>
            <p className="mt-2 font-mono text-xs leading-relaxed break-words text-slate-400">
              {parameterSum(layers)} ={" "}
              <span className="text-base text-green-300 tabular-nums">{grouped(parameters)}</span>
            </p>
            <p className="mt-2 text-xs leading-relaxed text-slate-400">
              {shape(topology)}: {grouped(parameters * 4)} bytes of F32 weights and biases that belong to this organism
              alone. Offspring move between the five networks by declared maps; a widened copy computes exactly what
              its parent did (before the child&apos;s own mutations), while a change of depth claims no such thing (plan §6.4, fixture C.16).
            </p>
          </div>
          <div className="rounded-2xl border border-amber-300/20 bg-amber-300/[0.04] p-4">
            <p className="text-[10px] font-black tracking-[0.2em] text-amber-200/80 uppercase">No motor output</p>
            <p className="mt-2 text-xs leading-relaxed text-slate-300">
              Nothing here steers. An organism moves by inherited responses to the same energy and chemical
              gradients, scaled by how much it was just granted to eat (plan §5.4). The network decides when to
              split, what to give the child, and what to write into the field.
            </p>
          </div>
        </div>
      </div>
    </DemoFrame>
  );
}
