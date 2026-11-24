// Render the v2 stills and write their manifest (lib/stills.json).
//
//   bun scripts/stills.ts --flife PATH --work DIR [--size 2160] [--cpu-workers N]
//                         [--stage2 PATH/stage2_summary_FINAL_104.json] [--only ID,ID]
//                         [--revision COMMIT]
//
// - Two sets, each world from its committed preset at its own seed (42), drawn
//   at tick 2,048, the tick the V2-17 Stage-2 campaign scored
//   (experiments/v2-emergence/README.md):
//   * `v2-controls`: each v2 preset as committed beside two of its controls,
//     the same preset with every v2 feature off (`--v1-compat`) and with
//     inheritance removed (model `field-ecology-v2-fresh-offspring`: each
//     newborn gets the genome a founder would have at its identity). The whole
//     torus, glow look, founder lineage lit by energy over the field's energy.
//   * `v2-views`: the four read-only v2 views (V2-16) of the same two worlds:
//     cell state and polarity of the tissue (zoomed on one island), trail
//     carrier and vascular conductance of the supercolony (the whole torus).
//     A zoomed view's centre was chosen by eye on its world's whole-torus
//     still, on the largest island; the spec records it.
// - Each world runs from tick 0 inside `flife render CONFIG`; the still is its
//   last frame. The manifest records each world's digest and live count from
//   the render's own records. With `--stage2`, each digest must also equal the
//   one the Stage-2 summary (kept outside git) recorded for the same bundle,
//   condition and seed, and the script refuses otherwise; the manifest then
//   records the match.
// - A work directory whose still already has a finished render (an end record)
//   made by the same flife binary (SHA-256) with the configuration and command
//   this script would use is adopted without rendering.
// - The manifest names the commit `--flife` was built from: HEAD, whose Rust
//   sources and presets must then be committed, or `--revision COMMIT`, an
//   ancestor of HEAD whose presets equal the working tree's
//   (scripts/source-revision.ts).
// - Each still is a JPEG (ffmpeg -q:v 3, as the films' posters) named by its
//   SHA-256 in public/media/, outside git (AGENTS.md); scripts/check-media.ts
//   checks them byte for byte before a build.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { ASSET_LIMIT } from "./media-options";
import { sourceRevision } from "./source-revision";

const WEB = fileURLToPath(new URL("..", import.meta.url));
const REPO = resolve(WEB, "..");
const TICK = 2048;
const JPEG = ["-q:v", "3"];

type Bundle = "tissue" | "supercolony";
type Condition = "treatment" | "v1-compat" | "fresh-offspring";

const PRESETS: Record<Bundle, { path: string; name: string; stage2: "a" | "b" }> = {
  tissue: { path: "presets/morphogenesis-tissue-v2.toml", name: "morphogenesis-tissue-v2", stage2: "a" },
  supercolony: { path: "presets/supercolony-v2.toml", name: "supercolony-v2", stage2: "b" },
};

type Spec = {
  id: string;
  set: "v2-controls" | "v2-views";
  bundle: Bundle;
  condition: Condition;
  label: string;
  caption: string;
  draw: string[];
  camera?: { center: [number, number]; zoom: number };
};

const WHOLE = ["--look", "glow", "--color", "lineage-energy", "--field-mapping", "energy"];

