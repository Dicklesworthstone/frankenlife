// Pure option parsing, validation and ffmpeg argument builders for
// scripts/media.ts and scripts/controls.ts (tested by media-options.test.ts).
//
// Keeps every V2 slug, colour mapping, field mapping and INV-14 rule aligned
// with crates/flife-core/src/v2.rs, crates/flife-cli/src/main.rs and
// crates/flife-manim/CONTRACT.md.

import { relative, resolve } from "node:path";

import type { CameraKey } from "./director";

/** Cloudflare Workers serves a static asset of at most 25 MiB. */
export const ASSET_LIMIT = 25 * 1024 * 1024;
/** Default target ceiling per Cloudflare-safe segment (22 MiB, leaving 3 MiB safety margin under ASSET_LIMIT). */
export const DEFAULT_SEGMENT_MAX_MIB = 22;
/** Hard maximum allowed for `--segment-max-mib` (strictly below Cloudflare's 25 MiB limit). */
export const MAX_SEGMENT_MAX_MIB = 24.5;

/** Every colour mapping `flife render --color` accepts (ColorMapping::ALL). */
export const MEDIA_COLORS = [
  "lineage",
  "lineage-energy",
  "affinity",
  "energy",
  "generation",
  "motion",
  "topology",
  "cell-state",
  "polarity",
] as const;
export type MediaColor = (typeof MEDIA_COLORS)[number];

/** Named field mappings `flife render --field-mapping` accepts, plus `none` (`--no-field`). */
export const MEDIA_FIELDS = ["energy", "composition", "trail-carrier", "vascular-conductance", "none"] as const;

/** Looks `flife render --look` accepts. */
export const MEDIA_LOOKS = ["glow", "raw", "sphere"] as const;
export type MediaLook = (typeof MEDIA_LOOKS)[number];

/** Camera modes for scripts/media.ts. */
export const CAMERA_MODES = ["auto", "none"] as const;
export type CameraMode = (typeof CAMERA_MODES)[number];

/**
 * The 21 canonical kebab-case `field-ecology-v2` feature slugs
 * (crates/flife-core/src/v2.rs, V2Feature::ALL).
 */
export const V2_FEATURE_SLUGS = [
  "oases",
  "senescence",
  "surplus-emission",
  "excitable-relay",
  "wake-offset",
  "so2-motor",
  "so2-budding",
  "hippo-crowding",
  "pcp-polarity",
  "emt-gate",
  "tight-junctions",
  "lysis-recycling",
  "symplectic-waste",
  "epigenetic-recurrence",
  "notch-delta",
  "trophallaxis",
  "cadherin-4d",
  "velocity-alignment",
  "vascular-field",
  "dual-carrier-trails",
  "v2-visual-mappings",
] as const;
export type V2FeatureSlug = (typeof V2_FEATURE_SLUGS)[number];

/** Convenience aliases accepted by `--disable` and `--enable` (V2Feature::expand_slug_or_alias). */
export const V2_FEATURE_ALIASES = ["all-v2", "bundle-a", "bundle-b"] as const;

const FEATURE_TOKENS = new Set<string>([...V2_FEATURE_SLUGS, ...V2_FEATURE_ALIASES]);
const COMPONENT_FIELD = /^component-(?:0|[1-9]|[12]\d|3[01])$/;

/** Validate a comma-separated `--disable` or `--enable` token list against V2Feature slugs and aliases. */
export function validateFeatureTokens(raw: string, flag: "--disable" | "--enable"): string[] {
  const parts = raw.split(",");
  if (parts.length === 0 || parts.some((part) => part.trim() === "")) {
    throw new Error(`${flag} expects a non-empty comma-separated list of v2 feature slugs or aliases, got "${raw}"`);
  }
  for (const part of parts) {
    if (!FEATURE_TOKENS.has(part)) {
      throw new Error(`${flag}: unknown v2 feature or alias "${part}"`);
    }
  }
  return parts;
}

/** Whether `value` is a valid `--field` choice (`none`, named mapping, or `component-0`..`component-31`). */
export function isValidFieldChoice(value: string): boolean {
  return (MEDIA_FIELDS as readonly string[]).includes(value) || COMPONENT_FIELD.test(value);
}

export type SegmentMode = "auto" | number;

