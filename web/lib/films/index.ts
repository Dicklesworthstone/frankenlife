// The films the page shows, in order: the hero opens on the first (lib/media.ts
// `film`), and the share card is cut from it. The zoom ladder and the controls
// are the confluent film's (lib/media.ts `filmById`). Each manifest is written
// by `bun run media -- --film <id>` (scripts/media.ts).

import type { FilmManifest } from "@/lib/media";

import confluent from "./confluent.json";

// A JSON import types each array as a list, not a pair; scripts/media.ts writes the FilmManifest shape.
export const FILM_MANIFESTS = [confluent] as unknown as FilmManifest[];
