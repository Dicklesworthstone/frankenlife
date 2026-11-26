"use client";

import { Pause, Play } from "lucide-react";
import { type Ref, useCallback, useEffect, useRef, useState } from "react";

import { Reticle } from "@/components/franken";
import { type FilmManifest, film as leadFilm } from "@/lib/media";
import { cn, grouped } from "@/lib/utils";

/**
 * The frame shown at `time` seconds into the film. A frame's own time reaches the page in whole microseconds
 * (e.g. 0.033333 at 30 fps or 0.016667 at 60 fps), just short of the frame when rounded down, so a thousandth of a
 * frame is allowed.
 */
export function frameAt(time: number, fps = leadFilm.video.fps, frames = leadFilm.video.frames): number {
  const maxFrame = Math.max(0, frames - 1);
  return Math.min(maxFrame, Math.max(0, Math.floor(time * fps + 1e-3)));
}

/**
 * Width of a `barUnits` world-unit scale bar as a percentage of the frame's width (`cqw`).
 * Matches `flife-manim`'s `Viewport::fit`, which scales by `min(Dw / Tw, Dh / Th)`:
 * on 1:1 frames (`1080×1080`, `2160×2160`) this is `100 * barUnits * zoom / Tw`, and on
 * 16:9 frames (`3840×2160`) of a square torus it scales by `H / W` (`9/16`) so the bar
 * matches the drawn pixels.
 */
export function scaleBarWidthCqw(
  barUnits: number,
  zoom: number,
  torus: readonly [number, number],
  frame: readonly [number, number],
): number {
  const [torusW, torusH] = torus;
  const [frameW, frameH] = frame;
  const pixelsPerUnit = zoom * Math.min(frameW / torusW, frameH / torusH);
  return ((barUnits * pixelsPerUnit) / frameW) * 100;
}

type Shot = { from: number; to: number; label: string; hold: boolean };

/**
 * What a view at `zoom` shows, as the readout names it: the whole torus only
 * at zoom 1 (to rounding), so that a view held at 1.03 is not called whole;
 * below 1.1 to two decimals, so it does not read as ×1.0.
 */
export function zoomView(zoom: number): { short: string; long: string } {
  if (zoom < 1.005) return { short: "whole", long: "the whole torus" };
  const shown = zoom < 1.1 ? zoom.toFixed(2) : zoom.toFixed(1);
  return { short: `×${shown}`, long: `zoom ${shown}` };
}

const HOLD_NOW = "absolute inset-y-0 rounded-full bg-green-300/80";
const HOLD_THEN = "absolute inset-y-0 rounded-full bg-green-500/35";

/** Everything the player derives from one film's manifest, computed once per film. */
function filmModel(manifest: FilmManifest) {
  const { video, poster, world, render } = manifest;
  /** The torus's extent in world units (the run's [world] width and height, recorded by scripts/media.ts). */
  const torusExtent: [number, number] = world.torus;
  const frameSize: [number, number] = [video.width, video.height];
  const secondsPerTick = world.timestep[0] / world.timestep[1];
  // flife render's own words for the colours, up to its colon: "founder lineage".
  const colour = render.meaning.split(":")[0]?.trim() || render.mapping;

  /**
   * The film's shots, from the camera keys in the manifest: a hold between two
   * keys of one zoom, a move between holds. A film without a camera is one shot.
   */
  const shots: Shot[] = (() => {
    const keys = render.camera?.keys ?? [];
    // No camera, or one key flife render holds for the whole film (scripts/media.ts --camera fixed): one shot.
    if (keys.length < 2) {
      const held = keys[0]?.zoom ?? 1;
      return [{ from: world.from.tick, to: world.to.tick, label: held < 1.005 ? "whole torus" : `held at ×${held}`, hold: true }];
    }
    return keys.slice(1).map((key, index) => {
      const start = keys[index]!;
      const hold = start.zoom === key.zoom;
      return {
        from: start.tick,
        to: key.tick,
        hold,
        label: !hold ? "" : zoomView(key.zoom).short === "whole" ? "whole" : `×${key.zoom}`,
      };
    });
  })();
  const span = world.to.tick - world.from.tick || 1;
  const position = (tick: number) => `${((tick - world.from.tick) / span) * 100}%`;

  /** What the readout says of frame `frame`, each from flife render's own record of it. */
  const readings = (frame: number) => {
    const tick = world.ticks[frame] ?? world.from.tick;
    const zoom = render.zoom?.[frame] ?? 1;
    // A scale bar in world units: the view spans torus / zoom units across the frame.
    const barUnits = zoom >= 2 ? 5 : 10;
    return {
      tick,
      tickText: grouped(tick),
      simulated: `+${((tick - world.from.tick) * secondsPerTick).toFixed(1)} s`,
      live: grouped(world.live[frame] ?? 0),
      view: zoomView(zoom).short,
      barWidth: `${scaleBarWidthCqw(barUnits, zoom, torusExtent, frameSize)}cqw`,
      barText: `${barUnits} world units`,
      elapsed: (tick - world.from.tick) / span,
      described: `tick ${grouped(tick)}, ${zoomView(zoom).long}`,
    };
  };

  return {
    video,
    poster,
    world,
    render,
    isSquare: video.width === video.height,
    last: video.frames - 1,
    colour,
    holds: shots.filter((shot) => shot.hold),
    position,
    readings,
    first: readings(0),
    label: `The ${world.name} preset, seed ${world.seed}, from tick ${grouped(world.from.tick)} to ${grouped(world.to.tick)}: organisms coloured by ${colour}${render.field_mapping === "none" ? "" : ` over the field's ${render.field_mapping}`}`,
  };
}