export type MediaOptions = {
  flife: string;
  runDir: string | null;
  config: string | null;
  work: string;
  ticks: number;
  every: number;
  width: number;
  height: number;
  fps: number;
  crf: number;
  segments: SegmentMode;
  segmentMaxBytes: number;
  masterOut: string | null;
  masterCrf: number;
  cameraMode: CameraMode;
  field: string;
  look: MediaLook;
  color: MediaColor | null;
  zooms: number[] | null;
  cpuWorkers: number | null;
  previewFrom: string | null;
  /** Exact left-to-right CLI feature override flags passed to `flife` (`--disable`, `--enable`, `--v1-compat`). */
  featureFlags: string[];
};

const MEDIA_VALUE_FLAGS = new Set([
  "flife",
  "run-dir",
  "config",
  "work",
  "ticks",
  "every",
  "size",
  "width",
  "height",
  "fps",
  "crf",
  "segments",
  "segment-max-mib",
  "master-out",
  "master-crf",
  "camera",
  "field",
  "look",
  "color",
  "zooms",
  "cpu-workers",
  "preview-from",
  "disable",
  "enable",
]);

function parseIntInRange(value: string, flag: string, min: number, max?: number): number {
  if (!/^\d+$/.test(value)) {
    throw new Error(`--${flag} must be an integer of at least ${min}`);
  }
  const parsed = Number(value);
  if (parsed < min || (max !== undefined && parsed > max)) {
    throw new Error(
      max !== undefined
        ? `--${flag} must be an integer between ${min} and ${max}`
        : `--${flag} must be an integer of at least ${min}`,
    );
  }
  return parsed;
}

/** Check whether `candidate` resolves inside `parent` (to keep archival masters out of public/media/). */
export function isInsideDirectory(candidate: string, parent: string): boolean {
  const rel = relative(resolve(parent), resolve(candidate));
  return rel === "" || (!rel.startsWith("..") && !rel.startsWith("/") && rel !== "..");
}

/**
 * Parse and validate `scripts/media.ts` arguments.
 *
 * Supports square (`--size 2160`) and arbitrary even dimensions (`--width 3840 --height 2160`),
 * arbitrary positive `--fps` (e.g. `60`), `V2-16` colour/field mappings, optional archival
 * `--master-out` outside `public/media/`, and founding-only `--disable`/`--enable`/`--v1-compat`
 * (refused with `--run-dir` per INV-14).
 */