const SPECS: Spec[] = [
  {
    id: "tissue-treatment",
    set: "v2-controls",
    bundle: "tissue",
    condition: "treatment",
    label: "Tissue, as committed",
    caption: "Every Bundle A mechanism on: porous islands of tissue, several lineages side by side.",
    draw: WHOLE,
  },
  {
    id: "tissue-v1-compat",
    set: "v2-controls",
    bundle: "tissue",
    condition: "v1-compat",
    label: "Every v2 feature off",
    caption: "The same preset with --v1-compat: nothing stops division in a crowd, and bodies jam into packed slabs.",
    draw: WHOLE,
  },
  {
    id: "tissue-fresh-offspring",
    set: "v2-controls",
    bundle: "tissue",
    condition: "fresh-offspring",
    label: "Inheritance removed",
    caption: "Each newborn gets a founder's genome instead of its parent's: the population all but dies out.",
    draw: WHOLE,
  },
  {
    id: "supercolony-treatment",
    set: "v2-controls",
    bundle: "supercolony",
    condition: "treatment",
    label: "Supercolony, as committed",
    caption: "Every Bundle B mechanism on: compact colonies with well-fed, brightly lit rims, between blooming oases.",
    draw: WHOLE,
  },
  {
    id: "supercolony-v1-compat",
    set: "v2-controls",
    bundle: "supercolony",
    condition: "v1-compat",
    label: "Every v2 feature off",
    caption: "The same preset with --v1-compat: colonies still form, larger and without lit rims; with no trail carrier or vascular field, no trails or veins are laid.",
    draw: WHOLE,
  },
  {
    id: "supercolony-fresh-offspring",
    set: "v2-controls",
    bundle: "supercolony",
    condition: "fresh-offspring",
    label: "Inheritance removed",
    caption: "Each newborn gets a founder's genome instead of its parent's: a few dozen organisms remain.",
    draw: WHOLE,
  },
  {
    id: "tissue-polarity",
    set: "v2-views",
    bundle: "tissue",
    condition: "treatment",
    label: "Polarity",
    caption: "--color polarity, sphere look: each cell's hue is the direction its polarity points, with a short tick on its apical side; grey cells have none. Patches of neighbouring cells point alike.",
    draw: ["--look", "sphere", "--color", "polarity", "--no-field"],
    // The largest island on the whole-torus still (two lineages, cyan over lilac).
    camera: { center: [11, 41], zoom: 3 },
  },
  {
    id: "tissue-cell-state",
    set: "v2-views",
    bundle: "tissue",
    condition: "treatment",
    label: "Cell state",
    caption: "--color cell-state: each cell keeps its lineage's hue; its lightness, and a small twist of hue, follow its first memory value h₀. At this tick h₀ varies little within a lineage, so the colours barely shift: no separate cell types yet (below).",
    draw: ["--look", "glow", "--color", "cell-state", "--field-mapping", "energy"],
    // The cyan island east of centre and the mixed strands below it.
    camera: { center: [58.5, 35], zoom: 3 },
  },
  {
    id: "supercolony-trail-carrier",
    set: "v2-views",
    bundle: "supercolony",
    condition: "treatment",
    label: "Trail carrier",
    caption: "--field-mapping trail-carrier: the uneatable second carrier, plum through amber to gold as more lies on the field. It halves in 23 s, so it marks where colonies passed in the last minute.",
    draw: ["--look", "glow", "--color", "lineage-energy", "--field-mapping", "trail-carrier"],
  },
  {
    id: "supercolony-vascular-conductance",
    set: "v2-views",
    bundle: "supercolony",
    condition: "treatment",
    label: "Vascular conductance",
    caption: "--field-mapping vascular-conductance: the field's faces, bright where flux has widened them, dark seams where they have pruned back.",
    draw: ["--look", "glow", "--color", "lineage-energy", "--field-mapping", "vascular-conductance"],
  },
];

function fail(message: string): never {
  console.error(`stills: ${message}`);
  process.exit(1);
}

function run(command: string, args: string[], stdout?: string): string {
  const result = spawnSync(command, args, { cwd: REPO, encoding: "utf8", maxBuffer: 1 << 28, stdio: ["ignore", "pipe", "inherit"] });
  if (result.error) fail(`${command}: ${result.error.message}`);
  if (result.status !== 0) fail(`${command} ${args.join(" ")} exited with ${result.status}`);
  if (stdout) writeFileSync(stdout, result.stdout ?? "");
  return result.stdout ?? "";
}

function sha256(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

type NdRecord = { record: string; [key: string]: unknown };
function records(path: string): NdRecord[] {
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line) as NdRecord);
}

// ─── Arguments ───────────────────────────────────────────────────────────────

const args = new Map<string, string>();
const argv = process.argv.slice(2);
for (let at = 0; at < argv.length; at += 2) {
  const key = argv[at];
  const value = argv[at + 1];
  if (!key?.startsWith("--") || value === undefined) fail(`expected --option value pairs, got ${argv.join(" ")}`);
  if (!["flife", "work", "size", "cpu-workers", "stage2", "only", "revision"].includes(key.slice(2))) fail(`unknown option ${key}`);
  args.set(key.slice(2), value);
}
const flife = resolve(args.get("flife") ?? fail("--flife is required"));
const work = resolve(args.get("work") ?? fail("--work is required"));
const size = Number(args.get("size") ?? "2160");
if (!Number.isInteger(size) || size < 2 || size % 2 !== 0) fail("--size must be an even integer");
const workers = args.has("cpu-workers") ? ["--cpu-workers", args.get("cpu-workers") as string] : [];
const only = args.has("only") ? new Set((args.get("only") as string).split(",")) : null;
const source = sourceRevision(REPO, args.get("revision") ?? null);
if ("error" in source) fail(source.error);
type Stage2Record = { bundle: string; condition: string; seed: number; tick: number; digest: string; live: number };
const stage2Path = args.has("stage2") ? resolve(args.get("stage2") as string) : null;
const stage2: Stage2Record[] = stage2Path ? (JSON.parse(readFileSync(stage2Path, "utf8")) as { records: Stage2Record[] }).records : [];

