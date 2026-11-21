"use client";

import { useState } from "react";

import { CATALOG, parameterCount, widths } from "@/components/explain/brain-math";
import { BasisBadge, DemoFrame, Readout, Segmented, Slider } from "@/components/explain/demo";
import {
  PROFILES,
  type Precision,
  STARTER_SOURCE_POWER,
  SWEEP_ROWS,
  count,
  gibibytes,
  mandatoryCost,
  rate,
  scale,
  sustainable,
  trim,
} from "@/components/explain/scale-math";
import { cn, grouped } from "@/lib/utils";

const CATALOG_SIZES = CATALOG.map((topology) => parameterCount(widths(topology)));

// The population slider is logarithmic: 1,000 to 1,000,000.
const LOG_MIN = 3;
const LOG_MAX = 6;
const toSlider = (population: number) => Math.round(Math.log10(population) * 100);
const fromSlider = (value: number) => Math.round(10 ** (value / 100));

// The bandwidth axis: 100 MB/s to 10 TB/s, logarithmic.
const AXIS_MIN = 1e8;
const AXIS_MAX = 1e13;
const AXIS_TICKS = [
  { at: 1e9, label: "1 GB/s" },
  { at: 1e10, label: "10 GB/s" },
  { at: 1e11, label: "100 GB/s" },
  { at: 1e12, label: "1 TB/s" },
  { at: 1e13, label: "10 TB/s" },
];
const position = (bytesPerSecond: number) =>
  Math.min(1, Math.max(0, (Math.log10(bytesPerSecond) - Math.log10(AXIS_MIN)) / (Math.log10(AXIS_MAX) - Math.log10(AXIS_MIN))));
const percent = (value: number) => `${(value * 100).toFixed(2)}%`;

/**
 * Measured on an Apple M5 Ultra, 2026-11-10, with other GPU work on the device:
 * the resident Metal controller kernel's F32 weight stream and a plain read of
 * the same bytes, fastest rounds (crates/flife-backend-metal/CONTRACT.md,
 * "Resident bandwidth (FL-030, plan §19.2)").
 */
const MEASURED_STREAM = [426e9, 748e9] as const;
const MEASURED_PLAIN_READ = [956e9, 1_082e9] as const;

