// `bun test`: the Worker's byte serving of the films from R2, against an in-memory bucket.
import { describe, expect, test } from "bun:test";

import { mediaWorker, parseRange } from "./worker";

const SIZE = 100_000;
const bytes = Uint8Array.from({ length: SIZE }, (_, index) => (index * 31 + 7) % 251);
const KEY = "media/film-0123456789ab.mp4";
const PATH = `/${KEY}`;

const worker = mediaWorker(new Map([[PATH, SIZE]]));

/**
 * R2 as the Worker uses it: head() and get() with an optional { offset, length } range, and every call counted.
 * As R2 does, a range starting past the object's end throws, and one running past it is cut short.
 */
function bucket(objects: Record<string, Uint8Array>) {
  const calls: string[] = [];
  return {
    calls,
    async head(key: string) {
      calls.push(`head ${key}`);
      const object = objects[key];
      return object ? { size: object.length, httpEtag: `"etag-${key}"` } : null;
    },
    async get(key: string, options?: { range?: { offset: number; length: number } }) {
      calls.push(`get ${key}${options?.range ? ` ${options.range.offset}+${options.range.length}` : ""}`);
      const object = objects[key];
      if (!object) return null;
      if (options?.range && options.range.offset >= object.length) throw new Error("InvalidRange (10039)");
      const from = options?.range?.offset ?? 0;
      const to = options?.range ? from + options.range.length : object.length;
      const slice = object.slice(from, to);
      // Delivered in uneven chunks, as a stream would be.
      let offset = 0;
      const body = new ReadableStream<Uint8Array>({
        pull(controller) {
          if (offset >= slice.length) return controller.close();
          const next = Math.min(slice.length, offset + 7_919);
          controller.enqueue(slice.slice(offset, next));
          offset = next;
        },
      });
      return { size: object.length, httpEtag: `"etag-${key}"`, body };
    },
  };
}

const IMMUTABLE = "public, max-age=31536000, immutable";

/**
 * The static-asset binding: the page, a share image, and a 404 page for anything else. As public/_headers does,
 * everything under /media/ is marked immutable, the 404 page included.
 */
const ASSETS = {
  async fetch(request: Request): Promise<Response> {
    const path = new URL(request.url).pathname;
    const headers: Record<string, string> = path.startsWith("/media/") ? { "Cache-Control": IMMUTABLE } : {};
    // The static export's share images, which have no extension to type them by.
    if (path === "/opengraph-image") {
      return new Response("card", { headers: { "Content-Type": "application/octet-stream" } });
    }
    if (path === "/media/poster-0123456789ab.jpg") return new Response("jpeg", { headers: { ...headers, "Content-Type": "image/jpeg" } });
    return new Response(path === "/" ? "page" : "not found", { status: path === "/" ? 200 : 404, headers });
  },
};

const env = () => ({ ASSETS, MEDIA: bucket({ [KEY]: bytes }) });

const get = (range?: string, method = "GET", at = env()) =>
  worker.fetch(new Request(`https://site.test${PATH}`, { method, headers: range ? { Range: range } : {} }), at);

