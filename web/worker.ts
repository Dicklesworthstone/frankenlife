// The Worker in front of the site (wrangler.jsonc's `run_worker_first`). It
// sends www.frankenlife.org to frankenlife.org, serves the media's byte
// ranges, and hands everything else to the static assets unchanged.
//
// Workers static assets answer a Range request with the whole file and a 200,
// and Safari will not play a video from a server that does that: it asks for
// `bytes=0-1` first and gives up without a 206. So for the media this Worker
// fetches the asset whole and streams back only the bytes asked for, as RFC
// 9110 byte serving. Everything else is the static export, served as it is.
//
// An asset's Response from the binding carries no Content-Length, so each
// file's size comes from its manifest (lib/films/*.json, lib/controls.json,
// lib/stills.json),
// which `bun run build` has checked against the file byte for byte
// (scripts/check-media.ts).

import { mediaFiles } from "./lib/media";
import { SHARE_IMAGE_PATHS, SHARE_IMAGE_TYPE } from "./lib/share";

type Env = { ASSETS: { fetch(request: Request): Promise<Response> } };

const IMMUTABLE = "public, max-age=31536000, immutable";

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

/** The bytes [start, end] of `body`, read through in order and dropped outside the range. */
function slice(body: ReadableStream<Uint8Array>, start: number, end: number): ReadableStream<Uint8Array> {
  const reader = body.getReader();
  let offset = 0;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) {
          controller.close();
          return;
        }
        const first = offset;
        offset += value.byteLength;
        // An empty chunk enqueues nothing, and a pull that enqueues nothing is not called again: read on.
        if (offset <= start || value.byteLength === 0) continue;
        const from = Math.max(0, start - first);
        const to = Math.min(value.byteLength, end + 1 - first);
        if (to > from) controller.enqueue(value.subarray(from, to));
        if (offset > end) {
          controller.close();
          await reader.cancel();
        }
        return;
      }
    },
    cancel(reason) {
      return reader.cancel(reason);
    },
  });
}

/** One `bytes=` range of a `size`-byte file as [start, end], `null` to serve it whole, or "unsatisfiable". */
export function parseRange(header: string, size: number): [number, number] | null | "unsatisfiable" {
  const match = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
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

/** Each medium's size in bytes, by its path on the site. */
const SIZES = new Map(mediaFiles.map((file) => [file.src, file.bytes]));

/** A Worker serving the media with these sizes; a file it has no size for is served whole. */
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
  if (!url.pathname.startsWith("/media/") || (request.method !== "GET" && request.method !== "HEAD")) {
    return env.ASSETS.fetch(request);
  }
  const headers = new Headers(request.headers);
  const range = headers.get("Range");
  headers.delete("Range");
  headers.delete("If-Range");
  const whole = await env.ASSETS.fetch(new Request(url, { method: "GET", headers }));
  const size = sizes.get(url.pathname) ?? Number.NaN;
  const out = new Headers(whole.headers);
  out.set("Accept-Ranges", "bytes");
  // The media are named by their SHA-256 (scripts/media.ts), so they never change in place.
  if (whole.ok) out.set("Cache-Control", IMMUTABLE);

  const head = request.method === "HEAD";
  const plain = !whole.ok || !whole.body || !Number.isFinite(size) || out.has("Content-Encoding");
  const wanted = range === null || plain ? null : parseRange(range, size);
  if (wanted === null) {
    if (head) await whole.body?.cancel();
    return new Response(head ? null : whole.body, { status: whole.status, headers: out });
  }
  if (wanted === "unsatisfiable") {
    await whole.body?.cancel();
    out.set("Content-Range", `bytes */${size}`);
    out.delete("Content-Length");
    return new Response(null, { status: 416, headers: out });
  }
  const [start, end] = wanted;
  out.set("Content-Range", `bytes ${start}-${end}/${size}`);
  out.set("Content-Length", String(end - start + 1));
  if (head) {
    await whole.body?.cancel();
    return new Response(null, { status: 206, headers: out });
  }
  const part = sized(slice(whole.body as ReadableStream<Uint8Array>, start, end), end - start + 1);
  return new Response(part, { status: 206, headers: out });
}
