// The films' manifests (lib/films/<id>.json, each written by `bun run media`,
// scripts/media.ts, and listed in lib/films/index.ts) and the stills'
// (lib/stills.json, written by scripts/stills.ts), all committed. The files
// they name are outside git. Each film's video is one MP4 in the R2 bucket
// that worker.ts serves at /media/ (scripts/upload-media.ts puts it there from
// web/media-r2/). The posters, cards and stills are static assets in
// public/media/, and `bun run build` refuses to build unless each one is there
// with its manifest's SHA-256 (scripts/check-media.ts).

import controlsManifest from "./controls.json";
import { FILM_MANIFESTS } from "./films";
import stillsManifest from "./stills.json";

export const MEDIA_SCHEMA = "frankenlife.website-media.v1";

export type MediaFile = {
  /** Its path on the site, under /media/, named by its SHA-256. */
  src: string;
  sha256: string;
  bytes: number;
};

export type MasterVideo = {
  /** Path outside the site (never uploaded) where the archival MP4, at a lower CRF than the web's, was written. */
  path: string;
  sha256: string;
  bytes: number;
  crf: number;
  encode: string[];
};

export type SmallVideo = MediaFile & {
  width: number;
  height: number;
  /** The ffmpeg arguments that encoded it from the film's PNG frames. */
  encode: string[];
};

export type FilmManifest = {
  schema: typeof MEDIA_SCHEMA;
  /** The film's id: lib/films/<id>.json, and the prefix of its files (scripts/media.ts --film). */
  id: string;
  /** The whole film as one MP4, served from R2 (worker.ts). */
  video: MediaFile & {
    width: number;
    height: number;
    fps: number;
    frames: number;
    /** The ffmpeg arguments that encoded it from flife's PNG frames. */
    encode: string[];
    /** The optional archival master of the same frames (scripts/media.ts --master-out), kept off the site. */
    master?: MasterVideo;
    /**
     * The same frames scaled down for a phone (scripts/media.ts --small), at its own CRF (--small-crf, in its
     * `encode`): what a narrow screen plays (components/film.tsx, PHONE), about half the bytes. Served from R2
     * like the film.
     */
    small?: SmallVideo;
  };
  poster: MediaFile & { frame: number };
  /** The social card: a 1200 × 630 crop of film frame `frame` (scripts/media.ts). */
  social: MediaFile & { width: number; height: number; frame: number };
  world: {
    /** The repository preset whose configuration hash the run has. */
    preset: string | null;
    name: string;
    config_hash: string;
    seed: number;
    founders: number;
    grid: [number, number];
    /** The torus's extent in world units (the run's [world] width and height). */
    torus: [number, number];
    timestep: [number, number];
    /** The registered model the world runs (flife-core's registry). */
    model?: string;
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
    /** Founding-only `field-ecology-v2` CLI feature flags (`--disable`/`--enable`/`--v1-compat`), if any. */
    features?: string[];
    /**
     * The camera path the film follows (CAMERA in the command), the rule
     * that chose it from a whole-world preview, and that rule's timings;
     * absent for a film of the whole torus.
     */
    camera?: {
      rule: string;
      options: { opening: number; transition: number; dwell: number; closing: number; zooms: number[] };
      keys: { tick: number; center: [number, number]; zoom: number }[];
    } | null;
    /** Each frame's zoom, as flife render recorded its view (1 is the whole torus). */
    zoom?: number[];
    mapping: string;
    meaning: string;
    look: string;
    /** `energy`, `composition`, `component-C`, `trail-carrier`, `vascular-conductance`, or `none` when hidden. */
    field_mapping: string;
    controllers: string;
    /** The commit the flife binary was built from: HEAD of a clean tree, or `--revision` (scripts/source-revision.ts). */
    revision: string;
    flife_sha256: string;
  };
};

/** Every film the page shows, in lib/films/index.ts's order; the first is the hero's opening film. */
export const films: FilmManifest[] = FILM_MANIFESTS;

/** The lead film, which the hero opens on and the share card is cut from. */
export const film: FilmManifest = (() => {
  const lead = films[0];
  if (!lead) throw new Error("lib/films/index.ts lists no film");
  return lead;
})();

/** A film by id; the page names only films lib/films/index.ts lists. */
export function filmById(id: string): FilmManifest {
  const found = films.find((entry) => entry.id === id);
  if (!found) throw new Error(`no film ${id} in lib/films/index.ts`);
  return found;
}

export const CONTROLS_SCHEMA = "frankenlife.website-controls.v1";

