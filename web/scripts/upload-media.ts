// Put each film's videos in R2, where worker.ts serves them, and check what
// the site serves.
//
//   bun scripts/upload-media.ts [--force] [--check] [--site https://frankenlife.org]
//
// Each film (lib/films/*.json) is one MP4, and perhaps a phone encode of it,
// each named by its SHA-256 and kept outside git in media-r2/ (scripts/media.ts
// writes them there). A video the site already serves at its full size is
// skipped; the others are checked byte for byte against their manifests and
// uploaded with wrangler to the R2 bucket that wrangler.jsonc binds as MEDIA,
// at the key `media/<name>`. `--force` uploads every video. `--check` uploads
// nothing: it asks the site for each video whole (a HEAD, whose ETag is R2's
// MD5 of the object) and for its first and last bytes, and compares them with
// media-r2/; a video with no local copy is checked for its size alone. Run it after `wrangler deploy`
// (`bun run deploy` does); it asks again for a minute, for an edge that is
// slow to answer or still runs the previous Worker.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, existsSync, openSync, readFileSync, readSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { films } from "../lib/media";
import { UPLOAD_LIMIT, checkFile, etagAgainstMd5, filmVideoFiles, siteOrigin } from "./media-verify";

const STORE = fileURLToPath(new URL("../media-r2", import.meta.url));
const WEB = fileURLToPath(new URL("..", import.meta.url));

function fail(message: string): never {
  console.error(`upload-media: ${message}`);
  process.exit(1);
}

/** The bucket wrangler.jsonc binds as MEDIA, the one worker.ts reads. */
function mediaBucket(): string {
  const config = readFileSync(`${WEB}/wrangler.jsonc`, "utf8");
  const binding = /\{\s*"binding"\s*:\s*"MEDIA"\s*,\s*"bucket_name"\s*:\s*"([^"]+)"/.exec(config);
  if (!binding?.[1]) fail("wrangler.jsonc binds no R2 bucket as MEDIA");
  return binding[1];
}
const BUCKET = mediaBucket();

const args = process.argv.slice(2);
const force = args.includes("--force");
const check = args.includes("--check");
const siteAt = args.indexOf("--site");
const siteArg = siteAt >= 0 ? args[siteAt + 1] : "https://frankenlife.org";
for (const [at, arg] of args.entries()) {
  if (!["--force", "--check", "--site"].includes(arg) && !(siteAt >= 0 && at === siteAt + 1)) {
    console.error(`upload-media: unknown argument ${arg}`);
    process.exit(2);
  }
}
const site = siteOrigin(siteArg) ?? fail(`--site needs an http(s) address, not ${JSON.stringify(siteArg ?? "")}`);

/** The bytes [start, start + length) of the local copy of `name`. */
function localBytes(name: string, start: number, length: number): Uint8Array {
  const fd = openSync(`${STORE}/${name}`, "r");
  const out = new Uint8Array(length);
  readSync(fd, out, 0, length, start);
  closeSync(fd);
  return out;
}

/** What the site answers for `name`: its status, Content-Length and ETag, from a HEAD; status 0 if unreachable. */
async function served(name: string): Promise<{ status: number; bytes: number; etag: string | null; error?: string }> {
  try {
    const response = await fetch(`${site}/media/${name}`, { method: "HEAD" });
    return {
      status: response.status,
      bytes: Number(response.headers.get("Content-Length") ?? Number.NaN),
      etag: response.headers.get("ETag"),
    };
  } catch (error) {
    return { status: 0, bytes: Number.NaN, etag: null, error: String(error) };
  }
}

/** How often, and how far apart, `--check` asks again: a new Worker takes up to a minute to reach every edge. */
const CHECK_TRIES = 7;
const CHECK_WAIT_MS = 10_000;

type Answer = { ok: boolean; said: string };

/** Ask until `ask` answers ok or the tries run out, saying why each time it asks again. */
async function patiently(what: string, ask: () => Promise<Answer>): Promise<Answer> {
  let answer = await ask();
  for (let attempt = 2; !answer.ok && attempt <= CHECK_TRIES; attempt += 1) {
    console.error(`upload-media: ${what}: ${answer.said}; asking again in ${CHECK_WAIT_MS / 1000} s`);
    await Bun.sleep(CHECK_WAIT_MS);
    answer = await ask();
  }
  console.error(`upload-media: ${answer.ok ? "ok" : "WRONG"} ${what}: ${answer.said}`);
  return answer;
}

