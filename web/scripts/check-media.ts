// Refuse to build unless every file lib/media.json names is in public/media/
// with the manifest's size and SHA-256, and nothing else is there: the build
// copies public/ into out/, so a stale render would be deployed beside the
// film. The media are outside git, so a fresh checkout has none until
// `bun run media` renders them.

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { film } from "../lib/media";

const PUBLIC = fileURLToPath(new URL("../public", import.meta.url));
let failed = false;
for (const file of [film.video, film.poster, film.social]) {
  const path = join(PUBLIC, file.src);
  if (!existsSync(path)) {
    console.error(`check-media: ${file.src} is missing; run \`bun run media\` (web/README.md)`);
    failed = true;
    continue;
  }
  const bytes = readFileSync(path);
  const hash = createHash("sha256").update(bytes).digest("hex");
  if (bytes.length !== file.bytes || hash !== file.sha256) {
    console.error(`check-media: ${file.src} is not the file lib/media.json names (${hash})`);
    failed = true;
  }
}
const named = new Set([film.video, film.poster, film.social].map((file) => file.src.replace(/^\/media\//, "")));
const media = join(PUBLIC, "media");
for (const name of existsSync(media) ? readdirSync(media) : []) {
  if (!named.has(name)) {
    console.error(`check-media: public/media/${name} is not in lib/media.json; move it out before building`);
    failed = true;
  }
}
if (failed) process.exit(1);
console.error("check-media: every file lib/media.json names is present, byte for byte");
