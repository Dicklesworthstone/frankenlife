"use client";

import { Pause, Play } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Reticle } from "@/components/franken";
import { controls, film } from "@/lib/media";
import { cn, grouped } from "@/lib/utils";

const { video, poster, world, render } = film;
/** The torus's width in world units (the preset's [world], recorded in lib/controls.json). */
const torusWidth = controls.torus[0];
const last = video.frames - 1;
const secondsPerTick = world.timestep[0] / world.timestep[1];
// flife render's own words for the colours, up to its colon: "founder lineage".
const colour = render.meaning.split(":")[0]?.trim() || render.mapping;

/** The frame shown at `time` seconds into the video. */
function frameAt(time: number): number {
  return Math.min(last, Math.max(0, Math.floor(time * video.fps + 1e-6)));
}

type Shot = { from: number; to: number; label: string; hold: boolean };

/**
 * The film's shots, from the camera keys in the manifest: a hold between two
 * keys of one zoom, a move between holds. A film without a camera is one shot.
 */
const SHOTS: Shot[] = (() => {
  const keys = render.camera?.keys ?? [];
  if (keys.length < 2) return [{ from: world.from.tick, to: world.to.tick, label: "whole torus", hold: true }];
  return keys.slice(1).map((key, index) => {
    const start = keys[index]!;
    const hold = start.zoom === key.zoom;
    return {
      from: start.tick,
      to: key.tick,
      hold,
      label: !hold ? "" : key.zoom < 1.05 ? "whole" : `×${key.zoom}`,
    };
  });
})();
const span = world.to.tick - world.from.tick || 1;
const position = (tick: number) => `${((tick - world.from.tick) / span) * 100}%`;

/**
 * The rendered world, with a readout of the frame on screen: its committed
 * tick, the simulated time since the film's first frame and the organisms
 * alive, each from flife render's own record of that frame.
 */
