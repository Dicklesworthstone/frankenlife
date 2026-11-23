// The film's manifest, written by `bun run media` (scripts/media.ts), and the
// controls' (lib/controls.json, written by scripts/controls.ts), both
// committed. The files they name live in public/media/, outside git, and
// `bun run build` refuses to build unless each one is there with its
// manifest's SHA-256 (scripts/check-media.ts).

import controlsManifest from "./controls.json";
import manifest from "./media.json";

export const MEDIA_SCHEMA = "frankenlife.website-media.v1";

export type MediaFile = {
  /** Its path on the site, under /media/, named by its SHA-256. */
  src: string;
  sha256: string;
  bytes: number;
};

export type MasterVideo = {
  /** Path outside public/media/ where the unconstrained archival MP4 was written. */
  path: string;
  sha256: string;
  bytes: number;
  crf: number;
  encode: string[];
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
    /** Optional unconstrained archival master (e.g. 4K60) written outside public/media/. */
    master?: MasterVideo;
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
    /** `git rev-parse HEAD` of a clean source tree when it rendered. */
    revision: string;
    flife_sha256: string;
  };
};

export const film = manifest as FilmManifest;

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

/** Every file the site serves from /media/, once each, as the manifests name them. */
export const mediaFiles: MediaFile[] = [
  ...new Map([film.video, film.poster, film.social, ...controls.stills].map((file) => [file.src, file])).values(),
];