export function parseMediaArgs(argv: readonly string[], publicMediaDir?: string): MediaOptions {
  const single = new Map<string, string>();
  const featureFlags: string[] = [];

  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (!token?.startsWith("--")) {
      throw new Error(`expected --option arguments, got ${argv.join(" ")}`);
    }
    const key = token.slice(2);
    if (key === "v1-compat") {
      featureFlags.push("--v1-compat");
      continue;
    }
    if (!MEDIA_VALUE_FLAGS.has(key)) {
      throw new Error(`unknown option --${key}`);
    }
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--")) {
      throw new Error(`--${key} requires a value`);
    }
    i += 1;
    if (key === "disable" || key === "enable") {
      const flag = `--${key}` as "--disable" | "--enable";
      validateFeatureTokens(value, flag);
      featureFlags.push(flag, value);
    } else {
      single.set(key, value);
    }
  }

  const required = (key: string) => {
    const value = single.get(key);
    if (!value) throw new Error(`--${key} is required`);
    return resolve(value);
  };

  const flife = required("flife");
  const work = required("work");
  const runDir = single.has("run-dir") ? resolve(single.get("run-dir") as string) : null;
  const config = single.has("config") ? resolve(single.get("config") as string) : null;

  if ((runDir === null) === (config === null)) {
    throw new Error("pass exactly one of --run-dir DIR or --config PATH");
  }
  if (runDir !== null && featureFlags.length > 0) {
    throw new Error(
      "v2 feature toggles (--disable/--enable/--v1-compat) are refused with --run-dir (INV-14): " +
        "features are baked into the checkpoint; pass --config PATH or bake them when creating the run",
    );
  }

  const ticks = parseIntInRange(single.get("ticks") ?? "9600", "ticks", 0);
  const every = parseIntInRange(single.get("every") ?? "32", "every", 1);
  const size = parseIntInRange(single.get("size") ?? "1080", "size", 2);
  const width = single.has("width") ? parseIntInRange(single.get("width") as string, "width", 2) : size;
  const height = single.has("height") ? parseIntInRange(single.get("height") as string, "height", 2) : size;

  // 4:2:0 yuv420p halves each dimension.
  if (width % 2 !== 0) throw new Error("--width (or --size) must be even");
  if (height % 2 !== 0) throw new Error("--height (or --size) must be even");

  const fps = parseIntInRange(single.get("fps") ?? "30", "fps", 1);
  const crf = parseIntInRange(single.get("crf") ?? "24", "crf", 0, 51);
  const segmentsRaw = single.get("segments") ?? "auto";
  const segments: SegmentMode =
    segmentsRaw === "auto" ? "auto" : parseIntInRange(segmentsRaw, "segments", 1);
  const segmentMaxMiB = Number(single.get("segment-max-mib") ?? String(DEFAULT_SEGMENT_MAX_MIB));
  if (!Number.isFinite(segmentMaxMiB) || segmentMaxMiB <= 0 || segmentMaxMiB > MAX_SEGMENT_MAX_MIB) {
    throw new Error(`--segment-max-mib must be a number in (0, ${MAX_SEGMENT_MAX_MIB}]`);
  }
  const segmentMaxBytes = Math.floor(segmentMaxMiB * 1024 * 1024);
  const masterOut = single.has("master-out") ? resolve(single.get("master-out") as string) : null;
  if (single.has("master-crf") && masterOut === null) {
    throw new Error("--master-crf requires --master-out PATH");
  }
  const masterCrf = parseIntInRange(single.get("master-crf") ?? "16", "master-crf", 0, 51);
  if (masterOut !== null && publicMediaDir !== undefined && isInsideDirectory(masterOut, publicMediaDir)) {
    throw new Error("--master-out must lie outside public/media/ (Cloudflare's 25 MiB web asset cap applies there)");
  }

  const cpuWorkers = single.has("cpu-workers")
    ? parseIntInRange(single.get("cpu-workers") as string, "cpu-workers", 1)
    : null;
  const previewFrom = single.has("preview-from") ? resolve(single.get("preview-from") as string) : null;

  const cameraModeRaw = single.get("camera") ?? "auto";
  if (!(CAMERA_MODES as readonly string[]).includes(cameraModeRaw)) {
    throw new Error(`--camera must be one of ${CAMERA_MODES.join(", ")}`);
  }
  const cameraMode = cameraModeRaw as CameraMode;

  const field = single.get("field") ?? "energy";
  if (!isValidFieldChoice(field)) {
    throw new Error(`--field must be one of ${MEDIA_FIELDS.join(", ")}, or component-0..31`);
  }

  const lookRaw = single.get("look") ?? "glow";
  if (!(MEDIA_LOOKS as readonly string[]).includes(lookRaw)) {
    throw new Error(`--look must be one of ${MEDIA_LOOKS.join(", ")}`);
  }
  const look = lookRaw as MediaLook;

  let color: MediaColor | null = null;
  if (single.has("color")) {
    const colorRaw = single.get("color") as string;
    if (!(MEDIA_COLORS as readonly string[]).includes(colorRaw)) {
      throw new Error(`--color must be one of ${MEDIA_COLORS.join(", ")}`);
    }
    color = colorRaw as MediaColor;
  }

  const zooms = single.has("zooms")
    ? (single.get("zooms") as string).split(",").map((text) => {
        const zoom = Number(text);
        if (!Number.isFinite(zoom) || zoom < 1) {
          throw new Error(`--zooms must list zooms of at least 1, got ${text}`);
        }
        return zoom;
      })
    : null;

  return {
    flife,
    runDir,
    config,
    work,
    ticks,
    every,
    width,
    height,
    fps,
    crf,
    segments,
    segmentMaxBytes,
    masterOut,
    masterCrf,
    cameraMode,
    field,
    look,
    color,
    zooms,
    cpuWorkers,
    previewFrom,
    featureFlags,
  };
}

/**
 * Compute the ffmpeg maxrate ceiling (kbps) that keeps a film of `frames` at `fps`
 * within `limitBytes` (Cloudflare's 25 MiB static asset limit), with 10% headroom
 * for the MP4 container and VBV overshoot.
 */
export function cloudflareCeilingKbps(frames: number, fps: number, limitBytes = ASSET_LIMIT): number {
  if (!Number.isFinite(frames) || frames <= 0) throw new Error("frames must be a positive number");
  if (!Number.isFinite(fps) || fps <= 0) throw new Error("fps must be a positive number");
  const seconds = frames / fps;
  return Math.max(100, Math.floor((0.9 * limitBytes * 8) / seconds / 1000));
}

const BT709_VF =
  "scale=out_color_matrix=bt709:out_range=tv:out_chroma_loc=left,format=yuv420p," +
  "setparams=range=tv:color_primaries=bt709:color_trc=iec61966-2-1:colorspace=bt709";

