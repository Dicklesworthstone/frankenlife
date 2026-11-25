// A film's camera path as one bar: how long it holds the whole torus, each shot and its zoom, and the moves
// between them, in the film's own seconds (from the manifest's camera keys, scripts/director.ts).

import type { FilmManifest } from "@/lib/media";
import { cn } from "@/lib/utils";

export type CameraSpan = {
  /** Holding one view (`hold`) or easing between two (`move`). */
  kind: "hold" | "move";
  /** The zoom held, or the zoom moved to; 1 is the whole torus. */
  zoom: number;
  /** Where the span starts and how long it lasts, as fractions of the film. */
  start: number;
  share: number;
  seconds: number;
};

type Key = { tick: number; zoom: number };

/**
 * The spans between consecutive camera keys over ticks `from`..`to`: a hold where both keys have the same zoom,
 * a move where they differ. A film shows a frame every `every` ticks at `fps`.
 */
export function cameraSpans(keys: Key[], from: number, to: number, every: number, fps: number): CameraSpan[] {
  const total = to - from;
  if (total <= 0 || keys.length < 2) return [];
  const spans: CameraSpan[] = [];
  for (let at = 1; at < keys.length; at += 1) {
    const a = keys[at - 1]!;
    const b = keys[at]!;
    const ticks = b.tick - a.tick;
    if (ticks <= 0) continue;
    spans.push({
      kind: a.zoom === b.zoom ? "hold" : "move",
      zoom: b.zoom,
      start: (a.tick - from) / total,
      share: ticks / total,
      seconds: ticks / every / fps,
    });
  }
  return spans;
}

const label = (zoom: number) => (zoom === 1 ? "whole torus" : `${zoom}×`);

export function CameraTrack({ film }: { film: FilmManifest }) {
  const { world, video, render } = film;
  const keys = render.camera?.keys ?? [];
  const spans = cameraSpans(keys, world.from.tick, world.to.tick, world.every, video.fps);
  if (spans.length === 0) return null;
  const length = video.frames / video.fps;
  const whole = spans.filter((span) => span.kind === "hold" && span.zoom === 1).reduce((sum, span) => sum + span.seconds, 0);
  return (
    <figure className="mt-5">
      <figcaption className="flex flex-wrap items-baseline justify-between gap-2 text-xs text-slate-500">
        <span className="font-mono tracking-[0.14em] text-slate-400 uppercase">The camera</span>
        <span className="tabular-nums">
          {whole.toFixed(1)} s of {length.toFixed(1)} s on the whole torus
        </span>
      </figcaption>
      <ol
        className="mt-2 flex h-9 overflow-hidden rounded-lg border border-white/10"
        aria-label={`The camera over ${length.toFixed(1)} seconds`}
      >
        {spans.map((span) => (
          <li
            key={span.start}
            style={{ width: `${span.share * 100}%` }}
            className={cn(
              "flex min-w-0 items-center justify-center overflow-hidden border-white/10 font-mono text-[10.5px] whitespace-nowrap tabular-nums not-first:border-l",
              span.kind === "move"
                ? "bg-[repeating-linear-gradient(135deg,rgba(255,255,255,0.06)_0_4px,transparent_4px_8px)] text-transparent"
                : span.zoom === 1
                  ? "bg-green-500/20 text-green-100"
                  : "bg-cyan-400/15 text-cyan-100",
            )}
          >
            {span.kind === "move" ? (
              <span className="sr-only">moving, {span.seconds.toFixed(1)} s</span>
            ) : (
              <>
                {/* A phone has room for the zoom alone; a wider screen for the view and its seconds. */}
                <span aria-hidden="true" className={cn("sm:hidden", span.share < 0.08 && "hidden")}>
                  {span.zoom === 1 ? "whole" : `${span.zoom}×`}
                </span>
                <span className={cn("sr-only", span.share >= 0.1 && "sm:not-sr-only")}>
                  {label(span.zoom)} · {span.seconds.toFixed(1)} s
                </span>
              </>
            )}
          </li>
        ))}
      </ol>
    </figure>
  );
}