describe("parseRange", () => {
  test("reads one range of each form", () => {
    expect(parseRange("bytes=0-1", SIZE)).toEqual([0, 1]);
    expect(parseRange("bytes=500-", SIZE)).toEqual([500, SIZE - 1]);
    expect(parseRange("bytes=-500", SIZE)).toEqual([SIZE - 500, SIZE - 1]);
    expect(parseRange("bytes=99990-200000", SIZE)).toEqual([99990, SIZE - 1]);
    expect(parseRange("bytes=-200000", SIZE)).toEqual([0, SIZE - 1]);
  });
  test("refuses what no byte satisfies", () => {
    expect(parseRange(`bytes=${SIZE}-`, SIZE)).toBe("unsatisfiable");
    expect(parseRange("bytes=10-5", SIZE)).toBe("unsatisfiable");
    expect(parseRange("bytes=-0", SIZE)).toBe("unsatisfiable");
  });
  test("serves several ranges or another unit whole", () => {
    expect(parseRange("bytes=0-1,5-9", SIZE)).toBeNull();
    expect(parseRange("items=0-1", SIZE)).toBeNull();
    expect(parseRange("bytes=-", SIZE)).toBeNull();
  });
  test("reads the unit in any case, around whitespace, and one byte at either end", () => {
    expect(parseRange("Bytes=0-1", SIZE)).toEqual([0, 1]);
    expect(parseRange("BYTES=-500", SIZE)).toEqual([SIZE - 500, SIZE - 1]);
    expect(parseRange("  bytes=0-1 ", SIZE)).toEqual([0, 1]);
    expect(parseRange("bytes=0-0", SIZE)).toEqual([0, 0]);
    expect(parseRange(`bytes=${SIZE - 1}-`, SIZE)).toEqual([SIZE - 1, SIZE - 1]);
    expect(parseRange("bytes=0-99999999999999999999999", SIZE)).toEqual([0, SIZE - 1]);
    expect(parseRange("bytes=0-", 0)).toBe("unsatisfiable");
    expect(parseRange("bytes=-1", 0)).toBe("unsatisfiable");
  });
});

describe("the films from R2", () => {
  test("answers Safari's first probe with a 206 of two bytes, read from R2 as two bytes", async () => {
    const at = env();
    const response = await get("bytes=0-1", "GET", at);
    expect(response.status).toBe(206);
    expect(response.headers.get("Content-Range")).toBe(`bytes 0-1/${SIZE}`);
    expect(response.headers.get("Content-Length")).toBe("2");
    expect(response.headers.get("Accept-Ranges")).toBe("bytes");
    expect(response.headers.get("Content-Type")).toBe("video/mp4");
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes.slice(0, 2));
    // A film the manifest sizes costs one R2 read, of only the bytes asked for.
    expect(at.MEDIA.calls).toEqual([`get ${KEY} 0+2`]);
  });

  test("returns exactly the bytes asked for, across chunk boundaries", async () => {
    for (const [start, end] of [
      [7_918, 7_920],
      [12_345, 67_890],
      [99_999, 99_999],
      [0, SIZE - 1],
    ] as const) {
      const response = await get(`bytes=${start}-${end}`);
      expect(response.status).toBe(206);
      expect(response.headers.get("Content-Length")).toBe(String(end - start + 1));
      expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes.slice(start, end + 1));
    }
    const tail = await get("bytes=-500");
    expect(tail.headers.get("Content-Range")).toBe(`bytes ${SIZE - 500}-${SIZE - 1}/${SIZE}`);
    expect(new Uint8Array(await tail.arrayBuffer())).toEqual(bytes.slice(SIZE - 500));
  });

  test("serves the whole film with Accept-Ranges, its ETag and an immutable cache when no range is asked", async () => {
    const response = await get();
    expect(response.status).toBe(200);
    expect(response.headers.get("Accept-Ranges")).toBe("bytes");
    expect(response.headers.get("Content-Length")).toBe(String(SIZE));
    expect(response.headers.get("ETag")).toBe(`"etag-${KEY}"`);
    expect(response.headers.get("Cache-Control")).toBe("public, max-age=31536000, immutable");
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes);
  });

  test("refuses an unsatisfiable range with an uncached 416 and the size, without reading R2", async () => {
    const at = env();
    const response = await get(`bytes=${SIZE}-`, "GET", at);
    expect(response.status).toBe(416);
    expect(response.headers.get("Content-Range")).toBe(`bytes */${SIZE}`);
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    expect(response.headers.get("Content-Type")).toBeNull();
    expect(at.MEDIA.calls).toEqual([]);
  });

  test("answers HEAD from R2's record of the whole film, with no body, a Range or not (RFC 9110 §14.2)", async () => {
    const at = env();
    for (const range of ["bytes=10-19", `bytes=${SIZE}-`, undefined]) {
      const response = await get(range, "HEAD", at);
      expect(response.status).toBe(200);
      expect(response.headers.get("Content-Length")).toBe(String(SIZE));
      expect(response.headers.get("Content-Range")).toBeNull();
      expect(response.headers.get("ETag")).toBe(`"etag-${KEY}"`);
      expect(response.body).toBeNull();
    }
    expect(at.MEDIA.calls).toEqual([`head ${KEY}`, `head ${KEY}`, `head ${KEY}`]);
  });

  test("refuses with an uncached 503 when R2 and the manifest disagree on the size, or R2 fails", async () => {
    // The manifest says one byte more than R2 holds: a range past R2's end, a whole GET and a HEAD.
    const oversized = mediaWorker(new Map([[PATH, SIZE + 1]]));
    for (const [method, range] of [
      ["GET", `bytes=${SIZE}-`],
      ["GET", undefined],
      ["HEAD", undefined],
    ] as const) {
      const response = await oversized.fetch(
        new Request(`https://site.test${PATH}`, { method, headers: range ? { Range: range } : {} }),
        env(),
      );
      expect(response.status).toBe(503);
      expect(response.headers.get("Cache-Control")).toBe("no-store");
    }
    const broken = { ...env(), MEDIA: { ...bucket({}), get: async () => Promise.reject(new Error("R2 down")) } };
    const failed = await worker.fetch(new Request(`https://site.test${PATH}`), broken);
    expect(failed.status).toBe(503);
    expect(failed.headers.get("Retry-After")).toBe("30");
  });

  test("measures in R2 a film no manifest sizes, and serves its range", async () => {
    const unsized = mediaWorker(new Map());
    const at = env();
    const response = await unsized.fetch(new Request(`https://site.test${PATH}`, { headers: { Range: "bytes=5-9" } }), at);
    expect(response.status).toBe(206);
    expect(response.headers.get("Content-Range")).toBe(`bytes 5-9/${SIZE}`);
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes.slice(5, 10));
    expect(at.MEDIA.calls).toEqual([`head ${KEY}`, `get ${KEY} 5+5`]);
  });

  test("answers a film R2 does not hold with the site's 404 page, never cached (it may be uploaded next)", async () => {
    for (const method of ["GET", "HEAD"]) {
      const response = await worker.fetch(
        new Request("https://site.test/media/nothing-0123456789ab.mp4", { method, headers: { Range: "bytes=0-1" } }),
        env(),
      );
      expect(response.status).toBe(404);
      expect(response.headers.get("Cache-Control")).toBe("no-store");
      if (method === "GET") expect(await response.text()).toBe("not found");
    }
  });
});

