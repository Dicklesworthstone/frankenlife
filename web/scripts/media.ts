// Render one of the site's films from a FrankenLife run directory (or
// configuration) and write its manifest (lib/films/<id>.json).
//
//   bun scripts/media.ts [--film ID] --flife PATH (--run-dir DIR | --config PATH) --work DIR
//     [--ticks 9600] [--every 32] [--size 1080 | --width 3840 --height 2160]
//     [--fps 30|60] [--crf 24] [--small HEIGHT [--small-crf CRF]]
//     [--master-out PATH] [--master-crf 16]
//     [--camera auto|none|fixed]
//     [--field energy|composition|component-C|trail-carrier|vascular-conductance|none]
//     [--look glow|raw|sphere]
//     [--color lineage|lineage-energy|affinity|energy|generation|motion|topology|cell-state|polarity]
//     [--zooms 2.5,4,6] [--cpu-workers N] [--preview-from WORK]
//     [--disable SLUGS] [--enable SLUGS] [--v1-compat]
//
// - With `--camera auto` (the default) the run is first rendered small, as
//   raw dots without the field (`--work`/preview), and scripts/director.ts
//   reads that preview's pixels to choose where each shot looks; the film is
//   then rendered with `flife render --camera` along that path. The camera
//   is presentation only: both renders reach the same digest, which the
//   script checks. `--zooms` gives the shots' zooms on this torus, in turn;
//   without it they span 51–102 world units (below). `--preview-from` reads
//   another --work's finished preview of the same run and ticks instead of
//   rendering it again (to try other zooms); the digest check still holds.
//
// - `flife render --run-dir` continues the run from its newest checkpoint
//   that restores, as `flife resume` would, and writes no checkpoint, journal
//   record or catalog row to the run (FrankenSQLite may rewrite the catalog's
//   log files when it opens it; flife-cli's CONTRACT.md). `--disable`,
//   `--enable` and `--v1-compat` are founding-only (INV-14) and are refused
//   with `--run-dir`; use `--config PATH` to render a founded V2 world with
//   CLI overrides (or bake them when creating `--run-dir`).
//
// - Supports both square 4K (`--size 2160` or `--width 2160 --height 2160`)
//   and 16:9 4K (`--width 3840 --height 2160`) at `--fps 60` (or any positive
//   fps). ffmpeg encodes the whole film as one web MP4 with flife's own
//   BT.709 colour chain, its quality set by `--crf` alone: the site serves it
//   from R2 (worker.ts), which has no per-file limit to fit. `--small HEIGHT`
//   also encodes the same frames scaled to that height for phones
//   (<ID>-small-<hash>.mp4), at `--small-crf` (default `--crf`). `media:4k60`
//   passes 1080 at CRF 26: at 1080 a 4K film's CRF 30 smears single bodies
//   that the 4K encode, scaled to 1080, keeps; 26 matches it, at about 56% of
//   its bytes, and at H.264 level 4.2, which phones decode, where 2160² at
//   60 fps needs 5.2, which not every phone does
//   (experiments/website-films/README.md, round 5).
//   Pass `--master-out PATH` (outside `public/media/`) to also encode an
//   archival MP4 at a lower CRF from the same PNG frames.
//
// - `--film ID` (default `film`) names the manifest, lib/films/<ID>.json, and
//   prefixes every file the film publishes (<ID>-<hash>.mp4,
//   <ID>-poster-<hash>.jpg, <ID>-social-<hash>.jpg). lib/films/index.ts lists
//   the films the page shows; add a new id there.
// - Each web file is named by its SHA-256. The video goes into media-r2/,
//   for scripts/upload-media.ts to put in R2; the poster and card go into
//   public/media/, as static assets. Both are outside git like every
//   FrankenLife recording (AGENTS.md); the committed manifest records each
//   hash and how it was made.
// - The manifest names the commit `--flife` was built from: HEAD, whose Rust
//   sources and presets must then be committed, or `--revision REV`, an
//   ancestor of HEAD whose presets equal the working tree's
//   (scripts/source-revision.ts).

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  type CameraKey,
  DIRECTOR_RULE,
  type DirectorOptions,
  FIXED_RULE,
  activities,
  cardFrame,
  direct,
  luminance,
  pacedOptions,
} from "./director";
import { ASSET_LIMIT, SOCIAL_CROP_VF, buildMasterEncodeArgs, buildWebEncodeArgs, parseMediaArgs } from "./media-options";
import { sourceRevision } from "./source-revision";

