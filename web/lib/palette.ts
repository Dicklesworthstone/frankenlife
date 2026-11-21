// Colours the explainers share with the film.

/**
 * flife-manim's LINEAGE_PALETTE (crates/flife-manim/src/mapping.rs), the
 * colours founder lineages wear in the film and the Studio. The diagrams
 * draw organisms in them so a lineage looks the same everywhere.
 */
export const LINEAGE = [
  "#2EC4B6", // teal
  "#9B5DE5", // violet
  "#F15BB5", // magenta
  "#9BF6FF", // pale cyan
  "#FFB627", // amber
  "#B5E61D", // lime
  "#FF6F59", // coral
  "#4EA8DE", // sky
  "#FEE440", // gold
  "#FF99C8", // rose
  "#7AE582", // mint
  "#C8B6FF", // lavender
] as const;

/** The page's semantic colours for the diagrams: the rule, its plausible wrong version, energy, chemistry. */
export const TONE = {
  rule: "#4ade80",
  wrong: "#fb7185",
  energy: "#fde68a",
  chemistry: "#67e8f9",
  dissipated: "#94a3b8",
  ink: "#020a02",
} as const;