type FilmModel = ReturnType<typeof filmModel>;
const MODELS = new Map<FilmManifest, FilmModel>();
/** One film's model, computed the first time the film is shown. */
function modelOf(manifest: FilmManifest): FilmModel {
  let model = MODELS.get(manifest);
  if (!model) {
    model = filmModel(manifest);
    MODELS.set(manifest, model);
  }
  return model;
}

/** The live dot's opacity at `now`: 1 to 0.35 and back every 2.2 s, eased at both ends, as `.pulse-dot` does it. */
function pulse(now: number): number {
  return 0.675 + 0.325 * Math.cos((2 * Math.PI * now) / 2200);
}

/**
 * Where the phone encode (manifest `video.small`, 1080 pixels square) plays instead of the 4K film: a screen at most
 * 640 CSS pixels wide, where the film spans about 1,300 device pixels at most (a 430-pixel phone at 3×), or at most
 * 640 tall: a short window, where the hero film is drawn smaller (measured in Chrome: 464 device pixels across at
 * 1366 × 640, 848 at 1440 × 600 at 2×, against 1,448 at 1440 × 900 at 2×), or a phone held sideways (the source is
 * chosen once, so turning it later changes nothing). Sideways, a phone draws the film across its whole width, about
 * 2,500 device pixels, so there this encode is softer than the 4K film would be: it is chosen for what a phone can
 * decode and download, not for its pixels. It is H.264 level 4.2, which phones decode, where 2160²
 * at 60 fps needs level 5.2, which not every phone does; and about 56% of the bytes. A browser that ignores `media`
 * on a video's source (Chrome and Firefox before 120) takes the first source, this one, wherever it is.
 */
export const PHONE = "(max-width: 640px), (max-height: 640px)";

/** How often the seek control's own value follows the film; nothing on screen reads it. */
const SEEK_EVERY_MS = 250;

/**
 * The rendered world, with a readout of the frame on screen: its committed
 * tick, the simulated time since the film's first frame and the organisms
 * alive, each from flife render's own record of that frame. The film is one
 * looping MP4 (served from R2 by worker.ts).
 *
 * The readout changes with every frame, so it is written to the page directly
 * (`show`), never through a React render.
 */