// ─── Each world ──────────────────────────────────────────────────────────────

/** The preset, with only `model` swapped for the fresh-offspring control (and its name, which nothing depends on). */
function configText(spec: Spec): string {
  const preset = PRESETS[spec.bundle];
  const text = readFileSync(join(REPO, preset.path), "utf8");
  if (spec.condition !== "fresh-offspring") return text;
  const swap = (input: string, key: string, from: string, to: string) => {
    const line = `${key} = "${from}"`;
    const count = input.split("\n").filter((each) => each === line).length;
    if (count !== 1) fail(`${preset.path} has ${count} lines \`${line}\`, not one`);
    const lines = input.split("\n");
    lines[lines.indexOf(line)] = `${key} = "${to}"`;
    return lines.join("\n");
  };
  return swap(swap(text, "model", "field-ecology-v2", "field-ecology-v2-fresh-offspring"), "name", preset.name, `${preset.name}-fresh-offspring`);
}

function renderArgs(spec: Spec, cameraPath: string | null): string[] {
  return [
    "--ticks",
    String(TICK),
    "--every",
    String(TICK),
    "--width",
    String(size),
    "--height",
    String(size),
    ...spec.draw,
    ...(spec.condition === "v1-compat" ? ["--v1-compat"] : []),
    ...(cameraPath ? ["--camera", cameraPath] : []),
  ];
}

const chosen = SPECS.filter((spec) => only === null || only.has(spec.id));
const flifeSha = sha256(flife);
for (const spec of chosen) {
  const dir = join(work, spec.id);
  mkdirSync(dir, { recursive: true });
  const config = join(dir, "config.toml");
  const text = configText(spec);
  if (existsSync(config) && readFileSync(config, "utf8") !== text) fail(`${config} is not the configuration this script writes`);
  writeFileSync(config, text);
  let cameraPath: string | null = null;
  if (spec.camera) {
    cameraPath = join(dir, "camera.json");
    const camera = `${JSON.stringify({ keys: [{ tick: TICK, center: spec.camera.center, zoom: spec.camera.zoom }] })}\n`;
    if (existsSync(cameraPath) && readFileSync(cameraPath, "utf8") !== camera) fail(`${cameraPath} is not the camera this script chooses`);
    writeFileSync(cameraPath, camera);
  }
  const command = renderArgs(spec, cameraPath);
  // A render is adopted only if this binary drew it with this command.
  const stamp = JSON.stringify({ flife_sha256: flifeSha, command });
  const commandPath = join(dir, "command.json");
  const done = records(join(dir, "render.ndjson")).some((record) => record.record === "end");
  if (done && existsSync(commandPath) && readFileSync(commandPath, "utf8") === stamp) {
    console.error(`stills: adopting the finished render in ${dir}`);
    continue;
  }
  writeFileSync(commandPath, stamp);
  console.error(`stills: rendering ${spec.id} to tick ${TICK}`);
  run(flife, ["render", config, ...command, ...workers, "--out", join(dir, "frames")], join(dir, "render.ndjson"));
}

// ─── The manifest ────────────────────────────────────────────────────────────

type PreviousManifest = { flife_sha256: string; revision: string; stills: { id: string }[] };
const previousManifest = existsSync(join(WEB, "lib", "stills.json"))
  ? (JSON.parse(readFileSync(join(WEB, "lib", "stills.json"), "utf8")) as PreviousManifest)
  : null;
