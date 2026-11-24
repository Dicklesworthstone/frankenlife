// The v2 mechanism atlas: each of field-ecology-v2's 21 toggleable features in
// plain words, the rule as the engine implements it, the committed presets'
// numbers, where its energy goes, what the Stage-2 ablations measured when it
// was taken away, and what is not shown. Sources: crates/flife-core/src/v2.rs
// (registry, bundle defaults), crates/flife-engine/src/{emission,contact,
// chemistry,lifecycle}.rs, crates/flife-field/src/transport.rs,
// presets/{morphogenesis-tissue-v2,supercolony-v2}.toml and
// experiments/v2-emergence/README.md (Stage-2: 4 seeds each, 42 and 43 for
// discovery, 101 and 102 held out, 2,048 ticks on a 320² field).
// Ecological numbers are means over the 4 seeds; "n/4" counts the seeds on
// which the difference had the stated sign. The page tells the entries as two
// step-throughs (the tissue's own mechanisms, then the supercolony's), a row of
// rules both share and a row of opt-ins; the charts compute the rules from
// components/explain/v2-atlas-math.ts, whose tests pin every value stated here.

import type { V2FeatureSlug } from "@/scripts/media-options";

/** Where the page tells an entry: in one world's step-through, or in the rows after them. */
export type AtlasStory = "tissue" | "supercolony" | "shared" | "opt-in" | "view";

export type AtlasChart =
  | "oases"
  | "senescence"
  | "surplus"
  | "relay"
  | "junctions"
  | "vascular"
  | "trails"
  | "hippo"
  | "gate"
  | "turn"
  | "kin"
  | "cadherin"
  | "motor"
  | "deposit";

export type AtlasEntry = {
  slug: V2FeatureSlug;
  /** A plain name for the mechanism. */
  name: string;
  /** One word, for a step's tab. */
  short: string;
  story: AtlasStory;
  /** On in each committed preset: Bundle A (tissue) and Bundle B (supercolony). */
  on: { tissue: boolean; supercolony: boolean };
  /** What it borrows from, in a phrase. */
  borrowed: string;
  /** The rule, in words. */
  rule: string;
  /** The rule, in symbols, as implemented. */
  formula?: string;
  /** The committed presets' numbers. */
  numbers: string;
  /** Where the energy goes: the ledger still balances. */
  ledger: string;
  /** The headline of what Stage-2 measured, for a large figure; `evidence` gives the rest. */
  effect?: { value: string; label: string };
  /** What Stage-2 measured when it was removed (or added), with its seed count. */
  evidence?: string;
  /** What this does not show, or what it costs. Always shown beside the headline. */
  caveat?: string;
  chart?: AtlasChart;
  /** A lib/stills.json still drawn with the view that shows this mechanism's state. */
  still?: string;
  /** A second mechanism told in the same step. */
  also?: V2FeatureSlug;
};

