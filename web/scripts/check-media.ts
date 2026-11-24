// Refuse to build unless every file the films (lib/films/*.json, including all
// segments of a multi-segment film) and the stills name is in public/media/ with
// its manifest's size and SHA-256, and nothing else is there: the build copies
// public/ into out/, so a stale render would be deployed beside the film. The
// media are outside git, so a fresh checkout has none until `bun run media`
// and scripts/controls.ts render them.

import { fileURLToPath } from "node:url";

import { controls, films, stills } from "../lib/media";
import { verifyMediaDirectory } from "./media-verify";

const PUBLIC = fileURLToPath(new URL("../public", import.meta.url));
const errors = verifyMediaDirectory(films, [controls, stills], PUBLIC);
if (errors.length > 0) {
  for (const error of errors) console.error(`check-media: ${error}`);
  process.exit(1);
}
console.error(`check-media: every file the ${films.length} films and the stills name is present, byte for byte`);