// The manifest names one binary and one revision for every still, so stills
// kept from an earlier run (--only) must have been drawn by the same ones.
if (only !== null && previousManifest && previousManifest.stills.some((still) => !only.has(still.id))) {
  if (previousManifest.flife_sha256 !== flifeSha || previousManifest.revision !== source.revision) {
    fail(
      `lib/stills.json was made by flife ${previousManifest.flife_sha256.slice(0, 12)} at ${previousManifest.revision.slice(0, 12)}, ` +
        `not ${flifeSha.slice(0, 12)} at ${source.revision.slice(0, 12)}: render every still again, without --only`,
    );
  }
}
const previous = previousManifest?.stills ?? [];
const media = join(WEB, "public", "media");
mkdirSync(media, { recursive: true });
const made = chosen.map((spec) => {
  const dir = join(work, spec.id);
  const parsed = records(join(dir, "render.ndjson"));
  const end = parsed.find((record) => record.record === "end") ?? fail(`${spec.id}: no end record`);
  const last = parsed.filter((record) => record.record === "frame").at(-1) ?? fail(`${spec.id}: no frame`);
  if (last.tick !== TICK || end.tick !== TICK) fail(`${spec.id}: its last frame is at tick ${String(last.tick)}, not ${TICK}`);
  const validated = JSON.parse(run(flife, ["validate", join(dir, "config.toml"), ...(spec.condition === "v1-compat" ? ["--v1-compat"] : [])])) as {
    config_hash: string;
    model: string;
    seed: number;
    v2_features?: string[];
  };
  const preset = PRESETS[spec.bundle];
  let stage2Match: { bundle: string; condition: string; seed: number; live: number } | null = null;
  if (stage2Path) {
    const recorded = stage2.find(
      (record) => record.bundle === preset.stage2 && record.condition === spec.condition && record.seed === validated.seed && record.tick === TICK,
    );
    if (!recorded) fail(`${spec.id}: the Stage-2 summary has no ${preset.stage2}/${spec.condition}/seed ${validated.seed} at tick ${TICK}`);
    if (recorded.digest !== end.digest) fail(`${spec.id} reached ${String(end.digest)}; Stage-2 recorded ${recorded.digest}`);
    if (recorded.live !== last.live) fail(`${spec.id} has ${String(last.live)} alive; Stage-2 recorded ${recorded.live}`);
    stage2Match = { bundle: preset.stage2, condition: spec.condition, seed: recorded.seed, live: recorded.live };
  }
  const png = String(last.path);
  const jpeg = join(dir, "still.jpg");
  run("ffmpeg", ["-y", "-loglevel", "error", "-i", png, ...JPEG, jpeg]);
  const bytes = readFileSync(jpeg).length;
  if (bytes > ASSET_LIMIT) fail(`${jpeg} is ${bytes} bytes, above Cloudflare's 25 MiB asset limit`);
  const hash = sha256(jpeg);
  const name = `${spec.set}-${spec.id}-${hash.slice(0, 12)}.jpg`;
  copyFileSync(jpeg, join(media, name));
  const view = last.view as { center: [number, number]; zoom: number };
  return {
    id: spec.id,
    set: spec.set,
    bundle: spec.bundle,
    condition: spec.condition,
    label: spec.label,
    caption: spec.caption,
    preset: preset.path,
    model: validated.model,
    seed: validated.seed,
    config_hash: validated.config_hash,
    features: validated.v2_features ?? [],
    src: `/media/${name}`,
    sha256: hash,
    bytes,
    width: size,
    height: size,
    tick: TICK,
    live: last.live as number,
    digest: end.digest as string,
    view,
    command: `flife render ${spec.condition === "fresh-offspring" ? "CONFIG" : preset.path} ${renderArgs(spec, spec.camera ? "CAMERA" : null).join(" ")} --out FRAMES`,
    meaning: end.meaning as string,
    stage2: stage2Match,
  };
});
// Stills not rendered this time (--only) keep their earlier entries.
const kept = (previous as typeof made).filter((still) => !made.some((each) => each.id === still.id));
const stills = [...kept, ...made].sort((a, b) => SPECS.findIndex((s) => s.id === a.id) - SPECS.findIndex((s) => s.id === b.id));

const manifest = {
  schema: "frankenlife.website-stills.v1",
  tick: TICK,
  jpeg: JPEG,
  // Claimed for the set only if every still, kept or new, was checked against the summary.
  stage2: stills.every((still) => still.stage2 !== null)
    ? "experiments/v2-emergence/README.md: each digest and live count equals the V2-17 Stage-2 summary's record (stage2_summary_FINAL_104.json, kept outside git) for the same bundle, condition and seed"
    : null,
  flife_sha256: flifeSha,
  revision: source.revision,
  stills,
};
writeFileSync(join(WEB, "lib", "stills.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.error(`stills: wrote lib/stills.json with ${stills.length} stills`);