export function Film({
  className,
  feather = false,
  manifest = leadFilm,
}: {
  className?: string;
  feather?: boolean;
  /** The film to play; the player keeps one film for its lifetime, so a caller that switches films keys it by id. */
  manifest?: FilmManifest;
}) {
  const {
    video,
    poster,
    world,
    isSquare,
    last,
    colour,
    holds: HOLDS,
    position,
    readings,
    first: FIRST,
    label,
  } = modelOf(manifest);
  const containerRef = useRef<HTMLDivElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const posterRef = useRef<HTMLImageElement>(null);
  const [playing, setPlaying] = useState(false);
  // Whether the film's own frames drive the readout and the live dot (`requestVideoFrameCallback`).
  const [perFrame, setPerFrame] = useState(false);

  const shown = useRef(0);
  const dotRef = useRef<HTMLSpanElement>(null);
  const tickRef = useRef<HTMLElement>(null);
  const simulatedRef = useRef<HTMLElement>(null);
  const liveRef = useRef<HTMLElement>(null);
  const viewRef = useRef<HTMLElement>(null);
  const barRef = useRef<HTMLSpanElement>(null);
  const barTextRef = useRef<HTMLSpanElement>(null);
  const elapsedRef = useRef<HTMLSpanElement>(null);
  const markerRef = useRef<HTMLSpanElement>(null);
  const holdRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const seekRef = useRef<HTMLInputElement>(null);
  const seekAt = useRef(0);
  const pausedOffScreen = useRef(false);

  /** Bring the seek control up to the frame on screen: late while the film plays, at once before it is used. */
  const syncSeek = useCallback(() => {
    const input = seekRef.current;
    if (!input) return;
    seekAt.current = performance.now();
    const value = String(shown.current);
    if (input.value !== value) input.value = value;
    input.setAttribute("aria-valuetext", readings(shown.current).described);
  }, []);

  /** Write frame `frame`'s record on the page, changing only what differs. */
  const show = useCallback(
    (frame: number, seekNow = false) => {
      if (frame !== shown.current) {
        shown.current = frame;
        const now = readings(frame);
        const text = (element: HTMLElement | null, value: string) => {
          if (element && element.textContent !== value) element.textContent = value;
        };
        text(tickRef.current, now.tickText);
        text(simulatedRef.current, now.simulated);
        text(liveRef.current, now.live);
        text(viewRef.current, now.view);
        text(barTextRef.current, now.barText);
        if (barRef.current && barRef.current.style.width !== now.barWidth) barRef.current.style.width = now.barWidth;
        if (elapsedRef.current) elapsedRef.current.style.transform = `scaleX(${now.elapsed})`;
        if (markerRef.current) markerRef.current.style.transform = `translateX(${now.elapsed * 100}%)`;
        HOLDS.forEach((shot, index) => {
          const element = holdRefs.current[index];
          const name = now.tick >= shot.from && now.tick <= shot.to ? HOLD_NOW : HOLD_THEN;
          if (element && element.className !== name) element.className = name;
        });
      }
      if (seekNow || performance.now() - seekAt.current > SEEK_EVERY_MS) syncSeek();
    },
    [syncSeek],
  );

  // The film waits for its poster (preload="none" until then): streaming from the start, it took the bandwidth the
  // poster needed, and a phone on a slow network painted the poster later (a median 5.1 s against 3.8 s with the
  // video held back, frankenlife.org at 390 px under Lighthouse's mobile throttling). Then it autoplays, unless the
  // reader asks for less motion or less data: they get the poster and the button.
  useEffect(() => {
    const element = videoRef.current;
    if (!element) return;
    const start = () => {
      // The film is one MP4 (4K, or on a phone its smaller encode), streamed by range as it plays.
      const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      const saving = (navigator as { connection?: { saveData?: boolean } }).connection?.saveData === true;
      if (element.preload === "none") {
        element.preload = "metadata";
        // Not playing: load its first frame and length, as preload="metadata" would have from the start.
        if ((still || saving) && element.paused && element.readyState === HTMLMediaElement.HAVE_NOTHING) element.load();
      }
      if (!still && !saving) {
        // Scrolled past before the poster came: it plays when it is back in view (the observer below).
        const box = containerRef.current?.getBoundingClientRect();
        if (box && (box.bottom <= 0 || box.top >= window.innerHeight)) {
          pausedOffScreen.current = true;
          return;
        }
        element.play().catch(() => {
          // Autoplay refused (a data saver, say): the poster stays, and the button plays it.
        });
      }
    };
    const image = posterRef.current;
    if (!image || (image.complete && image.naturalWidth > 0)) {
      start();
      return;
    }
    image.addEventListener("load", start, { once: true });
    image.addEventListener("error", start, { once: true });
    return () => {
      image.removeEventListener("load", start);
      image.removeEventListener("error", start);
    };
  }, []);

  // A frame-accurate readout, from the video's own frames.
  useEffect(() => {
    const element = videoRef.current;
    if (!element) return;
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let handle = 0;
    let stopped = false;
    const dot = dotRef.current;

    const currentGlobalFrame = (mediaTime: number) => frameAt(mediaTime, video.fps, video.frames);

    // Older Safari and Firefox have no per-frame callback; `timeupdate` is coarser but correct.
    const supported =
      typeof (element as { requestVideoFrameCallback?: unknown }).requestVideoFrameCallback === "function";
    if (supported) {
      setPerFrame(true);
      const onFrame = (now: number, metadata: VideoFrameCallbackMetadata) => {
        show(currentGlobalFrame(metadata.mediaTime), element.paused);
        // On the film's frames, not a CSS animation of its own: that would redraw the film at the display's rate.
        if (dot && !still && !element.paused) dot.style.opacity = pulse(now).toFixed(3);
        if (!stopped) handle = element.requestVideoFrameCallback(onFrame);
      };
      handle = element.requestVideoFrameCallback(onFrame);
      return () => {
        stopped = true;
        element.cancelVideoFrameCallback(handle);
      };
    }
    const onTime = () => show(currentGlobalFrame(element.currentTime));
    element.addEventListener("timeupdate", onTime);
    return () => element.removeEventListener("timeupdate", onTime);
  }, [show]);

  // Off screen the film pauses, so it decodes and redraws nothing while the reader is elsewhere; it plays on
  // when it comes back, but only if it was playing when it left (a reader's own pause stays).
  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(([entry]) => {
      const active = videoRef.current;
      if (!entry || !active) return;
      if (!entry.isIntersecting) {
        if (!active.paused) {
          pausedOffScreen.current = true;
          active.pause();
        }
      } else if (pausedOffScreen.current) {
        pausedOffScreen.current = false;
        active.play().catch(() => {});
      }
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  const toggle = () => {
    const element = videoRef.current;
    if (!element) return;
    pausedOffScreen.current = false;
    if (element.paused) element.play().catch(() => {});
    else element.pause();
  };

  /** Show frame `target`, playing or not: the seek lands in the middle of the frame, so rounding never shows a neighbour. */
  const seek = (target: number) => {
    const clamped = Math.min(last, Math.max(0, target));
    show(clamped, true);
    const element = videoRef.current;
    if (!element) return;
    // A seek before the film has started (it waits for its poster) loads it: a time set on a video that has
    // loaded nothing only takes effect once something loads it.
    if (element.preload === "none" && element.readyState === HTMLMediaElement.HAVE_NOTHING) {
      element.preload = "metadata";
      element.load();
    }
    element.currentTime = (clamped + 0.5) / video.fps;
  };

  return (
    <figure className={cn("relative", className)}>
      {/* A container, so the scale bar is sized in the frame's width (cqw), not its padded row's. */}
      <div
        ref={containerRef}
        className={cn("@container relative w-full overflow-hidden bg-black lg:rounded-[2rem]", isSquare && "aspect-square")}
        style={isSquare ? undefined : { aspectRatio: `${video.width} / ${video.height}` }}
      >
        {/* The poster, under the film until its first frame paints over it. A phone takes the 1080² AVIF, as it takes
            the small encode; anything else (or a browser without AVIF) takes the 2160² JPEG. A picture, because a
            video's poster attribute names one file for every screen: a phone on a slow network fetched the 3 MB JPEG
            for more than 16 s, and its largest paint waited 10 s for the film's own first frame. */}
        <picture>
          {poster.small ? <source media={PHONE} type="image/avif" srcSet={poster.small.src} /> : null}
          {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
          <img
            ref={posterRef}
            src={poster.src}
            alt=""
            width={video.width}
            height={video.height}
            fetchPriority="high"
            className={cn("absolute inset-0 h-full w-full object-cover", feather && "specimen-feather")}
          />
        </picture>
        <video
          ref={videoRef}
          className={cn("relative h-full w-full object-cover", feather && "specimen-feather")}
          width={video.width}
          height={video.height}
          muted
          loop
          playsInline
          // "metadata" once the poster has loaded (the effect above); React leaves the attribute alone after that.
          preload="none"
          onPlay={() => setPlaying(true)}
          onPause={() => {
            setPlaying(false);
            syncSeek();
            if (dotRef.current) dotRef.current.style.opacity = "";
          }}
          aria-label={label}
        >
          {/* The browser takes the first source whose media matches, once, when the film loads. */}
          {video.small ? <source src={video.small.src} type="video/mp4" media={PHONE} /> : null}
          <source src={video.src} type="video/mp4" />
        </video>
        <Reticle inset={14} size={18} />

        {/* What this is, on its face. */}
        <div className="pointer-events-none absolute top-4 left-4 z-10 flex items-center gap-2 rounded-full border border-white/10 bg-black/60 px-3 py-1.5 backdrop-blur-md sm:top-5 sm:left-5">
          <span
            ref={dotRef}
            aria-hidden="true"
            className={cn("h-1.5 w-1.5 rounded-full bg-green-400", playing && !perFrame && "pulse-dot")}
          />
          <span className="font-mono text-[10px] font-medium tracking-[0.16em] text-green-200 uppercase">
            Rendered by flife · not drawn
          </span>
        </div>
        <button
          type="button"
          onClick={toggle}
          className="absolute top-3 right-3 z-20 flex h-11 w-11 items-center justify-center rounded-xl border border-white/15 bg-black/60 text-white backdrop-blur-md transition-colors hover:border-green-400/60 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none sm:top-4 sm:right-4"
          aria-label={playing ? "Pause the film" : "Play the film"}
        >
          {playing ? <Pause className="h-4 w-4" /> : <Play className="h-4 w-4" />}
        </button>

        {/* The instrument's readout: the frame's own record, a scale bar, and the shots on the tick axis. */}
        <div className="absolute inset-x-0 bottom-0 z-10 bg-gradient-to-t from-black via-black/70 to-transparent px-4 pt-28 pb-3 sm:px-6 sm:pb-4">
          <div className="pointer-events-none flex items-end justify-between gap-4">
            <dl className="grid flex-1 grid-cols-4 gap-2 font-mono text-white sm:gap-4">
              <Readout label="Tick" value={FIRST.tickText} valueRef={tickRef} />
              <Readout label="Sim time" value={FIRST.simulated} valueRef={simulatedRef} />
              <Readout label="Alive" value={FIRST.live} valueRef={liveRef} />
              <Readout label="View" value={FIRST.view} valueRef={viewRef} />
            </dl>
          </div>
          <div className="pointer-events-none mt-3 flex items-center gap-3" aria-hidden="true">
            <span ref={barRef} className="relative h-2 shrink-0 border-x border-b border-white/70" style={{ width: FIRST.barWidth }} />
            <span ref={barTextRef} className="font-mono text-[10px] text-slate-300">
              {FIRST.barText}
            </span>
            <span className="ml-auto hidden font-mono text-[10px] text-slate-400 sm:inline">
              {grouped(world.founders)} founders ·{" "}
              {world.grid[0] === world.grid[1] ? `${world.grid[0]}²` : `${world.grid[0]}×${world.grid[1]}`} field · colour = {colour}
            </span>
          </div>
          <div className="relative mt-3 h-8 rounded-md has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-green-400/70">
            <div className="absolute inset-x-0 top-1 h-1.5 overflow-hidden rounded-full bg-white/[0.08]">
              {/* Elapsed first, the shots over it, so a hold stays readable once played. */}
              <span
                ref={elapsedRef}
                aria-hidden="true"
                className="absolute inset-0 origin-left bg-green-400/25"
                style={{ transform: `scaleX(${FIRST.elapsed})` }}
              />
              {HOLDS.map((shot, index) => (
                <span
                  key={shot.from}
                  ref={(element) => {
                    holdRefs.current[index] = element;
                  }}
                  aria-hidden="true"
                  className={FIRST.tick >= shot.from && FIRST.tick <= shot.to ? HOLD_NOW : HOLD_THEN}
                  style={{
                    left: position(shot.from),
                    width: `calc(${position(shot.to)} - ${position(shot.from)} - 2px)`,
                    marginLeft: "1px",
                  }}
                />
              ))}
            </div>
            {/* As wide as the axis, so its shift in percent of itself is the film's share of the axis. */}
            <span
              ref={markerRef}
              aria-hidden="true"
              className="pointer-events-none absolute inset-x-0 top-0 h-3.5"
              style={{ transform: `translateX(${FIRST.elapsed * 100}%)` }}
            >
              <span className="absolute top-0 left-0 h-3.5 w-[3px] -translate-x-1/2 rounded-full bg-green-200 shadow-[0_0_10px_rgba(134,239,172,0.9)]" />
            </span>
            {HOLDS.map((shot) => (
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
              ref={seekRef}
              type="range"
              min={0}
              max={last}
              step={1}
              defaultValue={0}
              onFocus={syncSeek}
              onPointerDown={syncSeek}
              onKeyDown={syncSeek}
              onChange={(event) => seek(Number(event.target.value))}
              aria-label="Seek the film by tick"
              aria-valuetext={FIRST.described}
              className="absolute inset-x-0 -top-2 h-8 w-full cursor-pointer opacity-0"
            />
          </div>
        </div>
      </div>
    </figure>
  );
}

/** `valueRef`'s text is rewritten every frame: its box is sized without it (one line), so a new value lays out nothing else. */
function Readout({ label, value, valueRef }: { label: string; value: string; valueRef: Ref<HTMLElement> }) {
  return (
    <div className="min-w-0">
      <dt className="truncate text-[9px] font-semibold tracking-[0.2em] text-green-200 uppercase [text-shadow:0_1px_6px_rgba(0,0,0,0.9)]">{label}</dt>
      <dd ref={valueRef} className="h-[20.8px] truncate text-[13px] tabular-nums contain-strict sm:h-6 sm:text-base">
        {value}
      </dd>
    </div>
  );
}
