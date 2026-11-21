// Render the film's world beside its model's two controls at one tick, and
// write their manifest (lib/controls.json).
//
//   bun scripts/controls.ts --flife PATH --work DIR [--cpu-workers N] [--parallel 2]
//
// - The world is presets/confluent-world.toml, the film's (lib/media.json's
//   world.preset), at its seed. The controls are that preset with only its
//   `model` changed to the model's two registered controls (plan §5.4;
//   flife-core's registry): `field-ecology-v1-contact-chemistry-only`, whose
//   networks are evaluated and charged but whose actions decode from an
//   all-zero output, and `field-ecology-v1-contact-fresh-offspring`, whose
//   newborns get the genome a founder would have at their identity instead
//   of their parent's mutated one. (Its `name` changes too; no world depends
//   on it.)
// - Each runs from tick 0 inside `flife render CONFIG` and is drawn at tick 0
//   and at the tick the showcase scored (2,400), with the film's settings:
//   1080 × 1080, the whole torus, no field, the sphere look, coloured by
//   founder lineage lit by energy. The still is the last frame.
// - The guard is the record: each world's digest at that tick must equal the
//   digest experiments/showcase/confluent.json records for the trial it
//   repeats (w80-o25, cw-chem, cw-fresh), and the full world's must equal the
//   film's starting digest (lib/media.json world.from). So each still is the
//   recorded run, drawn; the script refuses otherwise. The record's distance
//   and live count, and the README's words for each frame, are copied with
//   their source.
// - The manifest names the flife binary by its SHA-256, the source tree's
//   revision and whether crates/ or presets/ had uncommitted changes when it
//   ran. A `--work` whose condition already holds a finished render (its
//   end record) is adopted without rendering, if its config.toml is the one
//   this script would write.
// - Each still is a JPEG (ffmpeg -q:v 3, as the poster) named by its SHA-256
//   in public/media/, outside git (AGENTS.md); scripts/check-media.ts checks
//   them byte for byte before a build. A still that is the film's poster byte
//   for byte (the full world is, drawn and encoded the same way) is not
//   copied: the manifest names the poster.

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { arch, cpus, platform } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEB = fileURLToPath(new URL("..", import.meta.url));
const REPO = resolve(WEB, "..");
const PRESET = "presets/confluent-world.toml";
const RECORD_JSON = "experiments/showcase/confluent.json";
const RECORD_README = "experiments/showcase/README.md";
const TICK = 2400;
const SIZE = 1080;
const STYLE = ["--no-field", "--look", "sphere", "--color", "lineage-energy"];
const RENDER = ["--ticks", String(TICK), "--every", String(TICK), "--width", String(SIZE), "--height", String(SIZE), ...STYLE];
const JPEG = ["-q:v", "3"];

/** The three conditions, in the order the figure shows them. */
const CONDITIONS = [
  {
    id: "confluent",
    trial: "w80-o25",
    readme: "confluent-world",
    model: "field-ecology-v1-contact",
    name: "confluent-world",
    label: "The film's world",
    removed: null,
    rule: "Every rule on: each organism's own network chooses its actions from what it senses, and a child inherits its parent's genome, mutated.",
  },
  {
    id: "chemistry-only",
    trial: "cw-chem",
    readme: "field-ecology-v1-contact-chemistry-only",
    model: "field-ecology-v1-contact-chemistry-only",
    name: "confluent-world-chemistry-only",
    label: "Decisions removed",
    removed: "the networks' decisions",
    rule: "Every network still runs and is still charged for, but every organism's actions decode from an all-zero output, so no network decides anything.",
  },
  {
    id: "fresh-offspring",
    trial: "cw-fresh",
    readme: "field-ecology-v1-contact-fresh-offspring",
    model: "field-ecology-v1-contact-fresh-offspring",
    name: "confluent-world-fresh-offspring",
    label: "Inheritance removed",
    removed: "inheritance",
    rule: "Each newborn gets the genome a founder would have at its identity, unmutated, instead of its parent's mutated genome.",
  },
] as const;

