// The words beside the films against the films' own manifests: a panel that
// names the world a film shows must name the world its manifest records.

import { describe, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { join } from "node:path";

import { filmNotes, v2Bundles } from "@/lib/content";
import { filmById, films } from "@/lib/media";

const REPO = join(import.meta.dir, "..", "..");

describe("film words", () => {
  test("each v2 bundle names the preset its film's manifest records", () => {
    for (const bundle of v2Bundles) {
      expect(filmById(bundle.film).world.preset).toBe(bundle.filmPreset);
    }
  });

  test("every film's preset is a committed file, and every film has notes", () => {
    for (const each of films) {
      if (each.world.preset !== null) expect(existsSync(join(REPO, each.world.preset))).toBe(true);
      expect(filmNotes[each.id]).toBeDefined();
    }
  });
});
