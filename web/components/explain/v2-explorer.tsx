"use client";

import { AlertTriangle, Check, Copy, RotateCcw, Terminal } from "lucide-react";
import { type KeyboardEvent, useMemo, useRef, useState } from "react";

import { DemoFrame } from "@/components/explain/demo";
import {
  V2_EXPLORER_FEATURES,
  V2_PRESETS,
  type V2PresetId,
  buildMinimalV2CliFlags,
  computeV2Abi,
  formatV2CliCommand,
  nextRadioIndex,
  validateV2Selection,
} from "@/components/explain/v2-explorer-model";
import { cn } from "@/lib/utils";
import { atlasEntry } from "@/lib/v2-atlas";
import type { V2FeatureSlug } from "../../scripts/media-options";

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

  const columns = useMemo(() => {
    const where = (feature: (typeof V2_EXPLORER_FEATURES)[number]) =>
      feature.bundleADefault && feature.bundleBPreset
        ? "both"
        : feature.bundleADefault
          ? "tissue"
          : feature.bundleBPreset
            ? "supercolony"
            : "neither";
    const of = (group: string) => V2_EXPLORER_FEATURES.filter((feature) => where(feature) === group);
    return [
      [{ title: "The tissue's own", features: of("tissue") }],
      [{ title: "On in both presets", features: of("both") }],
      [
        { title: "The supercolony's own", features: of("supercolony") },
        { title: "Off in both", features: of("neither") },
      ],
    ];
  }, []);

  const tile = "rounded-2xl border border-white/[0.08] bg-black/40 p-4";
  const tileLabel = "font-mono text-[10px] tracking-[0.16em] text-slate-400 uppercase";

  return (
    <DemoFrame
      eyebrow="field-ecology-v2 · 21 switches"
      title="Pick the switches."
      basis="rule"
      source={
        <>
          Computed in your browser from the rules of <code className="text-slate-300">crates/flife-core/src/v2.rs</code>{" "}
          (<code className="text-slate-300">V2Feature::ALL</code>, <code className="text-slate-300">V2Config::abi</code>,{" "}
          <code className="text-slate-300">validate_v2</code>), as <code className="text-slate-300">flife --list-features</code>{" "}
          reports them. It configures a world and runs none, so it makes no ecological claim; the 104 Stage-2 runs are in{" "}
          <code className="text-slate-300">experiments/v2-emergence/README.md</code>.
        </>
      }
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div
          role="radiogroup"
          aria-label="Start from"
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
        {isModifiedFromPreset ? (
          <button
            type="button"
            onClick={resetToPreset}
            className="inline-flex min-h-11 items-center gap-1.5 rounded-xl border border-white/15 bg-black/40 px-3 text-xs font-bold text-slate-200 transition hover:border-green-400/40 hover:text-white focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
          >
            <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
            Back to {preset.shortLabel.toLowerCase()}
          </button>
        ) : null}
      </div>
      <p className="mt-3 text-[13px] leading-relaxed text-slate-400">
        {preset.note} <span className="text-slate-500">Base file:</span>{" "}
        <code className="[overflow-wrap:anywhere] text-slate-300">{preset.presetPath}</code>
      </p>

      <div className="mt-5 grid grid-cols-1 gap-x-6 gap-y-6 md:grid-cols-3">
        {columns.map((column, at) => (
          <div key={at} className="space-y-5">
            {column.map((group) => (
              <div key={group.title}>
                <p className="font-mono text-[10px] tracking-[0.16em] text-slate-500 uppercase">{group.title}</p>
                <ul className="mt-2 space-y-1.5">
                  {group.features.map((feature) => {
                    const active = enabled.has(feature.slug);
                    const hasIssue = boundary.issues.some((issue) => issue.slug === feature.slug);
                    return (
                      <li key={feature.slug}>
                        <button
                          type="button"
                          role="switch"
                          aria-checked={active}
                          title={feature.summary}
                          onClick={() => toggleSlug(feature.slug)}
                          className={cn(
                            "flex min-h-12 w-full items-center gap-3 rounded-xl border px-3 py-2 text-left transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                            hasIssue
                              ? "border-rose-400/50 bg-rose-500/[0.08]"
                              : active
                                ? "border-green-400/35 bg-green-500/[0.07] hover:border-green-400/55"
                                : "border-white/[0.07] bg-black/25 hover:border-white/20",
                          )}
                        >
                          <span
                            aria-hidden="true"
                            className={cn(
                              "relative h-4 w-7 shrink-0 rounded-full transition-colors",
                              hasIssue ? "bg-rose-400/70" : active ? "bg-green-500" : "bg-white/15",
                            )}
                          >
                            <span
                              className={cn(
                                "absolute top-0.5 h-3 w-3 rounded-full bg-white transition-all",
                                active ? "left-3.5" : "left-0.5",
                              )}
                            />
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className={cn("block text-[13px] leading-snug font-semibold", active ? "text-white" : "text-slate-400")}>
                              {atlasEntry(feature.slug).name}
                            </span>
                            <code className="block font-mono text-[10.5px] text-slate-500">{feature.slug}</code>
                          </span>
                          {feature.abiNote !== "none" && !feature.abiNote.startsWith("none") ? (
                            <span className={cn("max-w-[7.5rem] shrink-0 text-right font-mono text-[10px] leading-tight", active ? "text-cyan-200" : "text-slate-600")}>
                              {feature.abiNote}
                            </span>
                          ) : null}
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </div>
            ))}
          </div>
        ))}
      </div>

      {boundary.issues.length > 0 ? (
        <div
          role="alert"
          className="mt-5 flex flex-wrap items-start justify-between gap-3 rounded-2xl border border-rose-400/35 bg-rose-500/[0.08] p-4"
        >
          <div className="min-w-0 flex-1 space-y-1.5">
            <p className="flex items-center gap-2 font-mono text-xs font-bold tracking-[0.1em] text-rose-200 uppercase">
              <AlertTriangle className="h-4 w-4 shrink-0 text-rose-300" aria-hidden="true" />
              flife validate refuses this combination
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
            Make it valid
          </button>
        </div>
      ) : null}

      <dl className="mt-5 grid grid-cols-2 gap-3 lg:grid-cols-4">
        <div className={tile}>
          <dt className={tileLabel}>Switches on</dt>
          <dd className="mt-1.5 font-mono text-2xl font-extrabold text-white tabular-nums">
            {enabled.size}
            <span className="text-sm font-medium text-slate-500"> / 21</span>
          </dd>
        </div>
        <div className={tile}>
          <dt className={tileLabel}>Each network</dt>
          <dd className="mt-1.5 font-mono text-2xl font-extrabold text-white tabular-nums">
            {abi.inputWidth}
            <span className="mx-1.5 text-slate-500">→</span>
            {abi.actionWidth}
          </dd>
          <dd className="mt-1 text-[11px] leading-snug text-slate-500">
            inputs → decisions
            {abi.rawOutputWidth !== abi.actionWidth ? `, ${abi.rawOutputWidth} raw outputs` : ""}
            {abi.metalF16Aligned ? "" : "; not 4-aligned, so the CPU fallback runs it"}
          </dd>
          {abi.sensorDelta.length > 0 || abi.actionDelta.length > 0 ? (
            <dd className="mt-1 font-mono text-[10px] leading-snug break-words text-cyan-200/80">
              {[...abi.sensorDelta, ...abi.actionDelta].join(" · ")}
            </dd>
          ) : null}
        </div>
        <div className={tile}>
          <dt className={tileLabel}>The field</dt>
          <dd className="mt-1.5 font-mono text-2xl font-extrabold text-white tabular-nums">
            {abi.fieldPlanes}
            <span className="ml-1.5 text-xs font-medium text-slate-400">planes</span>
          </dd>
          <dd className="mt-1 text-[11px] leading-snug text-slate-500">
            {abi.fieldPlanes === 34 ? "25 metabolic and 9 for the trail carrier" : "energy and 32 chemical channels"}
          </dd>
        </div>
        <div className={tile}>
          <dt className={tileLabel}>Bodies</dt>
          <dd className={cn("mt-1.5 text-[15px] font-bold", boundary.inv08PureField ? "text-green-300" : "text-amber-200")}>
            {boundary.inv08PureField ? "Points" : "Bodies that touch"}
          </dd>
          <dd className="mt-1 text-[11px] leading-snug text-slate-500">
            {boundary.inv08PureField
              ? "Organisms meet only through the field (INV-08)."
              : preset.hasContact
                ? "The preset's [contact] table gives them bodies."
                : "A switch that needs bodies, on a preset without them: validate refuses it."}
          </dd>
        </div>
      </dl>

      <div className="mt-4 rounded-2xl border border-white/[0.08] bg-black/55 p-3.5 sm:p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <Terminal className="h-3.5 w-3.5 text-green-400" aria-hidden="true" />
            <span className="font-mono text-[10px] font-bold tracking-[0.16em] text-slate-400 uppercase">
              The command that founds it
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
              aria-label="Copy the flife command"
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
          {copied ? "Command copied" : ""}
        </span>
      </div>
    </DemoFrame>
  );
}
