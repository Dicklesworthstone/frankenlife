// FL-056 (plan §20.3): browser capability and artifact-size qualification for
// FrankenLife's recorded-world web presentation (`web/`).
//
// Enforces every concrete browser-delivery and artifact-size budget directly
// against the committed manifests (`lib/films/*.json`, `lib/controls.json`,
// `lib/stills.json`) and local media directories (`public/media/`, `media-r2/`):
// 1. Static asset ceiling: every static file is <= Cloudflare's 25 MiB limit
//    with verified SHA-256, and total `public/media/` payload stays lean.
// 2. First-paint poster budget: every film ships mobile (<512 KiB) and desktop
//    (<1 MiB) AVIF posters at least 3x smaller than its JPEG fallback.
// 3. R2 video streaming & mobile H.264 capability: every desktop 4K60 and
//    mobile 1080p encode fits single-PUT R2 limits (<300 MiB), encodes
//    `faststart` + 1-second keyframes for HTTP 206 Range seeking, and pairs a
//    strictly smaller mobile encode (`<= 1080p`, `<70%` of desktop bytes) so
//    phones decode H.264 Level 4.2 instead of 4K60 Level 5.2.
// 4. Plan §20.3 honesty & provenance: every film binds per-frame ticks/live
//    counts and world digests to `flife render` provenance, and confirms the
//    native 400k F16 weight bank (~7.99 GB) exceeds 32-bit WASM (`4 GiB`).

import { describe, expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

import { collectStaticMedia, controls, films, stills } from "../lib/media";
import { ASSET_LIMIT } from "./media-options";
import { UPLOAD_LIMIT, verifyFilmVideos, verifyMediaDirectory } from "./media-verify";

const PUBLIC_DIR = fileURLToPath(new URL("../public", import.meta.url));
const MEDIA_R2_DIR = fileURLToPath(new URL("../media-r2", import.meta.url));

const MAX_MOBILE_AVIF_BYTES = 512 * 1024;
const MAX_DESKTOP_AVIF_BYTES = 1024 * 1024;
const MAX_TOTAL_STATIC_MEDIA_BYTES = 32 * 1024 * 1024;
const WASM32_ADDRESS_SPACE_BYTES = 4 * 1024 * 1024 * 1024;

describe("FL-056 browser capability and artifact-size qualification", () => {
  test("every static media artifact satisfies byte-for-byte SHA-256 and Cloudflare 25 MiB asset caps", () => {
    expect(films.length).toBeGreaterThanOrEqual(3);
    expect(verifyMediaDirectory(films, [controls, stills], PUBLIC_DIR, ASSET_LIMIT)).toEqual([]);

    const staticFiles = collectStaticMedia(films, [controls, stills]);
    expect(staticFiles.length).toBeGreaterThan(0);

    let totalStaticBytes = 0;
    for (const file of staticFiles) {
      expect(file.bytes).toBeGreaterThan(0);
      expect(file.bytes).toBeLessThanOrEqual(ASSET_LIMIT);
      totalStaticBytes += file.bytes;
    }
    expect(totalStaticBytes).toBeLessThanOrEqual(MAX_TOTAL_STATIC_MEDIA_BYTES);
  });

  test("every recorded film ships compact mobile/desktop AVIF posters ahead of its JPEG fallback", () => {
    for (const manifest of films) {
      expect(manifest.poster.avif).toBeDefined();
      expect(manifest.poster.small).toBeDefined();
      const desktopAvif = manifest.poster.avif!;
      const mobileAvif = manifest.poster.small!;

      expect(mobileAvif.bytes).toBeLessThan(MAX_MOBILE_AVIF_BYTES);
      expect(desktopAvif.bytes).toBeLessThan(MAX_DESKTOP_AVIF_BYTES);
      expect(mobileAvif.bytes).toBeLessThan(desktopAvif.bytes);
      // AVIF first paint must cut bytes by at least 3x vs the 2160^2 fallback JPEG.
      expect(desktopAvif.bytes * 3).toBeLessThan(manifest.poster.bytes);
      expect(mobileAvif.width).toBeLessThanOrEqual(1080);
      expect(mobileAvif.height).toBeLessThanOrEqual(1080);
    }
  });

  test("every R2-streamed film satisfies single-PUT size, Range-seek encoding, and 1080p mobile decode budgets", () => {
    expect(verifyFilmVideos(films, MEDIA_R2_DIR, true)).toEqual([]);

    for (const manifest of films) {
      expect(manifest.video.bytes).toBeGreaterThan(0);
      expect(manifest.video.bytes).toBeLessThanOrEqual(UPLOAD_LIMIT);
      expect(manifest.video.encode).toContain("+faststart");
      expect(manifest.video.encode).toContain("yuv420p");

      expect(manifest.video.small).toBeDefined();
      const phone = manifest.video.small!;
      expect(phone.bytes).toBeGreaterThan(0);
      expect(phone.bytes).toBeLessThanOrEqual(UPLOAD_LIMIT);
      expect(phone.width).toBeLessThanOrEqual(1920);
      expect(phone.height).toBeLessThanOrEqual(1080);
      expect(phone.bytes).toBeLessThan(manifest.video.bytes * 0.7);
      expect(phone.encode).toContain("+faststart");
      expect(phone.encode).toContain("yuv420p");
    }
  });

  test("every recorded film carries complete simulation provenance and respects plan §20.3 WASM32 limits", () => {
    for (const manifest of films) {
      expect(manifest.world.config_hash).toMatch(/^[0-9a-f]{64}$/);
      expect(manifest.world.from.digest).toMatch(/^[0-9a-f]{64}$/);
      expect(manifest.world.to.digest).toMatch(/^[0-9a-f]{64}$/);
      expect(manifest.render.flife_sha256).toMatch(/^[0-9a-f]{64}$/);
      expect(manifest.render.revision.length).toBeGreaterThanOrEqual(7);
      expect(manifest.world.ticks.length).toBe(manifest.video.frames);
      expect(manifest.world.live.length).toBe(manifest.video.frames);
      expect(manifest.world.ticks[0]).toBe(manifest.world.from.tick);
      expect(manifest.world.ticks[manifest.world.ticks.length - 1]).toBe(manifest.world.to.tick);
    }

    // Plan §20.3: 400,000 independent F16 controllers (9,988 params each) need
    // 7.99 GB of weights alone, exceeding 32-bit WebAssembly's 4 GiB space.
    const native400kF16WeightBytes = 400_000 * 9_988 * 2;
    expect(native400kF16WeightBytes).toBeGreaterThan(WASM32_ADDRESS_SPACE_BYTES);
  });
});
