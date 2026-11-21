"use client";

// The fixture bench's panels (fixture-bench.tsx): one rule each, beside its
// plausible wrong version, computed by fixture-math.ts on the reader's numbers.

import { CircleCheck, CircleDashed, Play, RotateCcw, Shuffle } from "lucide-react";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";

import { DemoButton, Readout, Segmented, Slider, fixed } from "@/components/explain/demo";
import {
  C9_CHEMISTRY,
  FIXTURES,
  type Packet,
  birthChargeFirst,
  birthRule,
  decodeLinear,
  decodeTanhFirst,
  efficiency,
  minimumImage,
  minimumImageBetween,
  mix,
  mixtureDot,
  norm,
  packet,
  settle,
  settleMixture,
  settlePackets,
  settleRequested,
  signedFood,
  threadOrder,
  unambiguous,
  uptakeInOrder,
  uptakeRacing,
  uptakeRule,
  usefulGains,
  wrap,
} from "@/components/explain/fixture-math";
import { useFigureWidth, useReducedMotion } from "@/lib/hooks";
import { LINEAGE, TONE } from "@/lib/palette";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Shared parts.

const near = (a: number, b: number, tolerance = 1e-9) => Math.abs(a - b) <= tolerance;
const signed = (value: number, digits = 3) => `${value > 0 ? "+" : value < 0 ? "−" : ""}${fixed(Math.abs(value), digits)}`;

/** The rule and its wrong version, in a sentence each. */
function Versions({ rule, wrong }: { rule: ReactNode; wrong: ReactNode }) {
  return (
    <div className="grid grid-cols-1 gap-2 text-sm leading-relaxed sm:grid-cols-2">
      <p className="rounded-xl border border-green-500/25 bg-green-500/[0.06] px-3.5 py-2.5 text-slate-200">
        <span className="mr-1.5 text-[10px] font-black tracking-[0.18em] text-green-300 uppercase">The rule</span>
        {rule}
      </p>
      <p className="rounded-xl border border-rose-400/25 bg-rose-400/[0.06] px-3.5 py-2.5 text-slate-200">
        <span className="mr-1.5 text-[10px] font-black tracking-[0.18em] text-rose-300 uppercase">Plausible, wrong</span>
        {wrong}
      </p>
    </div>
  );
}

/**
 * Whether the figure is at the fixture's numbers and, if so, whether the
 * computed result equals the fixture's stated answer. Both are computed:
 * nothing here is printed as passing.
 */
function FixtureCheck({
  fixture,
  statement,
  atFixture,
  matches,
  onReset,
}: {
  fixture: string;
  statement: ReactNode;
  atFixture: boolean;
  matches: boolean;
  onReset: () => void;
}) {
  const state = !atFixture ? "off" : matches ? "match" : "mismatch";
  return (
    <div
      className={cn(
        "flex flex-wrap items-center justify-between gap-3 rounded-xl border px-4 py-3",
        state === "match" && "border-green-500/30 bg-green-500/[0.07]",
        state === "off" && "border-white/10 bg-black/20",
        state === "mismatch" && "border-rose-400/40 bg-rose-400/10",
      )}
      aria-live="polite"
    >
      <p className="flex min-w-0 items-start gap-2 text-sm leading-relaxed">
        {state === "match" ? (
          <CircleCheck className="mt-0.5 h-4 w-4 shrink-0 text-green-400" aria-hidden="true" />
        ) : (
          <CircleDashed className="mt-0.5 h-4 w-4 shrink-0 text-slate-500" aria-hidden="true" />
        )}
        <span className={state === "match" ? "text-green-200" : state === "mismatch" ? "text-rose-200" : "text-slate-400"}>
          {state === "match" ? (
            <>
              Matches fixture {fixture}: {statement}
            </>
          ) : state === "mismatch" ? (
            <>
              At {fixture}&apos;s numbers this page does not reproduce its answer ({statement}). That is a bug in this page.
            </>
          ) : (
            <>
              Off the fixture&apos;s numbers. {fixture} states: {statement}
            </>
          )}
        </span>
      </p>
      <DemoButton onClick={onReset} disabled={atFixture} label={`Reset to fixture ${fixture}`}>
        <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" /> Reset to {fixture}
      </DemoButton>
    </div>
  );
}

/** A line from `from` to `to` with a filled arrowhead, in SVG units. */
function Arrow({ from, to, color, width = 2.5 }: { from: [number, number]; to: [number, number]; color: string; width?: number }) {
  const dx = to[0] - from[0];
  const dy = to[1] - from[1];
  const length = Math.hypot(dx, dy);
  if (length < 0.5) return <circle cx={from[0]} cy={from[1]} r={2.5} fill={color} />;
  const ux = dx / length;
  const uy = dy / length;
  const head = Math.min(9, length * 0.6);
  const base: [number, number] = [to[0] - ux * head, to[1] - uy * head];
  const left: [number, number] = [base[0] - uy * head * 0.55, base[1] + ux * head * 0.55];
  const right: [number, number] = [base[0] + uy * head * 0.55, base[1] - ux * head * 0.55];
  return (
    <g>
      <line x1={from[0]} y1={from[1]} x2={base[0]} y2={base[1]} stroke={color} strokeWidth={width} strokeLinecap="round" />
      <polygon points={`${to[0]},${to[1]} ${left[0]},${left[1]} ${right[0]},${right[1]}`} fill={color} />
    </g>
  );
}

/** An SVG path through `f` on `[x0, x1]`, mapped by `X` and `Y`. */
function curve(f: (x: number) => number, x0: number, x1: number, X: (x: number) => number, Y: (y: number) => number, steps = 200) {
  let d = "";
  for (let i = 0; i <= steps; i++) {
    const x = x0 + ((x1 - x0) * i) / steps;
    d += `${i === 0 ? "M" : "L"}${X(x).toFixed(2)},${Y(f(x)).toFixed(2)}`;
  }
  return d;
}

const ORGANISMS = [
  { name: "A", color: LINEAGE[0] },
  { name: "B", color: LINEAGE[1] },
  { name: "C", color: LINEAGE[4] },
  { name: "D", color: LINEAGE[7] },
] as const;

/** Useful efficiencies η: A and B are C.1's; C and D are this page's. */
const EFFICIENCIES = [0.5, 0.25, 0.75, 0.4] as const;

// ---------------------------------------------------------------------------
// C.1 contended uptake.

type UptakeMode = "order" | "racing";

const C1_DEFAULT = { count: 2, cell: 10, requests: [8, 12, 6, 4], mode: "order" as UptakeMode, order: 0 };