/** One still: a world drawn by `flife render` at the controls' tick, the whole torus. */
export type ControlStill = MediaFile & {
  /** `confluent` (the film's world), `chemistry-only` or `fresh-offspring`. */
  id: string;
  label: string;
  /** The registered model the world runs. */
  model: string;
  /** What the control takes away, in words; null for the film's world. */
  removed: string | null;
  /** The condition's rule, in words (crates/flife-cli/src/experiment.rs; flife-engine's world.rs). */
  rule: string;
  config_hash: string;
  /** Whether the still is byte for byte the film's poster, and so served as it. */
  poster: boolean;
  width: number;
  height: number;
  tick: number;
  /** Organisms alive at `tick`, as flife render recorded the frame. */
  live: number;
  /** The world's SHA-256 digest at `tick`: the render's end record. */
  digest: string;
  controllers: string;
  /** The showcase trial this run repeats, as experiments/showcase records it. */
  record: { trial: string; live: number; digest: string; distance: number; by_eye: string };
};

export type ControlsManifest = {
  schema: typeof CONTROLS_SCHEMA;
  /** The film's preset; each control is it with only `change` (and its name) altered. */
  preset: string;
  seed: number;
  tick: number;
  change: string;
  /** The torus in world units, each still showing all of it. */
  torus: [number, number];
  /** An organism's body diameter in world units (the preset's [contact] diameter). */
  body_diameter: number;
  /** The population ceiling, the same in every condition. */
  ceiling: number;
  render: { command: string; jpeg: string[]; mapping: string; meaning: string; look: string; field_mapping: string };
  record: { json: string; readme: string; rule: string };
  flife_sha256: string;
  flife_version: string;
  /** `git rev-parse HEAD` when it ran, and the paths under crates/ and presets/ then uncommitted. */
  revision: string;
  uncommitted: string[];
  host: string;
  date: string;
  stills: ControlStill[];
};

// A JSON import types each array as a list, not a pair; scripts/controls.ts writes the shape above.
export const controls = controlsManifest as unknown as ControlsManifest;

/**
 * Every file the films and the still sets serve as static assets from public/media/ (each film's poster and
 * social card, and the stills), deduplicated by path. The films' videos are in R2 instead (`filmVideos`).
 */
export function collectStaticMedia(
  filmManifests: readonly FilmManifest[],
  stillSets: readonly { stills: readonly MediaFile[] }[],
): MediaFile[] {
  return [
    ...new Map(
      [
        ...filmManifests.flatMap((filmManifest) => [filmManifest.poster, filmManifest.social]),
        ...stillSets.flatMap((set) => set.stills),
      ].map((file) => [file.src, file]),
    ).values(),
  ];
}

/** One v2 still (scripts/stills.ts): a world from a v2 preset or one of its controls, drawn at the Stage-2 tick. */
export type V2Still = MediaFile & {
  id: string;
  set: "v2-controls" | "v2-views";
  bundle: "tissue" | "supercolony";
  condition: "treatment" | "v1-compat" | "fresh-offspring";
  label: string;
  caption: string;
  preset: string;
  model: string;
  seed: number;
  config_hash: string;
  features: string[];
  width: number;
  height: number;
  tick: number;
  live: number;
  digest: string;
  view: { center: [number, number]; zoom: number };
  command: string;
  meaning: string;
  /** The Stage-2 record this world's digest and live count equal, when the script was given the summary. */
  stage2: { bundle: string; condition: string; seed: number; live: number } | null;
};

export type StillsManifest = {
  schema: "frankenlife.website-stills.v1";
  tick: number;
  jpeg: string[];
  stage2: string | null;
  flife_sha256: string;
  /** The commit the flife binary was built from (scripts/source-revision.ts). */
  revision: string;
  stills: V2Still[];
};

/** The v2 stills (lib/stills.json, written by scripts/stills.ts). */
export const stills = stillsManifest as unknown as StillsManifest;

/** Every static file the site serves from /media/ (public/media/), once each, as the manifests name them. */
export const staticMedia: MediaFile[] = collectStaticMedia(films, [controls, stills]);

/** A film's videos: the film, then its phone encode if it has one, each as the site names it. */
export function videosOf(filmManifest: FilmManifest): MediaFile[] {
  const { video } = filmManifest;
  const files = [{ src: video.src, sha256: video.sha256, bytes: video.bytes }];
  if (video.small) files.push({ src: video.small.src, sha256: video.small.sha256, bytes: video.small.bytes });
  return files;
}

/** Every film's videos, served from R2 at their `src` (worker.ts). */
export const filmVideos: MediaFile[] = films.flatMap(videosOf);
