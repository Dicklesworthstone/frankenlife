// The film's manifest, written by `bun run media` (scripts/media.ts) and
// committed. The files it names live in public/media/, outside git, and
// `bun run build` refuses to build unless each one is there with the
// manifest's SHA-256 (scripts/check-media.ts).

import manifest from "./media.json";

export const MEDIA_SCHEMA = "frankenlife.website-media.v1";

export type MediaFile = {
  /** Its path on the site, under /media/, named by its SHA-256. */
  src: string;
  sha256: string;
  bytes: number;
};

export type FilmManifest = {
  schema: typeof MEDIA_SCHEMA;
  video: MediaFile & {
    width: number;
    height: number;
    fps: number;
    frames: number;
    /** The ffmpeg arguments that encoded it from flife's PNG frames. */
    encode: string[];
  };
  poster: MediaFile & { frame: number };
  social: MediaFile & { width: number; height: number };
  world: {
    /** The repository preset whose configuration hash the run has. */
    preset: string | null;
    name: string;
    config_hash: string;
    seed: number;
    founders: number;
    grid: [number, number];
    timestep: [number, number];
    /** The committed world the film starts from, by tick and digest. */
    from: { tick: number; digest: string };
    /** Each frame's tick and live organisms, in order. */
    ticks: number[];
    live: number[];
    every: number;
    /** The committed world at the last frame. */
    to: { tick: number; digest: string };
  };
  render: {
    /** The command, with the run directory as RUN and the frames as FRAMES. */
    command: string;
    mapping: string;
    meaning: string;
    look: string;
    field_mapping: string;
    controllers: string;
    /** `git rev-parse HEAD` of a clean source tree when it rendered. */
    revision: string;
    flife_sha256: string;
  };
};

export const film = manifest as FilmManifest;
