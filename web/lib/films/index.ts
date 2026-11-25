// The films the page shows, in order: the hero opens on the first (lib/media.ts
// `film`), and the share card is cut from it. The zoom ladder and the controls
// are the confluent film's (lib/media.ts `filmById`). Each manifest is written
// by `bun run media -- --film <id>` (scripts/media.ts). The two after the
// confluent film are its world as field-ecology-v2 with one mechanism on;
// lib/content.ts `filmNotes` names each one's committed configuration.

import type { FilmManifest } from "@/lib/media";

import confluent from "./confluent.json";
import confluentPolarity from "./confluent-polarity.json";
import confluentVeins from "./confluent-veins.json";

// A JSON import types each array as a list, not a pair; scripts/media.ts writes the FilmManifest shape.
export const FILM_MANIFESTS = [confluent, confluentVeins, confluentPolarity] as unknown as FilmManifest[];