export const atlas: AtlasEntry[] = [
  // ─── The tissue's own ─────────────────────────────────────────────────────
  {
    slug: "hippo-crowding",
    name: "Stop dividing when crowded",
    short: "Crowding",
    story: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "The Hippo–YAP pathway: contact inhibition of growth",
    rule: "An organism counts its neighbours within 0.375 world units. Its chance to divide falls with the count and reaches zero at four, and its network senses the count.",
    formula: "gate = clamp(1 − N / c_max, 0, 1)",
    numbers: "c_max = 3.25: 1, 0.69, 0.38, 0.08, then 0 for 0–4 neighbours; +1 sensor",
    ledger: "A gate on births; no energy moves.",
    effect: { value: "½", label: "as many close neighbours per cell as with every v2 switch off: 2.0 against 3.6 within 1.5 body widths, fewer on 4/4 seeds" },
    evidence:
      "That control turns off all 14 switches, not this one alone. Every alignment measure is higher in the preset too, by 0.135 to 0.144 (4/4).",
    chart: "hippo",
  },
  {
    slug: "pcp-polarity",
    name: "A front and a back",
    short: "Polarity",
    story: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Planar cell polarity in epithelia",
    rule: "Each body carries a polarity that turns toward its neighbours' and toward the local gradient. Bodies adhere more strongly side by side than end to end, cells wedge slightly, and daughters are placed beside the parent.",
    numbers: "turning mobility 2.0, gradient cue 0.25, initial strength up to 0.88, wedge curvature up to 0.35",
    ledger: "Forces between bodies move them; no energy changes hands.",
    evidence: "Removing it lowers kin polarization a little, by 0.018 (4/4), and local polarization by 0.013 (3/4).",
    still: "tissue-polarity",
  },
  {
    slug: "cadherin-4d",
    name: "Sticking to your own kind",
    short: "Adhesion",
    story: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Cadherins, the proteins that sort cells by type",
    rule: "Each organism inherits a four-dimensional tag, and two bodies stick as strongly as their tags point the same way. Who sticks to whom is inherited separately from what each eats.",
    formula: "adhesion = min(s_i, s_j)·max(0, cos θ)²",
    numbers: "tags mutate with σ = 0.05",
    ledger: "Adhesion is a force; no energy changes hands.",
    effect: { value: "−10%", label: "alive without it: 1,496 fewer organisms, lower on 4/4 seeds" },
    evidence: "Kin also share 6,589 fewer units of energy.",
    chart: "cadherin",
  },
  {
    slug: "trophallaxis",
    name: "Feeding your kin",
    short: "Sharing",
    story: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Social insects passing food mouth to mouth",
    rule: "Neighbours within 1.5 body widths whose inherited diets are alike pass energy from the richer to the poorer, faster the closer and more alike they are. A donor keeps a reserve and gives at most a quarter of what it holds above it per tick; 5% of every transfer is lost.",
    formula: "Δt·G·alike⁴·(1 − d/R)·(E_i − E_j), alike = max(0, cos(a_i, a_j)) ≥ 0.5",
    numbers: "G = 2/s, R = 0.375, efficiency 0.95, reserve 0.25, at most 25% of the energy above it per tick, exact in quanta of 2⁻³⁰",
    ledger: "Both sides round to the same quantum; the 5% loss is booked as trophallaxis loss.",
    effect: { value: "79,910", label: "units of energy passed between kin per run, in all 16 measurement windows on all 4 seeds" },
    caveat: "The sharing does not raise the count: without it 15,579 are alive against 15,446.",
    chart: "kin",
  },
  {
    slug: "symplectic-waste",
    name: "Waste someone else can eat",
    short: "Waste",
    story: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Cross-feeding, where one microbe's waste is another's food",
    rule: "Of the food an organism takes up and does not convert, 45% goes back as a byproduct whose chemistry is turned at right angles to its own diet. It is no food to the organism itself (mildly harmful at the presets' uptake threshold), and food or poison to others.",
    formula: "byproduct along J·a_eff, with a_eff · J·a_eff = 0",
    numbers: "ρ = 0.45 (tissue; on by default in Bundle B, off in the supercolony preset)",
    ledger: "Byproduct energy comes out of the unconverted uptake, which would otherwise be lost; the ledger balances.",
    effect: { value: "+8%", label: "alive without it (16,695 against 15,446), but less aligned" },
    evidence: "With it, local polarization (+0.026) and both pairwise alignments (+0.043, +0.032) are higher on all 4 seeds.",
    caveat: "“Symplectic” names the rotation J, a 90° turn in each pair of chemical channels; nothing here is Hamiltonian mechanics.",
    chart: "deposit",
  },
  {
    slug: "epigenetic-recurrence",
    name: "A slow memory that changes diet",
    short: "Memory",
    story: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Epigenetic cell state and differentiation",
    rule: "Each organism carries two state values that its network rewrites a little each tick, slowly unless pushed. The first turns what the organism eats; the second changes how it adheres. A newborn keeps 80% of its parent's state, while the parent keeps all of its own. The newborn's first value is also pushed down, by up to 0.8 and the harder the higher the parent's (an asymmetric, stem-like division); its second gets a little noise.",
    formula: "h′ = (1 − g)h + g·tanh c,  g = σ(z − 2.5);  a_eff = cos(πh₀/2)·a + sin(πh₀/2)·Ja",
    numbers: "2 state values; resting gate 0.076; newborn h₀ = 0.8·h₀ − 0.8·σ(3h₀), h₁ = 0.8·h₁ + noise (σ 0.05); +2 sensors, +4 raw outputs",
    ledger: "State holds no energy.",
    effect: { value: "32.6%", label: "of the state's variance lies within lineages, against 28.9% when networks cannot evolve (4/4)" },
    caveat: "At 2,048 ticks the state barely tracks crowding (r = +0.069), so there is no claim yet of distinct core and sheath cell types.",
    chart: "turn",
    still: "tissue-cell-state",
    also: "notch-delta",
  },
  {
    slug: "notch-delta",
    name: "Reading the neighbours' state",
    short: "Neighbours",
    story: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Notch–Delta lateral inhibition",
    rule: "A new sense reports the kin-weighted average of the neighbours' first state value. Whether a cell then differs from its neighbours is up to its network.",
    numbers: "+1 sensor; requires epigenetic recurrence",
    ledger: "A sense; no energy moves.",
    evidence: "Removing it changes little at 2,048 ticks: 15,725 alive against 15,446.",
  },

  // ─── The supercolony's ────────────────────────────────────────────────────
  {
    slug: "oases",
    name: "Oases that bloom and fade",
    short: "Oases",
    story: "supercolony",
    on: { tissue: false, supercolony: true },
    borrowed: "Patchy, seasonal food: tides, blooms, rain",
    rule: "Each of the 16 food sources swells and shrinks on its own phase of a 24-second cycle, never below 5% of its peak, while their total power stays the same.",
    formula: "w_j = f_min + (1 − f_min)·max(0, sin 2π(t/T + j/M))²,  P_j = M·P·w_j / Σw",
    numbers: "T = 24 s, f_min = 0.05, M = 16 sources",
    ledger: "Redistribution only: Σ P_j = M·P at every tick, so a bloom creates no energy.",
    effect: { value: "+53%", label: "alive without oases (32,517 against 21,308), in a world with less structure" },
    evidence:
      "Without them trail contrast falls by a third (peak z-score 6.82 against 10.24, 4/4), the vascular network stops being bimodal and organisms align 35% less (4/4).",
    chart: "oases",
  },
  {
    slug: "excitable-relay",
    name: "Pulses instead of a trickle",
    short: "Pulses",
    story: "supercolony",
    on: { tissue: false, supercolony: true },
    borrowed: "Excitable media: neurons, the cAMP waves of Dictyostelium",
    rule: "Each organism has a readiness that recovers over about a second, and it emits only in bursts, when readiness is above one half. A burst spends five times the usual rate, scaled by the readiness, and drains 85% of it. Neighbours sense the bursts; whether they answer is up to their evolved networks.",
    numbers: "threshold 0.5, depletion 0.85, recovery τ = 1 s, burst ×5: an emitter that always wants to fires every 21 ticks",
    ledger: "Readiness holds no energy; bursts spend surplus only.",
    effect: { value: "−37%", label: "alive without bursts (13,458 against 21,308), lower on 4/4 seeds" },
    evidence: "Without them the trails also smear, 4.3 times brighter on average instead of crisp.",
    chart: "relay",
  },
  {
    slug: "dual-carrier-trails",
    name: "A trail no one can eat",
    short: "Trails",
    story: "supercolony",
    on: { tissue: false, supercolony: true },
    borrowed: "Ant pheromone trails",
    rule: "A fifth of what an organism emits goes into a second carrier that feeds no one, decays slowly and barely spreads. Four new senses let networks read it.",
    numbers: "20% of emission, decay 0.030/s (half-life 23 s against the food field's 8.5 s), diffusion 0.0003; 34 field planes; +4 sensors",
    ledger: "Trail energy is booked separately and decays into the declared ledger; it is never food.",
    effect: { value: "17%", label: "fewer alive with the trails than without them: 21,308 against 25,695" },
    evidence: "The trails stand out from their surroundings (peak z-score 10.24) and lie apart from the food (decoupling 0.948).",
    chart: "trails",
    still: "supercolony-trail-carrier",
  },
  {
    slug: "vascular-field",
    name: "Veins that carve themselves",
    short: "Veins",
    story: "supercolony",
    on: { tissue: false, supercolony: true },
    borrowed: "Slime-mould (Physarum) tubes, and vessels that widen where flow is high",
    rule: "Every face between two field cells has a conductance. It widens where energy keeps flowing and shrinks back where the flow stops, so the field grows channels along the paths energy takes.",
    formula: "g ← g + Δt(α·s(r)(1 − g) − γ(g − g_min)),  s = r^μ/(1 + r^μ),  r = |F|/J₀",
    numbers: "α = 0.5/s, γ = 0.1/s, g_min = 0.025, μ = 1.5, J₀ = 0.25: a face under steady strong flux settles near 0.84",
    ledger: "Conductance scales the field's diffusivity, g·D_max with D_max = 0.30, ten times the base 0.030: a conservative flux that moves energy and never makes it.",
    effect: { value: "−33%", label: "local alignment without veins: 0.378 falls to 0.252 (4/4)" },
    evidence:
      "Kin alignment falls from 0.419 to 0.266 (4/4). The conductances are bimodal by Sarle's coefficient (0.608, above the 0.556 line), with a 7.0× spread between the 95th and 25th percentiles.",
    caveat: "The rule widens faces by flux, in the spirit of Murray's law; it is not Murray's cube law.",
    chart: "vascular",
    still: "supercolony-vascular-conductance",
  },
  {
    slug: "tight-junctions",
    name: "Membranes that slow diffusion",
    short: "Membranes",
    story: "supercolony",
    on: { tissue: true, supercolony: true },
    borrowed: "Tight junctions sealing an epithelium",
    rule: "The more organisms sit in a field cell, the less chemistry diffuses through its faces, so a colony keeps its own chemical neighbourhood. Both presets switch it on.",
    formula: "φ(n) = φ_min + (1 − φ_min)/(1 + κn), a face taking the harmonic mean of its two cells",
    numbers: "φ_min = 0.04, κ = 1.2: one occupant lets 48% through, two 32%",
    ledger: "Diffusion is still a conservative flux between cells; only its rate changes.",
    evidence: "In the supercolony, removing it lowers the vascular network's high-to-mid contrast (4/4) and the live count (3/4).",
    chart: "junctions",
  },
  {
    slug: "so2-motor",
    name: "Steering",
    short: "Steering",
    story: "supercolony",
    on: { tissue: true, supercolony: true },
    borrowed: "Chemotaxis, and swimming in circles along a contour",
    rule: "Four new network outputs set how strongly to follow the food gradient, follow the chemical gradient, circle along contours and push forward along the current heading. Every term is built from gradients and the heading, so rotating the world rotates the motion with it. Both presets switch it on.",
    formula: "a ≈ u[(1 + κ_E m_E)∇E + (1 + κ_C m_C)∇C + κ_⊥ m_⊥ ∇⊥] + f·a_max·E/(E + E_s)·m_h·ĥ  (schematic: inherited response weights and the clamp to a_max left out)",
    numbers: "κ_E = κ_C = 2, κ_⊥ = 1, propulsion share 0.5; +4 outputs",
    ledger: "Motion is paid for at the existing speed-squared cost.",
    effect: { value: "−11%", label: "alive in the supercolony without it (19,034 against 21,308, 3/4)" },
    evidence: "Its trails are fainter too (4/4).",
    chart: "motor",
  },

  // ─── Both worlds' budget ──────────────────────────────────────────────────
  {
    slug: "senescence",
    name: "Ageing that costs",
    short: "Ageing",
    story: "shared",
    on: { tissue: true, supercolony: true },
    borrowed: "Rising maintenance cost with age",
    rule: "Upkeep grows with the square of age, up to 64 times the young rate. No dice roll kills an old organism; it costs more to keep alive.",
    formula: "basal maintenance × (1 + min((age/τ)², M − 1))",
    numbers: "τ = 30 s, M = 64: twice the upkeep at 30 s, five times at 60 s, capped from about 238 s",
    ledger: "The extra upkeep is ordinary maintenance, dissipated and booked.",
    chart: "senescence",
  },
  {
    slug: "surplus-emission",
    name: "A reserve that cannot be spent",
    short: "Reserve",
    story: "shared",
    on: { tissue: true, supercolony: true },
    borrowed: "A cell's protected reserve",
    rule: "Emission spends only the energy above a floor, so a starving organism cannot signal itself to death.",
    formula: "spendable = max(E − E_floor, 0)",
    numbers: "E_floor = 0.45",
    ledger: "The same emission ledger; only its budget is capped.",
    chart: "surplus",
  },
  {
    slug: "lysis-recycling",
    name: "Corpses go back to the field",
    short: "Lysis",
    story: "shared",
    on: { tissue: true, supercolony: true },
    borrowed: "Cell lysis and decomposition",
    rule: "Below 0.10 energy an organism lyses. 52.5% of what it held returns to the field carrying the opposite of its diet: poison to its own kind, food to a lineage that eats the opposite.",
    numbers: "viability floor 0.10, recycled fraction 0.525, corpse chemistry −a_eff",
    ledger: "52.5% of the residual is deposited; the rest is booked as unrecycled death. Each joule is counted once.",
    evidence: "In the supercolony, removing it weakens trail contrast and alignment (4/4 each).",
    chart: "deposit",
  },
  {
    slug: "wake-offset",
    name: "Signals left behind",
    short: "Wake",
    story: "shared",
    on: { tissue: true, supercolony: true },
    borrowed: "A swimmer's wake; a polarized cell secreting from one side",
    rule: "Deposits land a little behind where an organism is heading (supercolony) or on the side its polarity points (tissue), so signals trail and orient instead of sitting on the emitter.",
    formula: "deposit at x − δ·v̂ (supercolony) or x + δ·p (tissue)",
    numbers: "δ = 0.125 world units",
    ledger: "Where a deposit lands, not how much.",
    chart: "deposit",
  },

  // ─── Off in both presets ──────────────────────────────────────────────────
  {
    slug: "velocity-alignment",
    name: "Moving together",
    short: "Flocking",
    story: "opt-in",
    on: { tissue: false, supercolony: false },
    borrowed: "Flocking",
    rule: "Each body's velocity is pulled toward its kin neighbours' average; it never speeds a body up.",
    numbers: "strength 0.20",
    ledger: "Purely dissipative: it can only remove kinetic energy.",
    effect: { value: "−38%", label: "alive when switched on in the tissue (9,574 against 15,446)" },
    evidence: "The tissue streams: kin polarization 0.939 against 0.757, kin alignment 0.896 against 0.554 (4/4).",
  },
  {
    slug: "so2-budding",
    name: "Choosing where a child lands",
    short: "Budding",
    story: "opt-in",
    on: { tissue: false, supercolony: false },
    borrowed: "Oriented budding in yeast and plants",
    rule: "Two network outputs bias where a daughter is placed relative to the energy gradient; some random jitter remains.",
    numbers: "+2 outputs; on by default in Bundle B, off in the supercolony preset",
    ledger: "Placement only; the split's energy rules are unchanged.",
    effect: { value: "−14%", label: "alive in the supercolony with it and symplectic waste added together (18,420 against 21,308)" },
    evidence: "The vascular contrast is weaker too.",
  },
  {
    slug: "emt-gate",
    name: "Letting go of the sheet",
    short: "Leaving",
    story: "opt-in",
    on: { tissue: false, supercolony: false },
    borrowed: "Epithelial–mesenchymal transition",
    rule: "One network output scales the organism's own adhesion between none and double, so a cell can leave its tissue.",
    formula: "adhesion × 2σ(o_adh)",
    numbers: "+1 output; off in both presets and in every Stage-2 condition",
    ledger: "Adhesion is a force; no energy changes hands.",
  },

  // ─── Seeing the hidden state ──────────────────────────────────────────────
  {
    slug: "v2-visual-mappings",
    name: "Four new views",
    short: "Views",
    story: "view",
    on: { tissue: true, supercolony: true },
    borrowed: "Stains and fluorescent reporters",
    rule: "Colour organisms by their memory state or polarity, or colour the field by trail energy or vascular conductance. Drawing reads the committed tick only.",
    numbers: "--color cell-state | polarity, --field-mapping trail-carrier | vascular-conductance",
    ledger: "It writes no state: with it off, every world digest is the same.",
  },
];

/** The entry for a slug; every slug has one. */
export function atlasEntry(slug: V2FeatureSlug): AtlasEntry {
  const found = atlas.find((entry) => entry.slug === slug);
  if (!found) throw new Error(`lib/v2-atlas.ts has no entry for ${slug}`);
  return found;
}

/** A world's step-through: its entries in order, less those another step tells. */
export function storySteps(story: "tissue" | "supercolony"): AtlasEntry[] {
  const told = new Set(atlas.map((entry) => entry.also).filter(Boolean));
  return atlas.filter((entry) => entry.story === story && !told.has(entry.slug));
}