/** The film whole: a 200 of its full size and, where media-r2/ holds it, R2's ETag equal to its MD5. */
async function wholeFilm(name: string, size: number, md5: string | null): Promise<Answer> {
  const answer = await served(name);
  if (answer.status === 0) return { ok: false, said: `unreachable (${answer.error})` };
  const etag = md5 === null ? "unknown" : etagAgainstMd5(answer.etag, md5);
  const ok = answer.status === 200 && answer.bytes === size && etag !== "different";
  const said =
    `${answer.status}, ${answer.bytes} bytes` +
    (etag === "same" ? ", R2's MD5 equal to media-r2/'s" : etag === "different" ? `, ETag ${answer.etag} is not media-r2/'s MD5 ${md5}` : "");
  return { ok, said };
}

/** Bytes [start, end] of a film: a 206 with that Content-Range and, where media-r2/ holds it, the same bytes. */
async function rangeOf(name: string, size: number, start: number, end: number, local: boolean): Promise<Answer> {
  let response: Response;
  try {
    response = await fetch(`${site}/media/${name}`, { headers: { Range: `bytes=${start}-${end}` } });
  } catch (error) {
    return { ok: false, said: `unreachable (${String(error)})` };
  }
  const range = response.headers.get("Content-Range");
  if (response.status !== 206 || range !== `bytes ${start}-${end}/${size}`) {
    // Not the range asked for: perhaps the whole film, so do not read it.
    await response.body?.cancel();
    return { ok: false, said: `${response.status} ${range ?? "no Content-Range"}` };
  }
  const body = new Uint8Array(await response.arrayBuffer());
  const same = !local || Buffer.from(body).equals(Buffer.from(localBytes(name, start, end - start + 1)));
  return { ok: same, said: `206 ${range}${local ? (same ? ", bytes as media-r2/ holds them" : ", bytes differ") : ""}` };
}

/** Every video the films serve: each film and its phone encode. */
const videos = films.flatMap(filmVideoFiles);

if (check) {
  let bad = 0;
  let unverified = 0;
  for (const { name, file } of videos) {
    const size = file.bytes;
    const local = existsSync(`${STORE}/${name}`);
    if (!local) unverified += 1;
    const md5 = local ? createHash("md5").update(readFileSync(`${STORE}/${name}`)).digest("hex") : null;
    if (!(await patiently(`${name} whole`, () => wholeFilm(name, size, md5))).ok) bad += 1;
    for (const [start, end] of [
      [0, 1],
      [Math.max(0, size - 4096), size - 1],
    ] as const) {
      if (!(await patiently(`${name} bytes ${start}-${end}`, () => rangeOf(name, size, start, end, local))).ok) bad += 1;
    }
  }
  if (bad > 0) fail(`${bad} answers were not the videos as their manifests and media-r2/ hold them`);
  console.error(
    `upload-media: ${site} serves all ${videos.length} videos of the ${films.length} films at their manifests' sizes` +
      (unverified > 0 ? `; ${unverified} with no copy in media-r2/ to compare bytes with` : ", each as media-r2/ holds it"),
  );
  process.exit(0);
}

let uploaded = 0;
for (const { name, file } of videos) {
  if (!force) {
    const answer = await served(name);
    if (answer.status === 200 && answer.bytes === file.bytes) {
      console.error(`upload-media: ${site} already serves ${name} (${answer.bytes} bytes)`);
      continue;
    }
  }
  const problem = checkFile(`${STORE}/${name}`, file);
  if (problem !== null) fail(`${problem}; render or copy it into media-r2/ first`);
  if (file.bytes > UPLOAD_LIMIT) {
    fail(`${name} is ${file.bytes} bytes, above wrangler's ${UPLOAD_LIMIT}-byte single-upload limit; encode at a higher --crf`);
  }
  console.error(`upload-media: uploading ${name} (${(file.bytes / 1048576).toFixed(1)} MiB) to ${BUCKET}`);
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
console.error(`upload-media: ${uploaded} of the ${videos.length} videos uploaded to ${BUCKET}`);
