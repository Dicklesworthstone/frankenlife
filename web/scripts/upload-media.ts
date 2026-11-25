// Put each film's video in R2, where worker.ts serves it, and check what the
// site serves.
//
//   bun scripts/upload-media.ts [--force] [--check] [--site https://frankenlife.org]
//
// Each film (lib/films/*.json) is one MP4 named by its SHA-256, kept outside
// git in media-r2/ (scripts/media.ts writes it there). A film the site already
// serves at its full size is skipped; the others are checked byte for byte
// against their manifests and uploaded with wrangler to the bucket
// frankenlife-media, at the key `media/<id>-<hash>.mp4`. `--force` uploads
// every film. `--check` uploads nothing: it asks the site for each film's
// first and last bytes and compares them with media-r2/ (run it after
// `wrangler deploy`; `bun run deploy` does).

import { spawnSync } from "node:child_process";
import { openSync, readSync, closeSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { films } from "../lib/media";
import { verifyFilmVideos, videoName } from "./media-verify";

const BUCKET = "frankenlife-media";
const STORE = fileURLToPath(new URL("../media-r2", import.meta.url));
const WEB = fileURLToPath(new URL("..", import.meta.url));

const args = process.argv.slice(2);
const force = args.includes("--force");
const check = args.includes("--check");
const siteAt = args.indexOf("--site");
const site = siteAt >= 0 ? (args[siteAt + 1] ?? "") : "https://frankenlife.org";
for (const arg of args) {
  if (!["--force", "--check", "--site", site].includes(arg)) {
    console.error(`upload-media: unknown argument ${arg}`);
    process.exit(2);
  }
}

function fail(message: string): never {
  console.error(`upload-media: ${message}`);
  process.exit(1);
}

/** The bytes [start, start + length) of the local copy of `name`. */
function localBytes(name: string, start: number, length: number): Uint8Array {
  const fd = openSync(`${STORE}/${name}`, "r");
  const out = new Uint8Array(length);
  readSync(fd, out, 0, length, start);
  closeSync(fd);
  return out;
}

/** What the site answers for `name`: its status and Content-Length, from a HEAD. */
async function served(name: string): Promise<{ status: number; bytes: number }> {
  const response = await fetch(`${site}/media/${name}`, { method: "HEAD" });
  return { status: response.status, bytes: Number(response.headers.get("Content-Length") ?? Number.NaN) };
}

/** How often, and how far apart, `--check` asks again: a new Worker takes up to a minute to reach every edge. */
const CHECK_TRIES = 7;
const CHECK_WAIT_MS = 10_000;

/** Whether the site serves bytes [start, end] of `name` as media-r2/ holds them, and what it said. */
async function probe(name: string, size: number, start: number, end: number): Promise<{ ok: boolean; said: string }> {
  const response = await fetch(`${site}/media/${name}`, { headers: { Range: `bytes=${start}-${end}` } });
  const body = new Uint8Array(await response.arrayBuffer());
  const same = Buffer.from(body).equals(Buffer.from(localBytes(name, start, end - start + 1)));
  const range = response.headers.get("Content-Range");
  const ok = response.status === 206 && range === `bytes ${start}-${end}/${size}` && same;
  return { ok, said: `${response.status} ${range ?? "no Content-Range"}${same ? "" : ", bytes differ"}` };
}

if (check) {
  let bad = 0;
  for (const film of films) {
    const name = videoName(film);
    const size = film.video.bytes;
    const probes: [number, number][] = [
      [0, 1],
      [size - 4096, size - 1],
    ];
    for (const [start, end] of probes) {
      let answer = await probe(name, size, start, end);
      for (let attempt = 2; !answer.ok && attempt <= CHECK_TRIES; attempt += 1) {
        console.error(`upload-media: ${name} bytes ${start}-${end}: ${answer.said}; asking again in ${CHECK_WAIT_MS / 1000} s`);
        await Bun.sleep(CHECK_WAIT_MS);
        answer = await probe(name, size, start, end);
      }
      if (!answer.ok) bad += 1;
      console.error(`upload-media: ${answer.ok ? "ok" : "WRONG"} ${name} bytes ${start}-${end}: ${answer.said}`);
    }
  }
  if (bad > 0) fail(`${bad} ranges were not served as media-r2/ holds them`);
  console.error(`upload-media: ${site} serves all ${films.length} films, first and last bytes as media-r2/ holds them`);
  process.exit(0);
}

let uploaded = 0;
for (const film of films) {
  const name = videoName(film);
  if (!force) {
    const answer = await served(name);
    if (answer.status === 200 && answer.bytes === film.video.bytes) {
      console.error(`upload-media: ${site} already serves ${name} (${answer.bytes} bytes)`);
      continue;
    }
  }
  const problems = verifyFilmVideos([film], STORE);
  if (problems.length > 0) fail(`${problems.join("; ")}; render or copy it into media-r2/ first`);
  console.error(`upload-media: uploading ${name} (${(film.video.bytes / 1048576).toFixed(1)} MiB) to ${BUCKET}`);
  const put = spawnSync(
    "bunx",
    [
      "wrangler",
      "r2",
      "object",
      "put",
      `${BUCKET}/media/${name}`,
      "--file",
      `${STORE}/${name}`,
      "--remote",
      "--content-type",
      "video/mp4",
      "--cache-control",
      "public, max-age=31536000, immutable",
    ],
    { cwd: WEB, stdio: "inherit" },
  );
  if (put.status !== 0) fail(`wrangler r2 object put exited with ${put.status}`);
  uploaded += 1;
}
console.error(`upload-media: ${uploaded} of ${films.length} films uploaded to ${BUCKET}`);