describe("the rest of the site", () => {
  test("sends www. to the apex, keeping the path and query", async () => {
    const response = await worker.fetch(new Request("https://www.frankenlife.org/media/x.mp4?t=1#top"), env());
    expect(response.status).toBe(301);
    expect(response.headers.get("Location")).toBe("https://frankenlife.org/media/x.mp4?t=1");
  });

  test("serves the share images as JPEG, and a missing one as the assets answer", async () => {
    const card = await worker.fetch(new Request("https://site.test/opengraph-image?0123abcd"), env());
    expect(card.status).toBe(200);
    expect(card.headers.get("Content-Type")).toBe("image/jpeg");
    expect(await card.text()).toBe("card");
    const missing = await worker.fetch(new Request("https://site.test/twitter-image"), env());
    expect(missing.status).toBe(404);
    expect(missing.headers.get("Content-Type")).not.toBe("image/jpeg");
  });

  test("passes the page and the static media to the assets, never to R2", async () => {
    const at = env();
    const page = await worker.fetch(new Request("https://site.test/"), at);
    expect(await page.text()).toBe("page");
    const poster = await worker.fetch(new Request("https://site.test/media/poster-0123456789ab.jpg"), at);
    expect(await poster.text()).toBe("jpeg");
    // A path that only looks like a film (a subdirectory) is not one.
    const nested = await worker.fetch(new Request("https://site.test/media/a/b.mp4"), at);
    expect(nested.status).toBe(404);
    expect(at.MEDIA.calls).toEqual([]);
  });
});
