// Render the site's film from a FrankenLife run directory and write its
// manifest (lib/media.json).
//
//   bun scripts/media.ts --flife PATH --run-dir DIR --work DIR
//     [--ticks 9600] [--every 32] [--size 1080] [--fps 30] [--crf 24]
//
// - `flife render --run-dir` continues the run from its newest checkpoint
//   that restores, as `flife resume` would, and writes no checkpoint, journal
//   record or catalog row to the run (FrankenSQLite may rewrite the catalog's
//   log files when it opens it; flife-cli's CONTRACT.md). Its PNG frames,
//   its NDJSON records and what made them (render.json) go to `--work`. A
//   `--work` that already holds a finished render is encoded again without
//   rendering, with the provenance it recorded (to try another `--crf`).
// - ffmpeg encodes the frames with flife's own colour chain (render.rs's
//   PNG_TO_H264: BT.709 limited range, 4:2:0, left chroma), at a CRF chosen
//   for the web. The particles move a few pixels a frame, so little carries
//   from one frame to the next: CRF 24 keeps them distinct at about 78 KB a
//   frame (23.6 MB for the site's 301 frames), CRF 28 blurred them into
//   blobs, and CRF 23 passed Cloudflare's limit. The poster and the social
//   card are the first frame.
// - Each file is named by its SHA-256 and copied into public/media/, which
//   is outside git like every FrankenLife recording (AGENTS.md); the
//   committed manifest records each hash and how it was made.
// - The Rust sources must be committed: the manifest names the revision, and
//   a dirty tree would make that a guess. Build `--flife` from that revision.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEB = fileURLToPath(new URL("..", import.meta.url));
const REPO = resolve(WEB, "..");
// Cloudflare serves a static asset of at most 25 MiB.
const ASSET_LIMIT = 25 * 1024 * 1024;

function fail(message: string): never {
  console.error(`media: ${message}`);
  process.exit(1);
}

function options(): Map<string, string> {
  const args = process.argv.slice(2);
  const found = new Map<string, string>();
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index];
    const value = args[index + 1];
    if (!key?.startsWith("--") || value === undefined) {
      fail(`expected --name value pairs, got ${args.join(" ")}`);
    }
    found.set(key.slice(2), value);
  }
  return found;
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

/** Copy `path` into public/media/ as `stem-<hash>.ext`. */
function publish(path: string, stem: string, extension: string) {
  const hash = sha256(path);
  const bytes = readFileSync(path).length;
  if (bytes > ASSET_LIMIT) fail(`${path} is ${bytes} bytes, above Cloudflare's 25 MiB asset limit`);
  const name = `${stem}-${hash.slice(0, 12)}.${extension}`;
  const media = join(WEB, "public", "media");
  mkdirSync(media, { recursive: true });
  copyFileSync(path, join(media, name));
  return { src: `/media/${name}`, sha256: hash, bytes };
}

type Validation = {
  name: string;
  config_hash: string;
  seed: number;
  founders: number;
  grid: [number, number];
  timestep: [number, number];
};

const flags = options();
for (const key of flags.keys()) {
  if (!["flife", "run-dir", "work", "ticks", "every", "size", "fps", "crf"].includes(key)) fail(`unknown option --${key}`);
}
// The paths are the caller's, relative to where it ran; flife and git run from the repository root.
const path = (key: string) => resolve(flags.get(key) ?? fail(`--${key} is required`));
const flife = path("flife");
const runDir = path("run-dir");
const work = path("work");
const count = (key: string, fallback: string, least: number) => {
  const value = flags.get(key) ?? fallback;
  if (!/^\d+$/.test(value) || Number(value) < least) fail(`--${key} must be an integer of at least ${least}`);
  return value;
};
const ticks = count("ticks", "9600", 0);
const every = count("every", "32", 1);
const size = count("size", "1080", 2);
const fps = count("fps", "30", 1);
const crf = count("crf", "24", 0);
// 4:2:0 video halves each dimension.
if (Number(size) % 2 !== 0) fail("--size must be even");

type Provenance = {
  revision: string;
  flife_sha256: string;
  render: string[];
  world: Validation;
  preset: string | null;
};

