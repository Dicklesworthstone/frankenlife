"use client";

import { ChevronLeft, ChevronRight, Quote, ScanSearch, Search, X } from "lucide-react";
import {
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type UIEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";

import { BasisBadge, DemoFrame, fixed } from "@/components/explain/demo";
import { siteConfig } from "@/lib/content";
import { type ControlStill, controls, filmById } from "@/lib/media";
import { TONE } from "@/lib/palette";

// The controls are the confluent world's, drawn as the confluent film is.
const film = filmById("confluent");
import { cn, grouped } from "@/lib/utils";

/**
 * The film's world beside its model's two controls (plan §5.4), each drawn by
 * `flife render` at the tick the showcase scored, the whole torus, with the
 * film's settings (lib/controls.json, scripts/controls.ts). A loupe shows the
 * same place in all three worlds at flife's own pixels.
 */

/** The loupe's diameter as a fraction of a still's width. */
const LOUPE = 0.4;
const ZOOMS = [
  { value: "3", label: "Loupe ×3" },
  { value: "6", label: "×6" },
] as const;
type Zoom = (typeof ZOOMS)[number]["value"];

type Spot = { x: number; y: number; pinned: boolean };

const OPENING: Spot = { x: 0.73, y: 0.78, pinned: true };

const clamp = (value: number) => Math.min(1, Math.max(0, value));

export function ControlsTrio() {
  const { stills } = controls;
  // The loupe opens on a lime labyrinth of the film's world, where the scatter of the inheritance control
  // also shows a few bodies; it is the reader's to move from there.
  const [spot, setSpot] = useState<Spot | null>(OPENING);
  // The spot a reader kept (a click, a tap or the arrow keys). A mouse's hover only previews: when it leaves a
  // still, the loupe goes back to the kept spot, or away if none was kept.
  const kept = useRef<Spot | null>(OPENING);
  // The spot kept when the loupe was last hidden (the button or Escape), for Show to bring back.
  const hidden = useRef<Spot>(OPENING);
  const place = useCallback((next: Spot | null) => {
    if (next === null && kept.current) hidden.current = kept.current;
    if (next === null || next.pinned) kept.current = next;
    setSpot(next);
  }, []);
  const unhover = useCallback(() => setSpot(kept.current), []);
  const [zoom, setZoom] = useState<Zoom>("3");
  const [width, setWidth] = useState(0);
  const [current, setCurrent] = useState(0);
  const scroller = useRef<HTMLOListElement>(null);

  // A still's width on screen, for the loupe's scale readout.
  useEffect(() => {
    const first = scroller.current?.querySelector("[data-still]");
    if (!first || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry?.contentRect.width ?? 0));
    observer.observe(first);
    return () => observer.disconnect();
  }, []);

  const onScroll = useCallback((event: UIEvent<HTMLOListElement>) => {
    const list = event.currentTarget;
    const items = Array.from(list.children) as HTMLElement[];
    const middle = list.scrollLeft + list.clientWidth / 2;
    let nearest = 0;
    let best = Number.POSITIVE_INFINITY;
    items.forEach((item, index) => {
      const distance = Math.abs(item.offsetLeft + item.offsetWidth / 2 - middle);
      if (distance < best) {
        best = distance;
        nearest = index;
      }
    });
    setCurrent(nearest);
  }, []);

  const goTo = useCallback((index: number) => {
    const list = scroller.current;
    const item = list?.children[index] as HTMLElement | undefined;
    if (!list || !item) return;
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    list.scrollTo({ left: item.offsetLeft - (list.clientWidth - item.offsetWidth) / 2, behavior: reduced ? "auto" : "smooth" });
  }, []);

  if (stills.length === 0) return null;
  const scale = Number(zoom);
  const span = (LOUPE / scale) * controls.torus[0];
  const pixels = width > 0 ? (scale * width) / (stills[0]?.width ?? 1080) : null;

  return (
    <DemoFrame
      eyebrow={`Plan §5.4 · the model and its two controls · seed ${controls.seed}`}
      title="Three worlds, one rule apart."
      basis="render"
      source={
        <>
          Each still is <code>{controls.render.command}</code> on the confluent film&apos;s preset ({controls.preset}) with only its{" "}
          <code>{controls.change}</code> changed, coloured as the confluent film is ({controls.render.mapping}, {controls.render.look}{" "}
          look). At tick {grouped(controls.tick)} each world&apos;s
          SHA-256 digest equals the one the record names for its trial (
          {stills.map((still) => still.record.trial).join(", ")} in {controls.record.json}), and the first is the film&apos;s
          own first frame. The words in quotes: {controls.record.readme}. Rendered {controls.date} on{" "}
          {controls.host}, flife SHA-256 {controls.flife_sha256.slice(0, 12)}…; lib/controls.json records the rest.
        </>
      }
    >
      <p className="max-w-3xl text-sm leading-relaxed text-slate-300 sm:text-base">
        Same preset, seed, {grouped(film.world.founders)} founders and food, {grouped(controls.tick)} ticks, drawn the
        same way. Only one rule differs.
      </p>

      <div className="mt-5 flex flex-wrap items-center gap-x-4 gap-y-3">
        {/* The magnification, as a radio group with thumb-sized buttons. */}
        <div
          role="radiogroup"
          aria-label="Loupe magnification"
          className="inline-flex gap-1 rounded-xl border border-white/10 bg-black/30 p-1"
        >
          {ZOOMS.map((option) => (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={option.value === zoom}
              onClick={() => setZoom(option.value)}
              className={cn(
                "min-h-11 min-w-11 rounded-lg px-3.5 text-xs font-bold transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                option.value === zoom ? "bg-lime-300 text-black" : "text-slate-300 hover:bg-white/5 hover:text-white",
              )}
            >
              {option.label}
            </button>
          ))}
        </div>
        <p className="flex min-w-0 items-center gap-2 text-xs leading-snug text-slate-400">
          <ScanSearch className="h-4 w-4 shrink-0 text-lime-300" aria-hidden="true" />
          <span className="hidden pointer-fine:inline">
            Point at any world, or click to keep the loupe: it shows the same place in all three.
          </span>
          <span className="pointer-fine:hidden">
            Tap a world to place a loupe; swipe to the next, and it stays on the same place.
          </span>
        </p>
        {/* One button that flips, so a keyboard's focus stays on it either way. */}
        <button
          type="button"
          onClick={() => place(spot ? null : hidden.current)}
          className="ml-auto inline-flex min-h-11 items-center gap-1.5 rounded-xl border border-white/15 bg-black/30 px-3.5 text-xs font-bold text-white transition hover:border-green-400/50 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
        >
          {spot ? <X className="h-3.5 w-3.5" aria-hidden="true" /> : <Search className="h-3.5 w-3.5" aria-hidden="true" />}
          {spot ? "Hide loupe" : "Show loupe"}
        </button>
      </div>

      <ol
        ref={scroller}
        onScroll={onScroll}
        aria-label="The confluent world and its two controls at the same tick"
        // slop-ok(S12-three-cards): the film's world and its two plan §5.4 controls, compared side by side.
        className="-mx-5 mt-5 flex snap-x snap-mandatory gap-4 overflow-x-auto px-5 pb-2 [scrollbar-width:none] sm:-mx-7 sm:px-7 lg:mx-0 lg:grid lg:grid-cols-3 lg:gap-5 lg:overflow-visible lg:px-0 lg:pb-0 [&::-webkit-scrollbar]:hidden"
      >
        {stills.map((still, index) => (
          <Panel
            key={still.id}
            still={still}
            index={index}
            count={stills.length}
            spot={spot}
            zoom={scale}
            onSpot={place}
            onLeave={unhover}
            founders={film.world.founders}
          />
        ))}
      </ol>

      {/* Phones and tablets: which world is in view, and a way to the others without swiping. */}
      <div className="mt-3 flex items-center justify-between gap-3 lg:hidden">
        <button
          type="button"
          onClick={() => goTo(Math.max(0, current - 1))}
          aria-disabled={current === 0}
          aria-label="The previous world"
          className="flex h-11 w-11 items-center justify-center rounded-xl border border-white/15 bg-black/30 text-white transition focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none aria-disabled:pointer-events-none aria-disabled:opacity-30"
        >
          <ChevronLeft className="h-4 w-4" aria-hidden="true" />
        </button>
        <div className="flex items-center gap-1">
          {stills.map((still, index) => (
            <button
              key={still.id}
              type="button"
              onClick={() => goTo(index)}
              aria-label={`Show ${still.label}`}
              aria-current={index === current ? "true" : undefined}
              className="flex h-11 min-w-11 items-center justify-center rounded-xl px-2 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
            >
              <span
                className={cn(
                  "h-2 rounded-full transition-[width,background-color] duration-300",
                  index === current ? "w-6" : "w-2 bg-white/25",
                )}
                style={index === current ? { backgroundColor: still.removed ? TONE.wrong : TONE.rule } : undefined}
              />
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={() => goTo(Math.min(stills.length - 1, current + 1))}
          aria-disabled={current === stills.length - 1}
          aria-label="The next world"
          className="flex h-11 w-11 items-center justify-center rounded-xl border border-white/15 bg-black/30 text-white transition focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none aria-disabled:pointer-events-none aria-disabled:opacity-30"
        >
          <ChevronRight className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>

      <p className="mt-3 text-xs leading-relaxed text-slate-400">
        <span className="font-bold text-slate-200">Colour:</span> {controls.render.meaning}.
      </p>
      <p className="mt-1.5 font-mono text-[11px] leading-relaxed text-slate-500 tabular-nums">
        {spot
          ? `The loupe spans ${fixed(span, 1)} of the torus's ${controls.torus[0]} world units${
              pixels ? `, at ${pixels.toFixed(1)} screen pixels to each pixel flife drew` : ""
            }. A body, ${controls.body_diameter} units across, is ${fixed(((stills[0]?.width ?? 1080) / controls.torus[0]) * controls.body_diameter, 1)} of flife's pixels.`
          : `Each still is ${stills[0]?.width ?? 1080} × ${stills[0]?.height ?? 1080} pixels of the whole ${controls.torus[0]} × ${controls.torus[1]} torus.`}
      </p>

      <div className="mt-6 flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-white/5 pt-5">
        <BasisBadge basis="record" />
        <p className="text-base leading-relaxed text-white">
          The domains come from emission, chosen by the networks, and inheritance is necessary.
        </p>
        <p className="text-xs text-slate-500">
          One seed per world, drawn, not a statistical test; the seeded tests are{" "}
          <a className="text-green-300 underline-offset-4 hover:underline" href={siteConfig.fl048}>
            FL-048
          </a>
          .
        </p>
      </div>
    </DemoFrame>
  );
}

function Panel({
  still,
  index,
  count,
  spot,
  zoom,
  onSpot,
  onLeave,
  founders,
}: {
  still: ControlStill;
  index: number;
  count: number;
  spot: Spot | null;
  zoom: number;
  onSpot: (spot: Spot | null) => void;
  /** A mouse left the still: back to the kept spot. */
  onLeave: () => void;
  founders: number;
}) {
  const removed = still.removed !== null;
  const tone = removed ? TONE.wrong : TONE.rule;
  const headingId = `control-${still.id}`;

  // The loupe draws the still as a CSS background, which `loading="lazy"` does not hold back: open from the start,
  // it fetched all three stills with the page, one of them the film's 3 MB poster, wherever the reader was. So it
  // waits for the still's own lazy load, then reuses that file.
  const image = useRef<HTMLImageElement>(null);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    if (image.current?.complete && image.current.naturalWidth > 0) setLoaded(true);
  }, []);

  const at = (event: ReactPointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return { x: clamp((event.clientX - rect.left) / rect.width), y: clamp((event.clientY - rect.top) / rect.height) };
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType === "touch") return;
    onSpot({ ...at(event), pinned: false });
  };
  // A click keeps the loupe where it lands. A touch places it only on a tap, so a swipe to the next world
  // (or down the page) leaves it where it was.
  const touch = useRef<{ x: number; y: number } | null>(null);
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType === "touch") {
      touch.current = { x: event.clientX, y: event.clientY };
      return;
    }
    onSpot({ ...at(event), pinned: true });
  };
  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const start = touch.current;
    touch.current = null;
    if (event.pointerType !== "touch" || !start) return;
    if (Math.hypot(event.clientX - start.x, event.clientY - start.y) < 10) onSpot({ ...at(event), pinned: true });
  };
  const onPointerLeave = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType !== "touch") onLeave();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? 0.1 : 0.025;
    const from = spot ?? { x: 0.5, y: 0.5, pinned: true };
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, -step],
      ArrowDown: [0, step],
    };
    const move = moves[event.key];
    if (move) {
      event.preventDefault();
      onSpot({ x: clamp(from.x + move[0]), y: clamp(from.y + move[1]), pinned: true });
    } else if (event.key === "Escape" && spot) {
      event.preventDefault();
      onSpot(null);
    } else if ((event.key === "Enter" || event.key === " ") && !spot) {
      event.preventDefault();
      onSpot(from);
    }
  };

  const share = still.live / founders;
  return (
    <li className="w-[84%] shrink-0 snap-center sm:w-[58%] lg:w-auto">
      <article aria-labelledby={headingId} className="flex h-full flex-col">
        <div
          data-still
          tabIndex={0}
          role="group"
          aria-roledescription="loupe"
          aria-label={`${still.label}. Arrow keys move a loupe that shows the same place in all three worlds; Escape hides it.`}
          onPointerMove={onPointerMove}
          onPointerDown={onPointerDown}
          onPointerUp={onPointerUp}
          onPointerCancel={() => {
            touch.current = null;
          }}
          onPointerLeave={onPointerLeave}
          onKeyDown={onKeyDown}
          className="group relative aspect-square cursor-crosshair overflow-hidden rounded-2xl border bg-black shadow-[0_24px_60px_-28px_rgba(0,0,0,0.9)] select-none focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
          style={{ borderColor: `${tone}40` }}
        >
          <img
            ref={image}
            src={still.src}
            alt={`${still.label}, drawn by flife at tick ${grouped(still.tick)}: ${still.record.by_eye}.`}
            width={still.width}
            height={still.height}
            loading="lazy"
            decoding="async"
            draggable={false}
            onLoad={() => setLoaded(true)}
            className="pointer-events-none h-full w-full"
          />
          {/* Shade under the label so it reads over any tissue. */}
          <span
            aria-hidden="true"
            className="pointer-events-none absolute inset-x-0 top-0 h-20 bg-gradient-to-b from-black/70 to-transparent"
          />
          <span className="pointer-events-none absolute top-3 left-3 inline-flex items-center gap-2 rounded-lg border border-white/10 bg-black/60 px-2 py-1 font-mono text-[11px] text-white backdrop-blur-sm">
            <span aria-hidden="true" className="h-2 w-2 rounded-full" style={{ backgroundColor: tone, boxShadow: `0 0 10px ${tone}` }} />
            {index + 1}/{count}
          </span>
          <span className="pointer-events-none absolute top-3 right-3 rounded-lg border border-white/10 bg-black/60 px-2 py-1 font-mono text-[11px] text-slate-200 tabular-nums backdrop-blur-sm">
            tick {grouped(still.tick)}
          </span>
          <span className="pointer-events-none absolute bottom-3 left-3 rounded-lg border border-white/10 bg-black/60 px-2 py-1 font-mono text-[11px] text-slate-200 tabular-nums backdrop-blur-sm">
            {grouped(still.live)} {still.live === 1 ? "body" : "bodies"}
          </span>
          {spot && loaded ? <Loupe src={still.src} spot={spot} zoom={zoom} tone={tone} /> : null}
        </div>

        <div className="mt-4 flex flex-1 flex-col">
          <p className="text-[10px] font-black tracking-[0.2em] uppercase" style={{ color: tone }}>
            {removed ? `Removed: ${still.removed}` : "Nothing removed"}
          </p>
          <h5 id={headingId} className="mt-1 text-lg text-white sm:text-xl">
            {still.label}
          </h5>
          <p className="mt-1.5 text-sm leading-relaxed text-slate-400">{still.rule}</p>
          <p className="mt-3 flex gap-2 text-sm leading-relaxed text-slate-200">
            <Quote className="mt-1 h-3.5 w-3.5 shrink-0 text-slate-500" aria-hidden="true" />
            <span>
              {still.record.by_eye}
              <span className="text-slate-500"> · the record, by eye</span>
            </span>
          </p>
          {/* Alive against the ceiling, on one bar: the full world fills it, the controls do not. */}
          <div className="mt-auto pt-4">
            <div className="flex items-baseline justify-between gap-3">
              <span className="text-[9px] font-black tracking-[0.2em] text-slate-500 uppercase">Alive</span>
              <span className="font-mono text-base text-white tabular-nums sm:text-lg">{grouped(still.live)}</span>
            </div>
            <div
              className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-white/[0.06]"
              role="img"
              aria-label={`${grouped(still.live)} of a ceiling of ${grouped(controls.ceiling)}`}
            >
              <div
                className="h-full rounded-full"
                style={{ width: `${Math.max(0.5, (still.live / controls.ceiling) * 100)}%`, backgroundColor: removed ? TONE.wrong : TONE.rule }}
              />
            </div>
            <p className="mt-1 text-[11px] leading-snug text-slate-500">
              {still.live === controls.ceiling
                ? "the ceiling"
                : `${share < 1 ? "fewer than" : `${fixed(share, 1)}×`} the ${grouped(founders)} founders`}
            </p>
          </div>
        </div>
      </article>
    </li>
  );
}

