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
// which the difference had the stated sign. The atlas's charts compute the
// rules from components/explain/v2-atlas-math.ts, whose tests pin every value
// stated here.

import type { V2FeatureSlug } from "@/scripts/media-options";

export type AtlasGroup = "world" | "budget" | "agency" | "tissue" | "memory" | "view";

export const atlasGroups: { id: AtlasGroup; title: string; line: string }[] = [
  {
    id: "world",
    title: "A field that changes",
    line: "Food that blooms and fades, membranes that slow it, veins that carve themselves, and a second chemical no one can eat.",
  },
  {
    id: "budget",
    title: "A body's budget",
    line: "Ageing that costs, a reserve that cannot be spent, and nothing wasted: the dead and the leftovers go back to the field.",
  },
  {
    id: "agency",
    title: "Brains get hands",
    line: "New outputs let each network steer and pulse, and new inputs let it smell trails. What it does with them is evolved, not written.",
  },
  {
    id: "tissue",
    title: "Becoming tissue",
    line: "Bodies that touch: they stop dividing when crowded, orient, stick to their own kind and share food with kin.",
  },
  {
    id: "memory",
    title: "Cells with a memory",
    line: "A small inherited state that the network rewrites slowly, which changes what the cell eats and lets neighbours read each other.",
  },
  {
    id: "view",
    title: "Seeing the hidden state",
    line: "Four ways to draw what the new mechanisms hold, read-only: drawing changes no digest.",
  },
];

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
  group: AtlasGroup;
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
  /** What Stage-2 measured when it was removed (or added), with its seed count. */
  evidence?: string;
  /** What this does not show, or what it costs. */
  caveat?: string;
  chart?: AtlasChart;
};

