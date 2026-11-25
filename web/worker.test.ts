// `bun test`: the Worker's byte serving of the films from R2, against an in-memory bucket.
import { describe, expect, test } from "bun:test";

import { mediaWorker, parseRange } from "./worker";

const SIZE = 100_000;
const bytes = Uint8Array.from({ length: SIZE }, (_, index) => (index * 31 + 7) % 251);
const KEY = "media/film-0123456789ab.mp4";
const PATH = `/${KEY}`;

const worker = mediaWorker(new Map([[PATH, SIZE]]));

/** R2 as the Worker uses it: head() and get() with an optional { offset, length } range, and every call counted. */
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

/** The static-asset binding: the page, a share image, and a 404 page for anything else. */
const ASSETS = {
  async fetch(request: Request): Promise<Response> {
    const path = new URL(request.url).pathname;
    // The static export's share images, which have no extension to type them by.
    if (path === "/opengraph-image") {
      return new Response("card", { headers: { "Content-Type": "application/octet-stream" } });
    }
    if (path === "/media/poster-0123456789ab.jpg") return new Response("jpeg", { headers: { "Content-Type": "image/jpeg" } });
    return new Response(path === "/" ? "page" : "not found", { status: path === "/" ? 200 : 404 });
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

  test("refuses an unsatisfiable range with 416 and the size, without reading R2", async () => {
    const at = env();
    const response = await get(`bytes=${SIZE}-`, "GET", at);
    expect(response.status).toBe(416);
    expect(response.headers.get("Content-Range")).toBe(`bytes */${SIZE}`);
    expect(at.MEDIA.calls).toEqual([]);
  });

  test("answers HEAD from R2's record of the film, with no body", async () => {
    const at = env();
    const ranged = await get("bytes=10-19", "HEAD", at);
    expect(ranged.status).toBe(206);
    expect(ranged.headers.get("Content-Length")).toBe("10");
    expect(ranged.body).toBeNull();
    const whole = await get(undefined, "HEAD", at);
    expect(whole.status).toBe(200);
    expect(whole.headers.get("Content-Length")).toBe(String(SIZE));
    expect(whole.headers.get("ETag")).toBe(`"etag-${KEY}"`);
    expect(whole.body).toBeNull();
    expect(at.MEDIA.calls).toEqual([`head ${KEY}`, `head ${KEY}`]);
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

  test("answers a film R2 does not hold with the site's 404 page", async () => {
    const response = await worker.fetch(
      new Request("https://site.test/media/nothing-0123456789ab.mp4", { headers: { Range: "bytes=0-1" } }),
      env(),
    );
    expect(response.status).toBe(404);
    expect(await response.text()).toBe("not found");
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
