// Pure option parsing, validation and ffmpeg argument builders for
// scripts/media.ts and scripts/controls.ts (tested by media-options.test.ts).
//
// Keeps every V2 slug, colour mapping, field mapping and INV-14 rule aligned
// with crates/flife-core/src/v2.rs, crates/flife-cli/src/main.rs and
// crates/flife-manim/CONTRACT.md.

import { relative, resolve } from "node:path";

/**
 * Cloudflare Workers serves a static asset of at most 25 MiB: the posters, cards and stills in public/media/.
 * The films are not static assets; they are served from R2 (worker.ts), so no film is split or capped to fit.
 */
export const ASSET_LIMIT = 25 * 1024 * 1024;

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

/**
 * Camera modes for scripts/media.ts: `auto`, the director's path from a
 * preview; `none`, the whole torus; `fixed`, the torus centre held at the one
 * zoom `--zooms` gives.
 */
export const CAMERA_MODES = ["auto", "none", "fixed"] as const;
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
/** A film id: it names lib/films/<id>.json and prefixes the film's files in public/media/. */
export const FILM_ID = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

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

export type MediaOptions = {
  /** The film's id: its manifest is lib/films/<id>.json and its files are named <id>-…; default `film`. */
  film: string;
  /** The commit `--flife` was built from, when not HEAD (scripts/source-revision.ts); null for HEAD. */
  revision: string | null;
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
  /** The phone encode's size, [width, height], from `--small HEIGHT` (the film's aspect kept); null for none. */
  small: [number, number] | null;
  /** The phone encode's CRF (`--small-crf`, default the film's): fewer pixels need a lower CRF to look the same. */
  smallCrf: number;
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
  "film",
  "revision",
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
  "small",
  "small-crf",
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

  const film = single.get("film") ?? "film";
  if (!FILM_ID.test(film)) {
    throw new Error(`--film must be a lower-case id of letters, digits and hyphens (it names lib/films/<id>.json), got "${film}"`);
  }
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
  // `--small 0` (the default) makes no phone encode.
  const smallHeight = parseIntInRange(single.get("small") ?? "0", "small", 0);
  let small: [number, number] | null = null;
  if (smallHeight > 0) {
    if (smallHeight % 2 !== 0 || smallHeight >= height) {
      throw new Error(`--small must be an even height below the film's ${height}, or 0 for no phone encode`);
    }
    // The film's aspect, its width rounded to an even number for 4:2:0.
    small = [Math.max(2, Math.round((width * smallHeight) / height / 2) * 2), smallHeight];
  }
  if (single.has("small-crf") && small === null) throw new Error("--small-crf requires --small HEIGHT");
  const smallCrf = single.has("small-crf") ? parseIntInRange(single.get("small-crf") as string, "small-crf", 0, 51) : crf;
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
  if (cameraMode === "fixed" && zooms?.length !== 1) {
    throw new Error("--camera fixed needs --zooms with exactly one zoom");
  }

  return {
    film,
    revision: single.get("revision") ?? null,
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
    small,
    smallCrf,
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

const BT709_VF =
  "scale=out_color_matrix=bt709:out_range=tv:out_chroma_loc=left,format=yuv420p," +
  "setparams=range=tv:color_primaries=bt709:color_trc=iec61966-2-1:colorspace=bt709";

/**
 * ffmpeg arguments for a film's web MP4, served from R2: quality set by the CRF alone, with no
 * bitrate cap. A keyframe every second (closed GOP, no scene-cut keyframes) keeps a seek anywhere in
 * the film to at most a second of decoding. With `size`, the frames are first scaled to it (Lanczos,
 * in the same scale filter as the colour conversion): the phone encode. x264 clamps `-keyint_min` to
 * half the GOP plus one (31 at 60 fps), so it changes nothing beside `-sc_threshold 0`; it stays so
 * that a re-encode of the published films reproduces their bytes (x264 writes its settings into the stream).
 */
export function buildWebEncodeArgs(crf: number, fps: number, size?: readonly [number, number]): string[] {
  if (!Number.isFinite(fps) || fps <= 0) throw new Error("fps must be a positive number");
  const gop = String(Math.max(1, Math.round(fps)));
  return [
    "-vf",
    size ? BT709_VF.replace("scale=", `scale=w=${size[0]}:h=${size[1]}:flags=lanczos:`) : BT709_VF,
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
    "-pix_fmt",
    "yuv420p",
    "-chroma_sample_location",
    "left",
    "-movflags",
    "+faststart",
    "-an",
  ];
}

/** ffmpeg arguments for the archival master MP4 (a lower CRF than the web's), written outside the site. */
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
 * The poster AVIFs' AV1 CRF. For the confluent film, against its 3.26 MB JPEG at 2160²: 668 KB at 2160², 253 KB at
 * the phone's 1080².
 */
export const POSTER_AVIF_CRF = 42;

/**
 * ffmpeg arguments that make an AVIF of a film's poster from its poster JPEG (`ffmpeg -i poster.jpg ARGS out.avif`):
 * at `size` (the film's own, or the phone encode's), one frame. The JPEG's samples keep their own colour, BT.601 at
 * full range, and are tagged so: drawn by Chrome, that is closer to the JPEG (mean error 3.1, 2.3 and 3.8 levels in
 * R, G and B, near-lossless at 1080²) than tagging them BT.709 (4.6, 2.1, 5.4) or a 4:2:0 JPEG (4.4, 2.8, 5.6). At
 * this CRF the error is 8.3, 5.7 and 9.7 levels, a little smoothing of single bodies the film soon replaces.
 */
export function buildPosterAvifArgs(size: readonly [number, number]): string[] {
  return [
    "-vf",
    `scale=w=${size[0]}:h=${size[1]}:flags=lanczos:in_range=full:out_range=full,format=yuv420p`,
    "-frames:v",
    "1",
    "-c:v",
    "libsvtav1",
    "-preset",
    "4",
    "-crf",
    String(POSTER_AVIF_CRF),
    "-colorspace",
    "bt470bg",
    "-color_range",
    "pc",
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