/** ffmpeg arguments for the Cloudflare-hosted web MP4 (capped at <= 25 MiB). */
export function buildWebEncodeArgs(crf: number, frames: number, fps: number, limitBytes = ASSET_LIMIT): string[] {
  const ceiling = `${cloudflareCeilingKbps(frames, fps, limitBytes)}k`;
  return [
    "-vf",
    BT709_VF,
    "-c:v",
    "libx264",
    "-preset",
    "slow",
    "-crf",
    String(crf),
    "-maxrate",
    ceiling,
    "-bufsize",
    ceiling,
    "-pix_fmt",
    "yuv420p",
    "-chroma_sample_location",
    "left",
    "-movflags",
    "+faststart",
    "-an",
  ];
}

/** ffmpeg arguments for an unconstrained high-quality archival master MP4 (e.g. 4K60 outside public/media/). */
export function buildMasterEncodeArgs(masterCrf: number): string[] {
  return [
    "-vf",
    BT709_VF,
    "-c:v",
    "libx264",
    "-preset",
    "slow",
    "-crf",
    String(masterCrf),
    "-pix_fmt",
    "yuv420p",
    "-chroma_sample_location",
    "left",
    "-movflags",
    "+faststart",
    "-an",
  ];
}

/**
 * ffmpeg arguments for a closed-GOP Cloudflare-safe segment MP4 (e.g. one shot of a 4K60 film).
 * Uses `-g` = 1 s closed GOP for instant clean seeking and zero B-frame bleed across segment
 * boundaries; omits `-maxrate` unless a last-resort fallback cap is supplied.
 */
export function buildSegmentEncodeArgs(crf: number, fps: number, fallbackMaxrateKbps?: number): string[] {
  const gop = String(Math.max(1, fps));
  return [
    "-vf",
    BT709_VF,
    "-c:v",
    "libx264",
    "-preset",
    "slow",
    "-crf",
    String(crf),
    "-g",
    gop,
    "-keyint_min",
    gop,
    "-sc_threshold",
    "0",
    ...(fallbackMaxrateKbps !== undefined
      ? ["-maxrate", `${fallbackMaxrateKbps}k`, "-bufsize", `${fallbackMaxrateKbps}k`]
      : []),
    "-pix_fmt",
    "yuv420p",
    "-chroma_sample_location",
    "left",
    "-movflags",
    "+faststart",
    "-an",
  ];
}

/** One contiguous frame slice `[startFrame, startFrame + frames)` of a film. */
export type SegmentRange = { startFrame: number; frames: number };

/**
 * Map interior camera shot boundaries (`cameraKeys`) onto frame indices in `(0, ticks.length)`.
 * Returns unique sorted frame indices where a camera hold starts or ends (natural cut/inflection
 * points where a segment transition is visually imperceptible).
 */
export function findCameraCutFrames(ticks: readonly number[], cameraKeys?: readonly CameraKey[] | null): number[] {
  if (!cameraKeys || cameraKeys.length < 3 || ticks.length < 2) return [];
  const cuts = new Set<number>();
  for (let i = 1; i < cameraKeys.length - 1; i += 1) {
    const prev = cameraKeys[i - 1]!;
    const curr = cameraKeys[i]!;
    const next = cameraKeys[i + 1]!;
    if (prev.zoom === curr.zoom && curr.zoom === next.zoom) continue;
    // Find the frame whose tick is nearest `curr.tick`.
    let best = 0;
    let bestDiff = Number.POSITIVE_INFINITY;
    for (let f = 0; f < ticks.length; f += 1) {
      const diff = Math.abs((ticks[f] ?? 0) - curr.tick);
      if (diff < bestDiff) {
        bestDiff = diff;
        best = f;
      }
    }
    if (best > 0 && best < ticks.length) cuts.add(best);
  }
  return [...cuts].sort((a, b) => a - b);
}

/**
 * Partition `totalFrames` into `targetCount` contiguous non-empty segments, snapping each boundary
 * to the nearest camera shot cut within ±35% of the average segment length when available, or
 * splitting evenly when no camera cuts exist.
 */
