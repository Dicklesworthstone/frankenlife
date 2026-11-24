// `bun test`: the Worker's byte serving, against an in-memory asset.
import { describe, expect, test } from "bun:test";

import { mediaWorker, parseRange } from "./worker";

const SIZE = 100_000;
const bytes = Uint8Array.from({ length: SIZE }, (_, index) => (index * 31 + 7) % 251);

const worker = mediaWorker(new Map([["/media/film.mp4", SIZE]]));

/**
 * The static-asset binding as workerd's behaves: the whole file and a 200,
 * whatever the Range, streamed with no Content-Length.
 */
const env = {
  ASSETS: {
    async fetch(request: Request): Promise<Response> {
      const path = new URL(request.url).pathname;
      // The static export's share images, which have no extension to type them by.
      if (path === "/opengraph-image") {
        return new Response("card", { headers: { "Content-Type": "application/octet-stream" } });
      }
      if (path !== "/media/film.mp4") return new Response("page", { status: path === "/" ? 200 : 404 });
      // Delivered in uneven chunks, as a stream would be.
      let offset = 0;
      const body = new ReadableStream<Uint8Array>({
        pull(controller) {
          if (offset >= SIZE) return controller.close();
          const next = Math.min(SIZE, offset + 7_919);
          controller.enqueue(bytes.slice(offset, next));
          offset = next;
        },
      });
      return new Response(body, { headers: { "Content-Type": "video/mp4" } });
    },
  },
};

const get = (range?: string, method = "GET") =>
  worker.fetch(
    new Request("https://site.test/media/film.mp4", { method, headers: range ? { Range: range } : {} }),
    env,
  );

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

describe("the media Worker", () => {
  test("answers Safari's first probe with a 206 of two bytes", async () => {
    const response = await get("bytes=0-1");
    expect(response.status).toBe(206);
    expect(response.headers.get("Content-Range")).toBe(`bytes 0-1/${SIZE}`);
    expect(response.headers.get("Content-Length")).toBe("2");
    expect(response.headers.get("Accept-Ranges")).toBe("bytes");
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes.slice(0, 2));
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
      expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes.slice(start, end + 1));
    }
  });

  test("serves the whole file with Accept-Ranges and an immutable cache when no range is asked", async () => {
    const response = await get();
    expect(response.status).toBe(200);
    expect(response.headers.get("Accept-Ranges")).toBe("bytes");
    expect(response.headers.get("Cache-Control")).toBe("public, max-age=31536000, immutable");
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes);
  });

  test("refuses an unsatisfiable range with 416 and the size", async () => {
    const response = await get(`bytes=${SIZE}-`);
    expect(response.status).toBe(416);
    expect(response.headers.get("Content-Range")).toBe(`bytes */${SIZE}`);
  });

  test("answers HEAD with the headers and no body", async () => {
    const response = await get("bytes=10-19", "HEAD");
    expect(response.status).toBe(206);
    expect(response.headers.get("Content-Length")).toBe("10");
    expect(response.body).toBeNull();
  });

  test("answers a HEAD without a range with the whole file's length", async () => {
    const response = await get(undefined, "HEAD");
    expect(response.status).toBe(200);
    expect(response.headers.get("Accept-Ranges")).toBe("bytes");
    expect(response.headers.get("Content-Length")).toBe(String(SIZE));
    expect(response.body).toBeNull();
  });

  test("serves a file it has no size for whole, still a 200", async () => {
    const unsized = mediaWorker(new Map());
    const response = await unsized.fetch(
      new Request("https://site.test/media/film.mp4", { headers: { Range: "bytes=0-1" } }),
      env,
    );
    expect(response.status).toBe(200);
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes);
  });

  test("sends www. to the apex, keeping the path and query", async () => {
    const response = await worker.fetch(new Request("https://www.frankenlife.org/media/x.mp4?t=1#top"), env);
    expect(response.status).toBe(301);
    expect(response.headers.get("Location")).toBe("https://frankenlife.org/media/x.mp4?t=1");
  });

  test("serves the share images as JPEG, and a missing one as the assets answer", async () => {
    const card = await worker.fetch(new Request("https://site.test/opengraph-image?0123abcd"), env);
    expect(card.status).toBe(200);
    expect(card.headers.get("Content-Type")).toBe("image/jpeg");
    expect(await card.text()).toBe("card");
    const missing = await worker.fetch(new Request("https://site.test/twitter-image"), env);
    expect(missing.status).toBe(404);
    expect(missing.headers.get("Content-Type")).not.toBe("image/jpeg");
  });

  test("passes everything outside /media/ to the assets", async () => {
    const page = await worker.fetch(new Request("https://site.test/"), env);
    expect(await page.text()).toBe("page");
    const missing = await worker.fetch(new Request("https://site.test/media/nothing.mp4", { headers: { Range: "bytes=0-1" } }), env);
    expect(missing.status).toBe(404);
  });
});