export const atlas: AtlasEntry[] = [
  // ─── A field that changes ─────────────────────────────────────────────────
  {
    slug: "oases",
    name: "Oases that bloom and fade",
    group: "world",
    on: { tissue: false, supercolony: true },
    borrowed: "Seasonal, patchy food: tides, blooms, rain",
    rule: "Each of the 16 food sources swells and shrinks on its own phase of a 24-second cycle, never below 5% of its share. The total power never changes; it moves.",
    formula: "w_j = f_min + (1 − f_min)·max(0, sin 2π(t/T + j/M))²,  P_j = M·P·w_j / Σw",
    numbers: "T = 24 s, f_min = 0.05, M = 16 sources (supercolony)",
    ledger: "Redistribution only: Σ P_j = M·P at every tick, so no energy is created by a bloom.",
    evidence:
      "Without oases the population rises (32,517 against 21,308 alive) but the structure flattens: trail contrast falls by a third (peak z-score 6.82 against 10.24, 4/4), the vascular network stops being bimodal, and organisms align less (−35%, 4/4).",
    chart: "oases",
  },
  {
    slug: "tight-junctions",
    name: "Membranes that slow diffusion",
    group: "world",
    on: { tissue: true, supercolony: true },
    borrowed: "Tight junctions sealing an epithelium",
    rule: "The more organisms sit in a field cell, the less chemistry diffuses through its faces, so a colony holds its own chemical neighbourhood.",
    formula: "φ(n) = φ_min + (1 − φ_min)/(1 + κn), a face taking the harmonic mean of its two cells",
    numbers: "φ_min = 0.04, κ = 1.2: one occupant lets 48% through, two 32%",
    ledger: "Diffusion is still a conservative flux between cells; only its rate changes.",
    evidence: "Supercolony: removing it lowers the vascular network's high-to-mid contrast (4/4) and the live count (3/4).",
    chart: "junctions",
  },
  {
    slug: "vascular-field",
    name: "Veins that carve themselves",
    group: "world",
    on: { tissue: false, supercolony: true },
    borrowed: "Slime-mould (Physarum) tubes, and vessels that widen where flow is high",
    rule: "Every face between two field cells has a conductance. Where energy keeps flowing it widens, where it stops it shrinks back, so the field grows a network of channels between food and eaters.",
    formula: "g ← g + Δt(α·s(r)(1 − g) − γ(g − g_min)),  s = r^μ/(1 + r^μ),  r = |F|/J₀",
    numbers: "α = 0.5/s, γ = 0.1/s, g_min = 0.025, μ = 1.5, J₀ = 0.25 (supercolony): a face under steady strong flux settles near 0.84",
    ledger: "Conductance scales a conservative diffusion flux (at most 0.30 per tick); it moves energy, never makes it.",
    evidence:
      "Bimodal by Sarle's coefficient (0.608, above the 0.556 line), with a 7.0× spread between the 95th and 25th percentiles. Removing it cuts organisms' local alignment from 0.378 to 0.252 and kin alignment from 0.419 to 0.266 (4/4).",
    caveat: "A flux-reinforcement rule in the spirit of Murray's law, not Murray's cube law itself.",
    chart: "vascular",
  },
  {
    slug: "dual-carrier-trails",
    name: "A trail no one can eat",
    group: "world",
    on: { tissue: false, supercolony: true },
    borrowed: "Ant pheromone trails",
    rule: "A fifth of what an organism emits goes into a second carrier that feeds no one, decays slowly and barely spreads. Four new senses let networks read it.",
    numbers: "20% of emission, decay 0.030/s (half-life 23 s against the food field's 8.5 s), diffusion 0.0003; 34 field planes; +4 sensors",
    ledger: "Trail energy is booked separately and decays into the declared ledger; it is never food.",
    evidence:
      "The trails stand out (peak z-score 10.24) and sit apart from the food (decoupling 0.948). They cost: without them 25,695 are alive against 21,308.",
    chart: "trails",
  },

  // ─── A body's budget ──────────────────────────────────────────────────────
  {
    slug: "senescence",
    name: "Ageing that costs",
    group: "budget",
    on: { tissue: true, supercolony: true },
    borrowed: "Rising maintenance cost with age",
    rule: "An organism's upkeep grows with the square of its age until it reaches 64 times the young rate. Nothing kills it by a dice roll; old organisms simply cost more to keep alive.",
    formula: "maintenance × (1 + min((age/τ)², M − 1))",
    numbers: "τ = 30 s, M = 64: twice the upkeep at 30 s, five times at 60 s, capped from about 238 s",
    ledger: "The extra upkeep is ordinary maintenance, dissipated and booked.",
    chart: "senescence",
  },
  {
    slug: "surplus-emission",
    name: "A reserve that cannot be spent",
    group: "budget",
    on: { tissue: true, supercolony: true },
    borrowed: "A cell's protected reserve",
    rule: "Emission can only spend the energy above a floor, so a starving organism cannot signal itself to death.",
    formula: "spendable = max(E − E_floor, 0)",
    numbers: "E_floor = 0.45",
    ledger: "The same emission ledger; only its budget is capped.",
    chart: "surplus",
  },
  {
    slug: "lysis-recycling",
    name: "Corpses go back to the field",
    group: "budget",
    on: { tissue: true, supercolony: true },
    borrowed: "Cell lysis and decomposition",
    rule: "An organism whose energy falls below a viability floor dies, and what it held goes back into the field carrying the opposite of its own diet: poison to its own kind, food to a lineage that eats the opposite.",
    numbers: "viability floor 0.10, corpse chemistry −a_eff",
    ledger: "The residual is recycled through the death ledger once, never also dissipated.",
    evidence: "Supercolony: removing it weakens trail contrast and alignment (4/4 each).",
    chart: "deposit",
  },
  {
    slug: "symplectic-waste",
    name: "Waste someone else can eat",
    group: "budget",
    on: { tissue: true, supercolony: false },
    borrowed: "Cross-feeding: one microbe's waste is another's food",
    rule: "Of the food an organism takes up but does not convert, 45% goes back as a byproduct whose chemistry is turned a right angle from its own diet: exactly neutral to itself, food or poison to others.",
    formula: "byproduct along J·a_eff, with a_eff · J·a_eff = 0",
    numbers: "ρ = 0.45 (tissue; on by default in Bundle B, off in the supercolony preset)",
    ledger: "Byproduct energy comes out of the unconverted uptake; the ledger balances to the quantum.",
    evidence: "Tissue: with it, local polarization (+0.026) and both pairwise alignments (+0.043, +0.032) are higher on all 4 seeds; without it slightly more are alive (16,695 against 15,446).",
    caveat: "“Symplectic” names the rotation J, a 90° turn in each pair of chemical channels; nothing here is Hamiltonian mechanics.",
    chart: "deposit",
  },

  // ─── Brains get hands ─────────────────────────────────────────────────────
  {
    slug: "so2-motor",
    name: "Steering",
    group: "agency",
    on: { tissue: true, supercolony: true },
    borrowed: "Chemotaxis, and swimming in circles along a contour",
    rule: "Four new network outputs set how strongly to follow the food gradient, follow the chemical gradient, circle along contours, and push forward along the current heading. Every term is built from gradients and the heading, so turning the world turns the motion with it.",
    formula: "a = u[(1 + κ_E m_E)∇E + (1 + κ_C m_C)∇C + κ_⊥ m_⊥ ∇⊥] + f·a_max·E/(E + E_s)·m_h·ĥ",
    numbers: "κ_E = κ_C = 2, κ_⊥ = 1, propulsion share 0.5; +4 outputs",
    ledger: "Motion is paid for at the existing speed-squared cost.",
    evidence: "Supercolony: without it 19,034 are alive against 21,308 (3/4), and trails are fainter (4/4).",
    chart: "motor",
  },
  {
    slug: "so2-budding",
    name: "Choosing where a child lands",
    group: "agency",
    on: { tissue: false, supercolony: false },
    borrowed: "Oriented budding in yeast and plants",
    rule: "Two network outputs choose the direction a daughter is placed, relative to the gradients, instead of at random.",
    numbers: "+2 outputs; on by default in Bundle B, off in the supercolony preset",
    ledger: "Placement only; the split's energy rules are unchanged.",
    evidence: "Tested with symplectic waste as one addition: 18,420 alive against 21,308, and a weaker vascular contrast.",
  },
  {
    slug: "wake-offset",
    name: "Signals left behind",
    group: "agency",
    on: { tissue: true, supercolony: true },
    borrowed: "A swimmer's wake; a polarized cell secreting from one side",
    rule: "In the supercolony an organism deposits a little behind where it is going; in the tissue, on the side its polarity points. Signals then trail and orient instead of sitting on the emitter.",
    formula: "deposit at x − δ·v̂ (supercolony) or x + δ·p (tissue)",
    numbers: "δ = 0.125 world units",
    ledger: "Where a deposit lands, not how much.",
    chart: "deposit",
  },
  {
    slug: "excitable-relay",
    name: "Pulses instead of a trickle",
    group: "agency",
    on: { tissue: false, supercolony: true },
    borrowed: "Excitable media: neurons, Dictyostelium cAMP waves",
    rule: "Each organism has a readiness that recovers over about a second. Emission only happens as a burst, five times the normal rate, when readiness is above one half; a burst spends 85% of it. Neighbours that sense a burst may burst in turn, so signals travel as waves.",
    numbers: "threshold 0.5, depletion 0.85, recovery τ = 1 s, burst ×5: an always-willing emitter fires every 21 ticks",
    ledger: "Readiness holds no energy; bursts spend surplus only.",
    evidence:
      "Removing it costs 37% of the population (13,458 against 21,308 alive, 4/4) and lets trails smear 4.3 times brighter on average instead of staying crisp.",
    chart: "relay",
  },

  // ─── Becoming tissue ──────────────────────────────────────────────────────
  {
    slug: "hippo-crowding",
    name: "Stop dividing when crowded",
    group: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "The Hippo–YAP pathway: contact inhibition of growth",
    rule: "Counting the neighbours within 0.375 world units, an organism's chance to divide falls linearly and reaches zero at four. Its network also senses the count.",
    formula: "gate = clamp(1 − N / c_max, 0, 1)",
    numbers: "c_max = 3.25: 1, 0.69, 0.38, 0.08, then 0 for 0–4 neighbours; +1 sensor",
    ledger: "A gate on births; no energy moves.",
    evidence:
      "Against the same preset with every v2 feature off (so not this one alone): 4.0× fewer overlapping neighbour pairs (30,616 against 123,294) and higher coherence on all four alignment measures (+0.135 to +0.144, 4/4).",
    chart: "hippo",
  },
  {
    slug: "pcp-polarity",
    name: "A front and a back",
    group: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Planar cell polarity in epithelia",
    rule: "Each body carries a polarity that turns toward its neighbours' and toward the local gradient. Adhesion is stronger side by side than end to end, cells wedge slightly, and daughters are placed beside the parent, so sheets form.",
    numbers: "turning mobility 2.0, gradient cue 0.25, initial strength up to 0.88, wedge curvature up to 0.35",
    ledger: "Forces between bodies move them; energy is not exchanged.",
    evidence: "Removing it lowers kin polarization (4/4) and local polarization (3/4) a little (about 0.02).",
  },
  {
    slug: "emt-gate",
    name: "Letting go of the sheet",
    group: "tissue",
    on: { tissue: false, supercolony: false },
    borrowed: "Epithelial–mesenchymal transition",
    rule: "One network output scales the organism's own adhesion between none and double, so a cell can choose to leave its tissue.",
    formula: "adhesion × 2σ(o_adh)",
    numbers: "+1 output; opt-in, off in both presets",
    ledger: "Adhesion is a force; no energy moves.",
  },
  {
    slug: "cadherin-4d",
    name: "Sticking to your own kind",
    group: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Cadherins: the proteins that sort cells by type",
    rule: "Each organism inherits a four-dimensional tag. Two bodies stick as strongly as their tags point the same way, so who sticks to whom is inherited separately from what each eats.",
    formula: "adhesion = min(s_i, s_j)·max(0, cos θ)²",
    numbers: "tags mutate with σ = 0.05",
    ledger: "Adhesion is a force; no energy moves.",
    evidence: "Removing it costs 1,496 organisms (−10%, 4/4) and 6,589 units of shared energy.",
    chart: "cadherin",
  },
  {
    slug: "trophallaxis",
    name: "Feeding your kin",
    group: "tissue",
    on: { tissue: true, supercolony: false },
    borrowed: "Social insects passing food mouth to mouth",
    rule: "Touching organisms of similar diet pass energy from the richer to the poorer, faster the closer and more alike they are. A donor keeps a reserve and gives at most a quarter of its energy per tick; 5% of every transfer is lost.",
    formula: "Δt·G·alike⁴·(1 − d/R)·(E_i − E_j), alike = max(0, cos(a_i, a_j)) ≥ 0.5",
    numbers: "G = 2/s, efficiency 0.95, reserve 0.25, at most 25% per tick, exact in quanta of 2⁻³⁰",
    ledger: "Both sides round to the same quantum; the 5% loss is booked as trophallaxis loss.",
    evidence: "Active in all 16 measurement windows on all 4 seeds, 79,910 units moved per run.",
    caveat: "Removing it leaves the population about the same (15,579 against 15,446): the transfers are real, but at 2,048 ticks they do not raise the count.",
    chart: "kin",
  },
  {
    slug: "velocity-alignment",
    name: "Moving together",
    group: "tissue",
    on: { tissue: false, supercolony: false },
    borrowed: "Flocking",
    rule: "Each body's velocity is pulled toward its kin neighbours' average, never speeding it up.",
    numbers: "strength 0.20; opt-in",
    ledger: "Purely dissipative: it can only remove kinetic energy.",
    evidence:
      "Turned on, it makes the tissue stream (kin alignment 0.939 against 0.757, 4/4) but costs a third of the population (9,574 against 15,446), as flocks overshoot their food.",
  },

  // ─── Cells with a memory ──────────────────────────────────────────────────
  {
    slug: "epigenetic-recurrence",
    name: "A slow memory that changes diet",
    group: "memory",
    on: { tissue: true, supercolony: false },
    borrowed: "Epigenetic cell state and differentiation",
    rule: "Each organism carries two state values that its network rewrites a little each tick, gated so that it changes slowly unless pushed. The first turns what the organism eats; the second changes how it adheres. Daughters inherit the state, and in the tissue one daughter is biased toward the stem state.",
    formula: "h′ = (1 − g)h + g·tanh c,  g = σ(z − 2.5);  a_eff = cos(πh₀/2)·a + sin(πh₀/2)·Ja",
    numbers: "2 state values; resting gate 0.076; +2 sensors, +4 raw outputs",
    ledger: "State holds no energy.",
    evidence: "Within clones, the state varies: 32.6% of its variance lies inside lineages, more than when networks cannot evolve (fixed-controller, 28.9%, 4/4).",
    caveat:
      "At 2,048 ticks the state barely tracks crowding (r = +0.069), so there is no claim yet of distinct core and sheath cell types.",
    chart: "turn",
  },
  {
    slug: "notch-delta",
    name: "Reading the neighbours' state",
    group: "memory",
    on: { tissue: true, supercolony: false },
    borrowed: "Notch–Delta lateral inhibition",
    rule: "A new sense reports the kin-weighted average of the neighbours' first state value. Whether a cell then differs from its neighbours is up to its network; nothing forces it.",
    numbers: "+1 sensor; requires epigenetic recurrence",
    ledger: "A sense; no energy moves.",
    evidence: "Removing it changes little at 2,048 ticks (15,725 against 15,446 alive).",
  },

  // ─── Seeing the hidden state ──────────────────────────────────────────────
  {
    slug: "v2-visual-mappings",
    name: "Four new views",
    group: "view",
    on: { tissue: true, supercolony: true },
    borrowed: "Stains and fluorescent reporters",
    rule: "Colour organisms by their memory state or polarity, or colour the field by trail energy or vascular conductance. Drawing reads the committed epoch only.",
    numbers: "--color cell-state | polarity, --field-mapping trail-carrier | vascular-conductance",
    ledger: "No state is written: disabling it leaves every world digest unchanged.",
  },
];