function fail(message: string): never {
  console.error(`controls: ${message}`);
  process.exit(1);
}

function options(): Map<string, string> {
  const args = process.argv.slice(2);
  const found = new Map<string, string>();
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index];
    const value = args[index + 1];
    if (!key?.startsWith("--") || value === undefined) fail(`expected --name value pairs, got ${args.join(" ")}`);
    found.set(key.slice(2), value);
  }
  for (const key of found.keys()) {
    if (!["flife", "work", "cpu-workers", "parallel"].includes(key)) fail(`unknown option --${key}`);
  }
  return found;
}

function run(command: string, args: string[]): string {
  const result = spawnSync(command, args, { cwd: REPO, encoding: "utf8", maxBuffer: 1 << 26, stdio: ["ignore", "pipe", "inherit"] });
  if (result.error) fail(`${command}: ${result.error.message}`);
  if (result.status !== 0) fail(`${command} ${args.join(" ")} exited with ${result.status}`);
  return result.stdout ?? "";
}

function sha256(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

type Record = { record: string; [key: string]: unknown };
function records(path: string): Record[] {
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line) as Record);
}

/** The preset with `model` and `name` replaced, failing if either line is not there exactly once. */
function configText(model: string, name: string): string {
  const preset = readFileSync(join(REPO, PRESET), "utf8");
  const swap = (text: string, key: string, from: string, to: string) => {
    const line = `${key} = "${from}"`;
    const count = text.split("\n").filter((each) => each === line).length;
    if (count !== 1) fail(`${PRESET} has ${count} lines \`${line}\`, not one`);
    return text.replace(`\n${line}\n`, `\n${key} = "${to}"\n`);
  };
  return swap(swap(preset, "model", "field-ecology-v1-contact", model), "name", "confluent-world", name);
}

/** A README row's words: the `| \`run\` | distance | live | by eye |` table under "### Controls". */
function byEye(readme: string, run: string): string {
  const section = /^### Controls\n([\s\S]*?)(?=^### )/m.exec(readme)?.[1] ?? fail(`${RECORD_README} has no "### Controls"`);
  const row = section.split("\n").find((line) => line.startsWith(`| \`${run}\` |`));
  const words = row?.split("|")[4]?.trim();
  return words || fail(`${RECORD_README} "### Controls" has no row for ${run}`);
}

function host(): string {
  if (platform() === "darwin") {
    const chip = spawnSync("sysctl", ["-n", "machdep.cpu.brand_string"], { encoding: "utf8" }).stdout.trim();
    const version = spawnSync("sw_vers", ["-productVersion"], { encoding: "utf8" }).stdout.trim();
    return `${chip || cpus()[0]?.model || arch()}, macOS ${version}`;
  }
  return `${cpus()[0]?.model ?? arch()}, ${platform()}`;
}

const flags = options();
const flife = resolve(flags.get("flife") ?? fail("--flife is required"));
const work = resolve(flags.get("work") ?? fail("--work is required"));
const workers = flags.has("cpu-workers") ? ["--cpu-workers", flags.get("cpu-workers") as string] : [];
const parallel = Math.max(1, Number(flags.get("parallel") ?? "2") || 1);

const film = JSON.parse(readFileSync(join(WEB, "lib", "media.json"), "utf8")) as {
  world: { preset: string | null; seed: number; from: { tick: number; digest: string } };
  poster: { src: string; sha256: string };
};
if (film.world.preset !== PRESET) fail(`the film's world is ${film.world.preset}, not ${PRESET}`);
if (film.world.from.tick !== TICK) fail(`the film starts at tick ${film.world.from.tick}, not ${TICK}`);
const trials = (JSON.parse(readFileSync(join(REPO, RECORD_JSON), "utf8")) as {
  trials: { trial: string; model: string; seed: number; tick: number; live: number; distance: number; digest: string }[];
}).trials;
const readme = readFileSync(join(REPO, RECORD_README), "utf8");