export function Film({ className, feather = false }: { className?: string; feather?: boolean }) {
  const ref = useRef<HTMLVideoElement>(null);
  const [frame, setFrame] = useState(0);
  const [playing, setPlaying] = useState(false);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    // The film is over 20 MB: a reader who asks for less motion or less data gets the poster and the button.
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const saving = (navigator as { connection?: { saveData?: boolean } }).connection?.saveData === true;
    if (!still && !saving) {
      element.play().catch(() => {
        // Autoplay refused (a data saver, say): the poster stays, and the button plays it.
      });
    }
    let handle = 0;
    let stopped = false;
    // React skips the render when the frame is the one already shown.
    const show = (time: number) => setFrame(frameAt(time));
    // Older Safari and Firefox have no per-frame callback; `timeupdate` is coarser but correct.
    const perFrame =
      typeof (element as { requestVideoFrameCallback?: unknown }).requestVideoFrameCallback === "function";
    if (perFrame) {
      const onFrame = (_now: number, metadata: VideoFrameCallbackMetadata) => {
        show(metadata.mediaTime);
        if (!stopped) handle = element.requestVideoFrameCallback(onFrame);
      };
      handle = element.requestVideoFrameCallback(onFrame);
      return () => {
        stopped = true;
        element.cancelVideoFrameCallback(handle);
      };
    }
    const onTime = () => show(element.currentTime);
    element.addEventListener("timeupdate", onTime);
    return () => element.removeEventListener("timeupdate", onTime);
  }, []);

  // Off screen the film pauses, so it decodes and redraws nothing while the reader is elsewhere; it plays on
  // when it comes back, but only if it was playing when it left (a reader's own pause stays).
  const pausedOffScreen = useRef(false);
  useEffect(() => {
    const element = ref.current;
    if (!element || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(([entry]) => {
      if (!entry) return;
      if (!entry.isIntersecting) {
        if (!element.paused) {
          pausedOffScreen.current = true;
          element.pause();
        }
      } else if (pausedOffScreen.current) {
        pausedOffScreen.current = false;
        element.play().catch(() => {});
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const toggle = () => {
    const element = ref.current;
    if (!element) return;
    pausedOffScreen.current = false;
    if (element.paused) {
      element.play().catch(() => {});
    } else {
      element.pause();
    }
  };

  /** Show frame `target`, playing or not. */
  const seek = (target: number) => {
    const element = ref.current;
    const clamped = Math.min(last, Math.max(0, target));
    setFrame(clamped);
    // The middle of the frame's interval, so rounding never lands on its neighbour.
    if (element) element.currentTime = (clamped + 0.5) / video.fps;
  };

  const tick = world.ticks[frame] ?? world.from.tick;
  const live = world.live[frame] ?? 0;
  const simulated = (tick - world.from.tick) * secondsPerTick;
  const zoom = render.zoom?.[frame] ?? 1;

  // A scale bar in world units: the view spans torus / zoom units across the frame.
  const barUnits = zoom >= 2 ? 5 : 10;
  const barPercent = (barUnits * zoom * 100) / torusWidth;

  return (
    <figure className={cn("relative", className)}>
      {/* A container, so the scale bar is sized in the frame's width (cqw), not its padded row's. */}
      <div className="@container relative aspect-square w-full overflow-hidden bg-black lg:rounded-[2rem]">
        <video
          ref={ref}
          className={cn("h-full w-full object-cover", feather && "specimen-feather")}
          src={video.src}
          poster={poster.src}
          width={video.width}
          height={video.height}
          muted
          loop
          playsInline
          preload="metadata"
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          aria-label={`The ${world.name} preset, seed ${world.seed}, from tick ${grouped(world.from.tick)} to ${grouped(world.to.tick)}: organisms coloured by ${colour}${render.field_mapping === "none" ? "" : ` over the field's ${render.field_mapping}`}`}
        />
        <Reticle inset={14} size={18} />

        {/* What this is, on its face. */}
        <div className="pointer-events-none absolute top-4 left-4 z-10 flex items-center gap-2 rounded-full border border-white/10 bg-black/60 px-3 py-1.5 backdrop-blur-md sm:top-5 sm:left-5">
          <span
            aria-hidden="true"
            className={cn("h-1.5 w-1.5 rounded-full bg-green-400", playing && "pulse-dot")}
          />
          <span className="font-mono text-[10px] font-medium tracking-[0.16em] text-green-200 uppercase">
            Rendered by flife · not drawn
          </span>
        </div>
        <button
          type="button"
          onClick={toggle}
          className="absolute top-3 right-3 z-20 flex h-11 w-11 items-center justify-center rounded-xl border border-white/15 bg-black/60 text-white backdrop-blur-md transition-colors hover:border-green-400/60 hover:text-green-300 sm:top-4 sm:right-4"
          aria-label={playing ? "Pause the film" : "Play the film"}
        >
          {playing ? <Pause className="h-4 w-4" /> : <Play className="h-4 w-4" />}
        </button>

        {/* The instrument's readout: the frame's own record, a scale bar, and the shots on the tick axis. */}
        <div className="absolute inset-x-0 bottom-0 z-10 bg-gradient-to-t from-black via-black/70 to-transparent px-4 pt-28 pb-3 sm:px-6 sm:pb-4">
          <div className="pointer-events-none flex items-end justify-between gap-4">
            <dl className="grid flex-1 grid-cols-4 gap-2 font-mono text-white sm:gap-4">
              <Readout label="Tick" value={grouped(tick)} />
              <Readout label="Sim time" value={`+${simulated.toFixed(1)} s`} />
              <Readout label="Alive" value={grouped(live)} />
              <Readout label="View" value={zoom < 1.05 ? "whole" : `×${zoom.toFixed(1)}`} />
            </dl>
          </div>
          <div className="pointer-events-none mt-3 flex items-center gap-3" aria-hidden="true">
            <span className="relative h-2 shrink-0 border-x border-b border-white/70" style={{ width: `${barPercent}cqw` }} />
            <span className="font-mono text-[10px] text-slate-300">{barUnits} world units</span>
            <span className="ml-auto hidden font-mono text-[10px] text-slate-400 sm:inline">
              {grouped(world.founders)} founders · {world.grid[0]}² field · colour = {colour}
            </span>
          </div>
          <div className="relative mt-3 h-8 rounded-md has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-green-400/70">
            <div className="absolute inset-x-0 top-1 h-1.5 overflow-hidden rounded-full bg-white/[0.08]">
              {/* Elapsed first, the shots over it, so a hold stays readable once played. */}
              <span aria-hidden="true" className="absolute inset-y-0 left-0 bg-green-400/25" style={{ width: position(tick) }} />
              {SHOTS.filter((shot) => shot.hold).map((shot) => (
                <span
                  key={shot.from}
                  aria-hidden="true"
                  className={
                    tick >= shot.from && tick <= shot.to
                      ? "absolute inset-y-0 rounded-full bg-green-300/80"
                      : "absolute inset-y-0 rounded-full bg-green-500/35"
                  }
                  style={{
                    left: position(shot.from),
                    width: `calc(${position(shot.to)} - ${position(shot.from)} - 2px)`,
                    marginLeft: "1px",
                  }}
                />
              ))}
            </div>
            <span
              aria-hidden="true"
              className="pointer-events-none absolute top-0 h-3.5 w-[3px] -translate-x-1/2 rounded-full bg-green-200 shadow-[0_0_10px_rgba(134,239,172,0.9)]"
              style={{ left: position(tick) }}
            />
            {SHOTS.filter((shot) => shot.hold).map((shot) => (
              <span
                key={`label-${shot.from}`}
                aria-hidden="true"
                className="pointer-events-none absolute top-3.5 -translate-x-1/2 font-mono text-[10px] text-slate-400"
                style={{ left: `calc((${position(shot.from)} + ${position(shot.to)}) / 2)` }}
              >
                {shot.label}
              </span>
            ))}
            <input
              type="range"
              min={0}
              max={last}
              step={1}
              value={frame}
              onChange={(event) => seek(Number(event.target.value))}
              aria-label="Seek the film by tick"
              aria-valuetext={`tick ${grouped(tick)}, ${zoom < 1.05 ? "the whole torus" : `zoom ${zoom.toFixed(1)}`}`}
              className="absolute inset-x-0 -top-2 h-8 w-full cursor-pointer opacity-0"
            />
          </div>
        </div>
      </div>
    </figure>
  );
}

function Readout({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <dt className="truncate text-[9px] font-semibold tracking-[0.2em] text-green-200 uppercase [text-shadow:0_1px_6px_rgba(0,0,0,0.9)]">{label}</dt>
      <dd className="truncate text-[13px] tabular-nums sm:text-base">{value}</dd>
    </div>
  );
}
