// Refuse to build unless every static file the films (their posters and social
// cards) and the stills name is in public/media/ with its manifest's size and
// SHA-256, and nothing else is there: the build copies public/ into out/, so a
// stale render would be deployed beside the page. The films' videos are not
// static assets: they are in R2 (scripts/upload-media.ts), and a local copy in
// media-r2/, when there is one, is checked too. The media are outside git, so a
// fresh checkout has none until `bun run media` and scripts/controls.ts render
// them.

import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { controls, films, stills } from "../lib/media";
import { verifyFilmVideos, verifyMediaDirectory, videoName } from "./media-verify";

const PUBLIC = fileURLToPath(new URL("../public", import.meta.url));
const FILMS = fileURLToPath(new URL("../media-r2", import.meta.url));
const errors = verifyMediaDirectory(films, [controls, stills], PUBLIC);
const local = films.filter((film) => existsSync(`${FILMS}/${videoName(film)}`));
errors.push(...verifyFilmVideos(local, FILMS));
if (errors.length > 0) {
  for (const error of errors) console.error(`check-media: ${error}`);
  process.exit(1);
}
console.error(
  `check-media: every static file the ${films.length === 1 ? "film" : `${films.length} films`} and the stills name ` +
    `is present, byte for byte; ${local.length} of ${films.length} videos are in media-r2/ and match`,
);