/** Why the scale is hard: the weight bank, its bandwidth and its food, as arithmetic you can move. */
export function ScaleCalculator() {
  const [population, setPopulation] = useState(400_000);
  const [parameters, setParameters] = useState(9_988);
  const [sweeps, setSweeps] = useState(30);
  const [precision, setPrecision] = useState<Precision>("f16");

  const result = scale(population, parameters, sweeps, precision);
  const profile = PROFILES.find((entry) => entry.population === population);
  const need = population * mandatoryCost(parameters);
  const ceiling = sustainable(parameters);
  const needShare = Math.min(1, need / Math.max(need, STARTER_SOURCE_POWER));
  const supplyShare = Math.min(1, STARTER_SOURCE_POWER / Math.max(need, STARTER_SOURCE_POWER));
  const stream = result.streamBytesPerSecond;
  const pastMeasured = stream > MEASURED_STREAM[1];

  return (
    <DemoFrame
      eyebrow="Why the scale is hard · README, plan §19"
      title="Every sweep streams the whole bank."
      basis="derived"
      source={
        <>
          Arithmetic from README.md &quot;Why the scale is hard&quot; and plan §19.2 and §19.5, not a benchmark. One
          multiply-accumulate per parameter per evaluation, biases included, as the README counts them; the README
          rounds 400,000 × 9,988 to 4.0 billion parameters, and these figures use the exact 3,995,200,000. The
          measured band: crates/flife-backend-metal/CONTRACT.md, resident bandwidth (FL-030).
        </>
      }
    >
      <div className="space-y-6">
        <div className="grid grid-cols-1 gap-x-8 gap-y-5 md:grid-cols-2">
          <div className="space-y-2.5">
            <Slider
              label="Live organisms"
              value={toSlider(population)}
              min={LOG_MIN * 100}
              max={LOG_MAX * 100}
              onChange={(value) => setPopulation(fromSlider(value))}
              display={`${grouped(population)}${profile ? ` · ${profile.name}` : ""}`}
            />
            <Segmented
              label="Operating profile populations: Reference, Interactive, Desktop, Target full"
              options={PROFILES.map((entry) => ({
                value: String(entry.population),
                label: grouped(entry.population),
              }))}
              value={String(population)}
              onChange={(value) => setPopulation(Number(value))}
            />
          </div>
          <div className="space-y-2.5">
            <Slider
              label="Mean parameters per organism"
              value={parameters}
              min={CATALOG_SIZES[0] ?? 1_164}
              max={CATALOG_SIZES[CATALOG_SIZES.length - 1] ?? 29_316}
              onChange={setParameters}
              display={grouped(parameters)}
            />
            <Segmented
              label="Catalog network sizes"
              options={CATALOG_SIZES.map((size) => ({ value: String(size), label: grouped(size) }))}
              value={String(parameters)}
              onChange={(value) => setParameters(Number(value))}
            />
          </div>
          <div className="space-y-2.5">
            <Slider
              label="Full-population sweeps per second"
              value={sweeps}
              min={1}
              max={128}
              onChange={setSweeps}
              display={`${sweeps} /s`}
            />
            <Segmented
              label="README's sweep rates"
              options={SWEEP_ROWS.map((row) => ({ value: String(row), label: `${row}/s` }))}
              value={String(sweeps)}
              onChange={(value) => setSweeps(Number(value))}
            />
          </div>
          <div className="space-y-2.5">
            <p className="text-xs text-slate-400">Canonical weights</p>
            <Segmented
              label="Weight precision"
              options={[
                { value: "f16", label: "F16 · 2 bytes" },
                { value: "f32", label: "F32 · 4 bytes" },
              ]}
              value={precision}
              onChange={setPrecision}
            />
            <p className="text-[11px] leading-snug text-slate-500">
              Compact F16 genomes are a distinct quantized evolutionary model, not a hidden storage trick (plan §10.3).
            </p>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-2.5 lg:grid-cols-4">
          <Readout label="Independent parameters" value={count(result.parameters)} note="no two organisms share one" />
          <Readout label="Weight bank" value={gibibytes(result.bankBytes)} note={`${precision.toUpperCase()}, before any other state`} />
          <Readout label="Multiply-accumulates" value={`${count(result.macsPerSecond, 3)}/s`} note="arithmetic is not the limit" />
          <Readout
            label="Weight stream"
            value={rate(stream)}
            tone={pastMeasured ? "wrong" : "rule"}
            note="every sweep reads every weight"
          />
        </div>

        {/* The required stream on a log axis, beside what one device was measured to stream. */}
        <div className="rounded-2xl border border-white/10 bg-black/30 p-4 pb-3">
          <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">
            Weight bytes a second, logarithmic
          </p>
          <div className="relative mt-9 mb-14 h-3" role="img" aria-label={`Required weight stream ${rate(stream)}; measured on an M5 Ultra, 426 to 748 GB/s for the F32 controller kernel and 956 to 1,082 GB/s for a plain read.`}>
            <div className="absolute inset-0 rounded-full bg-white/[0.06]" />
            {/* Measured: the kernel's stream and the plain read. */}
            <div
              className="absolute inset-y-0 rounded-sm bg-amber-300/45"
              style={{ left: percent(position(MEASURED_STREAM[0])), width: percent(position(MEASURED_STREAM[1]) - position(MEASURED_STREAM[0])) }}
            />
            <div
              className="absolute inset-y-0 rounded-sm bg-slate-300/30"
              style={{
                left: percent(position(MEASURED_PLAIN_READ[0])),
                width: percent(position(MEASURED_PLAIN_READ[1]) - position(MEASURED_PLAIN_READ[0])),
              }}
            />
            {AXIS_TICKS.map((tick) => (
              <div key={tick.at} className="absolute top-full" style={{ left: percent(position(tick.at)) }}>
                <div className="h-1.5 w-px bg-white/25" />
                <span
                  className={cn(
                    "absolute top-2 font-mono text-[10px] whitespace-nowrap text-slate-500",
                    tick.at === AXIS_MAX ? "-translate-x-full" : "-translate-x-1/2",
                  )}
                >
                  {tick.label}
                </span>
              </div>
            ))}
            <div
              className="absolute top-[calc(100%+1.6rem)] -translate-x-1/2 text-center text-[10px] leading-tight whitespace-nowrap text-amber-200/90"
              style={{ left: percent(position(Math.sqrt(MEASURED_STREAM[0] * MEASURED_PLAIN_READ[1]))) }}
            >
              measured, M5 Ultra
              <br />
              <span className="text-slate-400">kernel · plain read</span>
            </div>
            {/* Required. */}
            <div
              className="absolute top-1/2 transition-[left] duration-300"
              style={{ left: percent(position(stream)) }}
            >
              <span
                className={cn(
                  "absolute h-5 w-5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-[#020a02]",
                  pastMeasured ? "bg-rose-400 shadow-[0_0_16px_rgba(251,113,133,0.8)]" : "bg-green-400 shadow-[0_0_16px_rgba(74,222,128,0.8)]",
                )}
              />
              <span
                className={cn(
                  "absolute bottom-3 -translate-x-1/2 font-mono text-xs font-bold whitespace-nowrap",
                  pastMeasured ? "text-rose-300" : "text-green-300",
                )}
              >
                {rate(stream)}
              </span>
            </div>
          </div>
        </div>

        <div className="grid grid-cols-1 gap-4 lg:grid-cols-[1.2fr_1fr]">
          {/* README's table, recomputed for this population and network size. */}
          <div className="min-w-0 rounded-2xl border border-white/10 bg-black/30 p-4">
            <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">
              {grouped(population)} × {grouped(parameters)} parameters
            </p>
            <div className="mt-3 overflow-x-auto">
              <table className="w-full min-w-[22rem] text-right font-mono text-xs tabular-nums">
                <thead>
                  <tr className="text-[10px] tracking-wider text-slate-500 uppercase">
                    <th className="pb-2 text-left font-bold">Sweeps/s</th>
                    <th className="pb-2 font-bold">MAC/s</th>
                    <th className="pb-2 font-bold">F16 stream</th>
                    <th className="pb-2 font-bold">F32 stream</th>
                  </tr>
                </thead>
                <tbody>
                  {SWEEP_ROWS.map((row) => {
                    const half = scale(population, parameters, row, "f16");
                    const single = scale(population, parameters, row, "f32");
                    const on = row === sweeps;
                    return (
                      <tr key={row} className={cn("border-t border-white/5", on ? "text-green-200" : "text-slate-300")}>
                        <td className="py-1.5 text-left">{row}</td>
                        <td className="py-1.5">{count(half.macsPerSecond, 3)}</td>
                        <td className={cn("py-1.5", on && precision === "f16" && "font-bold text-green-300")}>
                          {rate(half.streamBytesPerSecond)}
                        </td>
                        <td className={cn("py-1.5", on && precision === "f32" && "font-bold text-green-300")}>
                          {rate(single.streamBytesPerSecond)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            <p className="mt-3 text-[11px] leading-snug text-slate-500">
              At 400,000 × 9,988 these are the README&apos;s rows. A headline FLOP/s figure cannot justify a real-time
              claim here; only sustained bandwidth with the real independent-weight layout can.
            </p>
          </div>

          {/* Food: plan §19.5's energy budget. */}
          <div className="min-w-0 rounded-2xl border border-white/10 bg-black/30 p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <p className="text-[10px] font-black tracking-[0.2em] text-slate-500 uppercase">And they must eat</p>
              <BasisBadge basis="derived" />
            </div>
            <div className="mt-4 space-y-3">
              <div>
                <p className="flex justify-between gap-3 text-xs">
                  <span className="text-slate-400">Mandatory cost</span>
                  <span className="font-mono text-rose-200 tabular-nums">{trim(need, 4)} energy/s</span>
                </p>
                <div className="mt-1 h-2 rounded-full bg-white/[0.06]">
                  <div className="h-2 rounded-full bg-rose-400/70 transition-[width] duration-300" style={{ width: percent(needShare) }} />
                </div>
              </div>
              <div>
                <p className="flex justify-between gap-3 text-xs">
                  <span className="text-slate-400">Starter&apos;s eight sources</span>
                  <span className="font-mono text-amber-200 tabular-nums">{grouped(STARTER_SOURCE_POWER)} energy/s</span>
                </p>
                <div className="mt-1 h-2 rounded-full bg-white/[0.06]">
                  <div className="h-2 rounded-full bg-amber-300/70 transition-[width] duration-300" style={{ width: percent(supplyShare) }} />
                </div>
              </div>
            </div>
            <p className="mt-3 text-xs leading-relaxed text-slate-400">
              At unit sensing scale each organism of this size pays at least{" "}
              <span className="font-mono text-slate-200">0.005 + 0.001 + {grouped(parameters)} × 128 × 2·10⁻⁹</span> ={" "}
              <span className="font-mono text-slate-200">{trim(mandatoryCost(parameters), 4)}</span> energy a simulated
              second, before moving, emitting or dividing. With perfect uptake the starter&apos;s sources could keep at
              most about <span className="font-mono text-slate-200">{grouped(Math.floor(ceiling))}</span> alive, so every
              profile declares its own environment.
            </p>
          </div>
        </div>

        <div className="rounded-2xl border border-amber-300/20 bg-amber-300/[0.04] p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-[10px] font-black tracking-[0.2em] text-amber-200/90 uppercase">Measured, not derived</p>
            <BasisBadge basis="record" />
          </div>
          <p className="mt-2 text-sm leading-relaxed text-slate-300">
            On an Apple M5 Ultra (2026-11-10, with other GPU work on the device), FrankenLife&apos;s resident Metal
            controller kernel streamed F32 weights at <span className="font-mono text-amber-200">426–748 GB/s</span>{" "}
            in its fastest rounds, 0.44–0.69 of a plain read of the same bytes, every output identical to the CPU
            reference. Against a four-billion-parameter F32 bank that is 27–47 full sweeps a second, for the
            controllers alone; sensing, the field, births and deaths come on top.
          </p>
          <p className="mt-2 text-sm leading-relaxed text-slate-400">
            The scale gate G7 asks for at least 380,000 live organisms averaging 9,000–11,000 parameters, at least 30
            full-evaluation ticks and 30 frames a second, for ten minutes on a named machine. It has not run.
          </p>
        </div>
      </div>
    </DemoFrame>
  );
}
