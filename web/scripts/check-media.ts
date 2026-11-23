// Refuse to build unless every file lib/media.json (including all segments of
// a multi-segment film) and lib/controls.json name is in public/media/ with
// its manifest's size and SHA-256, and nothing else is there: the build copies
// public/ into out/, so a stale render would be deployed beside the film. The
// media are outside git, so a fresh checkout has none until `bun run media`
// and scripts/controls.ts render them.

import { fileURLToPath } from "node:url";

import { controls, film } from "../lib/media";
import { verifyMediaDirectory } from "./media-verify";

const PUBLIC = fileURLToPath(new URL("../public", import.meta.url));
const errors = verifyMediaDirectory(film, controls, PUBLIC);
if (errors.length > 0) {
  for (const error of errors) console.error(`check-media: ${error}`);
  process.exit(1);
}
console.error("check-media: every file lib/media.json and lib/controls.json name is present, byte for byte");
