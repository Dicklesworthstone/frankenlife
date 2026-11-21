"use client";

import {
  ArrowRight,
  Brain,
  Droplets,
  Eye,
  Grid3x3,
  type LucideIcon,
  Navigation,
  Pause,
  Play,
  Skull,
  Split,
  Sun,
  Utensils,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { DemoFrame } from "@/components/explain/demo";
import { lifeCycle } from "@/lib/content";
import { useInView, useReducedMotion } from "@/lib/hooks";
import { LINEAGE, TONE } from "@/lib/palette";
import { cn } from "@/lib/utils";

const ICONS: Record<string, LucideIcon> = {
  sources: Sun,
  field: Grid3x3,
  sense: Eye,
  decide: Brain,
  feed: Utensils,
  move: Navigation,
  emit: Droplets,
  divide: Split,
  die: Skull,
};

const SIZE = 440;
const CENTER = SIZE / 2;
const RING = 168;
const NODE = 25;
const STEP_MS = 3600;

/** The step's place on the ring, clockwise from the top. */
function place(index: number, radius = RING): [number, number] {
  const angle = (index / lifeCycle.length) * 2 * Math.PI - Math.PI / 2;
  return [CENTER + radius * Math.cos(angle), CENTER + radius * Math.sin(angle)];
}

/** An arc of the ring from step `from` to step `to`, clockwise, stopping short of both nodes. */
function arc(from: number, to: number): string {
  const n = lifeCycle.length;
  const gap = (NODE + 6) / RING;
  const start = (from / n) * 2 * Math.PI - Math.PI / 2 + gap;
  const end = (to / n) * 2 * Math.PI - Math.PI / 2 - gap + (to <= from ? 2 * Math.PI : 0);
  const [x0, y0] = [CENTER + RING * Math.cos(start), CENTER + RING * Math.sin(start)];
  const [x1, y1] = [CENTER + RING * Math.cos(end), CENTER + RING * Math.sin(end)];
  const large = end - start > Math.PI ? 1 : 0;
  return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${RING} ${RING} 0 ${large} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}

const tone = (actor: "field" | "organism") => (actor === "field" ? TONE.chemistry : TONE.rule);

/**
 * One organism's life as the ordered tick runs it: what enters the field,
 * what each organism does with it, and how it returns. The ring is the
 * shared field; the organism in the middle is one of hundreds of thousands,
 * each with its own network.
 */
export function LifeCycle() {
  const [active, setActive] = useState(0);
  const [playing, setPlaying] = useState(true);
  const reduced = useReducedMotion();
  const frame = useRef<HTMLDivElement>(null);
  const inView = useInView(frame);
  const running = playing && !reduced && inView;

  useEffect(() => {
    if (!running) return;
    const handle = window.setInterval(() => setActive((step) => (step + 1) % lifeCycle.length), STEP_MS);
    return () => window.clearInterval(handle);
  }, [running]);

  const choose = (index: number) => {
    setActive(index);
    setPlaying(false);
  };

  const step = lifeCycle[active] ?? lifeCycle[0]!;
  const Icon = ICONS[step.id] ?? Sun;
  const previous = (active + lifeCycle.length - 1) % lifeCycle.length;

  return (
    <DemoFrame
      eyebrow="FieldEcologyV1 · one organism, one tick at a time"
      title="Sense, decide, feed, move, emit, divide, die."
      basis="diagram"
      source="README “How the world works” and “One tick”; plan §4–§8. Steps in the order the ordered tick runs them."
    >
      <div ref={frame} className="grid grid-cols-1 items-center gap-6 lg:grid-cols-[minmax(0,1.05fr)_minmax(0,1fr)]">
        <svg
          viewBox={`-74 -10 ${SIZE + 148} ${SIZE + 22}`}
          className="diagram mx-auto w-full max-w-[520px]"
          role="img"
          aria-label={`A ring of the nine steps of an organism's life, from sources through field, sense, decide, feed, move, emit and divide to death, which returns energy to the field. Showing step ${active + 1}: ${step.name}.`}
        >
          <defs>
            <radialGradient id="life-field" cx="0.5" cy="0.5" r="0.5">
              <stop offset="0" stopColor="#22c55e" stopOpacity="0.16" />
              <stop offset="0.62" stopColor="#0e7490" stopOpacity="0.08" />
              <stop offset="1" stopColor="#020a02" stopOpacity="0" />
            </radialGradient>
            <radialGradient id="life-cell" cx="0.38" cy="0.34" r="0.75">
              <stop offset="0" stopColor="#bef264" stopOpacity="0.95" />
              <stop offset="0.5" stopColor="#22c55e" stopOpacity="0.55" />
              <stop offset="1" stopColor="#052e16" stopOpacity="0.2" />
            </radialGradient>
            <marker id="life-arrow" viewBox="0 0 10 10" refX="6" refY="5" markerWidth="5" markerHeight="5" orient="auto">
              <path d="M0 0 L10 5 L0 10 z" fill="#4ade80" />
            </marker>
          </defs>

          <circle cx={CENTER} cy={CENTER} r={RING + 40} fill="url(#life-field)" />
          {/* The field: a faint grid, the one medium every organism shares. */}
          <g opacity="0.22" stroke="#67e8f9" strokeWidth="0.6">
            {Array.from({ length: 9 }, (_, i) => {
              const offset = CENTER - 112 + i * 28;
              return (
                <g key={i}>
                  <line x1={offset} y1={CENTER - 112} x2={offset} y2={CENTER + 112} />
                  <line x1={CENTER - 112} y1={offset} x2={CENTER + 112} y2={offset} />
                </g>
              );
            })}
          </g>

          {/* The ring and the step just taken. */}
          <circle cx={CENTER} cy={CENTER} r={RING} fill="none" stroke="rgba(255,255,255,0.09)" strokeWidth="2" />
          <path
            key={`arc-${active}`}
            d={arc(previous, active)}
            fill="none"
            stroke="#4ade80"
            strokeWidth="3"
            strokeLinecap="round"
            markerEnd="url(#life-arrow)"
            className={cn(!reduced && "life-arc")}
          />

          {/* Neighbours: other lineages, each with its own network, reached only through the field. */}
          {[0, 1, 2, 4, 6, 7].map((slot, i) => {
            const angle = (i / 6) * 2 * Math.PI + 0.4;
            const x = CENTER + 92 * Math.cos(angle);
            const y = CENTER + 92 * Math.sin(angle);
            return (
              <g key={slot} opacity={step.id === "emit" || step.id === "feed" ? 0.95 : 0.55}>
                <circle cx={x} cy={y} r={7} fill={LINEAGE[slot]} fillOpacity="0.28" stroke={LINEAGE[slot]} strokeWidth="1.2" />
                <circle cx={x} cy={y} r={2.4} fill={LINEAGE[slot]} />
              </g>
            );
          })}

          {/* The organism this ring follows. */}
          <circle
            cx={CENTER}
            cy={CENTER}
            r={step.id === "die" ? 30 : 38}
            fill={step.id === "die" ? "rgba(148,163,184,0.12)" : "url(#life-cell)"}
            stroke={step.id === "die" ? TONE.dissipated : "#4ade80"}
            strokeWidth="1.5"
            strokeDasharray={step.id === "die" ? "4 4" : undefined}
            style={{ transition: "r 600ms ease" }}
          />
          {step.id === "divide" ? (
            <circle cx={CENTER + 46} cy={CENTER - 30} r={15} fill="url(#life-cell)" stroke="#4ade80" strokeWidth="1.2" />
          ) : null}
          {/* Its network, drawn small: 104 in, two hidden layers, 36 out. */}
          <g opacity={step.id === "die" ? 0.25 : 0.9}>
            {[-16, 0, 16].map((dx, layer) =>
              [-12, -4, 4, 12].slice(0, layer === 1 ? 3 : 4).map((dy) => (
                <circle key={`${layer}-${dy}`} cx={CENTER + dx} cy={CENTER + dy + (layer === 1 ? 4 : 0)} r={2.2} fill="#ecfccb" />
              )),
            )}
          </g>
          <text x={CENTER} y={CENTER + 64} textAnchor="middle" className="mono" fontSize="13" fill="#94a3b8">
            {step.phases}
          </text>

          {/* The nine steps. */}
          {lifeCycle.map((entry, index) => {
            const [x, y] = place(index);
            const StepIcon = ICONS[entry.id] ?? Sun;
            const on = index === active;
            const color = tone(entry.actor);
            const [lx, ly] = place(index, RING + NODE + 17);
            return (
              <g key={entry.id} onClick={() => choose(index)} style={{ cursor: "pointer" }}>
                <circle
                  cx={x}
                  cy={y}
                  r={NODE}
                  fill={on ? color : "#051205"}
                  stroke={color}
                  strokeOpacity={on ? 1 : 0.45}
                  strokeWidth="1.5"
                  style={{ filter: on ? `drop-shadow(0 0 10px ${color})` : undefined, transition: "fill 300ms" }}
                />
                <StepIcon
                  x={x - 10}
                  y={y - 10}
                  width={20}
                  height={20}
                  color={on ? TONE.ink : color}
                  strokeWidth={2}
                  aria-hidden="true"
                />
                <text
                  x={lx}
                  y={ly + 4}
                  textAnchor={Math.abs(lx - CENTER) < 30 ? "middle" : lx > CENTER ? "start" : "end"}
                  fontSize="16"
                  fontWeight={on ? 800 : 600}
                  fill={on ? "#ffffff" : "#94a3b8"}
                >
                  {entry.name}
                </text>
              </g>
            );
          })}
        </svg>

        <div className="min-w-0 space-y-4">
          <div aria-live="polite" className="min-h-[13.5rem] rounded-2xl border border-white/10 bg-black/30 p-5">
            <div className="flex items-center gap-3">
              <span
                className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl"
                style={{ background: `${tone(step.actor)}22`, color: tone(step.actor) }}
              >
                <Icon className="h-5 w-5" aria-hidden="true" />
              </span>
              <div>
                <p className="text-[10px] font-black tracking-[0.2em] uppercase" style={{ color: tone(step.actor) }}>
                  {String(active + 1).padStart(2, "0")} · {step.actor === "field" ? "the shared field" : "one organism"}
                </p>
                <p className="text-xl font-black text-white">{step.name}</p>
              </div>
            </div>
            <p className="mt-4 text-sm leading-relaxed text-slate-300">{step.body}</p>
            {step.link ? (
              <a
                href={step.link.href}
                className="mt-4 inline-flex items-center gap-1.5 text-sm text-green-300 underline-offset-4 hover:underline"
              >
                {step.link.label} <ArrowRight className="h-3.5 w-3.5" aria-hidden="true" />
              </a>
            ) : null}
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => setPlaying((on) => !on)}
              disabled={reduced}
              aria-label={playing && !reduced ? "Stop stepping through the cycle" : "Step through the cycle"}
              className="flex h-8 w-8 items-center justify-center rounded-lg border border-white/15 text-slate-300 transition hover:border-green-400/50 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none disabled:opacity-40"
            >
              {playing && !reduced ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
            </button>
            <ol className="flex flex-wrap gap-1.5" aria-label="The steps">
              {lifeCycle.map((entry, index) => (
                <li key={entry.id}>
                  <button
                    type="button"
                    aria-current={index === active ? "step" : undefined}
                    onClick={() => choose(index)}
                    className={cn(
                      "rounded-lg border px-2.5 py-1 text-xs font-bold transition focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                      index === active
                        ? "border-green-400/60 bg-green-500/15 text-green-200"
                        : "border-white/10 text-slate-400 hover:border-white/25 hover:text-white",
                    )}
                  >
                    {entry.name}
                  </button>
                </li>
              ))}
            </ol>
          </div>

          <p className="text-xs leading-relaxed text-slate-400">
            <span className="font-bold text-cyan-200">The ring is the field.</span> In the baseline model an organism
            never touches another directly: no collisions, no attraction tables, no predation. Everything one does to
            another, it does by eating from the field or writing into it.
          </p>
        </div>
      </div>
    </DemoFrame>
  );
}
