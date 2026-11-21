"use client";

import type { ReactNode } from "react";

import { MicroLabel, Reticle } from "@/components/franken";
import { cn } from "@/lib/utils";

/**
 * What a figure is, said on its face (AGENTS.md: galleries are honest, and a
 * picture of a rule is never presented as a run).
 * - `rule`: the page computes the model's own rule on small numbers, as the
 *   plan's hand fixtures do. Not FrankenLife's code, not a simulation.
 * - `derived`: arithmetic from stated assumptions, not a measurement.
 * - `record`: figures the repository records (an experiment, a contract, a manifest).
 * - `diagram`: a schematic of how the software is organised.
 * - `render`: pixels `flife render` drew from a committed world; the manifest says how.
 * - `toy`: a small simulation written for this page, with FrankenLife's kinds of
 *   rules at toy scale. Not FrankenLife's code, and nothing it shows is a result.
 */
export type Basis = "rule" | "derived" | "record" | "diagram" | "render" | "toy";

const BASIS: Record<Basis, { label: string; className: string }> = {
  rule: {
    label: "The rule, computed here",
    className: "text-cyan-200 border-cyan-400/30 bg-cyan-400/10",
  },
  derived: {
    label: "Derived · arithmetic, not a benchmark",
    className: "text-amber-200 border-amber-400/30 bg-amber-400/10",
  },
  record: {
    label: "From the record",
    className: "text-green-300 border-green-500/30 bg-green-500/10",
  },
  diagram: {
    label: "Diagram · not simulation output",
    className: "text-slate-300 border-white/15 bg-white/5",
  },
  render: {
    label: "Rendered by flife · not drawn",
    className: "text-lime-200 border-lime-300/35 bg-lime-300/10",
  },
  toy: {
    label: "A toy in your browser · not FrankenLife",
    className: "text-violet-200 border-violet-300/35 bg-violet-300/10",
  },
};

export function BasisBadge({ basis, className }: { basis: Basis; className?: string }) {
  const style = BASIS[basis];
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center rounded-full border px-2.5 py-0.5 text-[10px] font-black tracking-[0.12em] whitespace-nowrap uppercase",
        style.className,
        className,
      )}
    >
      {style.label}
    </span>
  );
}

/** An explainer's frame: eyebrow, title, what kind of figure it is, the figure, and where its numbers come from. */
export function DemoFrame({
  eyebrow,
  title,
  basis,
  source,
  children,
  className,
}: {
  eyebrow: string;
  title: ReactNode;
  basis: Basis;
  /** Where the rule or the numbers are stated: "Plan §5.2 · fixture C.1". */
  source?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <figure className={cn("panel relative min-w-0 rounded-[1.75rem] p-5 sm:p-8", className)}>
      <Reticle inset={9} size={12} />
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
        <div className="min-w-0 space-y-2.5">
          <MicroLabel className="text-[color:var(--chapter,#4ade80)]">{eyebrow}</MicroLabel>
          <h4 className="text-xl font-extrabold tracking-[-0.025em] text-white sm:text-2xl">{title}</h4>
        </div>
        <BasisBadge basis={basis} />
      </div>
      <div className="mt-6">{children}</div>
      {source ? (
        <figcaption className="mt-6 flex gap-3 border-t border-white/[0.06] pt-4 font-mono text-[11px] leading-relaxed [overflow-wrap:anywhere] text-slate-500">
          <span aria-hidden="true" className="mt-[3px] h-2.5 w-2.5 shrink-0 rounded-full border border-white/20" />
          <span className="min-w-0">{source}</span>
        </figcaption>
      ) : null}
    </figure>
  );
}

export type Tone = "rule" | "wrong" | "neutral";

const TONE_ON: Record<Tone, string> = {
  rule: "bg-green-500 text-black",
  wrong: "bg-rose-400 text-black",
  neutral: "bg-white/15 text-white",
};

/** A small radio group of buttons: one choice of a few. */
export function Segmented<T extends string>({
  label,
  options,
  value,
  onChange,
  className,
}: {
  label: string;
  options: readonly { value: T; label: ReactNode; tone?: Tone }[];
  value: T;
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className={cn("inline-flex max-w-full flex-wrap gap-1 rounded-xl border border-white/10 bg-black/30 p-1", className)}
    >
      {options.map((option) => {
        const on = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => onChange(option.value)}
            className={cn(
              "rounded-lg px-3 py-1.5 text-xs font-bold transition focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
              on ? TONE_ON[option.tone ?? "neutral"] : "text-slate-300 hover:bg-white/5 hover:text-white",
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

/** A labelled range input with its value shown. */
export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
  display,
  className,
}: {
  label: ReactNode;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (value: number) => void;
  display?: ReactNode;
  className?: string;
}) {
  return (
    <label className={cn("block min-w-0", className)}>
      <span className="flex items-baseline justify-between gap-3">
        <span className="text-xs text-slate-400">{label}</span>
        <span className="font-mono text-xs text-white tabular-nums">{display ?? value}</span>
      </span>
      <input
        type="range"
        className="demo-range mt-2 w-full"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </label>
  );
}

const READOUT_TONE: Record<Tone, string> = {
  rule: "text-green-300",
  wrong: "text-rose-300",
  neutral: "text-white",
};

/** One figure with its label: an instrument's readout. */
export function Readout({
  label,
  value,
  note,
  tone = "neutral",
  className,
}: {
  label: ReactNode;
  value: ReactNode;
  note?: ReactNode;
  tone?: Tone;
  className?: string;
}) {
  return (
    <div className={cn("min-w-0 rounded-xl border border-white/10 bg-black/30 px-3 py-2.5", className)}>
      <p className="text-[9px] font-black tracking-[0.2em] text-slate-500 uppercase">{label}</p>
      <p className={cn("mt-1 font-mono text-base tabular-nums sm:text-lg", READOUT_TONE[tone])}>{value}</p>
      {note ? <p className="mt-1 text-[11px] leading-snug text-slate-500">{note}</p> : null}
    </div>
  );
}

/** A plain action button in the explainers' style. */
export function DemoButton({
  children,
  onClick,
  disabled,
  pressed,
  className,
  label,
}: {
  children: ReactNode;
  onClick: () => void;
  disabled?: boolean;
  pressed?: boolean;
  className?: string;
  label?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-pressed={pressed}
      aria-label={label}
      className={cn(
        "inline-flex items-center gap-2 rounded-xl border border-white/15 bg-black/30 px-3.5 py-2 text-xs font-bold text-white transition hover:border-green-400/50 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40",
        pressed && "border-green-400/60 text-green-300",
        className,
      )}
    >
      {children}
    </button>
  );
}

/** "3.000" → "3", "0.639355" stays: a number with at most `digits` decimals and no trailing zeros. */
export function fixed(value: number, digits = 3): string {
  if (!Number.isFinite(value)) return "—";
  const rounded = Number(value.toFixed(digits));
  return (Object.is(rounded, -0) ? 0 : rounded).toLocaleString("en-US", { maximumFractionDigits: digits });
}
