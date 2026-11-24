// The words beside the films against the films' presets (scripts/check-film-words.ts,
// which `bun run build` also runs). The check reads presets/ at the repository
// root; a remote runner that syncs only web/ lacks it, and says so here.

import { describe, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { join } from "node:path";

import { v2Bundles } from "@/lib/content";
import { filmById } from "@/lib/media";

import { filmWordErrors } from "../scripts/check-film-words";

const REPO = join(import.meta.dir, "..", "..");
const hasPresets = existsSync(join(REPO, "presets"));

describe("film words", () => {
  test("each v2 panel names the preset its film's manifest records", () => {
    for (const bundle of v2Bundles) expect(filmById(bundle.film).world.preset).toBe(bundle.filmPreset);
  });

  test.skipIf(!hasPresets)("name every change the film presets make, with their numbers (needs ../presets)", () => {
    expect(filmWordErrors(REPO)).toEqual([]);
  });
});
