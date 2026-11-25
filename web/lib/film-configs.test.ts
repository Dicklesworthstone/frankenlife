// Every film's world is a committed configuration: its preset, or, for a world
// with no preset, the file lib/content.ts filmNotes[id].config names (under
// experiments/). That file must describe the manifest's world (its name, model
// and seed), and the words beside the film must agree with it: a population
// ceiling the words name is the file's, and every frame sits at it; a film
// that names one v2 mechanism switches on exactly that one. The files sit at
// the repository root; a remote runner that syncs only web/ lacks them, and
// says so here.

import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { filmChoice, filmNotes } from "@/lib/content";
import { type FilmManifest, films } from "@/lib/media";

const REPO = join(import.meta.dir, "..", "..");
const hasRepo = existsSync(join(REPO, "presets")) && existsSync(join(REPO, "experiments", "website-films"));

// The [v2.*] table that switches each mechanism on (crates/flife-core/src/v2.rs reads these tables).
const TABLE: Record<string, string> = { "vascular-field": "vascular", "pcp-polarity": "polarity" };

type Toml = { [key: string]: unknown };

/** The committed configuration a film's world ran. */
const configPath = (film: FilmManifest): string | undefined => film.world.preset ?? filmNotes[film.id]?.config;

/** The [v2.*] tables a configuration switches on, by name. */
function enabledV2(config: Toml): string[] {
  const on: string[] = [];
  for (const [name, table] of Object.entries((config.v2 ?? {}) as Toml)) {
    if (table && typeof table === "object" && (table as Toml).enabled === true) on.push(name);
  }
  return on;
}

/** The ceiling a sentence names ("population ceiling of 262,144"), if it names one. */
function namedCeiling(words: string): number | undefined {
  const found = /population ceiling of ([\d,]+)/.exec(words);
  return found?.[1] ? Number(found[1].replaceAll(",", "")) : undefined;
}

describe("film configurations", () => {
  test("every film names its configuration; one without a preset also names a known mechanism", () => {
    for (const film of films) {
      expect(configPath(film)).toBeDefined();
      if (!film.world.preset) expect(TABLE[filmNotes[film.id]?.mechanism ?? ""]).toBeDefined();
    }
  });

  test.skipIf(!hasRepo)("each configuration is the manifest's world, as the words say (needs ../presets, ../experiments)", () => {
    for (const film of films) {
      const note = filmNotes[film.id];
      const config = Bun.TOML.parse(readFileSync(join(REPO, configPath(film) ?? ""), "utf8")) as Toml;
      expect(config.name).toBe(film.world.name);
      expect(config.model).toBe(film.world.model);
      expect(config.seed).toBe(film.world.seed);

      const ceiling = (config.population as Toml).maximum_count as number;
      const named = namedCeiling(note?.line ?? "");
      if (named !== undefined) {
        expect(named).toBe(ceiling);
        expect(film.world.live.filter((count) => count !== ceiling)).toEqual([]);
      }

      const on = enabledV2(config);
      if (note?.mechanism) expect(on).toEqual([TABLE[note.mechanism] ?? "a mechanism the test knows"]);
      else if (film.world.model?.startsWith("field-ecology-v2")) expect(on).toEqual([]);
    }
  });

  test("the v2 films are the confluent world over the same ticks, drawn the same way, as the selection paragraph says", () => {
    const confluent = films.find((film) => film.id === "confluent");
    expect(confluent).toBeDefined();
    if (!confluent) return;
    const { from, to, every, seed, founders, grid, torus } = confluent.world;
    const span = `${from.tick.toLocaleString("en-US")} to ${to.tick.toLocaleString("en-US")}`;
    expect(filmChoice.v2.some((paragraph) => paragraph.includes(`the same ticks, ${span}`))).toBe(true);
    for (const film of films.filter((each) => filmNotes[each.id]?.mechanism)) {
      expect([film.world.from.tick, film.world.to.tick, film.world.every]).toEqual([from.tick, to.tick, every]);
      expect([film.world.seed, film.world.founders, film.world.grid, film.world.torus]).toEqual([seed, founders, grid, torus]);
      expect(film.render.command).toBe(confluent.render.command);
      expect(film.render.camera?.options.zooms).toEqual(confluent.render.camera?.options.zooms);
    }
  });

  test("the v2 films' ceiling, as the selection paragraph names it, holds in every frame", () => {
    const named = filmChoice.v2.map(namedCeiling).find((value) => value !== undefined);
    expect(named).toBeDefined();
    const v2Films = films.filter((film) => filmNotes[film.id]?.mechanism);
    expect(v2Films.length).toBe(2);
    for (const film of v2Films) expect(film.world.live.filter((count) => count !== named)).toEqual([]);
  });
});
