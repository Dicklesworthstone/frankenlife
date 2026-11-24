"use client";

import { AlertTriangle, Check, Copy, RotateCcw, Terminal } from "lucide-react";
import { type KeyboardEvent, useMemo, useRef, useState } from "react";

import { DemoFrame } from "@/components/explain/demo";
import {
  V2_EXPLORER_FEATURES,
  V2_PRESETS,
  type V2Boundary,
  type V2PresetId,
  buildMinimalV2CliFlags,
  computeV2Abi,
  formatV2CliCommand,
  nextRadioIndex,
  validateV2Selection,
} from "@/components/explain/v2-explorer-model";
import { cn } from "@/lib/utils";
import type { V2FeatureSlug } from "../../scripts/media-options";

const BOUNDARY_STYLE: Record<V2Boundary, { label: string; className: string }> = {
  "pure-field": {
    label: "Pure-field",
    className: "border-green-500/30 bg-green-500/10 text-green-300",
  },
  contact: {
    label: "Contact",
    className: "border-amber-400/30 bg-amber-400/10 text-amber-200",
  },
  "mode-dependent": {
    label: "Mode-dep",
    className: "border-sky-400/30 bg-sky-400/10 text-sky-200",
  },
  presentation: {
    label: "INV-09",
    className: "border-cyan-400/30 bg-cyan-400/10 text-cyan-200",
  },
};