/** A magnifier over `src` at `spot`, `zoom` times the still's displayed size. */
function Loupe({ src, spot, zoom, tone }: { src: string; spot: Spot; zoom: number; tone: string }) {
  // Where `spot` sits in a background `zoom / LOUPE` times the loupe's size, so it lands at the loupe's centre.
  const position = (value: number) => ((value * zoom - LOUPE / 2) / (zoom - LOUPE)) * 100;
  return (
    <div
      aria-hidden="true"
      data-loupe
      className="pointer-events-none absolute aspect-square rounded-full border-2 bg-black"
      style={{
        width: `${LOUPE * 100}%`,
        left: `calc(${spot.x * 100}% - ${LOUPE * 50}%)`,
        top: `calc(${spot.y * 100}% - ${LOUPE * 50}%)`,
        borderColor: "rgba(255,255,255,0.85)",
        boxShadow: `0 0 0 1px rgba(0,0,0,0.7), 0 0 24px ${tone}55, 0 18px 40px rgba(0,0,0,0.7)`,
        backgroundImage: `url(${src})`,
        backgroundRepeat: "no-repeat",
        backgroundSize: `${(zoom / LOUPE) * 100}%`,
        backgroundPosition: `${position(spot.x)}% ${position(spot.y)}%`,
      }}
    >
      <span className="absolute top-1/2 left-1/2 h-3 w-px -translate-x-1/2 -translate-y-1/2 bg-white/70" />
      <span className="absolute top-1/2 left-1/2 h-px w-3 -translate-x-1/2 -translate-y-1/2 bg-white/70" />
    </div>
  );
}
