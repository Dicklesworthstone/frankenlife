// Refuse to build unless the words beside the v2 films state every way their
// presets differ from the committed presets, with the presets' own numbers.
// The site says "every other parameter is the preset's" (lib/content.ts
// filmChoice), so a film preset that changes a key the copy does not name, or
// by another amount, would make the page false. It reads presets/ at the
// repository root, which a remote test runner syncing only web/ lacks, so
// `bun run build` runs it locally before every deploy.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { FILM_CEILING, filmChoice, filmNotes, v2Bundles } from "../lib/content";
import { films } from "../lib/media";

type Toml = { [key: string]: unknown };

/** A preset's keys, flattened to dotted paths. */
function keys(repo: string, path: string): Map<string, unknown> {
  const flat = new Map<string, unknown>();
  const walk = (table: Toml, prefix: string) => {
    for (const [key, value] of Object.entries(table)) {
      const at = prefix ? `${prefix}.${key}` : key;
      if (value !== null && typeof value === "object" && !Array.isArray(value)) walk(value as Toml, at);
      else flat.set(at, value);
    }
  };
  walk(Bun.TOML.parse(readFileSync(join(repo, path), "utf8")) as Toml, "");
  return flat;
}

const grouped = (value: unknown) => Number(value).toLocaleString("en-US");

/** A film preset and its committed preset, read once: `film(key)`, `base(key)`, and the ratio of their total food. */
type Pair = { film: (key: string) => unknown; base: (key: string) => unknown; food: number };

/**
 * Each film preset beside its committed preset: the keys the copy says it
 * changes, and the phrases each piece of copy must contain, from the presets'
 * numbers. `choice` is lib/content.ts filmChoice.v2[0], which lists every
 * change; the film's note and its panel's filmWorld name each more briefly.
 */
const WORLDS = [
  {
    film: "presets/morphogenesis-tissue-v2-dense.toml",
    base: "presets/morphogenesis-tissue-v2.toml",
    changes: ["population.maximum_count", "sources.count", "sources.power_per_source", "sources.sigma_world_units", "v2.crowding.arrest_threshold"],
    choice: (p: Pair) => [
      `${p.food} times the preset's food, from ${p.film("sources.count")} wide sources`,
      `${p.film("v2.crowding.arrest_threshold")} neighbours instead of ${p.base("v2.crowding.arrest_threshold")}`,
    ],
    brief: (p: Pair) => [
      `${p.food} times`,
      `${p.film("sources.count")} wide sources`,
      `${p.film("v2.crowding.arrest_threshold")} neighbours instead of ${p.base("v2.crowding.arrest_threshold")}`,
      "twice",
    ],
    words: () => [filmNotes.tissue?.line ?? "", v2Bundles.find((each) => each.id === "tissue")?.filmWorld ?? ""],
  },
  {
    film: "presets/supercolony-v2-dense.toml",
    base: "presets/supercolony-v2.toml",
    changes: ["population.maximum_count", "reproduction.child_position_offset_radius", "sources.count", "sources.power_per_source"],
    choice: (p: Pair) => [
      `${p.film("sources.count")} oases where the preset has ${p.base("sources.count")}, ${p.food} times its food`,
      `up to ${p.film("reproduction.child_position_offset_radius")} world units from their parent instead of ${p.base("reproduction.child_position_offset_radius")}`,
    ],
    brief: (p: Pair) => [`${p.film("sources.count")} oases`, `${p.food} times`, "children landing farther", "twice"],
    words: () => [filmNotes.supercolony?.line ?? "", v2Bundles.find((each) => each.id === "supercolony")?.filmWorld ?? ""],
  },
] as const;

/** Every way the film words and the presets disagree; none when they agree. */
export function filmWordErrors(repo: string): string[] {
  const errors: string[] = [];
  for (const film of films) {
    if (film.world.preset !== null && !existsSync(join(repo, film.world.preset))) {
      errors.push(`the ${film.id} film's manifest names ${film.world.preset}, which is not in the repository`);
    }
    if (!filmNotes[film.id]) errors.push(`lib/content.ts filmNotes has no entry for the ${film.id} film`);
  }
  for (const bundle of v2Bundles) {
    const recorded = films.find((film) => film.id === bundle.film)?.world.preset;
    if (recorded !== bundle.filmPreset) {
      errors.push(`the ${bundle.id} panel names ${bundle.filmPreset}, but its film's manifest records ${recorded}`);
    }
  }

  const choice = filmChoice.v2[0] ?? "";
  for (const world of WORLDS) {
    const [film, base] = [keys(repo, world.film), keys(repo, world.base)];
    const changed = [...new Set([...film.keys(), ...base.keys()])]
      .filter((key) => key !== "name" && JSON.stringify(film.get(key)) !== JSON.stringify(base.get(key)))
      .sort();
    if (changed.join(" ") !== [...world.changes].sort().join(" ")) {
      errors.push(`${world.film} changes [${changed.join(", ")}] from ${world.base}; the copy names [${world.changes.join(", ")}]`);
    }
    const power = (preset: Map<string, unknown>) => Number(preset.get("sources.count")) * Number(preset.get("sources.power_per_source"));
    const pair: Pair = { film: (key) => film.get(key), base: (key) => base.get(key), food: power(film) / power(base) };

    const [ceilingFrom, ceilingTo] = [Number(base.get("population.maximum_count")), Number(film.get("population.maximum_count"))];
    if (ceilingTo !== FILM_CEILING) errors.push(`${world.film} has a ceiling of ${grouped(ceilingTo)}; lib/content.ts FILM_CEILING is ${grouped(FILM_CEILING)}`);
    if (ceilingTo !== 2 * ceilingFrom) errors.push(`${world.film}'s ceiling is not twice ${world.base}'s, as the copy says`);

    for (const phrase of [...world.choice(pair), `a ceiling of ${grouped(ceilingTo)} rather than ${grouped(ceilingFrom)}`]) {
      if (!choice.includes(phrase)) errors.push(`filmChoice.v2 does not say "${phrase}" (${world.film})`);
    }
    for (const words of world.words()) {
      for (const phrase of world.brief(pair)) {
        if (!words.includes(phrase)) errors.push(`"${words.slice(0, 60)}…" does not say "${phrase}" (${world.film})`);
      }
    }
  }
  return errors;
}

if (import.meta.main) {
  const repo = fileURLToPath(new URL("../..", import.meta.url));
  const errors = filmWordErrors(repo);
  if (errors.length > 0) {
    for (const error of errors) console.error(`check-film-words: ${error}`);
    process.exit(1);
  }
  console.error(`check-film-words: the words beside the films name every change the ${WORLDS.length} film presets make, with their numbers`);
}
