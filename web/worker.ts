// The Worker in front of the site (wrangler.jsonc's `run_worker_first`). It
// sends www.frankenlife.org to frankenlife.org, serves the films from R2 with
// byte ranges, and hands everything else to the static assets unchanged.
//
// Each film is one MP4 in the R2 bucket bound as MEDIA, at the key its path
// names without the leading slash (`media/<id>-<hash>.mp4`), put there by
// scripts/upload-media.ts. Safari plays no video from a server that answers a
// Range request with the whole file: it asks for `bytes=0-1` first and gives
// up without a 206. So the Worker answers ranges itself, as RFC 9110 byte
// serving, reading only the bytes asked for from R2. A film's size comes from
// its manifest (lib/films/*.json), or from R2 for a file no manifest names
// (an older film a cached page may still ask for); where both are known and
// differ, the Worker answers 503 rather than send wrong ranges. A HEAD is
// answered from R2's own record. Nothing but a film's bytes is cached: a 404,
// a 416 or a 503 is `no-store`.
//
// The posters, cards and stills are static assets in public/media/, served as
// they are (public/_headers marks them immutable).

import { filmVideos } from "./lib/media";
import { SHARE_IMAGE_PATHS, SHARE_IMAGE_TYPE } from "./lib/share";

/** What the Worker uses of an R2 object (workerd's R2Object and R2ObjectBody). */
type R2Object = { size: number; httpEtag: string };
type R2ObjectBody = R2Object & { body: ReadableStream<Uint8Array> };
type R2Range = { offset: number; length: number };
type R2Bucket = {
  head(key: string): Promise<R2Object | null>;
  get(key: string, options?: { range?: R2Range }): Promise<R2ObjectBody | null>;
};
type Env = { ASSETS: { fetch(request: Request): Promise<Response> }; MEDIA: R2Bucket };

const IMMUTABLE = "public, max-age=31536000, immutable";

/** A film's path: one MP4 directly under /media/, named by letters, digits, dots, dashes and underscores. */
const FILM_PATH = /^\/media\/[A-Za-z0-9._-]+\.mp4$/;

// workerd's stream of a declared length: a Response from a plain stream is sent
// chunked, without the Content-Length a player expects of a 206. Absent elsewhere.
const FixedLength = (globalThis as { FixedLengthStream?: new (length: number) => TransformStream<Uint8Array, Uint8Array> })
  .FixedLengthStream;

/** `stream`, declared `length` bytes long where the runtime can say so. */
function sized(stream: ReadableStream<Uint8Array>, length: number): ReadableStream<Uint8Array> {
  if (!FixedLength) return stream;
  const { readable, writable } = new FixedLength(length);
  // Not awaited: the copy runs as the reader drains `readable`.
  stream.pipeTo(writable).catch(() => {});
  return readable;
}

/** One `bytes=` range of a `size`-byte file as [start, end], `null` to serve it whole, or "unsatisfiable". */
export function parseRange(header: string, size: number): [number, number] | null | "unsatisfiable" {
  // Range units are case-insensitive (RFC 9110 §14.1).
  const match = /^bytes=(\d*)-(\d*)$/i.exec(header.trim());
  // Several ranges, or another unit: a server may answer with the whole file.
  if (!match) return null;
  const [, from = "", to = ""] = match;
  if (from === "" && to === "") return null;
  if (from === "") {
    const suffix = Number(to);
    if (suffix === 0 || size === 0) return "unsatisfiable";
    return [Math.max(0, size - suffix), size - 1];
  }
  const start = Number(from);
  const end = to === "" ? size - 1 : Math.min(Number(to), size - 1);
  if (start >= size || start > end) return "unsatisfiable";
  return [start, end];
}

/** Each film's size in bytes, by its path on the site. */
const SIZES = new Map(filmVideos.map((file) => [file.src, file.bytes]));

/** A Worker serving the films with these sizes; a film it has no size for is measured in R2. */
export function mediaWorker(sizes: ReadonlyMap<string, number>) {
  return { fetch: (request: Request, env: Env) => serve(request, env, sizes) };
}

export default mediaWorker(SIZES);