export function UptakePanel() {
  const [count, setCount] = useState(C1_DEFAULT.count);
  const [cell, setCell] = useState(C1_DEFAULT.cell);
  const [requests, setRequests] = useState<number[]>(C1_DEFAULT.requests);
  const [mode, setMode] = useState<UptakeMode>(C1_DEFAULT.mode);
  const [orderIndex, setOrderIndex] = useState(C1_DEFAULT.order);

  const active = requests.slice(0, count);
  const order = threadOrder(count, orderIndex);
  const rule = uptakeRule(cell, active);
  const wrong = mode === "order" ? uptakeInOrder(cell, active, order) : uptakeRacing(cell, active);
  const ruleGrants = rule.grants.map((grant) => grant.granted);
  const wrongGrants = wrong.grants.map((grant) => grant.granted);
  const ledger = usefulGains(ruleGrants, EFFICIENCIES);
  const ruleTotal = ruleGrants.reduce((sum, grant) => sum + grant, 0);
  const wrongTotal = wrongGrants.reduce((sum, grant) => sum + grant, 0);

  const atFixture =
    count === FIXTURES.c1.requests.length &&
    cell === FIXTURES.c1.cell &&
    FIXTURES.c1.requests.every((request, id) => active[id] === request);
  const matches =
    FIXTURES.c1.grants.every((grant, id) => near(ruleGrants[id] ?? Number.NaN, grant)) &&
    FIXTURES.c1.gains.every((gain, id) => near(ledger.gains[id] ?? Number.NaN, gain)) &&
    near(ledger.dissipated, FIXTURES.c1.dissipated);

  const reset = () => {
    setCount(C1_DEFAULT.count);
    setCell(C1_DEFAULT.cell);
    setRequests(C1_DEFAULT.requests);
    setMode(C1_DEFAULT.mode);
    setOrderIndex(C1_DEFAULT.order);
  };
  const shuffle = () => {
    // The next order in a fixed sequence that differs from this one.
    const current = order.join();
    for (let next = orderIndex + 1; next < orderIndex + 64; next++) {
      if (threadOrder(count, next).join() !== current) {
        setOrderIndex(next);
        return;
      }
    }
  };
  const orderText = order.map((id) => ORGANISMS[id]?.name ?? "?").join(" → ");

  return (
    <div className="space-y-5">
      <Versions
        rule={
          <>
            Sum every request into the cell&apos;s demand <i>A</i>, then grant each organism <i>α q</i> with{" "}
            <i>α</i> = min(1, <i>e</i>/<i>A</i>), in whole quanta. No order of threads can change who eats.
          </>
        }
        wrong={
          <>
            One pass: each thread subtracts its request and clamps the cell at zero. Who eats depends on who ran
            first, and racing threads can spend the same energy twice.
          </>
        }
      />
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <UptakeFigure
          title="The rule · any thread order"
          tone="rule"
          cell={cell}
          requests={active}
          grants={ruleGrants}
          stack={active.map((_, id) => id)}
        />
        <UptakeFigure
          title={mode === "order" ? `Wrong · one pass, ${orderText}` : "Wrong · racing threads"}
          tone="wrong"
          cell={cell}
          requests={active}
          grants={wrongGrants}
          stack={mode === "order" ? order : active.map((_, id) => id)}
          created={wrong.created}
        />
      </div>
      <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
        <Readout label="Rule: grants" value={ruleGrants.map((grant) => fixed(grant)).join(" · ")} tone="rule" note={`α = ${fixed(rule.alpha, 3)}`} />
        <Readout
          label="Wrong: grants"
          value={wrongGrants.map((grant) => fixed(grant)).join(" · ")}
          tone="wrong"
          note={mode === "order" ? `in order ${orderText}` : "each read the same cell"}
        />
        <Readout
          label="Rule: ledger"
          value={`−${fixed(ruleTotal)} = ${fixed(ledger.gains.reduce((sum, gain) => sum + gain, 0))} + ${fixed(ledger.dissipated)}`}
          note="field loss = useful gains + dissipation"
        />
        <Readout
          label="Wrong: energy"
          value={wrong.created > 0 ? `+${fixed(wrong.created)} created` : `${fixed(wrongTotal)} of ${fixed(cell)}`}
          tone={wrong.created > 0 ? "wrong" : "neutral"}
          note={wrong.created > 0 ? `granted ${fixed(wrongTotal)} from a cell of ${fixed(cell)}, then clamped to 0` : "granted from the cell"}
        />
      </div>
      <div className="grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2">
        <div className="space-y-4">
          <Segmented
            label="Organisms on the cell"
            options={[2, 3, 4].map((n) => ({ value: String(n), label: `${n} organisms` }))}
            value={String(count)}
            onChange={(value) => {
              setCount(Number(value));
              setOrderIndex(0);
            }}
          />
          <Slider label="Energy in the cell, e" value={cell} min={1} max={20} onChange={setCell} />
          <Segmented
            label="Wrong version"
            options={[
              { value: "order", label: "One pass, in order", tone: "wrong" },
              { value: "racing", label: "Racing threads", tone: "wrong" },
            ]}
            value={mode}
            onChange={setMode}
          />
          <div className="flex flex-wrap gap-2">
            <DemoButton onClick={shuffle} disabled={mode !== "order"}>
              <Shuffle className="h-3.5 w-3.5" aria-hidden="true" /> Shuffle thread order
            </DemoButton>
            <DemoButton
              onClick={() => {
                setCount(3);
                setCell(10);
                setRequests([5, 5, 5, requests[3] ?? 4]);
                setOrderIndex(0);
              }}
            >
              Three-way tie
            </DemoButton>
          </div>
        </div>
        <div className="space-y-4">
          {active.map((request, id) => (
            <Slider
              key={ORGANISMS[id]?.name}
              label={
                <span className="inline-flex items-center gap-1.5">
                  <span className="h-2 w-2 rounded-full" style={{ background: ORGANISMS[id]?.color }} aria-hidden="true" />
                  {ORGANISMS[id]?.name} requests (η = {EFFICIENCIES[id]})
                </span>
              }
              value={request}
              min={0}
              max={16}
              onChange={(value) => setRequests((all) => all.map((old, index) => (index === id ? value : old)))}
            />
          ))}
          <p className="text-[11px] leading-relaxed text-slate-500">
            Quanta here are whole units; in FrankenLife a quantum is the cell&apos;s F32 unit in the last place. Leftover
            quanta go to the largest remainders, ties to the lower identity (try the three-way tie). η for A and B are
            C.1&apos;s; C&apos;s and D&apos;s are this page&apos;s.
          </p>
        </div>
      </div>
      <FixtureCheck
        fixture="C.1"
        statement="grants 4 and 6, gains 2 and 1.5, dissipation 6.5"
        atFixture={atFixture}
        matches={matches}
        onReset={reset}
      />
    </div>
  );
}

