// Each preset card states its file's world: name, model, founders, ceiling, field, torus and Δt. The files
// sit at the repository root; a remote runner that syncs only web/ lacks them, and says so here.

import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { presets } from "@/lib/content";

const REPO = join(import.meta.dir, "..", "..");
const hasRepo = existsSync(join(REPO, "presets"));

type Toml = { [key: string]: unknown };

const read = (file: string): Toml => Bun.TOML.parse(readFileSync(join(REPO, "presets", `${file}.toml`), "utf8")) as Toml;

/** The dotted keys whose values differ between two configurations. */
function differences(a: Toml, b: Toml, at = ""): string[] {
  const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
  return keys.flatMap((key) => {
    const [x, y] = [a[key], b[key]];
    const path = at ? `${at}.${key}` : key;
    const tables = x && y && typeof x === "object" && typeof y === "object" && !Array.isArray(x) && !Array.isArray(y);
    if (tables) return differences(x as Toml, y as Toml, path);
    return JSON.stringify(x) === JSON.stringify(y) ? [] : [path];
  });
}

describe("preset cards", () => {
  test.skipIf(!hasRepo)("each card states its preset file's world (needs ../presets)", () => {
    for (const preset of presets) {
      const config = read(preset.file);
      const world = config.world as Toml;
      const population = config.population as Toml;
      expect(config.name).toBe(preset.file);
      // A card may add a note to the model ("field-ecology-v2 · Bundle A (contact)").
      expect(preset.model.split(" · ")[0]).toBe(config.model as string);
      expect(population.initial_count).toBe(preset.founders);
      expect(population.maximum_count).toBe(preset.ceiling);
      expect([world.grid_width, world.grid_height]).toEqual([preset.grid, preset.grid]);
      expect([world.width, world.height]).toEqual([preset.torus, preset.torus]);
      expect(`${world.tick_numerator}/${world.tick_denominator}`).toBe(preset.dt);
    }
  });

  test.skipIf(!hasRepo)("a variant is its card's world with only the change its words name (needs ../presets)", () => {
    const variants = presets.filter((preset) => preset.variant);
    expect(variants.map((preset) => preset.variant?.file)).toEqual(["desktop-multirate-4t"]);
    for (const preset of variants) {
      const variant = read(preset.variant?.file ?? "");
      expect(variant.name).toBe(preset.variant?.file);
      expect(differences(read(preset.file), variant)).toEqual(["controller.evaluation_interval_ticks", "name"]);
      expect((variant.controller as Toml).evaluation_interval_ticks).toBe(4);
      expect(preset.variant?.body).toContain("every fourth tick");
    }
  });
});