const WEB = fileURLToPath(new URL("..", import.meta.url));
const REPO = resolve(WEB, "..");
const PUBLIC_MEDIA = join(WEB, "public", "media");
// The films' videos, outside git and outside the static export: scripts/upload-media.ts puts each in R2.
const FILM_STORE = join(WEB, "media-r2");

function fail(message: string): never {
  console.error(`media: ${message}`);
  process.exit(1);
}

function run(command: string, args: string[], stdout?: string): string {
  const fd = stdout === undefined ? undefined : openSync(stdout, "w");
  const result = spawnSync(command, args, {
    cwd: REPO,
    encoding: "utf8",
    maxBuffer: 1 << 26,
    stdio: ["ignore", fd ?? "pipe", "inherit"],
  });
  if (fd !== undefined) closeSync(fd);
  if (result.error) fail(`${command}: ${result.error.message}`);
  if (result.status !== 0) fail(`${command} ${args.join(" ")} exited with ${result.status}`);
  return result.stdout ?? "";
}

function sha256(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/** Copy `path` into `dir` as `stem-<hash>.ext`, served at /media/ under that name. */
function publish(path: string, stem: string, extension: string, dir = PUBLIC_MEDIA) {
  const hash = sha256(path);
  const bytes = readFileSync(path).length;
  if (dir === PUBLIC_MEDIA && bytes > ASSET_LIMIT) {
    fail(`${path} is ${bytes} bytes, above Cloudflare's 25 MiB static asset limit`);
  }
  const name = `${stem}-${hash.slice(0, 12)}.${extension}`;
  mkdirSync(dir, { recursive: true });
  copyFileSync(path, join(dir, name));
  return { src: `/media/${name}`, sha256: hash, bytes };
}

type Validation = {
  name: string;
  model: string;
  config_hash: string;
  seed: number;
  founders: number;
  grid: [number, number];
  timestep: [number, number];
};

let opts;
try {
  opts = parseMediaArgs(process.argv.slice(2), PUBLIC_MEDIA);
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}

const {
  film: filmId,
  revision: revisionArg,
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
} = opts;

// The world's threads for both renders, if given (no result depends on them; the run's [execution]
// cpu_workers otherwise). Not part of the recorded render arguments, which name what was drawn.
const workers = cpuWorkers !== null ? ["--cpu-workers", String(cpuWorkers)] : [];
const style = [
  ...(field === "none" ? ["--no-field"] : ["--field-mapping", field]),
  "--look",
  look,
  ...(color ? ["--color", color] : []),
];
// The preview the director reads: a multiple of its 128-cell grid.
const PREVIEW = "512";

type Provenance = {
  revision: string;
  flife_sha256: string;
  render: string[];
  features?: string[];
  source: "run-dir" | "config";
  founding_digest?: string;
  camera: { rule: string; options: DirectorOptions; keys: CameraKey[] } | null;
  world: Validation;
  preset: string | null;
};

const frames = join(work, "frames");
const records = join(work, "render.ndjson");
const made = join(work, "render.json");
const renderArgs = [
  "--ticks",
  String(ticks),
  "--every",
  String(every),
  "--width",
  String(width),
  "--height",
  String(height),
  ...style,
];
const cameraFile = join(work, "camera.json");
const sourceTarget = runDir !== null ? ["--run-dir", runDir] : [config as string, ...featureFlags];
const sourceConfigPath = runDir !== null ? join(runDir, "config.toml") : (config as string);
// The torus in world units: [world] width and height of the run's configuration.
const torusExtent: [number, number] = (() => {
  // The sentinel "[" ends the section when [world] is the file's last.
  const section = /^\[world\]\s*$([\s\S]*?)(?=^\[)/m.exec(`${readFileSync(sourceConfigPath, "utf8")}\n[`)?.[1] ?? "";
  const dimension = (key: string) => {
    const value = Number(new RegExp(`^${key}\\s*=\\s*([0-9.eE+-]+)`, "m").exec(section)?.[1]);
    return Number.isFinite(value) && value > 0 ? value : fail(`the run's [world] ${key} is not a positive number`);
  };
  return [dimension("width"), dimension("height")];
})();

let provenance: Provenance;
if (existsSync(made)) {
  provenance = JSON.parse(readFileSync(made, "utf8"));
  if (provenance.render.join(" ") !== renderArgs.join(" ")) {
    fail(`${work} holds a render with ${provenance.render.join(" ")}, not ${renderArgs.join(" ")}`);
  }
  const savedFeatures = provenance.features ?? [];
  if (savedFeatures.join(" ") !== featureFlags.join(" ")) {
    fail(`${work} holds features [${savedFeatures.join(" ")}], not [${featureFlags.join(" ")}]`);
  }
  console.error(`media: encoding the finished render in ${work} again (its run; input is not re-simulated)`);
} else {
  if (existsSync(frames)) fail(`${frames} exists without a finished render; choose another --work`);
  const source = sourceRevision(REPO, revisionArg);
  if ("error" in source) fail(source.error);
  const world: Validation = JSON.parse(run(flife, ["validate", sourceConfigPath, ...featureFlags]));
  const presetPath = join("presets", `${world.name}.toml`);
  let preset: string | null = null;
  if (existsSync(join(REPO, presetPath))) {
    const committed: Validation = JSON.parse(run(flife, ["validate", presetPath]));
    if (committed.config_hash === world.config_hash) preset = presetPath;
  }
  mkdirSync(work, { recursive: true });
  let foundingDigest: string | undefined;
  if (runDir === null) {
    // Capture the exact tick-0 founding world digest so world.from always records a verified SHA-256 digest.
    const zeroRecords = run(flife, ["run", config as string, "--ticks", "0", "--every", "0", ...featureFlags])
      .split("\n")
      .filter((line) => line.trim() !== "")
      .map((line) => JSON.parse(line) as { record: string; tick?: number; digest?: string });
    const zeroEnd = zeroRecords.find((record) => record.record === "end");
    if (!zeroEnd || zeroEnd.tick !== 0 || typeof zeroEnd.digest !== "string") {
      fail("could not determine the tick-0 founding digest from `flife run --ticks 0`");
    }
    foundingDigest = zeroEnd.digest;
  }
  let camera: Provenance["camera"] = null;
  if (cameraMode === "auto") {
    const preview = join(work, "preview");
    const previewRecords = join(work, "preview.ndjson");
    const previewArgs = ["--ticks", String(ticks), "--every", String(every), "--width", PREVIEW, "--height", PREVIEW];
    if (previewFrom !== null) {
      // Its records name its own frames; the copy is what the digest check below reads.
      const reused = join(previewFrom, "preview.ndjson");
      if (!existsSync(reused)) fail(`${reused} does not exist`);
      copyFileSync(reused, previewRecords);
      console.error(`media: reading the preview in ${reused} for the camera`);
    } else {
      console.error(`media: previewing ${ticks} ticks of ${runDir ?? config} for the camera`);
      run(
        flife,
        ["render", ...sourceTarget, ...previewArgs, ...workers, "--no-field", "--look", "raw", "--out", preview],
        previewRecords,
      );
    }
    const previewed = readFileSync(previewRecords, "utf8")
      .split("\n")
      .filter((line) => line.trim() !== "")
      .map((line) => JSON.parse(line) as { record: string; tick?: number; path?: string });
    const shots = previewed.filter((record) => record.record === "frame");
    const maps = activities(shots.map((record) => luminance(record.path ?? fail("a preview frame without a path"))));
    const frameCount = shots.length;
    const extent = torusExtent;
    // The shots' zooms were chosen for a 256-unit torus; each is scaled to this one so that a shot spans the
    // same 51–102 world units on any torus of 128 units or more (a 128-unit torus zooms half as far); no zoom
    // goes below 1.25.
    const span = Math.min(...extent) / 256;
    // The site's pacing (director.ts FILM_PACING): a long look at the whole torus first, then the shots.
    const directorOptions: DirectorOptions = pacedOptions(
      frameCount,
      zooms ?? [3, 4.5, 2.5, 5, 3.5].map((zoom) => Math.max(1.25, Math.round(zoom * span * 100) / 100)),
    );
    const keys = direct(
      shots.map((record) => record.tick ?? fail("a preview frame without a tick")),
      maps,
      extent,
      directorOptions,
    );
    camera = { rule: DIRECTOR_RULE, options: directorOptions, keys };
    writeFileSync(cameraFile, `${JSON.stringify({ keys }, null, 2)}\n`);
    console.error(`media: ${keys.length} camera keys`);
  } else if (cameraMode === "fixed") {
    // One key, which flife render holds before and after it: the torus centre at the one zoom given.
    const zoom = zooms?.[0] ?? fail("--camera fixed needs one --zooms value");
    const keys: CameraKey[] = [{ tick: 0, center: [torusExtent[0] / 2, torusExtent[1] / 2], zoom }];
    camera = { rule: FIXED_RULE, options: { opening: 0, transition: 0, dwell: 0, closing: 0, zooms: [zoom] }, keys };
    writeFileSync(cameraFile, `${JSON.stringify({ keys }, null, 2)}\n`);
    console.error(`media: the torus centre held at zoom ${zoom}`);
  }
  console.error(`media: rendering ${ticks} ticks of ${runDir ?? config} at ${width}×${height}, a frame every ${every}`);
  run(
    flife,
    ["render", ...sourceTarget, ...renderArgs, ...workers, ...(camera ? ["--camera", cameraFile] : []), "--out", frames],
    records,
  );
  provenance = {
    revision: source.revision,
    flife_sha256: sha256(flife),
    render: renderArgs,
    ...(featureFlags.length > 0 ? { features: featureFlags } : {}),
    source: runDir !== null ? "run-dir" : "config",
    ...(foundingDigest !== undefined ? { founding_digest: foundingDigest } : {}),
    camera,
    world,
    preset,
  };
  writeFileSync(made, `${JSON.stringify(provenance, null, 2)}\n`);
}
const { world, preset } = provenance;

type Record = { record: string; [key: string]: unknown };
const parsed: Record[] = readFileSync(records, "utf8")
  .split("\n")
  .filter((line) => line.trim() !== "")
  .map((line) => JSON.parse(line) as Record);
const start = parsed.find((record) => record.record === "start") ?? fail("no start record");
const end = parsed.find((record) => record.record === "end") ?? fail("no end record");
const shown = parsed.filter((record) => record.record === "frame");
const runStart = start.run as { tick: number; digest: string } | undefined;
const from =
  runStart ??
  (provenance.founding_digest !== undefined
    ? { tick: (shown[0]?.tick as number | undefined) ?? 0, digest: provenance.founding_digest }
    : fail("the start record names no run and no founding digest was recorded"));
if (end.frames !== shown.length) fail(`the end record counts ${String(end.frames)} frames, ${shown.length} printed`);
// The camera is presentation only: the preview and the film reach the same world.
const previewRecords = join(work, "preview.ndjson");
if (provenance.camera && existsSync(previewRecords)) {
  const previewEnd = readFileSync(previewRecords, "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line) as Record)
    .find((record) => record.record === "end");
  if (previewEnd?.digest !== end.digest) {
    fail(`the preview ended at ${String(previewEnd?.digest)}, the film at ${String(end.digest)}`);
  }
}

const ticksList = shown.map((record) => record.tick as number);
const first = join(frames, "frame-000000.png");

// The whole film as one MP4 at the CRF given, uncapped: R2 serves it whatever its size.
const encode = buildWebEncodeArgs(crf, fps);
const encoded = join(work, "film.mp4");
run("ffmpeg", ["-y", "-loglevel", "error", "-framerate", String(fps), "-i", join(frames, "frame-%06d.png"), ...encode, encoded]);
const primaryVideo = publish(encoded, filmId, "mp4", FILM_STORE);

// The phone encode: the same frames scaled down, at its own CRF; a narrow screen plays it (components/film.tsx).
let smallVideo: { src: string; sha256: string; bytes: number; width: number; height: number; encode: string[] } | undefined;
if (small !== null) {
  const smallEncode = buildWebEncodeArgs(smallCrf, fps, small);
  const smallEncoded = join(work, "film-small.mp4");
  run("ffmpeg", ["-y", "-loglevel", "error", "-framerate", String(fps), "-i", join(frames, "frame-%06d.png"), ...smallEncode, smallEncoded]);
  smallVideo = { ...publish(smallEncoded, `${filmId}-small`, "mp4", FILM_STORE), width: small[0], height: small[1], encode: smallEncode };
  console.error(`media: phone encode ${small[0]}×${small[1]} at CRF ${smallCrf}, ${smallVideo.bytes} bytes`);
}

let master:
  | { path: string; sha256: string; bytes: number; crf: number; encode: string[] }
  | undefined;
if (masterOut !== null) {
  const masterEncode = buildMasterEncodeArgs(masterCrf);
  mkdirSync(dirname(masterOut), { recursive: true });
  run("ffmpeg", [
    "-y",
    "-loglevel",
    "error",
    "-framerate",
    String(fps),
    "-i",
    join(frames, "frame-%06d.png"),
    ...masterEncode,
    masterOut,
  ]);
  master = {
    path: masterOut,
    sha256: sha256(masterOut),
    bytes: readFileSync(masterOut).length,
    crf: masterCrf,
    encode: masterEncode,
  };
  console.error(`media: archival master ${masterOut} (${master.bytes} bytes, CRF ${masterCrf})`);
}

const poster = join(work, "poster.jpg");
run("ffmpeg", ["-y", "-loglevel", "error", "-i", first, "-q:v", "3", poster]);
const card = provenance.camera ? cardFrame(provenance.camera.keys, ticksList) : 0;
const cardSource = join(frames, `frame-${String(card).padStart(6, "0")}.png`);
if (!existsSync(cardSource)) fail(`the social card's frame ${cardSource} is missing`);
const social = join(work, "social.jpg");
run("ffmpeg", ["-y", "-loglevel", "error", "-i", cardSource, "-vf", SOCIAL_CROP_VF, "-q:v", "3", social]);

const controllers = (end.controllers as { backend?: string } | undefined)?.backend ?? "unknown";
const savedFeatures = provenance.features ?? [];
const commandSource =
  provenance.source === "config"
    ? `${preset ?? "CONFIG"}${savedFeatures.length > 0 ? ` ${savedFeatures.join(" ")}` : ""}`
    : "--run-dir RUN";
const manifest = {
  schema: "frankenlife.website-media.v1",
  id: filmId,
  video: {
    ...primaryVideo,
    width,
    height,
    fps,
    frames: shown.length,
    encode,
    ...(master ? { master } : {}),
    ...(smallVideo ? { small: smallVideo } : {}),
  },
  poster: { ...publish(poster, `${filmId}-poster`, "jpg"), frame: 0 },
  social: { ...publish(social, `${filmId}-social`, "jpg"), width: 1200, height: 630, frame: card },
  world: {
    preset,
    name: world.name,
    config_hash: world.config_hash,
    seed: world.seed,
    founders: world.founders,
    grid: world.grid,
    torus: torusExtent,
    timestep: world.timestep,
    model: world.model,
    from: { tick: from.tick, digest: from.digest },
    ticks: ticksList,
    live: shown.map((record) => record.live as number),
    every,
    to: { tick: end.tick as number, digest: end.digest as string },
  },
  render: {
    command: `flife render ${commandSource} ${renderArgs.join(" ")}${provenance.camera ? " --camera CAMERA" : ""} --out FRAMES`,
    ...(savedFeatures.length > 0 ? { features: savedFeatures } : {}),
    camera: provenance.camera,
    // Each frame's zoom as flife render recorded it (1 is the whole torus), for the film's readout.
    zoom: shown.map((record) => {
      const view = record.view as { zoom?: number } | undefined;
      return Math.round((view?.zoom ?? 1) * 100) / 100;
    }),
    mapping: end.mapping as string,
    meaning: end.meaning as string,
    look: end.look as string,
    // `none` when the field is not drawn: flife names a mapping either way.
    field_mapping: end.field === false ? "none" : (end.field_mapping as string),
    controllers,
    revision: provenance.revision,
    flife_sha256: provenance.flife_sha256,
  },
};
mkdirSync(join(WEB, "lib", "films"), { recursive: true });
const manifestPath = join(WEB, "lib", "films", `${filmId}.json`);
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
console.error(`media: wrote ${manifestPath}; lib/films/index.ts must list "${filmId}" for the page to show it`);
console.error(
  `media: ${shown.length} frames at ${width}×${height} ${fps} fps, ticks ${from.tick} to ${String(end.tick)}, ` +
    `video ${manifest.video.bytes} bytes in media-r2/; \`bun run upload-media\` puts it in R2`,
);