export function UptakeFigure({
  title,
  tone,
  cell,
  requests,
  grants,
  stack,
  created = 0,
}: {
  title: string;
  tone: "rule" | "wrong";
  cell: number;
  requests: number[];
  grants: number[];
  /** The order the grants are drawn into the cell's bar. */
  stack: number[];
  created?: number;
}) {
  const { ref, width: across } = useFigureWidth(300);
  // Bars run from `left` to two units short of the edge; a request of 268 / 11 fills them, as at 300 across.
  const left = 30;
  const span = across - 32;
  const scale = (11 * span) / 268;
  const rowHeight = 30;
  const height = 64 + requests.length * rowHeight;
  const colour = tone === "rule" ? TONE.rule : TONE.wrong;
  let cursor = left;
  const segments = stack.map((id) => {
    const width = (grants[id] ?? 0) * scale;
    const segment = { id, x: cursor, width };
    cursor += width;
    return segment;
  });
  const cellEnd = left + cell * scale;
  const label = `${title}. Cell of ${fixed(cell)}; ${requests
    .map((request, id) => `${ORGANISMS[id]?.name} asks ${fixed(request)}, gets ${fixed(grants[id] ?? 0)}`)
    .join("; ")}${created > 0 ? `; ${fixed(created)} units created` : ""}.`;
  return (
    <div className={cn("rounded-2xl border bg-black/25 p-3", tone === "rule" ? "border-green-500/20" : "border-rose-400/20")}>
      <p className={cn("mb-2 text-[10px] font-black tracking-[0.16em] uppercase", tone === "rule" ? "text-green-300" : "text-rose-300")}>
        {title}
      </p>
      <div ref={ref}>
        <svg viewBox={`0 0 ${across} ${height}`} className="diagram block h-auto w-full" role="img" aria-label={label}>
          <defs>
            <pattern id={`hatch-${tone}`} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
              <line x1="0" y1="0" x2="0" y2="6" stroke={TONE.wrong} strokeWidth="2" />
            </pattern>
          </defs>
          <text x={0} y={22} fontSize={11} fill="#94a3b8">
            cell
          </text>
          {/* What the cell holds, outlined; what it gives, coloured by organism; what is left, faint. */}
          <rect x={left} y={10} width={cell * scale} height={16} rx={3} fill={TONE.energy} fillOpacity={0.12} stroke={TONE.energy} strokeOpacity={0.6} />
          {/* The cell can only give what it holds; anything granted past its end was created (the hatch). */}
          {segments.map((segment) => (
            <rect
              key={segment.id}
              x={segment.x}
              y={10}
              width={Math.max(0, Math.min(segment.width, cellEnd - segment.x))}
              height={16}
              fill={ORGANISMS[segment.id]?.color}
              fillOpacity={0.85}
            />
          ))}
          {created > 0 ? (
            <>
              <rect
                x={cellEnd}
                y={10}
                width={Math.max(0, Math.min(created * scale, across - 2 - cellEnd))}
                height={16}
                fill={`url(#hatch-${tone})`}
                stroke={TONE.wrong}
              />
              <text x={Math.min(cellEnd + 4, across - 80)} y={44} fontSize={11} fill={TONE.wrong} className="mono">
                +{fixed(created)} from nothing
              </text>
            </>
          ) : null}
          <line x1={cellEnd} y1={6} x2={cellEnd} y2={30} stroke={TONE.energy} strokeWidth={1.5} />
          {requests.map((request, id) => {
            const y = 58 + id * rowHeight;
            const grant = grants[id] ?? 0;
            const organism = ORGANISMS[id];
            return (
              <g key={organism?.name ?? id}>
                <circle cx={10} cy={y + 6} r={8} fill={organism?.color} />
                <text x={10} y={y + 10} fontSize={10} fontWeight={800} textAnchor="middle" fill={TONE.ink}>
                  {organism?.name}
                </text>
                <rect
                  x={left}
                  y={y}
                  width={Math.min(request * scale, span)}
                  height={12}
                  rx={3}
                  fill="none"
                  stroke={organism?.color}
                  strokeOpacity={0.6}
                  strokeDasharray="3 3"
                />
                <rect x={left} y={y} width={Math.min(grant * scale, span)} height={12} rx={3} fill={organism?.color} fillOpacity={grant > 0 ? 0.85 : 0} />
                <text
                  x={Math.min(left + Math.max(request, grant) * scale + 6, across - 38)}
                  y={y + 10}
                  fontSize={11}
                  fill={grant === 0 && request > 0 ? colour : "#e2e8f0"}
                  className="mono"
                >
                  {fixed(request)}→{fixed(grant)}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// C.3 opposing chemical signatures.

const C3_DEFAULT = { a: 3, b: 5, angle: 180 };

export function SignalsPanel() {
  const [energyA, setEnergyA] = useState(C3_DEFAULT.a);
  const [energyB, setEnergyB] = useState(C3_DEFAULT.b);
  const [angle, setAngle] = useState(C3_DEFAULT.angle);

  const a = packet(energyA, 0);
  const b = packet(energyB, (angle * Math.PI) / 180);
  const cell = mix([a, b]);
  const food = signedFood([a, b]);
  const moment = norm(cell.m);

  const atFixture = energyA === FIXTURES.c3.a.e && energyB === FIXTURES.c3.b.e && angle === 180;
  const matches = near(cell.e, FIXTURES.c3.energy) && near(cell.m[0], FIXTURES.c3.moment) && near(cell.m[1], 0);
  const reset = () => {
    setEnergyA(C3_DEFAULT.a);
    setEnergyB(C3_DEFAULT.b);
    setAngle(C3_DEFAULT.angle);
  };

  return (
    <div className="space-y-5">
      <Versions
        rule={
          <>
            A cell holds spendable energy <i>e</i> ≥ 0 and a signed chemical moment <i>m</i> with ‖<i>m</i>‖ ≤ <i>e</i>.
            Mixing adds both: opposite chemistry cancels, energy never does.
          </>
        }
        wrong={
          <>
            Let the signed chemical channels be the food. Then two opposite signals annihilate each other&apos;s food,
            and mixing destroys (or, undone, creates) energy.
          </>
        }
      />
      <SignalsFigure a={a} b={b} cell={cell} food={food} />
      <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
        <Readout label="Rule: cell energy" value={fixed(cell.e)} tone="rule" note={`${fixed(energyA)} + ${fixed(energyB)}, whatever the chemistry`} />
        <Readout label="Rule: chemistry ‖m‖" value={fixed(moment)} note={`≤ ${fixed(cell.e)}: inside the cone`} />
        <Readout
          label="Wrong: food left"
          value={fixed(food)}
          tone={near(food, cell.e) ? "neutral" : "wrong"}
          note={near(food, cell.e) ? "aligned: nothing cancels" : `${fixed(cell.e - food)} units destroyed by mixing`}
        />
        <Readout label="Composition z = m/e" value={fixed(moment / cell.e, 3)} note="how pure the cell's chemistry is" />
      </div>
      {/* slop-ok(S12-three-cards): fixture C.3 has exactly three inputs. */}
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <Slider label="Packet A's energy" value={energyA} min={1} max={6} onChange={setEnergyA} />
        <Slider label="Packet B's energy" value={energyB} min={1} max={6} onChange={setEnergyB} />
        <Slider label="Packet B's chemistry, relative to A" value={angle} min={0} max={359} onChange={setAngle} display={`${angle}°`} />
      </div>
      <p className="text-[11px] leading-relaxed text-slate-500">
        A flat slice of the 32 chemical channels. Each packet is drawn as a disk of radius <i>e</i> with its moment as an
        arrow, on the cone&apos;s edge (‖<i>m</i>‖ = <i>e</i>): the purest chemistry energy can carry. At 180° the
        chemistry is opposite.
      </p>
      <FixtureCheck fixture="C.3" statement="e = 8 and m = −2a; the energy has not become 2" atFixture={atFixture} matches={matches} onReset={reset} />
    </div>
  );
}

export function SignalsFigure({ a, b, cell, food }: { a: Packet; b: Packet; cell: Packet; food: number }) {
  const { ref, width } = useFigureWidth(480);
  // The layout is drawn for 480 across at 6 units to the energy (the cell's disk reaches radius 72), and fitted,
  // centred, to the width: narrower, the disks shrink and the text does not.
  const fit = Math.min(1, (width - 24) / 456);
  const scale = 6 * fit;
  const at = (x: number) => width / 2 + (x - 240) * fit;
  // On a phone each packet's numbers take two lines, so neighbours' labels cannot meet.
  const narrow = width < 420;
  const y = narrow ? 90 : 98;
  const name = y + 76 * Math.max(fit, 0.7) + 14;
  const height = narrow ? name + 40 : name + 26;
  const disk = (p: Packet, x: number, label: string) => {
    const moment = `‖m‖ ${fixed(norm(p.m))}`;
    return (
      <g>
        <circle cx={x} cy={y} r={p.e * scale} fill={TONE.energy} fillOpacity={0.1} stroke={TONE.energy} strokeOpacity={0.55} />
        <Arrow from={[x, y]} to={[x + p.m[0] * scale, y - p.m[1] * scale]} color={TONE.chemistry} />
        <text x={x} y={name} fontSize={13} textAnchor="middle" fill="#e2e8f0" fontWeight={700}>
          {label}
        </text>
        {narrow ? (
          <>
            <text x={x} y={name + 16} fontSize={11} textAnchor="middle" fill="#94a3b8" className="mono">
              e {fixed(p.e)}
            </text>
            <text x={x} y={name + 31} fontSize={11} textAnchor="middle" fill="#94a3b8" className="mono">
              {moment}
            </text>
          </>
        ) : (
          <text x={x} y={name + 16} fontSize={11} textAnchor="middle" fill="#94a3b8" className="mono">
            e {fixed(p.e)} · {moment}
          </text>
        )}
      </g>
    );
  };
  const ma = norm(a.m);
  const mb = norm(b.m);
  return (
    <div ref={ref} className="mx-auto max-w-[36rem] overflow-hidden rounded-2xl border border-white/10 bg-black/25">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        className="diagram block h-auto w-full"
        role="img"
        aria-label={`Packet A: energy ${fixed(a.e)}, moment ${fixed(ma)}. Packet B: energy ${fixed(b.e)}, moment ${fixed(mb)}. The cell: energy ${fixed(cell.e)}, moment ${fixed(norm(cell.m))}. If the signals were the food, ${fixed(food)}.`}
      >
        {disk(a, at(62), "Packet A")}
        <text x={at(124)} y={y + 7} fontSize={22} textAnchor="middle" fill="#64748b">
          +
        </text>
        {disk(b, at(188), "Packet B")}
        <text x={at(256)} y={y + 7} fontSize={22} textAnchor="middle" fill="#64748b">
          =
        </text>
        {disk(cell, at(368), "The cell")}
        {/* What would be left if the signal were the food. */}
        <circle cx={at(368)} cy={y} r={Math.max(0.5, food * scale)} fill="none" stroke={TONE.wrong} strokeWidth={1.8} strokeDasharray="4 3" />
        <text x={12} y={20} fontSize={11} fill={TONE.energy}>
          ○ energy
        </text>
        <text x={76} y={20} fontSize={11} fill={TONE.chemistry}>
          → chemistry
        </text>
        <text x={width - 12} y={20} fontSize={11} textAnchor="end" fill={TONE.wrong}>
          {narrow ? "- - signals as food" : "- - if signals were food"}
        </text>
      </svg>
    </div>
  );
}

// ---------------------------------------------------------------------------
// C.2 toxicity clipping.

const C2_DEFAULT = { energy: 2, absorbed: 3, requested: 5 };

export function ToxicityPanel() {
  const [energy, setEnergy] = useState(C2_DEFAULT.energy);
  const [absorbed, setAbsorbed] = useState(C2_DEFAULT.absorbed);
  const [requested, setRequested] = useState(C2_DEFAULT.requested);

  const rule = settle(absorbed, 0, requested, energy);
  const wrong = settleRequested(absorbed, 0, requested);
  const ruleAfter = energy + rule.net;
  const wrongAfter = energy + wrong.net;
  // Clamping the wrong version's organism at zero instead: what the ledger records against what left.
  const unaccounted = wrong.dissipated - (absorbed + energy - Math.max(0, wrongAfter));

  const atFixture = energy === FIXTURES.c2.energy && absorbed === FIXTURES.c2.absorbed && requested === FIXTURES.c2.requestedToxicity;
  const matches = near(rule.toxicity, FIXTURES.c2.toxicity) && near(rule.dissipated, FIXTURES.c2.dissipated) && near(wrong.dissipated, FIXTURES.c2.wrongRecorded);
  const reset = () => {
    setEnergy(C2_DEFAULT.energy);
    setAbsorbed(C2_DEFAULT.absorbed);
    setRequested(C2_DEFAULT.requested);
  };

  return (
    <div className="space-y-5">
      <Versions
        rule={
          <>
            Toxicity debits what the organism can actually pay, <i>T</i> = min(<i>T</i>
            <sub>requested</sub>, <i>E</i> + <i>G</i>), and the ledger dissipates <i>U</i> − <i>G</i> + <i>T</i>.
          </>
        }
        wrong={<>Debit the requested toxicity. The organism goes below zero, or, clamped, the ledger records energy that never left.</>}
      />
      <ToxicityFigure energy={energy} ruleAfter={ruleAfter} wrongAfter={wrongAfter} />
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Ledger
          tone="rule"
          rows={[
            ["Field", -absorbed],
            ["Organism", rule.net],
            ["Dissipated", rule.dissipated],
          ]}
          verdict={`${fixed(absorbed)} + ${fixed(-rule.net)} left; ${fixed(rule.dissipated)} dissipated. Balanced.`}
          ok
        />
        <Ledger
          tone="wrong"
          rows={[
            ["Field", -absorbed],
            ["Organism", wrong.net],
            ["Dissipated", wrong.dissipated],
          ]}
          verdict={
            wrongAfter < 0
              ? `The organism holds ${signed(wrongAfter)}: negative energy (INV-01). Clamped at 0 instead, the ledger records ${fixed(wrong.dissipated)} where ${fixed(wrong.dissipated - unaccounted)} left (INV-03).`
              : "Here the organism can pay: both versions agree."
          }
          ok={wrongAfter >= 0}
        />
      </div>
      {/* slop-ok(S12-three-cards): the toxicity fixture has exactly three inputs. */}
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <Slider label="Organism's energy, E" value={energy} min={0} max={6} step={0.5} onChange={setEnergy} />
        <Slider label="Absorbed from a useless packet, U" value={absorbed} min={0.5} max={6} step={0.5} onChange={setAbsorbed} />
        <Slider label="Requested toxicity" value={requested} min={0} max={10} step={0.5} onChange={setRequested} />
      </div>
      <FixtureCheck
        fixture="C.2"
        statement="actual toxicity 2; the field loses 3, the organism 2, and dissipation is 5, not 8"
        atFixture={atFixture}
        matches={matches}
        onReset={reset}
      />
    </div>
  );
}

export function ToxicityFigure({ energy, ruleAfter, wrongAfter }: { energy: number; ruleAfter: number; wrongAfter: number }) {
  const { ref, width } = useFigureWidth(470);
  const height = 128;
  const min = -10;
  const max = 8;
  // The rows' names sit left of the plot, and the values hang off the bars' ends, so the plot is inset on both sides.
  const left = 84;
  const right = width - 16;
  const X = (value: number) => left + ((value - min) / (max - min)) * (right - left);
  const zero = X(0);
  // Said in full where it fits beside the zero line; on a phone, in fewer words.
  const exhausted = right - zero >= 150 ? "0: exhausted, it dies" : "0: it dies";
  const before = X(energy);
  const row = (y: number, label: string, after: number, colour: string) => {
    const value = after === 0 ? exhausted : signed(after);
    const valueX = X(after) + (after < 0 ? -6 : 6);
    // Monospace at 11 px is 0.6 em, 6.6 px, a character.
    const valueWidth = value.length * 6.6;
    return (
      <g>
        <text x={14} y={y + 12} fontSize={12} fill="#cbd5e1" fontWeight={700}>
          {label}
        </text>
        <rect x={Math.min(zero, X(after))} y={y} width={Math.abs(X(after) - zero)} height={16} rx={3} fill={colour} fillOpacity={0.8} />
        <line x1={before} y1={y - 4} x2={before} y2={y + 20} stroke={TONE.energy} strokeWidth={2} strokeDasharray="3 2" />
        {/* A chip under the value, so the dotted marker never runs through it. */}
        <rect x={(after < 0 ? valueX - valueWidth : valueX) - 3} y={y} width={valueWidth + 6} height={16} rx={3} fill={TONE.ink} />
        <text x={valueX} y={y + 12} fontSize={11} textAnchor={after < 0 ? "end" : "start"} fill={colour} className="mono">
          {value}
        </text>
      </g>
    );
  };
  return (
    <div ref={ref} className="mx-auto max-w-[46rem] overflow-hidden rounded-2xl border border-white/10 bg-black/25">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        className="diagram block h-auto w-full"
        role="img"
        aria-label={`The organism held ${fixed(energy)}. After the rule it holds ${fixed(ruleAfter)}; after the wrong version, ${fixed(wrongAfter)}.`}
      >
        {/* Below the "before" label's row, so the label can sit across the zero line. */}
        <rect x={X(min)} y={20} width={zero - X(min)} height={82} fill={TONE.wrong} fillOpacity={0.06} />
        <text x={zero - 8} y={118} fontSize={11} textAnchor="end" fill={TONE.wrong}>
          negative energy: impossible
        </text>
        <line x1={zero} y1={20} x2={zero} y2={102} stroke="#64748b" />
        <text x={zero} y={118} fontSize={11} textAnchor="middle" fill="#64748b" className="mono">
          0
        </text>
        <text x={before} y={16} fontSize={11} textAnchor={before > right - 30 ? "end" : "middle"} fill={TONE.energy}>
          before: {fixed(energy)}
        </text>
        {row(26, "Rule", ruleAfter, TONE.rule)}
        {row(64, "Wrong", wrongAfter, TONE.wrong)}
      </svg>
    </div>
  );
}

function Ledger({ tone, rows, verdict, ok }: { tone: "rule" | "wrong"; rows: [string, number][]; verdict: string; ok: boolean }) {
  return (
    <div className={cn("rounded-xl border bg-black/25 p-3.5", tone === "rule" ? "border-green-500/20" : "border-rose-400/20")}>
      <p className={cn("text-[10px] font-black tracking-[0.16em] uppercase", tone === "rule" ? "text-green-300" : "text-rose-300")}>
        {tone === "rule" ? "The rule's ledger" : "The wrong version's ledger"}
      </p>
      <dl className="mt-2 space-y-1 font-mono text-sm">
        {rows.map(([label, value]) => (
          <div key={label} className="flex justify-between gap-3">
            <dt className="text-slate-400">{label}</dt>
            <dd className="text-white tabular-nums">{signed(value)}</dd>
          </div>
        ))}
      </dl>
      <p className={cn("mt-2 text-xs leading-relaxed", ok ? "text-green-200/90" : "text-rose-200")}>{verdict}</p>
    </div>
  );
}

// ---------------------------------------------------------------------------
// C.9 packet chemistry is not mixture chemistry.

/** A dark outline under a label, so a curve that crosses it breaks around the letters. */
const HALO = { stroke: TONE.ink, strokeWidth: 4, strokeLinejoin: "round", paintOrder: "stroke" } as const;

/** Energy gained per unit absorbed at composition `dot`: η where it feeds, κη where it poisons. */
const perUnit = (dot: number) => {
  const eta = efficiency(dot, C9_CHEMISTRY);
  return eta >= 0 ? eta : C9_CHEMISTRY.kappa * eta;
};

const C9_DEFAULT = -1;

export function MixturePanel() {
  const [dot, setDot] = useState(C9_DEFAULT);
  const packets = [
    { u: 1, dot: 1 },
    { u: 1, dot },
  ];
  // Energy enough that toxicity is never clipped, as C.9 requires.
  const holding = 10;
  const rule = settlePackets(packets, holding, C9_CHEMISTRY);
  const wrong = settleMixture(packets, holding, C9_CHEMISTRY);
  const average = mixtureDot(packets);

  const atFixture = dot === C9_DEFAULT;
  const matches = near(rule.net, FIXTURES.c9.perPacket, 1e-11) && near(wrong.net, FIXTURES.c9.mixture, 1e-11);

  const { ref, width } = useFigureWidth(540);
  const X = (d: number) => 52 + ((d + 1) / 2) * (width - 70);
  const yMin = -0.25;
  const yMax = 0.85;
  const Y = (v: number) => 20 + ((yMax - v) / (yMax - yMin)) * 190;
  const p1: [number, number] = [X(1), Y(perUnit(1))];
  const p2: [number, number] = [X(dot), Y(perUnit(dot))];
  const pm: [number, number] = [X(average), Y(perUnit(average))];
  const chordMid: [number, number] = [(p1[0] + p2[0]) / 2, (p1[1] + p2[1]) / 2];

  return (
    <div className="space-y-5">
      <Versions
        rule={
          <>
            Evaluate <i>η</i> = η<sub>max</sub> tanh(<i>k</i> <i>a</i>·<i>z</i> − <i>b</i>) on each granted packet
            separately: food is credited, poison is charged at κ.
          </>
        }
        wrong={<>Average the packets&apos; chemistry first, then apply the same rule once. Cheaper, and a different answer.</>}
      />
      <div ref={ref} className="mx-auto max-w-[46rem] overflow-hidden rounded-2xl border border-white/10 bg-black/25">
        <svg
          viewBox={`0 0 ${width} 252`}
          className="diagram block h-auto w-full"
          role="img"
          aria-label={`Energy gained per unit absorbed against composition along the affinity. Packet 1 at 1, packet 2 at ${fixed(dot, 2)}, their mixture at ${fixed(average, 2)}. Per packet the organism nets ${signed(rule.net, 6)}; on the mixture ${signed(wrong.net, 6)}.`}
        >
          <rect x={X(-1)} y={20} width={X(1) - X(-1)} height={Y(0) - 20} fill={TONE.rule} fillOpacity={0.04} />
          <rect x={X(-1)} y={Y(0)} width={X(1) - X(-1)} height={210 - Y(0)} fill={TONE.wrong} fillOpacity={0.06} />
          <line x1={X(-1)} y1={Y(0)} x2={X(1)} y2={Y(0)} stroke="#64748b" />
          <text x={X(-1) - 6} y={Y(0) + 4} fontSize={11} textAnchor="end" fill="#64748b" className="mono">
            0
          </text>
          <text x={X(-1) - 6} y={Y(0.8) + 4} fontSize={11} textAnchor="end" fill="#64748b" className="mono">
            0.8
          </text>
          <text x={X(1) - 6} y={Y(0) - 8} fontSize={11} textAnchor="end" fill={TONE.rule}>
            feeds ↑
          </text>
          <text x={X(1) - 6} y={Y(0) + 17} fontSize={11} textAnchor="end" fill={TONE.wrong}>
            poisons ↓
          </text>
          <path d={curve(perUnit, -1, 1, X, Y)} fill="none" stroke="#e2e8f0" strokeWidth={2} />
          {[-1, 0, 1].map((tick) => (
            <text
              key={tick}
              x={X(tick)}
              y={228}
              fontSize={11}
              textAnchor={tick === -1 ? "start" : tick === 1 ? "end" : "middle"}
              fill="#94a3b8"
              className="mono"
            >
              {tick === -1 ? "−1 against" : tick === 0 ? "0 neutral" : "along +1"}
            </text>
          ))}
          <text x={X(0)} y={246} fontSize={11} textAnchor="middle" fill="#64748b">
            {width < 480 ? "a·z: chemistry along the affinity" : "a·z: the packet\u2019s chemistry along the organism\u2019s affinity"}
          </text>
          {/* The chord: the per-packet result is twice its midpoint. */}
          <line x1={p1[0]} y1={p1[1]} x2={p2[0]} y2={p2[1]} stroke={TONE.rule} strokeWidth={1.5} strokeDasharray="4 3" />
          <circle cx={chordMid[0]} cy={chordMid[1]} r={5} fill={TONE.rule} />
          <circle cx={p1[0]} cy={p1[1]} r={6} fill={LINEAGE[0]} stroke={TONE.ink} strokeWidth={1.5} />
          <circle cx={p2[0]} cy={p2[1]} r={6} fill={LINEAGE[1]} stroke={TONE.ink} strokeWidth={1.5} />
          <rect x={pm[0] - 5} y={pm[1] - 5} width={10} height={10} fill={TONE.wrong} transform={`rotate(45 ${pm[0]} ${pm[1]})`} />
          <text x={p1[0] - 8} y={p1[1] - 10} fontSize={11} textAnchor="end" fill={LINEAGE[0]} {...HALO}>
            packet 1
          </text>
          {/* Left of the point, away from the "feeds"/"poisons" labels at the right, unless it is near the left edge. */}
          <text x={p2[0] + (dot <= -0.2 ? 8 : -8)} y={p2[1] + (perUnit(dot) < 0 ? 18 : perUnit(dot) < 0.3 ? -10 : 18)} fontSize={11} textAnchor={dot <= -0.2 ? "start" : "end"} fill={LINEAGE[1]} {...HALO}>
            packet 2
          </text>
          {/* Under the diamond, clear of the "poisons" label at the right even on a narrow phone; where the diamond sits on
              packet 2 (both packets alike), to its left, between the packets' labels. */}
          {Math.hypot(pm[0] - p2[0], pm[1] - p2[1]) < 40 ? (
            <text x={pm[0] - 10} y={pm[1] + 4} fontSize={11} textAnchor="end" fill={TONE.wrong} {...HALO}>
              mixture
            </text>
          ) : (
            <text x={Math.min(Math.max(pm[0], 80), width - 30)} y={pm[1] + 22} fontSize={11} textAnchor="middle" fill={TONE.wrong} {...HALO}>
              mixture
            </text>
          )}
        </svg>
      </div>
      <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
        <Readout label="Rule: per packet" value={signed(rule.net, 6)} tone="rule" note="gain − toxicity, over both packets" />
        <Readout label="Wrong: on the mixture" value={signed(wrong.net, 6)} tone="wrong" note={`one packet of 2 at a·z = ${fixed(average, 2)}`} />
        <Readout label="Packet 1 (z = a)" value={signed(perUnit(1), 6)} note="η along the affinity" />
        <Readout label="Packet 2" value={signed(perUnit(dot), 6)} note={perUnit(dot) < 0 ? "poison, charged at κ = 0.2" : "food"} />
      </div>
      <Slider label="Packet 2's chemistry along the affinity, a·z" value={dot} min={-1} max={1} step={0.05} onChange={setDot} display={fixed(dot, 2)} />
      <p className="text-[11px] leading-relaxed text-slate-500">
        η<sub>max</sub> 0.8, <i>k</i> 4, <i>b</i> 0.15, κ 0.2 (C.9). Both packets carry 1 unit, and the organism holds
        enough energy that toxicity is not clipped. Because the curve bends, the average of the packets (the green dot,
        times two) is not the curve at their average (the rose diamond).
      </p>
      <FixtureCheck
        fixture="C.9"
        statement="+0.639355310069 per packet, −0.047643210759 on the mixture"
        atFixture={atFixture}
        matches={matches}
        onReset={() => setDot(C9_DEFAULT)}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// C.11 output logits.

const C11_DEFAULT = 8;

export function LogitPanel() {
  const [logit, setLogit] = useState(C11_DEFAULT);
  const right = decodeLinear(logit);
  const wrong = decodeTanhFirst(logit);
  const atFixture = logit === C11_DEFAULT;
  const matches = near(right, FIXTURES.c11.linear, 5e-9) && near(wrong, FIXTURES.c11.tanhFirst, 5e-9);

  const { ref, width } = useFigureWidth(540);
  const X = (l: number) => 46 + ((l + 10) / 20) * (width - 66);
  const Y = (v: number) => 16 + (1 - v) * 180;
  const low = decodeTanhFirst(-100);
  const high = decodeTanhFirst(100);

  return (
    <div className="space-y-5">
      <Versions
        rule={<>The network&apos;s last layer is linear. Its logit is decoded once, by sigmoid, into a gate or rate.</>}
        wrong={<>Give the output layer a hidden-style tanh, then decode. Every decision is squeezed into 0.27–0.73.</>}
      />
      <div ref={ref} className="mx-auto max-w-[46rem] overflow-hidden rounded-2xl border border-white/10 bg-black/25">
        <svg
          viewBox={`0 0 ${width} 238`}
          className="diagram block h-auto w-full"
          role="img"
          aria-label={`Decoded value against the logit. At ${fixed(logit, 1)} the rule decodes ${fixed(right, 8)}; the wrong version ${fixed(wrong, 8)}.`}
        >
          <rect x={X(-10)} y={Y(high)} width={X(10) - X(-10)} height={Y(low) - Y(high)} fill={TONE.wrong} fillOpacity={0.08} />
          <text x={X(-10) + 6} y={Y(high) + 15} fontSize={11} fill={TONE.wrong}>
            {width < 480 ? (
              <>
                <tspan>all the wrong version</tspan>
                <tspan x={X(-10) + 6} dy={14}>
                  can ever say
                </tspan>
              </>
            ) : (
              "all the wrong version can ever say"
            )}
          </text>
          {[0, 0.5, 1].map((v) => (
            <g key={v}>
              <line x1={X(-10)} y1={Y(v)} x2={X(10)} y2={Y(v)} stroke="#334155" strokeDasharray={v === 0.5 ? "3 3" : undefined} />
              <text x={X(-10) - 6} y={Y(v) + 4} fontSize={11} textAnchor="end" fill="#64748b" className="mono">
                {v}
              </text>
            </g>
          ))}
          <path d={curve(decodeTanhFirst, -10, 10, X, Y)} fill="none" stroke={TONE.wrong} strokeWidth={2.2} />
          <path d={curve(decodeLinear, -10, 10, X, Y)} fill="none" stroke={TONE.rule} strokeWidth={2.4} />
          <line x1={X(logit)} y1={Y(1)} x2={X(logit)} y2={Y(0)} stroke="#94a3b8" strokeDasharray="2 3" />
          <circle cx={X(logit)} cy={Y(right)} r={5.5} fill={TONE.rule} stroke={TONE.ink} strokeWidth={1.5} />
          <circle cx={X(logit)} cy={Y(wrong)} r={5.5} fill={TONE.wrong} stroke={TONE.ink} strokeWidth={1.5} />
          {[-10, -5, 0, 5, 10].map((tick) => (
            <text key={tick} x={X(tick)} y={214} fontSize={11} textAnchor="middle" fill="#94a3b8" className="mono">
              {tick}
            </text>
          ))}
          <text x={X(0)} y={232} fontSize={11} textAnchor="middle" fill="#64748b">
            the output layer&apos;s logit
          </text>
        </svg>
      </div>
      <div className="grid grid-cols-2 gap-2">
        <Readout label="Rule: sigmoid(logit)" value={fixed(right, 8)} tone="rule" />
        <Readout label="Wrong: sigmoid(tanh(logit))" value={fixed(wrong, 8)} tone="wrong" />
      </div>
      <Slider label="Logit" value={logit} min={-10} max={10} step={0.1} onChange={setLogit} display={fixed(logit, 1)} />
      <FixtureCheck
        fixture="C.11"
        statement="a logit of 8 decodes near 0.99966465; tanh first gives about 0.73105853"
        atFixture={atFixture}
        matches={matches}
        onReset={() => setLogit(C11_DEFAULT)}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// C.15 skipped render epochs on a torus.

const C15_DEFAULT = FIXTURES.c15.moved;
const WIDTH = FIXTURES.c15.width;
const FROM = FIXTURES.c15.from;

export function WrapPanel() {
  const [moved, setMoved] = useState<number>(C15_DEFAULT);
  const [t, setT] = useState<number | null>(null);
  const reduced = useReducedMotion();
  const frame = useRef(0);

  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  const to = wrap(FROM + moved, WIDTH);
  const safe = unambiguous(moved, WIDTH);
  const shortest = minimumImage(FROM, to, WIDTH);
  const wrongHalf = minimumImageBetween(FROM, to, WIDTH, 0.5);
  const ruleHalf = safe ? wrap(FROM + moved / 2, WIDTH) : null;

  const play = () => {
    cancelAnimationFrame(frame.current);
    if (reduced) {
      setT(1);
      return;
    }
    const start = performance.now();
    const tick = (now: number) => {
      const progress = Math.min(1, (now - start) / 1800);
      setT(progress);
      if (progress < 1) frame.current = requestAnimationFrame(tick);
    };
    frame.current = requestAnimationFrame(tick);
  };

  const atFixture = moved === C15_DEFAULT;
  const matches = near(shortest, -FIXTURES.c15.reverse) && near(wrongHalf, FIXTURES.c15.halfwayWrong) && !safe;

  // Moving dots, while playing: the wrong version along the minimum image; the rule along the true path, or snapping.
  const wrongDot = t === null ? null : minimumImageBetween(FROM, to, WIDTH, t);
  const ruleDot = t === null ? null : safe ? wrap(FROM + t * moved, WIDTH) : to;

  return (
    <div className="space-y-5">
      <Versions
        rule={
          <>
            Between two drawn epochs, interpolate only if no organism can have moved half the torus. Otherwise the
            gap is ambiguous: snap to the later state and draw no path.
          </>
        }
        wrong={<>Always take the shortest way round (the minimum image). Past half the torus, it draws the motion backwards.</>}
      />
      <div className="grid grid-cols-1 items-center gap-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <WrapFigure moved={moved} to={to} safe={safe} shortest={shortest} wrongDot={wrongDot} ruleDot={ruleDot} />
        <div className="space-y-3">
          <div className="grid grid-cols-2 gap-2">
            <Readout label="Moved forward" value={fixed(moved)} note={`${FROM} → ${fixed(to)} on a ${WIDTH}-wide torus`} />
            <Readout
              label="Minimum image"
              value={shortest < 0 ? `${fixed(-shortest)} back` : `${fixed(shortest)} fwd`}
              tone={shortest < 0 ? "wrong" : "neutral"}
              note="the shortest way round"
            />
            <Readout
              label="Rule: halfway frame"
              value={ruleHalf === null ? "snaps" : fixed(ruleHalf, 1)}
              tone="rule"
              note={safe ? "interpolated: the gap is unambiguous" : `to ${fixed(to)}; no in-between path drawn`}
            />
            <Readout label="Wrong: halfway frame" value={fixed(wrongHalf, 1)} tone={safe ? "neutral" : "wrong"} note={safe ? "same as the rule" : "drawn going the wrong way"} />
          </div>
          <DemoButton onClick={play}>
            <Play className="h-3.5 w-3.5" aria-hidden="true" /> Play the gap
          </DemoButton>
        </div>
      </div>
      <Slider label="Distance moved between the two drawn epochs (at the speed limit)" value={moved} min={0} max={127} onChange={(value) => { cancelAnimationFrame(frame.current); setMoved(value); setT(null); }} />
      <p className="text-[11px] leading-relaxed text-slate-500">
        The viewer does not know how far an organism went, only how far it could have gone: ticks × Δt × maximum speed.
        Here the organism moved at the limit, so the bound is the distance. The ring is one axis of the torus, its seam
        at the top.
      </p>
      <FixtureCheck
        fixture="C.15"
        statement="80 units forward on a width-128 torus; the minimum image would draw 48 in reverse"
        atFixture={atFixture}
        matches={matches}
        onReset={() => {
          cancelAnimationFrame(frame.current);
          setMoved(C15_DEFAULT);
          setT(null);
        }}
      />
    </div>
  );
}

export function WrapFigure({
  moved,
  to,
  safe,
  shortest,
  wrongDot,
  ruleDot,
}: {
  moved: number;
  to: number;
  safe: boolean;
  shortest: number;
  wrongDot: number | null;
  ruleDot: number | null;
}) {
  const cx = 150;
  const cy = 140;
  const radius = 100;
  const point = (x: number, r = radius): [number, number] => {
    const angle = -Math.PI / 2 + (2 * Math.PI * x) / WIDTH;
    return [cx + r * Math.cos(angle), cy + r * Math.sin(angle)];
  };
  /** The arc from `from` going `delta` units (negative: backwards), drawn at radius `r`. */
  const arc = (from: number, delta: number, r: number) => {
    if (Math.abs(delta) < 0.01) return "";
    const [x0, y0] = point(from, r);
    const [x1, y1] = point(from + delta, r);
    const large = Math.abs(delta) > WIDTH / 2 ? 1 : 0;
    const sweep = delta > 0 ? 1 : 0;
    return `M${x0.toFixed(2)},${y0.toFixed(2)} A${r},${r} 0 ${large} ${sweep} ${x1.toFixed(2)},${y1.toFixed(2)}`;
  };
  const start = point(FROM);
  const end = point(to);
  const seam = point(0, radius + 14);
  return (
    <svg
      viewBox="0 0 300 290"
      className="diagram h-auto w-full max-w-sm justify-self-center"
      role="img"
      aria-label={`A ring for one axis of a ${WIDTH}-unit torus. The organism moved ${fixed(moved)} units forward from ${FROM} to ${fixed(to)}. The minimum image goes ${shortest < 0 ? `${fixed(-shortest)} units backwards` : `${fixed(shortest)} units forwards`}. ${safe ? "The gap is unambiguous, so the rule interpolates." : "The gap is ambiguous, so the rule snaps."}`}
    >
      <circle cx={cx} cy={cy} r={radius} fill="none" stroke="#334155" strokeWidth={10} />
      <line x1={cx} y1={cy - radius - 8} x2={cx} y2={cy - radius + 8} stroke="#94a3b8" strokeWidth={2} />
      <text x={seam[0]} y={seam[1] - 2} fontSize={10} textAnchor="middle" fill="#94a3b8" className="mono">
        0 = {WIDTH}
      </text>
      {/* What actually happened: forward. Drawn as the rule's path only when the viewer can know it. */}
      <path
        d={arc(FROM, moved, radius)}
        fill="none"
        stroke={safe ? TONE.rule : "#94a3b8"}
        strokeWidth={safe ? 5 : 2}
        strokeDasharray={safe ? undefined : "4 4"}
        strokeLinecap="round"
      />
      {/* The minimum image, inside the ring. */}
      {!safe ? <path d={arc(FROM, shortest, radius - 18)} fill="none" stroke={TONE.wrong} strokeWidth={5} strokeLinecap="round" /> : null}
      <circle cx={start[0]} cy={start[1]} r={7} fill={TONE.ink} stroke={LINEAGE[0]} strokeWidth={2.5} />
      <circle cx={end[0]} cy={end[1]} r={7} fill={LINEAGE[0]} />
      {wrongDot !== null && !safe ? (
        <circle cx={point(wrongDot, radius - 18)[0]} cy={point(wrongDot, radius - 18)[1]} r={6} fill={TONE.wrong} stroke={TONE.ink} strokeWidth={1.5} />
      ) : null}
      {ruleDot !== null ? (
        <circle cx={point(ruleDot)[0]} cy={point(ruleDot)[1]} r={6} fill={TONE.rule} stroke={TONE.ink} strokeWidth={1.5} />
      ) : null}
      <text x={cx} y={cy - 6} fontSize={12} textAnchor="middle" fill={safe ? TONE.rule : TONE.wrong} fontWeight={800}>
        {safe ? "unambiguous" : "ambiguous gap"}
      </text>
      <text x={cx} y={cy + 12} fontSize={11} textAnchor="middle" fill="#94a3b8">
        {safe ? "interpolate" : "rule snaps · wrong goes back"}
      </text>
      <text x={150} y={284} fontSize={10} textAnchor="middle" fill="#64748b">
        ○ {FROM} before · ● {fixed(to)} after
      </text>
    </svg>
  );
}

// ---------------------------------------------------------------------------
// C.5 birth.

const SHARES = [
  { value: "1/4", share: 1 / 4 },
  { value: "1/3", share: 1 / 3 },
  { value: "1/2", share: 1 / 2 },
  { value: "2/3", share: 2 / 3 },
] as const;
type ShareKey = (typeof SHARES)[number]["value"];

const C5_DEFAULT = { parent: 10, share: "1/3" as ShareKey, accepted: "accepted" as "accepted" | "refused" };
const BIRTH_COST = FIXTURES.c5.cost;

export function BirthPanel() {
  const [parent, setParent] = useState(C5_DEFAULT.parent);
  const [shareKey, setShareKey] = useState<ShareKey>(C5_DEFAULT.share);
  const [outcome, setOutcome] = useState(C5_DEFAULT.accepted);
  const share = SHARES.find((option) => option.value === shareKey)?.share ?? 1 / 3;
  const accepted = outcome === "accepted";
  const rule = birthRule(parent, BIRTH_COST, share, accepted);
  const wrong = birthChargeFirst(parent, BIRTH_COST, share, accepted);
  const fixture = birthRule(parent, BIRTH_COST, share, true);

  const atFixture = parent === FIXTURES.c5.parent && shareKey === "1/3" && accepted;
  const matches =
    near(fixture.child ?? Number.NaN, FIXTURES.c5.child) && near(fixture.parent, FIXTURES.c5.kept) && near(fixture.dissipated, FIXTURES.c5.dissipated);

  return (
    <div className="space-y-5">
      <Versions
        rule={
          <>
            A birth is one transaction: identity, genome, slot and energy transfer commit together, or nothing does. A
            refused birth charges no energy and resets no cooldown.
          </>
        }
        wrong={<>Charge the birth cost and the child&apos;s endowment, then try to allocate the child. If that fails, the energy is gone.</>}
      />
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <BirthBars title="The rule" tone="rule" before={parent} birth={rule} />
        <BirthBars title="Charge first, then allocate" tone="wrong" before={parent} birth={wrong} />
      </div>
      <div className="grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2">
        <Slider label="Parent's energy" value={parent} min={2} max={20} onChange={setParent} />
        <div className="space-y-3">
          <Segmented
            label="Child's share of what is left"
            options={SHARES.map((option) => ({ value: option.value, label: `share ${option.value}` }))}
            value={shareKey}
            onChange={setShareKey}
          />
          <Segmented
            label="The child's allocation"
            options={[
              { value: "accepted", label: "Allocation accepted", tone: "rule" },
              { value: "refused", label: "Allocation refused", tone: "wrong" },
            ]}
            value={outcome}
            onChange={setOutcome}
          />
        </div>
      </div>
      <p className="text-[11px] leading-relaxed text-slate-500">
        Birth cost {BIRTH_COST}. The split is <i>R</i> = <i>E</i> − cost; the child gets <i>β R</i> and the parent keeps
        the rest. A refusal can come from the population&apos;s slots or the genome quota, both reserved before anything
        is charged.
      </p>
      <FixtureCheck
        fixture="C.5"
        statement="parent 10, cost 1, share 1/3: the child receives 3 and the parent keeps 6; dissipation 1"
        atFixture={atFixture}
        matches={matches}
        onReset={() => {
          setParent(C5_DEFAULT.parent);
          setShareKey(C5_DEFAULT.share);
          setOutcome(C5_DEFAULT.accepted);
        }}
      />
    </div>
  );
}

export function BirthBars({ title, tone, before, birth }: { title: string; tone: "rule" | "wrong"; before: number; birth: ReturnType<typeof birthRule> }) {
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const { ref, width } = useFigureWidth(290);
  // 20 units (the slider's most) fill the bar, 20 in from each side.
  const scale = (width - 40) / 20;
  const parts: { label: string; value: number; fill: string }[] = [
    { label: "parent", value: birth.parent, fill: LINEAGE[0] },
    { label: "child", value: birth.child ?? 0, fill: LINEAGE[10] },
    { label: "dissipated", value: birth.dissipated, fill: TONE.dissipated },
    { label: "vanished", value: birth.vanished, fill: `url(#vanished-${id})` },
  ].filter((part) => part.value > 0);
  let cursor = 20;
  return (
    <div className={cn("rounded-2xl border bg-black/25 p-3", tone === "rule" ? "border-green-500/20" : "border-rose-400/20")}>
      <p className={cn("mb-2 text-[10px] font-black tracking-[0.16em] uppercase", tone === "rule" ? "text-green-300" : "text-rose-300")}>
        {title}
      </p>
      <div ref={ref}>
        <svg
          viewBox={`0 0 ${width} 76`}
          className="diagram block h-auto w-full"
          role="img"
          aria-label={`${title}: before, the parent holds ${fixed(before)}. After: ${parts.map((part) => `${part.label} ${fixed(part.value)}`).join(", ")}.`}
        >
          <defs>
            <pattern id={`vanished-${id}`} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
              <line x1="0" y1="0" x2="0" y2="6" stroke={TONE.wrong} strokeWidth="2" />
            </pattern>
          </defs>
          <text x={20} y={12} fontSize={11} fill="#94a3b8">
            before
          </text>
          <rect x={20} y={16} width={before * scale} height={16} rx={3} fill={LINEAGE[0]} fillOpacity={0.85} />
          <text x={20} y={50} fontSize={11} fill="#94a3b8">
            after
          </text>
          {parts.map((part) => {
            const x = cursor;
            cursor += part.value * scale;
            return (
              <rect
                key={part.label}
                x={x}
                y={54}
                width={part.value * scale}
                height={16}
                fill={part.fill}
                fillOpacity={0.85}
                stroke={part.label === "vanished" ? TONE.wrong : "none"}
              />
            );
          })}
        </svg>
      </div>
      <p className="mt-1 font-mono text-[11px] leading-relaxed text-slate-400">
        {parts.map((part) => `${part.label} ${fixed(part.value, 2)}`).join(" · ")}
      </p>
      {birth.child === null ? (
        <p className={cn("mt-1 text-xs", tone === "rule" ? "text-green-200/90" : "text-rose-200")}>
          {tone === "rule"
            ? "Refused: no child, nothing charged, no cooldown reset."
            : `No child exists, but ${fixed(birth.vanished, 2)} was paid to it: the ledger cannot close.`}
        </p>
      ) : null}
    </div>
  );
}