// Render what is not already rendered, `parallel` at a time.
async function renderAll(): Promise<void> {
  const pending: (() => Promise<void>)[] = [];
  for (const condition of CONDITIONS) {
    const dir = join(work, condition.id);
    const config = join(dir, "config.toml");
    const text = configText(condition.model, condition.name);
    mkdirSync(dir, { recursive: true });
    if (existsSync(config) && readFileSync(config, "utf8") !== text) fail(`${config} is not the configuration this script writes`);
    if (records(join(dir, "render.ndjson")).some((record) => record.record === "end")) {
      console.error(`controls: adopting the finished render in ${dir}`);
      continue;
    }
    if (existsSync(join(dir, "frames", "frame-000001.png"))) fail(`${dir} holds frames without an end record; choose another --work`);
    writeFileSync(config, text);
    pending.push(
      () =>
        new Promise<void>((done) => {
          console.error(`controls: rendering ${condition.id} to tick ${TICK}`);
          const out = openSync(join(dir, "render.ndjson"), "w");
          const err = openSync(join(dir, "render.log"), "w");
          const child = spawn(flife, ["render", config, ...RENDER, ...workers, "--out", join(dir, "frames")], {
            cwd: REPO,
            stdio: ["ignore", out, err],
          });
          child.on("exit", (status) => {
            closeSync(out);
            closeSync(err);
            if (status !== 0) fail(`flife render of ${condition.id} exited with ${status}; ${join(dir, "render.log")}`);
            done();
          });
        }),
    );
  }
  const running: Promise<void>[] = [];
  for (const job of pending) {
    if (running.length >= parallel) await Promise.race(running);
    const promise = job().then(() => {
      running.splice(running.indexOf(promise), 1);
    });
    running.push(promise);
  }
  await Promise.all(running);
}

await renderAll();

const media = join(WEB, "public", "media");
mkdirSync(media, { recursive: true });
const ceilings = new Set<number>();
const stills = CONDITIONS.map((condition) => {
  const dir = join(work, condition.id);
  const parsed = records(join(dir, "render.ndjson"));
  const end = parsed.find((record) => record.record === "end") ?? fail(`${condition.id}: no end record`);
  const last = parsed.filter((record) => record.record === "frame").at(-1) ?? fail(`${condition.id}: no frame`);
  if (last.tick !== TICK || end.tick !== TICK) fail(`${condition.id}: its last frame is at tick ${String(last.tick)}, not ${TICK}`);
  const view = last.view as { center: [number, number]; zoom: number };
  if (view.zoom !== 1) fail(`${condition.id}: the still is at zoom ${view.zoom}, not the whole torus`);
  // An adopted render is checked only by its config.toml: it must also be drawn as RENDER draws, or the
  // manifest's command, mapping and size would misdescribe it.
  if (end.look !== "sphere" || end.mapping !== "lineage-energy" || end.field !== false) {
    fail(`${condition.id} was drawn ${String(end.look)}, ${String(end.mapping)}, field ${String(end.field)}; not ${STYLE.join(" ")}`);
  }
  const frame = readFileSync(String(last.path));
  if (frame.readUInt32BE(16) !== SIZE || frame.readUInt32BE(20) !== SIZE) fail(`${String(last.path)} is not ${SIZE} × ${SIZE}`);
  const trial = trials.find((each) => each.trial === condition.trial) ?? fail(`${RECORD_JSON} has no trial ${condition.trial}`);
  if (trial.model !== condition.model || trial.seed !== film.world.seed || trial.tick !== TICK) {
    fail(`${condition.trial} is ${trial.model} seed ${trial.seed} at tick ${trial.tick}, not this condition`);
  }
  if (end.digest !== trial.digest) fail(`${condition.id} reached ${String(end.digest)}; ${condition.trial} recorded ${trial.digest}`);
  if (condition.id === "confluent" && end.digest !== film.world.from.digest) fail(`the full world is not the film's tick ${TICK}`);
  if (last.live !== trial.live) fail(`${condition.id} has ${String(last.live)} alive; ${condition.trial} recorded ${trial.live}`);
  const validated = JSON.parse(run(flife, ["validate", join(dir, "config.toml")])) as {
    config_hash: string;
    model: string;
    seed: number;
    maximum_population: number;
  };
  if (validated.model !== condition.model) fail(`${condition.id} validates as ${validated.model}`);
  ceilings.add(validated.maximum_population);
  const png = String(last.path);
  const jpeg = join(dir, "still.jpg");
  run("ffmpeg", ["-y", "-loglevel", "error", "-i", png, ...JPEG, jpeg]);
  const hash = sha256(jpeg);
  // The film's world at the film's first tick, drawn and encoded as the poster was, is the poster byte for
  // byte; the page then serves (and the reader has cached) one file, not two.
  const poster = hash === film.poster.sha256;
  const name = `controls-${condition.id}-${hash.slice(0, 12)}.jpg`;
  if (!poster) copyFileSync(jpeg, join(media, name));
  return {
    id: condition.id,
    label: condition.label,
    model: condition.model,
    removed: condition.removed,
    rule: condition.rule,
    config_hash: validated.config_hash,
    src: poster ? film.poster.src : `/media/${name}`,
    poster,
    sha256: hash,
    bytes: readFileSync(jpeg).length,
    width: SIZE,
    height: SIZE,
    tick: TICK,
    live: last.live as number,
    digest: end.digest as string,
    controllers: (end.controllers as { backend?: string } | undefined)?.backend ?? "unknown",
    record: {
      trial: condition.trial,
      live: trial.live,
      digest: trial.digest,
      distance: trial.distance,
      by_eye: byEye(readme, condition.readme),
    },
  };
});