const frames = join(work, "frames");
const records = join(work, "render.ndjson");
const made = join(work, "render.json");
const renderArgs = ["--ticks", ticks, "--every", every, "--width", size, "--height", size];
let provenance: Provenance;
if (existsSync(made)) {
  provenance = JSON.parse(readFileSync(made, "utf8"));
  if (provenance.render.join(" ") !== renderArgs.join(" ")) {
    fail(`${work} holds a render with ${provenance.render.join(" ")}, not ${renderArgs.join(" ")}`);
  }
  console.error(`media: encoding the finished render in ${work} again (its run; --run-dir is not read)`);
} else {
  if (existsSync(frames)) fail(`${frames} exists without a finished render; choose another --work`);
  const dirty = run("git", ["status", "--porcelain", "--", "crates", "presets", "Cargo.toml", "Cargo.lock"]);
  if (dirty.trim() !== "") fail(`commit the Rust sources first; uncommitted:\n${dirty}`);
  const world: Validation = JSON.parse(run(flife, ["validate", join(runDir, "config.toml")]));
  const presetPath = join("presets", `${world.name}.toml`);
  let preset: string | null = null;
  if (existsSync(join(REPO, presetPath))) {
    const committed: Validation = JSON.parse(run(flife, ["validate", presetPath]));
    if (committed.config_hash === world.config_hash) preset = presetPath;
  }
  mkdirSync(work, { recursive: true });
  console.error(`media: rendering ${ticks} ticks of ${runDir}, a frame every ${every}`);
  run(flife, ["render", "--run-dir", runDir, ...renderArgs, "--out", frames], records);
  provenance = {
    revision: run("git", ["rev-parse", "HEAD"]).trim(),
    flife_sha256: sha256(flife),
    render: renderArgs,
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
const from = start.run as { tick: number; digest: string } | undefined;
if (!from) fail("the start record names no run");
if (end.frames !== shown.length) fail(`the end record counts ${String(end.frames)} frames, ${shown.length} printed`);

const encode = [
  "-vf",
  "scale=out_color_matrix=bt709:out_range=tv:out_chroma_loc=left,format=yuv420p," +
    "setparams=range=tv:color_primaries=bt709:color_trc=iec61966-2-1:colorspace=bt709",
  "-c:v",
  "libx264",
  "-preset",
  "slow",
  "-crf",
  crf,
  "-pix_fmt",
  "yuv420p",
  "-chroma_sample_location",
  "left",
  "-movflags",
  "+faststart",
  "-an",
];
const video = join(work, "film.mp4");
const first = join(frames, "frame-000000.png");
run("ffmpeg", ["-y", "-loglevel", "error", "-framerate", fps, "-i", join(frames, "frame-%06d.png"), ...encode, video]);
const poster = join(work, "poster.jpg");
run("ffmpeg", ["-y", "-loglevel", "error", "-i", first, "-q:v", "3", poster]);
const social = join(work, "social.jpg");
run("ffmpeg", ["-y", "-loglevel", "error", "-i", first, "-vf", "scale=1200:-1,crop=1200:630", "-q:v", "3", social]);

const controllers = (end.controllers as { backend?: string } | undefined)?.backend ?? "unknown";
const manifest = {
  schema: "frankenlife.website-media.v1",
  video: {
    ...publish(video, "film", "mp4"),
    width: Number(size),
    height: Number(size),
    fps: Number(fps),
    frames: shown.length,
    encode,
  },
  poster: { ...publish(poster, "poster", "jpg"), frame: 0 },
  social: { ...publish(social, "social", "jpg"), width: 1200, height: 630 },
  world: {
    preset,
    name: world.name,
    config_hash: world.config_hash,
    seed: world.seed,
    founders: world.founders,
    grid: world.grid,
    timestep: world.timestep,
    from: { tick: from.tick, digest: from.digest },
    ticks: shown.map((record) => record.tick as number),
    live: shown.map((record) => record.live as number),
    every: Number(every),
    to: { tick: end.tick as number, digest: end.digest as string },
  },
  render: {
    command: `flife render --run-dir RUN ${renderArgs.join(" ")} --out FRAMES`,
    mapping: end.mapping as string,
    meaning: end.meaning as string,
    look: end.look as string,
    field_mapping: end.field_mapping as string,
    controllers,
    revision: provenance.revision,
    flife_sha256: provenance.flife_sha256,
  },
};
writeFileSync(join(WEB, "lib", "media.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.error(
  `media: ${shown.length} frames, ticks ${from.tick} to ${String(end.tick)}, video ${manifest.video.bytes} bytes`,
);
