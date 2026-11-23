// Pure verification of lib/media.json + lib/controls.json against public/media/
// (used by scripts/check-media.ts and tested directly by bun test).

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { type ControlsManifest, type FilmManifest, collectMediaFiles } from "../lib/media";
import { ASSET_LIMIT } from "./media-options";

/** Validate structural contiguity and Cloudflare size bounds on optional `film.video.segments`. */
export function validateSegmentsManifest(filmManifest: FilmManifest, assetLimit = ASSET_LIMIT): string[] {
  const errors: string[] = [];
  const { video, world } = filmManifest;
  const segments = video.segments;
  if (!segments || segments.length === 0) return errors;

  const first = segments[0]!;
  if (first.src !== video.src || first.sha256 !== video.sha256 || first.bytes !== video.bytes) {
    errors.push(
      `video.src/sha256/bytes (${video.src}, ${video.bytes} B) must match segments[0] (${first.src}, ${first.bytes} B)`,
    );
  }

  let expectedFrame = 0;
  for (let i = 0; i < segments.length; i += 1) {
    const seg = segments[i]!;
    if (seg.frames <= 0) {
      errors.push(`segment ${i} (${seg.src}) has non-positive frames (${seg.frames})`);
    }
    if (seg.start_frame !== expectedFrame) {
      errors.push(`segment ${i} (${seg.src}) starts at frame ${seg.start_frame}, expected ${expectedFrame}`);
    }
    if (seg.bytes > assetLimit) {
      errors.push(`segment ${i} (${seg.src}) is ${seg.bytes} bytes, above the ${assetLimit}-byte asset limit`);
    }
    const expectedStartTick = world.ticks[seg.start_frame];
    const expectedEndTick = world.ticks[seg.start_frame + seg.frames - 1];
    if (expectedStartTick !== undefined && seg.start_tick !== expectedStartTick) {
      errors.push(`segment ${i} (${seg.src}) start_tick ${seg.start_tick} !== world.ticks[${seg.start_frame}] (${expectedStartTick})`);
    }
    if (expectedEndTick !== undefined && seg.end_tick !== expectedEndTick) {
      errors.push(
        `segment ${i} (${seg.src}) end_tick ${seg.end_tick} !== world.ticks[${seg.start_frame + seg.frames - 1}] (${expectedEndTick})`,
      );
    }
    expectedFrame += seg.frames;
  }

  if (expectedFrame !== video.frames) {
    errors.push(`segments cover ${expectedFrame} frames, but video.frames is ${video.frames}`);
  }
  return errors;
}

/**
 * Verify that `publicDir/media/` holds byte-for-byte every file named by `filmManifest`
 * (including every multi-segment MP4) and `controlsManifest`, and nothing extra.
 */
export function verifyMediaDirectory(
  filmManifest: FilmManifest,
  controlsManifest: ControlsManifest,
  publicDir: string,
  assetLimit = ASSET_LIMIT,
): string[] {
  const errors = validateSegmentsManifest(filmManifest, assetLimit);
  const files = collectMediaFiles(filmManifest, controlsManifest);

  for (const file of files) {
    if (file.bytes > assetLimit) {
      errors.push(`${file.src} is ${file.bytes} bytes, above the ${assetLimit}-byte asset limit`);
    }
    const path = join(publicDir, file.src);
    if (!existsSync(path)) {
      errors.push(`${file.src} is missing; run \`bun run media\` or scripts/controls.ts (web/README.md)`);
      continue;
    }
    const bytes = readFileSync(path);
    const hash = createHash("sha256").update(bytes).digest("hex");
    if (bytes.length !== file.bytes || hash !== file.sha256) {
      errors.push(`${file.src} is not the file its manifest names (${hash})`);
    }
  }

  const named = new Set(files.map((file) => file.src.replace(/^\/media\//, "")));
  const media = join(publicDir, "media");
  for (const name of existsSync(media) ? readdirSync(media) : []) {
    if (!named.has(name)) {
      errors.push(`public/media/${name} is in neither lib/media.json nor lib/controls.json; move it out before building`);
    }
  }

  return errors;
}