export function V2Explorer() {
  const [presetId, setPresetId] = useState<V2PresetId>("bundle-a");
  const preset = V2_PRESETS.find((item) => item.id === presetId) ?? V2_PRESETS[0]!;
  const [enabled, setEnabled] = useState<Set<V2FeatureSlug>>(() => new Set(preset.targetEnabled));
  const [subcommand, setSubcommand] = useState<"run" | "validate">("run");
  const [copied, setCopied] = useState(false);
  const presetBtnRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const subcmdBtnRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const SUBCOMMANDS = ["run", "validate"] as const;

  const onPresetKeyDown = (index: number) => (event: KeyboardEvent<HTMLButtonElement>) => {
    const next = nextRadioIndex(index, V2_PRESETS.length, event.key);
    if (next === null) return;
    event.preventDefault();
    const target = V2_PRESETS[next];
    if (target) selectPreset(target.id);
    presetBtnRefs.current[next]?.focus();
  };

  const onSubcommandKeyDown = (index: number) => (event: KeyboardEvent<HTMLButtonElement>) => {
    const next = nextRadioIndex(index, SUBCOMMANDS.length, event.key);
    if (next === null) return;
    event.preventDefault();
    const target = SUBCOMMANDS[next];
    if (target) setSubcommand(target);
    subcmdBtnRefs.current[next]?.focus();
  };

  const selectPreset = (nextId: V2PresetId) => {
    const next = V2_PRESETS.find((item) => item.id === nextId);
    if (!next) return;
    setPresetId(next.id);
    setEnabled(new Set(next.targetEnabled));
    setCopied(false);
  };

  const toggleSlug = (slug: V2FeatureSlug) => {
    setEnabled((prev) => {
      const next = new Set(prev);
      if (next.has(slug)) next.delete(slug);
      else next.add(slug);
      return next;
    });
    setCopied(false);
  };

  const resetToPreset = () => {
    setEnabled(new Set(preset.targetEnabled));
    setCopied(false);
  };

  const baseSet = useMemo(() => new Set<V2FeatureSlug>(preset.baseEnabled), [preset]);
  const abi = useMemo(() => computeV2Abi(enabled, preset), [enabled, preset]);
  const boundary = useMemo(
    () => validateV2Selection(enabled, preset.hasContact, preset),
    [enabled, preset],
  );
  const cliFlags = useMemo(() => buildMinimalV2CliFlags(baseSet, enabled), [baseSet, enabled]);
  const cliCommand = useMemo(
    () => formatV2CliCommand(subcommand, preset.presetPath, cliFlags),
    [subcommand, preset.presetPath, cliFlags],
  );

  const activeCounts = useMemo(() => {
    let verified = 0;
    let inFlight = 0;
    for (const f of V2_EXPLORER_FEATURES) {
      if (!enabled.has(f.slug)) continue;
      if (f.verification === "verified") verified += 1;
      else inFlight += 1;
    }
    return { total: enabled.size, verified, inFlight };
  }, [enabled]);

  const isModifiedFromPreset = useMemo(() => {
    if (enabled.size !== preset.targetEnabled.length) return true;
    return preset.targetEnabled.some((slug) => !enabled.has(slug));
  }, [enabled, preset]);

  const handleFixIssues = () => {
    const needsContact = boundary.issues.some((issue) => issue.code === "contact-required");
    const needsRecurrence = boundary.issues.some((issue) => issue.code === "notch-requires-recurrence");
    if (needsContact) {
      // Switch base preset to Bundle A (`[contact]` present) while keeping the user's selected slugs.
      setPresetId("bundle-a");
    }
    if (needsRecurrence) {
      setEnabled((prev) => {
        const next = new Set(prev);
        next.delete("notch-delta");
        return next;
      });
    }
    setCopied(false);
  };

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(cliCommand);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      setCopied(false);
    }
  };

  return (
    <DemoFrame
      eyebrow="field-ecology-v2 · interactive ABI & CLI toggle calculator"
      title="21 mechanisms, toggled clean."
      basis="rule"
      source={
        <>
          Computed deterministically from <code className="text-slate-300">crates/flife-core/src/v2.rs</code>{" "}
          (<code className="text-slate-300">V2Feature::ALL</code>,{" "}
          <code className="text-slate-300">V2Config::abi</code>, <code className="text-slate-300">validate_v2</code>)
          and <code className="text-slate-300">flife --list-features</code>. Configuration and ABI calculator only —
          not simulation output. Verified &amp; closed (18/18 V2 beads — all 21 slugs + V2-10/V2-17 Stage-2):
          V2-01..V2-17 + V2-EPIC. Stage-2 G5/G9 outcomes and honest residuals are recorded in{" "}
          <code className="text-slate-300">experiments/v2-emergence/README.md</code>; this widget makes no ecological
          claims.
        </>
      }
    >
      {/* Preset bundle selector & reset */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div
          role="radiogroup"
          aria-label="field-ecology-v2 preset bundles"
          className="inline-flex max-w-full flex-wrap gap-1 rounded-xl border border-white/10 bg-black/40 p-1"
        >
          {V2_PRESETS.map((item, idx) => {
            const selected = item.id === presetId && !isModifiedFromPreset;
            const isBase = item.id === presetId;
            return (
              <button
                key={item.id}
                ref={(el) => {
                  presetBtnRefs.current[idx] = el;
                }}
                type="button"
                role="radio"
                aria-checked={isBase}
                tabIndex={isBase ? 0 : -1}
                onKeyDown={onPresetKeyDown(idx)}
                onClick={() => selectPreset(item.id)}
                className={cn(
                  "min-h-11 rounded-lg px-3.5 text-xs font-bold transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                  selected
                    ? "bg-green-500 text-black"
                    : isBase
                      ? "bg-green-500/20 text-green-200 ring-1 ring-green-400/40"
                      : "text-slate-300 hover:bg-white/5 hover:text-white",
                )}
              >
                <span className="hidden sm:inline">{item.label}</span>
                <span className="sm:hidden">{item.shortLabel}</span>
              </button>
            );
          })}
        </div>

        <div className="flex min-w-0 flex-wrap items-center gap-2">
          {isModifiedFromPreset ? (
            <button
              type="button"
              onClick={resetToPreset}
              className="inline-flex min-h-11 items-center gap-1.5 rounded-xl border border-white/15 bg-black/40 px-3 text-xs font-bold text-slate-200 transition hover:border-green-400/40 hover:text-white focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
            >
              <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
              Reset {preset.shortLabel}
            </button>
          ) : null}
          <span className="min-w-0 font-mono text-[11px] [overflow-wrap:anywhere] text-slate-400">
            Base: <code className="text-slate-200">{preset.presetPath}</code>
          </span>
        </div>
      </div>

      <p className="mt-3 text-xs leading-relaxed text-slate-400">{preset.note}</p>

      {/* Live ABI, field-plane, boundary, and verification telemetry */}
      <dl className="mt-5 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <div className="rounded-2xl border border-white/[0.08] bg-black/40 p-4">
          <dt className="font-mono text-[10px] tracking-[0.16em] text-slate-400 uppercase">Field planes</dt>
          <dd className="mt-1.5 font-mono text-2xl font-extrabold text-white tabular-nums">
            {abi.fieldPlanes}
            <span className="ml-2 text-xs font-medium text-slate-400">
              {abi.fieldPlanes === 34 ? "dual-carrier F32" : "single-carrier F32"}
            </span>
          </dd>
          <dd className="mt-1 text-[11px] text-slate-500">
            {abi.fieldPlanes === 34 ? "25 metabolic + 9 Carrier-1 trails" : "1 energy + 32 chemical moments"}
          </dd>
        </div>

        <div className="rounded-2xl border border-white/[0.08] bg-black/40 p-4">
          <dt className="font-mono text-[10px] tracking-[0.16em] text-slate-400 uppercase">Controller ABI</dt>
          <dd className="mt-1.5 font-mono text-2xl font-extrabold text-white tabular-nums">
            {abi.inputWidth}
            <span className="mx-1.5 text-slate-500">→</span>
            {abi.actionWidth}
            {abi.rawOutputWidth !== abi.actionWidth ? (
              <span className="ml-1.5 text-xs font-medium text-cyan-200">({abi.rawOutputWidth} raw)</span>
            ) : null}
          </dd>
          <dd className="mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 font-mono text-[10px] text-slate-400">
            <span
              className={cn(
                "shrink-0 rounded-full border px-1.5 py-0.5 uppercase",
                abi.metalF16Aligned
                  ? "border-green-500/30 bg-green-500/10 text-green-300"
                  : "border-amber-400/30 bg-amber-400/10 text-amber-200",
              )}
            >
              {abi.metalF16Aligned ? "Metal F16 4-aligned" : "Unaligned · CPU fallback"}
            </span>
            {abi.sensorDelta.length > 0 || abi.actionDelta.length > 0 ? (
              <span className="min-w-0 break-words text-slate-500">
                {[...abi.sensorDelta, ...abi.actionDelta].join(" · ")}
              </span>
            ) : (
              <span className="text-slate-500">Base 104 → 36</span>
            )}
          </dd>
        </div>

        <div className="rounded-2xl border border-white/[0.08] bg-black/40 p-4">
          <dt className="font-mono text-[10px] tracking-[0.16em] text-slate-400 uppercase">INV-08 boundary</dt>
          <dd className="mt-1.5 flex items-center gap-2 font-mono text-base font-bold">
            <span
              className={cn(
                "inline-flex rounded-full border px-2.5 py-0.5 text-xs",
                boundary.inv08PureField
                  ? "border-green-500/35 bg-green-500/10 text-green-300"
                  : "border-amber-400/35 bg-amber-400/10 text-amber-200",
              )}
            >
              {boundary.inv08PureField ? "Pure-field (INV-08)" : "Contact ([contact])"}
            </span>
          </dd>
          <dd className="mt-1.5 text-[11px] text-slate-500">
            {boundary.inv08PureField
              ? "All organism interactions pass solely through the field."
              : preset.hasContact
                ? "Direct contact enabled; inv08_pure_field = false."
                : "Contact slug selected on a pure-field preset — refused by validate."}
          </dd>
        </div>

        <div className="rounded-2xl border border-white/[0.08] bg-black/40 p-4">
          <dt className="font-mono text-[10px] tracking-[0.16em] text-slate-400 uppercase">Active slugs</dt>
          <dd className="mt-1.5 font-mono text-2xl font-extrabold text-white tabular-nums">
            {activeCounts.total}
            <span className="text-sm font-medium text-slate-500"> / 21</span>
          </dd>
          <dd className="mt-1 flex flex-wrap gap-x-2.5 gap-y-0.5 font-mono text-[11px] tabular-nums">
            <span className="text-green-300">{activeCounts.verified} verified</span>
            <span className="text-amber-200">{activeCounts.inFlight} in flight</span>
          </dd>
        </div>
      </dl>

      {/* Live validation refusal warning (exact v2.rs contract) */}
      {boundary.issues.length > 0 ? (
        <div
          role="alert"
          className="mt-4 flex flex-wrap items-start justify-between gap-3 rounded-2xl border border-rose-400/35 bg-rose-500/[0.08] p-4"
        >
          <div className="min-w-0 flex-1 space-y-1.5">
            <p className="flex items-center gap-2 font-mono text-xs font-bold tracking-[0.1em] text-rose-200 uppercase">
              <AlertTriangle className="h-4 w-4 shrink-0 text-rose-300" aria-hidden="true" />
              flife validate refuses this combination ({boundary.issues.length}{" "}
              {boundary.issues.length === 1 ? "issue" : "issues"})
            </p>
            <ul className="space-y-1 font-mono text-xs text-rose-100/90">
              {boundary.issues.map((issue) => (
                <li key={`${issue.code}-${issue.slug}`}>• {issue.detail}</li>
              ))}
            </ul>
          </div>
          <button
            type="button"
            onClick={handleFixIssues}
            className="min-h-11 shrink-0 rounded-xl border border-rose-300/40 bg-rose-400/15 px-3.5 text-xs font-bold text-rose-100 transition hover:bg-rose-400/25 focus-visible:ring-2 focus-visible:ring-rose-300 focus-visible:outline-none"
          >
            Resolve refusal
          </button>
        </div>
      ) : null}

      {/* Minimal live CLI command bar + Copy */}
      <div className="mt-4 rounded-2xl border border-white/[0.08] bg-black/55 p-3.5 sm:p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <Terminal className="h-3.5 w-3.5 text-green-400" aria-hidden="true" />
            <span className="font-mono text-[10px] font-bold tracking-[0.16em] text-slate-400 uppercase">
              Minimal founding CLI command (INV-14)
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <div
              role="radiogroup"
              aria-label="CLI subcommand"
              className="inline-flex rounded-lg border border-white/10 bg-black/40 p-0.5 font-mono text-[11px]"
            >
              {SUBCOMMANDS.map((cmd, idx) => (
                <button
                  key={cmd}
                  ref={(el) => {
                    subcmdBtnRefs.current[idx] = el;
                  }}
                  type="button"
                  role="radio"
                  aria-checked={subcommand === cmd}
                  tabIndex={subcommand === cmd ? 0 : -1}
                  onKeyDown={onSubcommandKeyDown(idx)}
                  onClick={() => setSubcommand(cmd)}
                  className={cn(
                    "min-h-11 rounded-md px-3 font-bold transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none sm:min-h-9",
                    subcommand === cmd ? "bg-white/15 text-white" : "text-slate-400 hover:text-slate-200",
                  )}
                >
                  {cmd}
                </button>
              ))}
            </div>
            <button
              type="button"
              onClick={handleCopy}
              aria-label="Copy flife CLI command"
              className="inline-flex min-h-11 items-center gap-1.5 rounded-lg border border-white/15 bg-white/[0.05] px-3.5 font-mono text-[11px] font-bold text-slate-200 transition hover:border-green-400/40 hover:text-white focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none sm:min-h-9"
            >
              {copied ? (
                <>
                  <Check className="h-3.5 w-3.5 text-green-300" aria-hidden="true" />
                  <span className="text-green-300">Copied</span>
                </>
              ) : (
                <>
                  <Copy className="h-3.5 w-3.5" aria-hidden="true" />
                  Copy
                </>
              )}
            </button>
          </div>
        </div>
        <pre className="mt-3 max-w-full overflow-x-auto font-mono text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere] text-green-200 sm:text-[13px] sm:whitespace-pre">
          <code>{cliCommand}</code>
        </pre>
        <span className="sr-only" aria-live="polite">
          {copied ? "CLI command copied to clipboard" : ""}
        </span>
      </div>

      {/* Verification legend */}
      <div className="mt-5 flex flex-wrap items-center justify-between gap-2 border-t border-white/[0.06] pt-4 text-[11px] text-slate-400">
        <span>Click any of the 21 registered slugs to toggle it and recompute minimal CLI flags &amp; ABI:</span>
        <div className="flex flex-wrap items-center gap-3 font-mono text-[10px]">
          <span className="inline-flex items-center gap-1.5 text-green-300">
            <span aria-hidden="true" className="h-2 w-2 rounded-full bg-green-400" />
            Verified &amp; closed (all 21 slugs, 18/18 V2 beads: V2-01..V2-17 + V2-EPIC)
          </span>
          <span className="text-slate-400">104-run Stage-2 G5/G9 recorded</span>
        </div>
      </div>

      {/* Interactive 21-slug toggle matrix */}
      <ul className="mt-3 grid grid-cols-1 gap-2.5 md:grid-cols-2 xl:grid-cols-3">
        {V2_EXPLORER_FEATURES.map((feature) => {
          const active = enabled.has(feature.slug);
          const hasIssue = boundary.issues.some((issue) => issue.slug === feature.slug);
          const badge = BOUNDARY_STYLE[feature.boundary];
          return (
            <li key={feature.slug} className="min-w-0">
              <button
                type="button"
                role="switch"
                aria-checked={active}
                onClick={() => toggleSlug(feature.slug)}
                className={cn(
                  "group flex h-full w-full flex-col justify-between rounded-2xl border p-3.5 text-left transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                  hasIssue
                    ? "border-rose-400/50 bg-rose-500/[0.08]"
                    : active
                      ? "border-green-400/35 bg-green-500/[0.07] hover:border-green-400/55"
                      : "border-white/[0.07] bg-black/25 hover:border-white/20 hover:bg-black/40",
                )}
              >
                <div className="w-full">
                  <div className="flex flex-wrap items-center justify-between gap-1.5">
                    <span className="inline-flex min-w-0 items-center gap-2">
                      <span
                        aria-hidden="true"
                        className={cn(
                          "h-2.5 w-2.5 shrink-0 rounded-full border transition-colors",
                          hasIssue
                            ? "border-rose-300 bg-rose-400"
                            : active
                              ? "border-green-300 bg-green-400"
                              : "border-white/25 bg-transparent",
                        )}
                      />
                      <code
                        className={cn(
                          "font-mono text-[13px] font-bold break-words",
                          active ? "text-white" : "text-slate-400 group-hover:text-slate-200",
                        )}
                      >
                        {feature.slug}
                      </code>
                    </span>
                    <div className="flex flex-wrap items-center gap-1">
                      <span
                        className={cn(
                          "rounded-full border px-1.5 py-0.5 font-mono text-[9px] font-medium tracking-[0.06em] uppercase",
                          feature.verification === "verified"
                            ? "border-green-500/30 bg-green-500/10 text-green-300"
                            : "border-amber-400/30 bg-amber-400/10 text-amber-200",
                        )}
                      >
                        {feature.module} · {feature.verification === "verified" ? "Verified" : "In flight"}
                      </span>
                      <span
                        className={cn(
                          "rounded-full border px-1.5 py-0.5 font-mono text-[9px] font-medium tracking-[0.06em] uppercase",
                          badge.className,
                        )}
                      >
                        {badge.label}
                      </span>
                    </div>
                  </div>
                  <p
                    className={cn(
                      "mt-2 text-xs leading-relaxed",
                      active ? "text-slate-300" : "text-slate-500 group-hover:text-slate-400",
                    )}
                  >
                    {feature.summary}
                  </p>
                </div>

                <p className="mt-2.5 flex w-full flex-wrap items-center justify-between gap-x-2 gap-y-0.5 border-t border-white/[0.05] pt-2 font-mono text-[10px] text-slate-500">
                  <span>
                    {feature.bundleADefault && feature.bundleBDefault
                      ? "Bundles A + B"
                      : feature.bundleADefault
                        ? "Bundle A"
                        : feature.bundleBDefault
                          ? "Bundle B"
                          : "Opt-in"}
                  </span>
                  <span className={cn(active && feature.abiNote !== "none" ? "text-cyan-200" : "text-slate-500")}>
                    ABI: {feature.abiNote}
                  </span>
                </p>
              </button>
            </li>
          );
        })}
      </ul>
    </DemoFrame>
  );
}