export function planSegmentRanges(
  totalFrames: number,
  ticks: readonly number[],
  cameraKeys: readonly CameraKey[] | null | undefined,
  targetCount: number,
): SegmentRange[] {
  if (totalFrames <= 0) throw new Error("totalFrames must be positive");
  const count = Math.max(1, Math.min(Math.floor(targetCount), totalFrames));
  if (count === 1) return [{ startFrame: 0, frames: totalFrames }];

  const candidates = findCameraCutFrames(ticks, cameraKeys);
  const idealStep = totalFrames / count;
  const snapRadius = idealStep * 0.35;
  const boundaries: number[] = [0];

  for (let k = 1; k < count; k += 1) {
    const ideal = Math.round(k * idealStep);
    const minAllowed = (boundaries[boundaries.length - 1] ?? 0) + 1;
    const maxAllowed = totalFrames - (count - k);
    let chosen = Math.min(maxAllowed, Math.max(minAllowed, ideal));

    let bestDist = Number.POSITIVE_INFINITY;
    for (const candidate of candidates) {
      if (candidate < minAllowed || candidate > maxAllowed) continue;
      const dist = Math.abs(candidate - ideal);
      if (dist <= snapRadius && dist < bestDist) {
        bestDist = dist;
        chosen = candidate;
      }
    }
    boundaries.push(chosen);
  }
  boundaries.push(totalFrames);

  const ranges: SegmentRange[] = [];
  for (let i = 0; i < boundaries.length - 1; i += 1) {
    const startFrame = boundaries[i]!;
    const endFrame = boundaries[i + 1]!;
    ranges.push({ startFrame, frames: endFrame - startFrame });
  }
  return ranges;
}

/**
 * Subdivide one oversized segment into two non-empty halves, preferring an interior camera cut
 * inside the middle 50% of the range and falling back to the exact midpoint.
 */
export function splitSegmentRange(
  range: SegmentRange,
  ticks: readonly number[],
  cameraKeys?: readonly CameraKey[] | null,
): [SegmentRange, SegmentRange] {
  if (range.frames < 2) {
    throw new Error(`cannot subdivide a ${range.frames}-frame segment starting at ${range.startFrame}`);
  }
  const mid = range.startFrame + Math.floor(range.frames / 2);
  const low = range.startFrame + Math.max(1, Math.floor(range.frames * 0.25));
  const high = range.startFrame + Math.min(range.frames - 1, Math.ceil(range.frames * 0.75));
  const candidates = findCameraCutFrames(ticks, cameraKeys);
  let cut = mid;
  let bestDist = Number.POSITIVE_INFINITY;
  for (const candidate of candidates) {
    if (candidate < low || candidate > high) continue;
    const dist = Math.abs(candidate - mid);
    if (dist < bestDist) {
      bestDist = dist;
      cut = candidate;
    }
  }
  return [
    { startFrame: range.startFrame, frames: cut - range.startFrame },
    { startFrame: cut, frames: range.startFrame + range.frames - cut },
  ];
}

/**
 * ffmpeg crop filter for the 1200 × 630 social card: fills 1200 × 630 from any aspect ratio
 * (1:1 1080/2160, 16:9 3840×2160, or wider) before taking the centred crop.
 */
export const SOCIAL_CROP_VF = "scale=1200:630:force_original_aspect_ratio=increase,crop=1200:630";

export type ControlsOptions = {
  flife: string;
  work: string;
  size: number;
  cpuWorkers: number | null;
  parallel: number;
};

const CONTROLS_VALUE_FLAGS = new Set(["flife", "work", "size", "cpu-workers", "parallel"]);

/** Parse and validate `scripts/controls.ts` arguments (adds `--size`, default 1080, e.g. 2160 for 4K stills). */
export function parseControlsArgs(argv: readonly string[]): ControlsOptions {
  const single = new Map<string, string>();
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key?.startsWith("--") || value === undefined || value.startsWith("--")) {
      throw new Error(`expected --name value pairs, got ${argv.join(" ")}`);
    }
    const name = key.slice(2);
    if (!CONTROLS_VALUE_FLAGS.has(name)) {
      throw new Error(`unknown option --${name}`);
    }
    single.set(name, value);
  }
  const flifeRaw = single.get("flife");
  if (!flifeRaw) throw new Error("--flife is required");
  const workRaw = single.get("work");
  if (!workRaw) throw new Error("--work is required");

  const size = parseIntInRange(single.get("size") ?? "1080", "size", 2);
  if (size % 2 !== 0) throw new Error("--size must be even");
  const cpuWorkers = single.has("cpu-workers")
    ? parseIntInRange(single.get("cpu-workers") as string, "cpu-workers", 1)
    : null;
  const parallel = parseIntInRange(single.get("parallel") ?? "2", "parallel", 1);

  return {
    flife: resolve(flifeRaw),
    work: resolve(workRaw),
    size,
    cpuWorkers,
    parallel,
  };
}
