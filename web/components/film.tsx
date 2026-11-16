"use client";

import { Pause, Play } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { FrankenBolt, MicroLabel } from "@/components/franken";
import { film } from "@/lib/media";
import { grouped } from "@/lib/utils";

const { video, poster, world, render } = film;
const last = video.frames - 1;
const secondsPerTick = world.timestep[0] / world.timestep[1];
// flife render's own words for the colours, up to its colon: "founder lineage".
const colour = render.meaning.split(":")[0]?.trim() || render.mapping;

/** The frame shown at `time` seconds into the video. */
function frameAt(time: number): number {
  return Math.min(last, Math.max(0, Math.floor(time * video.fps + 1e-6)));
}

/**
 * The rendered world, with a readout of the frame on screen: its committed
 * tick, the simulated time since the film's first frame and the organisms
 * alive, each from flife render's own record of that frame.
 */
export function Film() {
  const ref = useRef<HTMLVideoElement>(null);
  const [frame, setFrame] = useState(0);
  const [playing, setPlaying] = useState(false);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    // The film is about 10 MB: a reader who asks for less motion or less data gets the poster and the button.
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

  const toggle = () => {
    const element = ref.current;
    if (!element) return;
    if (element.paused) {
      element.play().catch(() => {});
    } else {
      element.pause();
    }
  };

  const tick = world.ticks[frame] ?? world.from.tick;
  const live = world.live[frame] ?? 0;
  const simulated = (tick - world.from.tick) * secondsPerTick;
  const zoom = render.zoom?.[frame] ?? 1;

  return (
    <figure className="glass-modern relative overflow-hidden rounded-3xl p-2 sm:p-3">
      <FrankenBolt className="absolute top-3 left-3 z-10" />
      <FrankenBolt className="absolute top-3 right-3 z-10" />
      <div className="relative aspect-square w-full overflow-hidden rounded-2xl bg-black">
        <video
          ref={ref}
          className="h-full w-full object-cover"
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
          aria-label={`The ${world.name} preset, seed ${world.seed}, from tick ${grouped(world.from.tick)} to ${grouped(world.to.tick)}: organisms coloured by ${colour} over the field's ${render.field_mapping}`}
        />
        <div className="pointer-events-none absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/85 via-black/40 to-transparent px-4 pt-16 pb-4 sm:px-5">
          <dl className="grid grid-cols-4 gap-3 font-mono text-white">
            <div>
              <dt className="text-[9px] font-black tracking-[0.25em] text-green-400/80 uppercase">Tick</dt>
              <dd className="text-sm tabular-nums sm:text-base">{grouped(tick)}</dd>
            </div>
            <div>
              <dt className="text-[9px] font-black tracking-[0.25em] text-green-400/80 uppercase">Sim time</dt>
              <dd className="text-sm tabular-nums sm:text-base">+{simulated.toFixed(1)} s</dd>
            </div>
            <div>
              <dt className="text-[9px] font-black tracking-[0.25em] text-green-400/80 uppercase">Alive</dt>
              <dd className="text-sm tabular-nums sm:text-base">{grouped(live)}</dd>
            </div>
            <div>
              <dt className="text-[9px] font-black tracking-[0.25em] text-green-400/80 uppercase">View</dt>
              <dd className="text-sm tabular-nums sm:text-base">{zoom < 1.05 ? "whole" : `×${zoom.toFixed(1)}`}</dd>
            </div>
          </dl>
        </div>
        <button
          type="button"
          onClick={toggle}
          className="absolute top-3 right-3 flex h-10 w-10 items-center justify-center rounded-xl border border-white/15 bg-black/60 text-white backdrop-blur transition hover:border-green-400/60 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
          aria-label={playing ? "Pause the film" : "Play the film"}
        >
          {playing ? <Pause className="h-4 w-4" /> : <Play className="h-4 w-4" />}
        </button>
      </div>
      <figcaption className="flex flex-wrap items-center justify-between gap-2 px-2 pt-3 pb-1">
        <span className="inline-flex items-center gap-2">
          <span className="pulse-dot h-1.5 w-1.5 rounded-full bg-green-400" aria-hidden="true" />
          <MicroLabel>Rendered by flife, not drawn</MicroLabel>
        </span>
        <span className="font-mono text-[11px] text-slate-400">
          {grouped(world.founders)} founders · {world.grid[0]}² field · colour = {colour}
        </span>
      </figcaption>
    </figure>
  );
}
