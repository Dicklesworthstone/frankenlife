// Pure verification of the films' manifests (lib/films/*.json) and the still
// sets' (lib/controls.json, lib/stills.json) against the files on disk: the
// static media in public/media/ and the films' videos in web/media-r2/ (used by
// scripts/check-media.ts and scripts/upload-media.ts, and tested directly by
// bun test).

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { type FilmManifest, type MediaFile, collectStaticMedia } from "../lib/media";
import { ASSET_LIMIT } from "./media-options";

/** Whether `path` holds exactly `file`'s bytes: an error message if not, null if so. */
export function checkFile(path: string, file: MediaFile): string | null {
  if (!existsSync(path)) return `${file.src} is missing from ${path}`;
  const bytes = readFileSync(path);
  const hash = createHash("sha256").update(bytes).digest("hex");
  if (bytes.length !== file.bytes || hash !== file.sha256) return `${path} is not the file its manifest names (${hash})`;
  return null;
}

/** The name a film's video has under /media/ and in R2: `<id>-<first 12 hex of its SHA-256>.mp4`. */
export function videoName(filmManifest: FilmManifest): string {
  return `${filmManifest.id}-${filmManifest.video.sha256.slice(0, 12)}.mp4`;
}

/**
 * Verify that `publicDir/media/` holds byte-for-byte every static file named by `filmManifests` (posters and
 * social cards) and `stillSets`, each within Cloudflare's static asset limit, and nothing extra; and that each
 * film names one video at `/media/<id>-<hash>.mp4`. The build copies public/ into out/, so a stray file there
 * would be deployed; the videos are served from R2 and are never in public/.
 */
export function verifyMediaDirectory(
  filmManifests: readonly FilmManifest[],
  stillSets: readonly { stills: readonly MediaFile[] }[],
  publicDir: string,
  assetLimit = ASSET_LIMIT,
): string[] {
  const errors: string[] = [];
  const ids = new Set<string>();
  for (const filmManifest of filmManifests) {
    if (ids.has(filmManifest.id)) errors.push(`two films have the id ${filmManifest.id}`);
    ids.add(filmManifest.id);
    const expected = `/media/${videoName(filmManifest)}`;
    if (filmManifest.video.src !== expected) {
      errors.push(`${filmManifest.id}: its video is ${filmManifest.video.src}, not ${expected}`);
    }
  }
  const files = collectStaticMedia(filmManifests, stillSets);

  for (const file of files) {
    if (file.bytes > assetLimit) {
      errors.push(`${file.src} is ${file.bytes} bytes, above the ${assetLimit}-byte asset limit`);
    }
    const problem = checkFile(join(publicDir, file.src), file);
    if (problem !== null) {
      errors.push(
        problem.includes("is missing")
          ? `${file.src} is missing; run \`bun run media\`, scripts/controls.ts or scripts/stills.ts (web/README.md)`
          : problem,
      );
    }
  }

  const named = new Set(files.map((file) => file.src.replace(/^\/media\//, "")));
  const media = join(publicDir, "media");
  for (const name of existsSync(media) ? readdirSync(media) : []) {
    if (!named.has(name)) {
      errors.push(
        `public/media/${name} is named by no manifest's static files (lib/films/*.json posters and cards, ` +
          `lib/controls.json, lib/stills.json); move it out before building (films belong in media-r2/)`,
      );
    }
  }

  return errors;
}

/** Each film's video checked in `filmDir` (web/media-r2/): the errors, for films whose local copy is wrong or absent. */
export function verifyFilmVideos(filmManifests: readonly FilmManifest[], filmDir: string): string[] {
  return filmManifests
    .map((filmManifest) => checkFile(join(filmDir, videoName(filmManifest)), filmManifest.video))
    .filter((problem): problem is string => problem !== null);
}
