// A film whose world has no committed preset names the configuration it ran
// (lib/content.ts filmNotes[id].config, under experiments/). That file must
// describe the manifest's world: its name, model and seed, the population
// ceiling the page states when every frame sits at it, and exactly one v2
// mechanism, the one the note names. The files sit at the repository root; a
// remote runner that syncs only web/ lacks them, and says so here.

import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { filmNotes } from "@/lib/content";
import { films } from "@/lib/media";

const REPO = join(import.meta.dir, "..", "..");
const hasExperiments = existsSync(join(REPO, "experiments", "website-films"));

// The [v2.*] table that switches each mechanism on (crates/flife-core/src/v2.rs reads these tables).
const TABLE: Record<string, string> = { "vascular-field": "vascular", "pcp-polarity": "polarity" };

type Toml = { [key: string]: unknown };

/** The [v2.*] tables a configuration switches on, by name. */
function enabledV2(config: Toml): string[] {
  const on: string[] = [];
  const v2 = (config.v2 ?? {}) as Toml;
  for (const [name, table] of Object.entries(v2)) {
    if (table && typeof table === "object" && (table as Toml).enabled === true) on.push(name);
  }
  return on;
}

describe("film configurations", () => {
  test("a film without a preset names its configuration and its one mechanism", () => {
    for (const film of films) {
      if (film.world.preset) continue;
      expect(filmNotes[film.id]?.config).toBeDefined();
      expect(TABLE[filmNotes[film.id]?.mechanism ?? ""]).toBeDefined();
    }
  });

  test.skipIf(!hasExperiments)("each named configuration is the manifest's world (needs ../experiments)", () => {
    for (const film of films) {
      const note = filmNotes[film.id];
      if (!note?.config) continue;
      const config = Bun.TOML.parse(readFileSync(join(REPO, note.config), "utf8")) as Toml;
      expect(config.name).toBe(film.world.name);
      expect(config.model).toBe(film.world.model);
      expect(config.seed).toBe(film.world.seed);
      const ceiling = (config.population as Toml).maximum_count;
      if (film.world.live.every((count) => count === film.world.live[0])) expect(film.world.live[0]).toBe(ceiling as number);
      expect(enabledV2(config)).toEqual([TABLE[note.mechanism ?? ""] ?? "a mechanism the test knows"]);
    }
  });
});
