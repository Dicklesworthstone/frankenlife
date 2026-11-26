"use client";

import { Monitor, MousePointerClick } from "lucide-react";
import { useState } from "react";

import { DemoFrame } from "@/components/explain/demo";
import { type Cap, GESTURES, GROUPS, KEYS, type KeyGroup, ROWS, SPLIT_ROWS } from "@/components/explain/studio-keymap";
import { TONE } from "@/lib/palette";
import { cn } from "@/lib/utils";

const COLOR = Object.fromEntries(GROUPS.map((group) => [group.id, group.color])) as Record<KeyGroup, string>;

/**
 * What the native Studio does from the keyboard: each live key coloured by
 * what it touches, its action on hover, focus or tap (crates/flife-studio).
 */
export function StudioKeys() {
  const [selected, setSelected] = useState("Space");
  const [focusGroup, setFocusGroup] = useState<KeyGroup | null>(null);
  const shown = KEYS[selected] ?? KEYS.Space!;
  const shownGroup = GROUPS.find((group) => group.id === shown.group);

  const renderCap = (cap: Cap, key: string) => {
    const live = KEYS[cap.label];
    const style = { gridColumn: `span ${cap.span} / span ${cap.span}` };
    if (!live) {
      return (
        <div key={key} style={style} className="p-[2px]" aria-hidden="true">
          <div
            className={cn(
              "flex h-8 items-center justify-center rounded-md text-[11px] text-slate-600 sm:h-10",
              cap.label && "border border-white/[0.05] bg-white/[0.02]",
            )}
          >
            {cap.label}
          </div>
        </div>
      );
    }
    const color = COLOR[live.group];
    const on = selected === cap.label;
    // A key outside the chosen group loses its colour, not its legibility: its label stays 6:1
    // (opacity alone could not keep magenta or coral at WCAG AA's 4.5:1 and still read as dimmed).
    const dimmed = !on && focusGroup !== null && focusGroup !== live.group;
    return (
      <div key={key} style={style} className="p-[2px]">
        <button
          type="button"
          aria-pressed={on}
          aria-label={`${live.name}: ${live.action}`}
          onMouseEnter={() => setSelected(cap.label)}
          onFocus={() => setSelected(cap.label)}
          onClick={() => setSelected(cap.label)}
          className={cn(
            "flex h-8 w-full items-center justify-center rounded-md border font-mono text-xs font-bold transition sm:h-10",
            "focus-visible:ring-2 focus-visible:ring-white/80 focus-visible:outline-none",
          )}
          style={
            dimmed
              ? { color: TONE.muted, background: "transparent", borderColor: "rgba(255,255,255,0.1)" }
              : {
                  color: on ? "#020a02" : color,
                  background: on ? color : `${color}1f`,
                  borderColor: `${color}${on ? "" : "66"}`,
                  boxShadow: on ? `0 0 18px -2px ${color}` : undefined,
                }
          }
        >
          {cap.label}
        </button>
      </div>
    );
  };

  return (
    <DemoFrame
      eyebrow="flife-studio · macOS"
      title="A microscope with a keyboard."
      basis="diagram"
      source={
        <>
          crates/flife-studio/src/controls.rs (keys by virtual key code, so any layout) and CONTRACT.md; brush
          amounts from src/brush.rs; README.md. Keys drawn dim do nothing in the Studio.
        </>
      }
    >
      <div className="space-y-5">
        {/* The keyboard: whole from 640 px, each row 30 half-key columns; below, in two halves of 16 columns
            (SPLIT_COLUMNS), so each key is wide enough to tap. The layout not shown is display: none. */}
        <div className="rounded-2xl border border-white/10 bg-black/40 p-2 sm:p-3">
          <div className="max-sm:hidden">
            {ROWS.map((row, rowIndex) => (
              <div key={rowIndex} className="grid grid-cols-[repeat(30,minmax(0,1fr))]">
                {row.map((cap, capIndex) => renderCap(cap, `${rowIndex}-${capIndex}`))}
              </div>
            ))}
          </div>
          <div className="space-y-3 sm:hidden">
            {SPLIT_ROWS.map((half, halfIndex) => (
              <div key={halfIndex}>
                {half.map((row, rowIndex) => (
                  <div key={rowIndex} className="grid grid-cols-[repeat(16,minmax(0,1fr))]">
                    {row.map((cap, capIndex) => renderCap(cap, `${halfIndex}-${rowIndex}-${capIndex}`))}
                  </div>
                ))}
              </div>
            ))}
          </div>
        </div>

        {/* The chosen key. */}
        <div aria-live="polite" className="flex min-h-[4.5rem] items-start gap-3 rounded-xl border border-white/10 bg-black/30 p-3 sm:p-4">
          <span
            className="flex h-9 min-w-9 shrink-0 items-center justify-center rounded-lg px-2 font-mono text-sm font-black text-[#020a02]"
            style={{ background: COLOR[shown.group] }}
          >
            {selected}
          </span>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <p className="text-[10px] font-black tracking-[0.2em] uppercase" style={{ color: COLOR[shown.group] }}>
                {shownGroup?.name}
              </p>
              {shown.v2 ? (
                <span className="rounded-full border border-cyan-400/30 bg-cyan-400/10 px-2 py-0.5 font-mono text-[9px] font-medium tracking-[0.08em] text-cyan-200 uppercase">
                  V2 · v2-visual-mappings
                </span>
              ) : null}
            </div>
            <p className="mt-1 text-sm leading-relaxed text-slate-200">{shown.action}</p>
          </div>
        </div>

        {/* The groups: a legend that also highlights its keys. */}
        <ul className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          {GROUPS.map((group) => {
            const on = focusGroup === group.id;
            return (
              <li key={group.id}>
                <button
                  type="button"
                  aria-pressed={on}
                  onClick={() => setFocusGroup(on ? null : group.id)}
                  className={cn(
                    "flex h-full w-full items-start gap-2.5 rounded-xl border px-3 py-2 text-left transition focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                    on ? "border-white/30 bg-white/[0.06]" : "border-white/10 bg-black/20 hover:border-white/20",
                  )}
                >
                  <span aria-hidden="true" className="mt-1 h-2.5 w-2.5 shrink-0 rounded-sm" style={{ background: group.color }} />
                  <span className="min-w-0">
                    <span className="block text-xs font-bold text-white">{group.name}</span>
                    <span className="block text-[11px] leading-snug text-slate-400">{group.summary}</span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>

        {/* The pointer. */}
        <div className="rounded-xl border border-white/10 bg-black/20 p-3 sm:p-4">
          <p className="mb-2 inline-flex items-center gap-2 text-[10px] font-black tracking-[0.2em] text-amber-200/90 uppercase">
            <MousePointerClick className="h-3.5 w-3.5" aria-hidden="true" /> The pointer
          </p>
          <dl className="space-y-1.5 text-sm">
            {GESTURES.map((entry) => (
              <div key={entry.gesture} className="flex gap-3">
                <dt className="w-12 shrink-0 font-mono text-xs leading-6 text-amber-200">{entry.gesture}</dt>
                <dd className="leading-relaxed text-slate-300">{entry.action}</dd>
              </div>
            ))}
          </dl>
        </div>

        <p className="flex items-start gap-2 text-xs leading-relaxed text-slate-400">
          <Monitor className="mt-0.5 h-4 w-4 shrink-0 text-slate-500" aria-hidden="true" />
          <span>
            The Studio is macOS, drawn through Metal. Look, field, camera and selection keys change only the picture
            (INV-09): nine styles of one 1,200-tick render all end at the same digest. The v2 views on keys 8, 9 and C
            only read state too, and cost nothing on v1. Brushes are commands applied at the next open boundary.
            Headless Cinema export (<code className="text-slate-300">flife-studio --export</code>) replays a timeline at
            a chosen size (<code className="text-slate-300">--export-size 1080p|4k|WxH</code>) and a constant frame rate (
            <code className="text-slate-300">--export-fps</code>), and can sample exact ticks (
            <code className="text-slate-300">--export-every-ticks</code>) and follow a JSON camera path (
            <code className="text-slate-300">--export-camera</code>). The founding-only v2 flags (
            <code className="text-slate-300">--disable</code>, <code className="text-slate-300">--enable</code>,{" "}
            <code className="text-slate-300">--v1-compat</code>) are refused with{" "}
            <code className="text-slate-300">--resume</code>.
          </span>
        </p>
      </div>
    </DemoFrame>
  );
}
