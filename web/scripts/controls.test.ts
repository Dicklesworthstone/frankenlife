// lib/controls.json against what it claims: each still is the recorded trial's
// world (same digest and live count), the film's world is the film's own first
// frame, and each file is named by its own hash.

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { controls, film, mediaFiles } from "../lib/media";

const REPO = fileURLToPath(new URL("../..", import.meta.url));
const trials = (
  JSON.parse(readFileSync(join(REPO, controls.record.json), "utf8")) as {
    trials: { trial: string; model: string; seed: number; tick: number; live: number; distance: number; digest: string }[];
  }
).trials;

describe("the controls' manifest", () => {
  test("names the film's preset, seed and starting tick", () => {
    expect(controls.preset).toBe(film.world.preset as string);
    expect(controls.seed).toBe(film.world.seed);
    expect(controls.tick).toBe(film.world.from.tick);
  });

  test("each still repeats its recorded trial exactly", () => {
    for (const still of controls.stills) {
      const trial = trials.find((each) => each.trial === still.record.trial);
      expect(trial).toBeDefined();
      expect(trial?.model).toBe(still.model);
      expect(trial?.seed).toBe(controls.seed);
      expect(trial?.tick).toBe(still.tick);
      expect(still.digest).toBe(trial?.digest as string);
      expect(still.live).toBe(trial?.live as number);
      expect(still.record.distance).toBe(trial?.distance as number);
    }
  });

  test("the film's world is the film's first frame, and every other still is a control", () => {
    if (controls.stills.length === 0) return;
    const full = controls.stills.filter((still) => still.removed === null);
    expect(full).toHaveLength(1);
    expect(full[0]?.digest).toBe(film.world.from.digest);
    expect(full[0]?.model).toBe(film.world.model as string);
    const model = film.world.model as string;
    expect(controls.stills.map((still) => still.model).sort()).toEqual(
      [model, `${model}-chemistry-only`, `${model}-fresh-offspring`].sort(),
    );
  });

  test("each still is named by its hash, or is the poster byte for byte, and is served beside the film", () => {
    for (const still of controls.stills) {
      if (still.poster) {
        expect(still.src).toBe(film.poster.src);
        expect(still.sha256).toBe(film.poster.sha256);
        expect(still.bytes).toBe(film.poster.bytes);
      } else {
        expect(still.src).toMatch(new RegExp(`^/media/controls-${still.id}-${still.sha256.slice(0, 12)}\\.jpg$`));
      }
      expect(mediaFiles.some((file) => file.src === still.src && file.sha256 === still.sha256)).toBe(true);
    }
    expect(new Set(mediaFiles.map((file) => file.src)).size).toBe(mediaFiles.length);
  });
});