const end = records(join(work, "confluent", "render.ndjson")).find((record) => record.record === "end") as Record;
if (ceilings.size !== 1) fail(`the conditions' population ceilings differ: ${[...ceilings].join(", ")}`);
// The torus in world units (the preset's [world] width and height) and a body's diameter ([contact]).
const presetText = readFileSync(join(REPO, PRESET), "utf8");
const setting = (table: string, key: string) => {
  const section = new RegExp(`^\\[${table}\\]\\s*$([\\s\\S]*?)(?=^\\[)`, "m").exec(presetText)?.[1] ?? "";
  const value = Number(new RegExp(`^${key}\\s*=\\s*([0-9.eE+-]+)`, "m").exec(section)?.[1]);
  return Number.isFinite(value) && value > 0 ? value : fail(`${PRESET}'s [${table}] ${key} is not a positive number`);
};
const dimension = (key: string) => setting("world", key);
const dirty = run("git", ["status", "--porcelain", "--", "crates", "presets", "Cargo.toml", "Cargo.lock"])
  .split("\n")
  .filter((line) => line.trim() !== "")
  .map((line) => line.slice(3));
const manifest = {
  schema: "frankenlife.website-controls.v1",
  preset: PRESET,
  seed: film.world.seed,
  tick: TICK,
  change: "model",
  torus: [dimension("width"), dimension("height")],
  body_diameter: setting("contact", "diameter"),
  ceiling: [...ceilings][0] as number,
  render: {
    command: `flife render CONFIG ${RENDER.join(" ")} --out FRAMES`,
    jpeg: JPEG,
    mapping: end.mapping as string,
    meaning: end.meaning as string,
    look: end.look as string,
    field_mapping: "none",
  },
  record: {
    json: RECORD_JSON,
    readme: `${RECORD_README}, "### Controls"`,
    rule: "plan §5.4: a registered variant runs beside its two controls",
  },
  flife_sha256: sha256(flife),
  flife_version: run(flife, ["--version"]).trim(),
  revision: run("git", ["rev-parse", "HEAD"]).trim(),
  uncommitted: dirty,
  host: host(),
  date: new Date().toISOString().slice(0, 10),
  stills,
};
writeFileSync(join(WEB, "lib", "controls.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.error(
  `controls: ${stills.map((still) => `${still.id} ${still.live} alive, ${still.digest.slice(0, 8)}…, ${still.bytes} bytes`).join("; ")}`,
);