/** The site's one address; a request to www. is sent there. */
const APEX = "frankenlife.org";

async function serve(request: Request, env: Env, sizes: ReadonlyMap<string, number>): Promise<Response> {
  const url = new URL(request.url);
  if (url.hostname === `www.${APEX}`) {
    url.hostname = APEX;
    url.hash = "";
    return Response.redirect(url.toString(), 301);
  }
  // The share images have no extension in the static export, so the assets cannot tell their type.
  if (SHARE_IMAGE_PATHS.includes(url.pathname)) {
    const image = await env.ASSETS.fetch(request);
    if (!image.ok) return image;
    const headers = new Headers(image.headers);
    headers.set("Content-Type", SHARE_IMAGE_TYPE);
    return new Response(image.body, { status: image.status, headers });
  }
  if (FILM_PATH.test(url.pathname) && (request.method === "GET" || request.method === "HEAD")) {
    return film(request, env, url.pathname, sizes.get(url.pathname));
  }
  return env.ASSETS.fetch(request);
}

/** A film from R2: whole, or the one range asked for; a film R2 does not hold is the site's 404 page. */
async function film(request: Request, env: Env, path: string, known: number | undefined): Promise<Response> {
  try {
    return await filmFromR2(request, env, path, known);
  } catch {
    // R2 refused or failed: say so, briefly and uncached, rather than Cloudflare's own error page.
    return unavailable();
  }
}

async function filmFromR2(request: Request, env: Env, path: string, known: number | undefined): Promise<Response> {
  const key = path.slice(1);
  const head = request.method === "HEAD";
  let size = known;
  let etag: string | null = null;
  if (head || size === undefined) {
    const stored = await env.MEDIA.head(key);
    if (stored === null) return notFound(request, env);
    // A manifest and R2 that disagree on a film's size would send wrong ranges: refuse rather than guess.
    if (size !== undefined && stored.size !== size) return unavailable();
    size = stored.size;
    etag = stored.httpEtag;
  }
  const headers = new Headers({
    "Accept-Ranges": "bytes",
    "X-Content-Type-Options": "nosniff",
  });
  // RFC 9110 §14.2: Range applies to GET alone; a HEAD describes the whole film.
  const range = head ? null : request.headers.get("Range");
  const wanted = range === null ? null : parseRange(range, size);
  if (wanted === "unsatisfiable") {
    headers.set("Content-Range", `bytes */${size}`);
    headers.set("Cache-Control", "no-store");
    return new Response(null, { status: 416, headers });
  }
  headers.set("Content-Type", "video/mp4");
  // The films are named by their SHA-256 (scripts/media.ts), so they never change in place.
  headers.set("Cache-Control", IMMUTABLE);
  const [start, end] = wanted ?? [0, size - 1];
  const length = end - start + 1;
  if (wanted !== null) headers.set("Content-Range", `bytes ${start}-${end}/${size}`);
  headers.set("Content-Length", String(length));
  const status = wanted === null ? 200 : 206;
  if (head) {
    if (etag !== null) headers.set("ETag", etag);
    return new Response(null, { status, headers });
  }
  const object = await env.MEDIA.get(key, wanted === null ? undefined : { range: { offset: start, length } });
  if (object === null) return notFound(request, env);
  if (object.size !== size) {
    await object.body.cancel();
    return unavailable();
  }
  headers.set("ETag", object.httpEtag);
  return new Response(sized(object.body, length), { status, headers });
}

/**
 * The static export's answer, its 404 page, for a path that names no film in R2: never cached, since the film
 * may be uploaded a moment later (public/_headers would otherwise mark it immutable, as it does /media/*).
 */
async function notFound(request: Request, env: Env): Promise<Response> {
  const page = await env.ASSETS.fetch(request);
  const headers = new Headers(page.headers);
  headers.set("Cache-Control", "no-store");
  return new Response(page.body, { status: page.status, statusText: page.statusText, headers });
}

/** A film R2 cannot serve as its manifest says, for now: a 503, uncached, worth asking again shortly. */
function unavailable(): Response {
  return new Response("This film is unavailable for a moment.\n", {
    status: 503,
    headers: { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store", "Retry-After": "30" },
  });
}
