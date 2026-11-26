// Every word and number on the site outside the explainers (components/explain/,
// whose figures cite their sources in their own captions). Each figure here is
// one the repository itself states (README.md, the plan, the crates'
// CONTRACT.md files, experiments/fl048, experiments/showcase and
// experiments/v2-emergence), one a film's or still's manifest records
// (lib/films/*.json, lib/stills.json), or, in `branchObservation`, one
// this page observed and dates; none is a target dressed as a result.

import { filmById } from "@/lib/media";

// Chapter 1 zooms into the confluent film; its organisms on its first frame, from flife render's record of that frame.
const filmAlive = (filmById("confluent").world.live[0] ?? 0).toLocaleString("en-US");

export const siteConfig = {
  name: "FrankenLife",
  // Where Cloudflare serves the site: wrangler.jsonc's custom domain (www. redirects here).
  url: "https://frankenlife.org",
  title: "FrankenLife: artificial life you can watch, replay and check",
  description:
    "A real-time artificial-life laboratory in memory-safe Rust. Organisms with their own heritable neural networks feed, signal, move, split and die through one shared energy field, and every committed tick is deterministic to the bit.",
  github: "https://github.com/Dicklesworthstone/frankenlife",
  plan: "https://github.com/Dicklesworthstone/frankenlife/blob/main/COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md",
  readme: "https://github.com/Dicklesworthstone/frankenlife#readme",
  fl048: "https://github.com/Dicklesworthstone/frankenlife/blob/main/experiments/fl048/README.md",
  showcase: "https://github.com/Dicklesworthstone/frankenlife/blob/main/experiments/showcase/README.md",
  v2Report: "https://github.com/Dicklesworthstone/frankenlife/blob/main/experiments/v2-emergence/README.md",
  websiteFilms: "https://github.com/Dicklesworthstone/frankenlife/blob/main/experiments/website-films/README.md",
  license: "https://github.com/Dicklesworthstone/frankenlife/blob/main/LICENSE",
  agents: "https://github.com/Dicklesworthstone/frankenlife/blob/main/AGENTS.md",
};

// What the films show, in four tiles (the manifests in lib/films/, experiments/showcase/README.md,
// experiments/website-films/README.md, web/scripts/director.ts). lib/film-configs.test.ts checks the
// ceiling against every film's frames. How the worlds were chosen is in those READMEs (siteConfig links).
export const filmFacts = {
  ceiling: 262_144,
  tiles: [
    {
      value: "262,144",
      label: "alive in every frame",
      body: "The world's population ceiling. Each organism is a body 0.25 units across on an 80-unit torus.",
    },
    {
      value: "Hue · light",
      label: "lineage · its own energy",
      body: "A founder's descendants share a hue. Each is lit by its own energy: black with none, white at 1.8 times what it needs to divide.",
    },
    {
      value: "1 switch",
      label: "between the films",
      body: "The v2 films regrow the confluent world from tick 0 with one mechanism on, veins or polarity, at the engine's defaults. With it off, the run reaches the confluent film's own digest.",
    },
    {
      value: "From the run",
      label: "where the camera looks",
      body: "A small preview of the same frames finds where organisms changed most, and each shot eases there. The zooms and timings are set by hand.",
    },
  ],
};

/**
 * What each film is, for its tab and the line beneath it (lib/films/index.ts
 * lists the films; each manifest records how it was made). A world with no
 * preset names the committed configuration it ran (`config`, whose
 * config_hash is the manifest's) and the one v2 mechanism it switches on.
 */
export const filmNotes: Record<
  string,
  { tab: string; chip: string; title: string; line: string; config?: string; mechanism?: string }
> = {
  confluent: {
    tab: "Confluent",
    chip: "v1 · contact",
    title: "Confluent world",
    line: "field-ecology-v1-contact at its population ceiling of 262,144, drawn as spheres coloured by lineage and lit by their own energy. No v2 mechanism is on.",
  },
  // The ceiling and the one mechanism are checked against each config by lib/film-configs.test.ts.
  "confluent-veins": {
    tab: "Veins",
    chip: "v2 · vascular-field",
    title: "Confluent world, with veins",
    config: "experiments/website-films/confluent-veins.toml",
    mechanism: "vascular-field",
    line: "field-ecology-v2 with one mechanism on, vascular-field: faces of the field that keep carrying energy widen and the rest shrink back. The confluent world's founders, seed and food, grown from tick 0, at the population ceiling of 262,144 in every frame and drawn the same way.",
  },
  "confluent-polarity": {
    tab: "Polarity",
    chip: "v2 · pcp-polarity",
    title: "Confluent world, with polarity",
    config: "experiments/website-films/confluent-polarity.toml",
    mechanism: "pcp-polarity",
    line: "field-ecology-v2 with one mechanism on, pcp-polarity: each body's polarity turns toward its neighbours' and the local gradient, and daughters land beside the parent. The confluent world's founders, seed and food, grown from tick 0, at the population ceiling of 262,144 in every frame and drawn the same way.",
  },
};

/**
 * The page's six chapters, in order. Each wears one colour of the film's
 * lineage palette (`LINEAGE[colour]`, lib/palette.ts), as the film colours each
 * founder's descendants; `short` is the phone dock's label.
 */
export type Chapter = {
  id: string;
  number: string;
  short: string;
  label: string;
  title: string;
  thesis: string;
  colour: number;
  sections: { id: string; label: string }[];
};

export const chapters: Chapter[] = [
  {
    id: "world",
    number: "01",
    short: "World",
    label: "The world",
    title: "Every speck is an [individual].",
    thesis:
      `${filmAlive} organisms at once, each with a body, an energy budget and its own neural network. Zoom in to one.`,
    colour: 0,
    sections: [
      { id: "zoom", label: "From the world to one brain" },
      { id: "what", label: "One tick in a life" },
      { id: "alive", label: "A toy world to run" },
    ],
  },
  {
    id: "different",
    number: "02",
    short: "Different",
    label: "Why it is different",
    title: "The structure is [earned].",
    thesis:
      "Artificial-life imagery is easy to fake. FrankenLife forbids the shortcuts, and its controls show the structure vanish when a cause is removed.",
    colour: 5,
    sections: [
      { id: "different", label: "Four forbidden shortcuts" },
      { id: "controls", label: "Remove a cause" },
      { id: "film", label: "What the films are" },
    ],
  },
  {
    id: "machinery",
    number: "03",
    short: "How",
    label: "How it works",
    title: "The [machinery].",
    thesis:
      "Independent brains, an explicit model with one energy ledger, rules pinned by answers you can compute by hand, and a tick that is published whole or not at all.",
    colour: 3,
    sections: [
      { id: "brains", label: "Independent brains" },
      { id: "world", label: "The model" },
      { id: "rules", label: "Check it by hand" },
      { id: "tick", label: "One tick" },
    ],
  },
  {
    id: "v2",
    number: "04",
    short: "V2",
    label: "Version 2",
    title: "Tissue and [supercolonies].",
    thesis:
      "21 mechanisms, each a switch. Two presets combine them into a tissue and a supercolony; 104 runs measured each one's removal.",
    colour: 1,
    sections: [
      { id: "bundles", label: "Two worlds" },
      { id: "tissue", label: "The tissue" },
      { id: "supercolony", label: "The supercolony" },
      { id: "shared", label: "The other switches" },
      { id: "ablations", label: "Take one away" },
      { id: "configure", label: "Compose your own" },
      { id: "unshown", label: "Not shown yet" },
    ],
  },
  {
    id: "evidence",
    number: "05",
    short: "Evidence",
    label: "What it can show",
    title: "Claims, [kept apart].",
    thesis:
      "Replays that land on the same digest, experiments declared before they ran, the scale in arithmetic, and what is not done.",
    colour: 4,
    sections: [
      { id: "replay", label: "Rewind and branch" },
      { id: "science", label: "What the experiments found" },
      { id: "scale", label: "Why the scale is hard" },
      { id: "status", label: "Honest status" },
    ],
  },
  {
    id: "lab",
    number: "06",
    short: "Lab",
    label: "Use it",
    title: "Your [laboratory].",
    thesis:
      "Ten worlds to open, a native Studio to watch and touch them, and a command line that records everything.",
    colour: 7,
    sections: [
      { id: "worlds", label: "Ten worlds" },
      { id: "studio", label: "Studio and command line" },
      { id: "stack", label: "The Franken stack" },
      { id: "try", label: "Run it yourself" },
    ],
  },
];

export type LifeStep = {
  id: string;
  name: string;
  /** Who acts: the shared field, or one organism. */
  actor: "field" | "organism";
  /** Where it happens in the ordered tick (README "One tick"). */
  phases: string;
  body: string;
  /** Where on this page the rule behind it can be checked. */
  link?: { href: string; label: string };
};

// One organism's life, in the order the tick runs it (README "How the world works", plan §4–§8).
export const lifeCycle: LifeStep[] = [
  {
    id: "sources",
    name: "Sources",
    actor: "field",
    phases: "phase 2",
    body: "Seeded Gaussian patches drift across the torus on smooth paths. Each injects a fixed power per simulated second, with a fixed chemical direction. Apart from a researcher's brush, they are the only food that enters the world.",
  },
  {
    id: "field",
    name: "Field",
    actor: "field",
    phases: "phases 2 and 9",
    body: "Every cell holds energy and a 32-channel signed chemical signature that can never exceed it. One positive stencil spreads both, so energy is conserved before its declared decay.",
    link: { href: "#rules", label: "Signals cannot counterfeit food (C.3)" },
  },
  {
    id: "sense",
    name: "Sense",
    actor: "organism",
    phases: "phases 3–4",
    body: "Each organism reads energy, the 32 chemical channels and their gradients at its own inherited sensing scale, plus its own energy, age, velocity and recent change: 104 numbers.",
    link: { href: "#brains", label: "The 104 inputs" },
  },
  {
    id: "decide",
    name: "Decide",
    actor: "organism",
    phases: "phase 4",
    body: "Its own network, whose weights no other organism shares, turns those 104 numbers into 36: when to split, how much to give a child, whether to emit, what chemistry and how much.",
    link: { href: "#brains", label: "Every organism, its own brain" },
  },
  {
    id: "feed",
    name: "Feed",
    actor: "organism",
    phases: "phases 5–6",
    body: "It draws on the cells beneath it, and a contended cell shares in proportion. Its inherited affinity decides whether each packet feeds or poisons it.",
    link: { href: "#rules", label: "Who eats, by hand (C.1, C.9)" },
  },
  {
    id: "move",
    name: "Move",
    actor: "organism",
    phases: "phase 7",
    body: "No network output moves it. Inherited responses to the field's gradients accelerate it, drag slows it, and moving costs energy in proportion to speed squared.",
  },
  {
    id: "emit",
    name: "Emit",
    actor: "organism",
    phases: "phase 7, sensed after phase 9",
    body: "It can pay energy to write the chemistry its network chose into the field. Neighbours sense it on the next tick, and the field never receives more than was paid, so no loop can amplify energy.",
  },
  {
    id: "divide",
    name: "Divide",
    actor: "organism",
    phases: "phases 7–8",
    body: "A split costs energy and makes a child with a mutated copy of the parent's genome. Identity, genome, slot and energy transfer commit together, or nothing happens.",
    link: { href: "#rules", label: "A birth is a transaction (C.5)" },
  },
  {
    id: "die",
    name: "Die",
    actor: "organism",
    phases: "phase 8, returned in 9",
    body: "An organism with zero energy dies; the check runs after every stage. Part of what it held returns to the field as food and chemistry.",
  },
];

// README TL;DR: "A shared-weight network, an unaccounted energy source, a cull that keeps the screen
// attractive, or a renderer benchmarked with inference switched off can each produce a gorgeous video
// of nothing evolving." Each shortcut beside the rule that forbids it.
export const shortcuts = [
  {
    shortcut: "One network, copied",
    temptation: "Hundreds of thousands of organisms that are really one shared policy: cheap, fast and smooth.",
    rule: "Every organism carries its own weights. Organisms with the same topology are batched to share a kernel and a memory layout but no parameters, and a rare topology gets a correct fallback that runs the same controller.",
    refs: ["INV-13", "plan §6.5"],
  },
  {
    shortcut: "Energy from nowhere",
    temptation: "A world that thrives on food nobody paid for, or an overspent cell quietly zeroed.",
    rule: "An energy ledger closes on every committed tick: total energy changes only by injection, removal and dissipation, within a declared residual. No correction may add energy to make it balance.",
    refs: ["INV-03", "plan §4.4"],
  },
  {
    shortcut: "A quiet cull",
    temptation: "Ugly regions removed, an extinct world reseeded, an aesthetic score steering selection.",
    rule: "No hidden culling, no reseeding after extinction and no aesthetic fitness inside the world. A capacity limit is explicit and recorded, and failed and extinct runs are kept.",
    refs: ["INV-12", "plan §7.5, §15.5"],
  },
  {
    shortcut: "A renderer with no brains",
    temptation: "A smooth frame rate from a world that is not thinking, or one that skips ticks under load.",
    rule: "The Studio reports committed ticks per second and simulated time apart from frames per second. The engine never skips ticks, enlarges Δt or lowers neural cadence under load while keeping the model's name.",
    refs: ["plan §12.2", "§22.5"],
  },
];

// README TL;DR: what exists, and what FrankenLife composes. Each line says what the other work shows,
// in the README's words; none of it claims what that work lacks.
export const priorWork = [
  { name: "Neural cellular automata", shows: "learn one shared local rule" },
  { name: "Lenia, Flow-Lenia, Particle Lenia", shows: "show what continuous local rules, mass conservation and interactive exploration can do" },
  { name: "ScriptBots-style simulators", shows: "give agents direct interactions and pluggable brains" },
  { name: "Neural-Kimchi-4B", shows: "shows the target look at the target scale" },
];

export const composes = [
  { title: "An independent heritable brain for every organism", body: "Weights start random and change only by inherited mutation. Nothing is trained by gradient." },
  { title: "Field-only interaction under an energy ledger", body: "Organisms touch each other only by eating and writing the shared field, and the books balance every tick." },
  { title: "A deterministic tick that survives cancellation and crashes", body: "Checkpoint, kill, resume and branch: the same commands reach the same world, bit for bit." },
  { title: "Live, inspectable, recordable animation of that same state", body: "FrankenManim draws committed epochs. Watching, zooming or recording never changes the world." },
];

// What this page observed when it checked its own branch commands: the reference preset kept for 512 ticks,
// stepped 64, branched as a control and with one energy brush, all three resumed to tick 1,024. Run on
// 2026-11-21 with the flife-studio target's release `flife` (built from the working tree on 2026-11-20,
// SHA-256 a159e6bf…), controllers on Metal, 8 CPU workers.
export const branchObservation = {
  date: "2026-11-21",
  host: "Apple M5 Ultra, macOS 27.0.1",
  binary: "a159e6bfbf1e8b8e",
  tick: 1024,
  from: 576,
  brush: "256 energy at cell (64, 64), tick 640",
  digests: [
    { name: "The parent, resumed", digest: "2ade32c2f3e74f6f54181019774181e2aa0857bd79bd6c5a6f774f85e46a8183", same: true },
    { name: "Control branch", digest: "2ade32c2f3e74f6f54181019774181e2aa0857bd79bd6c5a6f774f85e46a8183", same: true },
    { name: "Brushed branch", digest: "0d910c5d40a76a1511bcef2eafccdf7bb748a60460fb81519aa4d91f785d8cc7", same: false },
  ],
};

export type Stat = { value: string; label: string; note: string };

export const heroStats: Stat[] = [
  {
    value: "9,988",
    label: "parameters per brain",
    note: "The default network, 104 → 64 → 32 → 36. No two organisms share one.",
  },
  {
    value: "21",
    label: "v2 mechanisms, each a switch",
    note: "Set when a world is founded. All off, a v2 run is v1 bit for bit; 104 of 104 Stage-2 runs balanced their ledger to 10⁻⁸.",
  },
  {
    value: "Bit for bit",
    label: "CPU = Metal",
    note: "Controllers, field, sensing and uptake on the Mac's GPU reach the CPU reference's world exactly.",
  },
  {
    value: "≈10/s",
    label: "ticks, 100k organisms",
    note: "The Desktop ecology world on 16 workers of an M5 Ultra. Measured, not a passed gate.",
  },
];

export const worldModel = [
  {
    name: "World",
    body: "A two-dimensional periodic torus. Organisms have continuous positions; the field is a regular grid.",
  },
  {
    name: "Field",
    body: "Each cell holds energy and a 32-component signed chemical moment. One positive five-point stencil diffuses both, conserving energy before declared decay.",
  },
  {
    name: "Sources",
    body: "Seeded Gaussian patches drift across the torus on smooth paths, each injecting a fixed power with a fixed chemical direction per simulated second.",
  },
  {
    name: "Sensing",
    body: "Energy, 32 composition values and their gradients at a heritable sensing scale, plus the organism's own energy, age, velocity and recent energy change: 104 inputs.",
  },
  {
    name: "Brain",
    body: "Dense tanh networks with a linear output layer. Offspring can move between catalog topologies of 1,164 to 29,316 parameters.",
  },
  {
    name: "Decisions",
    body: "36 baseline outputs: a reproduction hazard, a child's energy share, an emission gate, a 32-dimensional emission direction and a spending rate, with no motor output on v1 (modular field-ecology-v2 appends optional SO(2) motor, budding, EMT and recurrent channels; §04.6).",
  },
  {
    name: "Uptake",
    body: "A two-pass demand and grant step. A contended cell grants in whole quanta, so no cell is overdrawn and thread order never decides who eats.",
  },
  {
    name: "Chemistry",
    body: "A heritable affinity decides whether each granted packet feeds or poisons. Toxicity is clipped to the energy the organism actually holds.",
  },
  {
    name: "Motion",
    body: "Heritable responses to gradients give an acceleration; drag is integrated exactly, and movement costs energy in proportion to speed squared.",
  },
  {
    name: "Emission",
    body: "An organism can pay energy to deposit chemistry at an efficiency of at most one. Deposits are first sensed on the next tick, so no loop can amplify energy.",
  },
  {
    name: "Reproduction",
    body: "A split makes a mutated child. Identity, genome, slot and energy transfer commit together, or nothing happens.",
  },
  {
    name: "Death",
    body: "An organism with zero energy dies; the check runs after every stage. Part of the residue returns to the field.",
  },
];

export const tickPhases = [
  "Admit commands at their recorded tick boundary",
  "Transport the field and inject sources into one immutable observation field",
  "Build the sensing pyramid and spatial bins",
  "Sense and evaluate due controllers, all from the same epoch",
  "Resolve uptake demand into cell grants",
  "Settle useful gain, toxicity and maintenance",
  "Propose affordable motion, then emission, then reproduction",
  "Resolve capacity, construct births and collect deaths, in stable identity order",
  "Reduce deposits and recycling into the next field",
  "Validate the invariants and publish state, events and accounting together",
];

// Modular `field-ecology-v2` feature registry (crates/flife-core/src/v2.rs, V2Feature::ALL;
// `flife --list-features`). Every `field-ecology-v1*` config omits `[v2]` so v1 hashes,
// descriptors and checkpoints stay byte-identical.
export type V2FeatureEntry = {
  slug: string;
  bundleA: boolean;
  bundleB: boolean;
  boundary: "pure-field" | "mode-dependent" | "contact" | "presentation";
  abi: string;
  summary: string;
};

export const v2Features: V2FeatureEntry[] = [
  {
    slug: "oases",
    bundleA: false,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "Sixteen food sources bloom and fade on staggered phases of a 24 s cycle; total power is constant.",
  },
  {
    slug: "senescence",
    bundleA: true,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "Upkeep grows with the square of age, up to 64× the young rate; a cost, never a dice roll.",
  },
  {
    slug: "surplus-emission",
    bundleA: true,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "Emission spends only the energy above a 0.45 floor, so no organism signals itself to death.",
  },
  {
    slug: "excitable-relay",
    bundleA: false,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "Emission comes only in bursts, when a recovering readiness passes one half.",
  },
  {
    slug: "wake-offset",
    bundleA: true,
    bundleB: true,
    boundary: "pure-field",
    abi: "+1 action (neural pump mode)",
    summary: "Deposits land a little behind the emitter's motion, or on the side its polarity points.",
  },
  {
    slug: "so2-motor",
    bundleA: true,
    bundleB: true,
    boundary: "pure-field",
    abi: "+4 actions",
    summary: "Four outputs steer along food and chemical gradients, around contours and along the heading.",
  },
  {
    slug: "so2-budding",
    bundleA: false,
    bundleB: true,
    boundary: "pure-field",
    abi: "+2 actions",
    summary: "Two outputs bias where, relative to the energy gradient, a daughter is placed.",
  },
  {
    slug: "hippo-crowding",
    bundleA: true,
    bundleB: false,
    boundary: "mode-dependent",
    abi: "+1 sensor (if exposed)",
    summary: "Division stops as neighbours crowd in: the birth gate reaches zero at four.",
  },
  {
    slug: "pcp-polarity",
    bundleA: true,
    bundleB: false,
    boundary: "contact",
    abi: "none",
    summary: "Each body keeps a polarity that aligns with its neighbours; daughters are placed beside it.",
  },
  {
    slug: "emt-gate",
    bundleA: false,
    bundleB: false,
    boundary: "contact",
    abi: "+1 action",
    summary: "One output scales the organism's own adhesion, so a cell can leave its tissue.",
  },
  {
    slug: "tight-junctions",
    bundleA: true,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "Occupied field cells let chemistry through more slowly: 48% with one organism, 32% with two.",
  },
  {
    slug: "lysis-recycling",
    bundleA: true,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "Below 0.10 energy an organism lyses; about half its energy returns, carrying the opposite of its diet.",
  },
  {
    slug: "symplectic-waste",
    bundleA: true,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "45% of unconverted food returns as a byproduct turned a right angle from the eater's own diet.",
  },
  {
    slug: "epigenetic-recurrence",
    bundleA: true,
    bundleB: false,
    boundary: "pure-field",
    abi: "+S sensors, +2S raw outputs",
    summary: "Two inherited state values, rewritten slowly by the network; the first turns the diet.",
  },
  {
    slug: "notch-delta",
    bundleA: true,
    bundleB: false,
    boundary: "contact",
    abi: "+1 sensor (neighbor-mean mode)",
    summary: "A sense of the kin-weighted mean of the neighbours' first state value.",
  },
  {
    slug: "trophallaxis",
    bundleA: true,
    bundleB: false,
    boundary: "contact",
    abi: "none",
    summary: "Touching kin pass energy from richer to poorer, in exact quanta, losing 5% of each transfer.",
  },
  {
    slug: "cadherin-4d",
    bundleA: true,
    bundleB: false,
    boundary: "contact",
    abi: "none",
    summary: "An inherited 4-D tag decides who sticks to whom, separately from diet.",
  },
  {
    slug: "velocity-alignment",
    bundleA: false,
    bundleB: false,
    boundary: "contact",
    abi: "none",
    summary: "Each body's velocity is pulled toward its kin neighbours' mean; it never speeds anyone up.",
  },
  {
    slug: "vascular-field",
    bundleA: false,
    bundleB: true,
    boundary: "pure-field",
    abi: "none",
    summary: "Field faces widen where energy keeps flowing and shrink back where it stops: self-carved veins.",
  },
  {
    slug: "dual-carrier-trails",
    bundleA: false,
    bundleB: true,
    boundary: "pure-field",
    abi: "+4 sensors, 34 field planes",
    summary: "A fifth of emission goes into a second carrier no one can eat: a slow trail with 4 new senses.",
  },
  {
    slug: "v2-visual-mappings",
    bundleA: true,
    bundleB: true,
    boundary: "presentation",
    abi: "none (INV-09 read-only)",
    summary: "Four read-only views: cell state, polarity, trail carrier and vascular conductance.",
  },
];

/** The two v2 presets, side by side (presets/*.toml; `flife validate`; experiments/v2-emergence/README.md). */
export const v2Bundles = [
  {
    id: "tissue",
    name: "Tissue",
    tag: "Bundle A",
    preset: "presets/morphogenesis-tissue-v2.toml",
    facts: ["10,954 founders", "14 switches on", "bodies 0.25 units across that push and stick", "108 inputs → 44 outputs"],
    picture:
      "Cells stop dividing in a crowd, orient, stick to cells with matching tags and pass food to kin. The preset grows porous islands, often several lineages side by side.",
    measured: [
      { label: "Alive at tick 2,048", value: "15,446", note: "mean of 4 seeds" },
      { label: "Close neighbours", value: "½", note: "as many as with v2 off: 2.0 against 3.6; 4/4" },
      { label: "Energy passed to kin", value: "79,910", note: "units per run, in every window" },
    ],
    // One of this preset's mechanisms, switched on alone in the confluent world (filmNotes, filmFacts).
    // The film's is at the engine's defaults; this preset starts bodies more strongly polarised (0.88, not 0.3).
    film: "confluent-polarity",
    filmLine: "One of the films at the top of the page switches on polarity, one of this preset's mechanisms, alone in the confluent world, at the engine's default settings rather than this preset's.",
  },
  {
    id: "supercolony",
    name: "Supercolony",
    tag: "Bundle B",
    preset: "presets/supercolony-v2.toml",
    facts: ["16,384 founders", "11 switches on", "points: nothing pushes or pulls (INV-08)", "108 inputs → 40 outputs"],
    picture:
      "Compact, moving colonies with well-fed, brightly lit rims, between food sources that bloom and fade. All they share passes through the field: bursts of emission, slow trails and the veins that flow carves in it.",
    measured: [
      { label: "Alive at tick 2,048", value: "21,308", note: "mean of 4 seeds" },
      { label: "Alignment from veins", value: "+50%", note: "0.378 against 0.252 without; 4/4" },
      { label: "Alive from bursts", value: "+58%", note: "against no relay; 4/4" },
    ],
    // The film's veins are at the engine's defaults; this preset saturates the flux later (0.25, not 0.1).
    film: "confluent-veins",
    filmLine: "One of the films at the top of the page switches on veins, one of this preset's mechanisms, alone in the confluent world, at the engine's default settings rather than this preset's.",
  },
] as const;

/**
 * Stage-2 ablations: mean live count at tick 2,048 over 4 seeds, per condition
 * (experiments/v2-emergence/README.md §4.1, §4.2). `kind` says what changed.
 */
export type AblationRow = { condition: string; live: number; kind: "treatment" | "control" | "minus" | "plus"; note?: string };

export const v2Ablations: { bundle: string; preset: string; rows: AblationRow[] }[] = [
  {
    bundle: "Tissue (Bundle A)",
    preset: "morphogenesis-tissue-v2",
    rows: [
      { condition: "as committed", live: 15446, kind: "treatment" },
      { condition: "every v2 feature off", live: 34489, kind: "control", note: "packed slabs, 1.8× the close neighbours per cell" },
      { condition: "decisions removed", live: 27603, kind: "control", note: "less coherent on all 4 seeds" },
      { condition: "networks cannot evolve", live: 15186, kind: "control" },
      { condition: "inheritance removed", live: 36, kind: "control", note: "429× fewer" },
      { condition: "− cadherin-4d", live: 13950, kind: "minus" },
      { condition: "− pcp-polarity", live: 15708, kind: "minus" },
      { condition: "− symplectic-waste", live: 16695, kind: "minus" },
      { condition: "− trophallaxis", live: 15579, kind: "minus" },
      { condition: "− epigenetic state and notch", live: 15286, kind: "minus" },
      { condition: "− notch-delta", live: 15725, kind: "minus" },
      { condition: "− lysis-recycling", live: 15591, kind: "minus" },
      { condition: "+ velocity-alignment", live: 9574, kind: "plus", note: "streams together, 38% fewer alive" },
    ],
  },
  {
    bundle: "Supercolony (Bundle B)",
    preset: "supercolony-v2",
    rows: [
      { condition: "as committed", live: 21308, kind: "treatment" },
      { condition: "every v2 feature off", live: 21075, kind: "control", note: "colonies without lit rims, and neither trails nor veins" },
      { condition: "decisions removed", live: 20436, kind: "control", note: "no trails at all" },
      { condition: "networks cannot evolve", live: 19942, kind: "control", note: "evolving ones keep 10% more on held-out seeds" },
      { condition: "inheritance removed", live: 317, kind: "control", note: "67× fewer" },
      { condition: "− vascular-field", live: 20113, kind: "minus", note: "alignment falls by a third" },
      { condition: "− dual-carrier-trails", live: 25695, kind: "minus", note: "trails cost 17%" },
      { condition: "− excitable-relay", live: 13458, kind: "minus" },
      { condition: "− oases", live: 32517, kind: "minus", note: "more alive, less structure" },
      { condition: "− so2-motor", live: 19034, kind: "minus" },
      { condition: "− lysis-recycling", live: 22047, kind: "minus" },
      { condition: "− tight-junctions", live: 21050, kind: "minus" },
      { condition: "+ so2-budding, symplectic-waste", live: 18420, kind: "plus" },
    ],
  },
];

/** What the v2 report says is not shown, or costs (experiments/v2-emergence/README.md §4.4). */
export const v2Residuals = [
  {
    title: "No separate cell types yet",
    body: "Clones vary in their memory state, but at 2,048 ticks it barely tracks how crowded a cell is (r = +0.069). Distinct core and sheath cells would need |r| above 0.3, which longer runs may or may not reach.",
  },
  {
    title: "Sharing food does not raise the count",
    body: "Kin pass 79,910 units of energy per run, yet without trophallaxis as many are alive (15,579 against 15,446). Each transfer loses 5%.",
  },
  {
    title: "Organisms without decisions do well in the tissue",
    body: "With every network output zeroed, 27,603 are alive against 15,446: they spend nothing on emission or steering, and kin feed them. They are less coherent on every alignment measure, on all 4 seeds, and a little more crowded (1.2× the close neighbours per cell).",
  },
  {
    title: "Smoother gliders align more",
    body: "In the supercolony, worlds without bursts or trails align more at short range than the committed preset (0.493 to 0.672 against 0.378). Within the committed dynamics, veins and oases raise alignment.",
  },
  {
    title: "Trails cost a sixth of the colony",
    body: "A fifth of emission goes into an uneatable carrier; without it 25,695 are alive against 21,308.",
  },
  {
    title: "Not yet rerun on the new tissue",
    body: "The lesion assay (H4), which removes a disc of organisms and compares the wound's edge with the interior, ran on an earlier, confluent version of the tissue, not on these islands.",
  },
];

export type GateState = "built" | "partial" | "open";

export const gates: { id: string; name: string; state: GateState; detail: string }[] = [
  {
    id: "G0",
    name: "The stack composes",
    state: "built",
    detail: "One lockset; every probe matches its oracle on macOS arm64 and Linux x86-64.",
  },
  {
    id: "G1",
    name: "Hand fixtures and arithmetic",
    state: "built",
    detail: "Fixtures pinned bit for bit; world digests identical on both hosts.",
  },
  {
    id: "G2",
    name: "A first world lives, renders and pauses",
    state: "built",
    detail: "The 4,096-organism preset breeds, mutates and dies, live in a native Metal window.",
  },
  {
    id: "G3",
    name: "Checkpoint replay and crash recovery",
    state: "built",
    detail: "A run killed at any step of a checkpoint or command resumes to the uninterrupted world.",
  },
  {
    id: "G4",
    name: "CPU and Metal parity",
    state: "built",
    detail: "Controllers, field, sensing and uptake on Metal, each bit for bit the CPU reference's.",
  },
  {
    id: "G5",
    name: "Visual review",
    state: "partial",
    detail: "Passed by Claude's review on the owner's delegation, from the whole torus to 32×, with one limit: density bins do not refine with zoom.",
  },
  {
    id: "G6",
    name: "30 fps with p95 input latency ≤ 100 ms",
    state: "partial",
    detail: "The small profile: 30 fps at 1080p and 4K, p95 under 50 ms, on an M5 Ultra. The 25k and 100k profiles are not yet verified.",
  },
  {
    id: "G7",
    name: "400,000 organisms at 30 ticks/s",
    state: "open",
    detail: "Not reached. The 100,000-organism Desktop world steps at 23 to 24 ticks/s on an M5 Ultra, with a quarter of the organisms G7 asks for.",
  },
  {
    id: "G8",
    name: "Eight-hour endurance",
    state: "open",
    detail: "Not run.",
  },
  {
    id: "G9",
    name: "Research claims",
    state: "partial",
    detail: "Five declared hypotheses tested with held-out seeds (below); the release's claims are not yet made.",
  },
  {
    id: "G10",
    name: "Export alignment",
    state: "open",
    detail: "Not run.",
  },
];

export type Verdict = "met" | "narrow" | "not";

/**
 * One measure, seed by seed: on `k` of `n` paired seeds the effect went the
 * stated way, in the discovery split and in the held-out split
 * (experiments/fl048/README.md).
 */
export type SeedStrip = { measure: string; discovery: [number, number]; heldOut: [number, number] };

export const hypotheses: {
  id: string;
  name: string;
  verdict: Verdict;
  label: string;
  result: string;
  strips: SeedStrip[];
}[] = [
  {
    id: "H1",
    name: "Persistent heterogeneous colonies",
    verdict: "met",
    label: "Met · a low bar",
    result: "Every run that reached tick 8,192 ended with clustered, spatially autocorrelated colonies: 32 of 32 in E2, 16 of 16 E1 controls, and 112 of 112 runs across every ablation. The bar is low: the criterion is clustering at one sample, not colonies persisting through time, and it is met without controllers, emission or inheritance.",
    strips: [{ measure: "E1 control meets the colony criterion", discovery: [8, 8], heldOut: [8, 8] }],
  },
  {
    id: "H2",
    name: "Controllers add behaviour beyond chemistry",
    verdict: "not",
    label: "Not supported",
    result: "Zeroing the controllers' decisions leaves more organisms alive, but that ablation also stops emission, and removing emission alone has the same effect. Compared with no-emission (post hoc), the decisions' own effect is inconsistent on every measure.",
    strips: [
      { measure: "Chemistry-only: more alive than control", discovery: [8, 8], heldOut: [7, 8] },
      { measure: "Post hoc, chemistry-only against no-emission: more alive", discovery: [5, 8], heldOut: [4, 8] },
    ],
  },
  {
    id: "H3",
    name: "Paid emission supports niche construction",
    verdict: "narrow",
    label: "Met narrowly",
    result: "Emission evens the field (Gini about 1.5% lower) but leaves how organisms cluster unchanged, and costs population. That is environment modification; niche construction is not demonstrated.",
    strips: [
      { measure: "Field Gini lower with emission", discovery: [6, 8], heldOut: [7, 8] },
      { measure: "Organism Morisita higher without emission", discovery: [4, 8], heldOut: [4, 8] },
    ],
  },
  {
    id: "H4",
    name: "Visual structures are functional",
    verdict: "narrow",
    label: "Rule met · not established",
    result: "Removing a colony's boundary changed the field over the colony more than removing its interior, as declared. Post hoc, 85–98% of that effect is the emptied disk refilling. A functional boundary is not established.",
    strips: [
      { measure: "Field over the colony: boundary effect largest", discovery: [3, 4], heldOut: [3, 4] },
      { measure: "Survival: boundary-specific", discovery: [1, 4], heldOut: [1, 4] },
    ],
  },
  {
    id: "H5",
    name: "Architecture variation affects ecology",
    verdict: "not",
    label: "Not supported",
    result: "Fixing every organism's topology raised the final population on the discovery seeds and lowered it on the held-out seeds: the effect reverses.",
    strips: [{ measure: "Fixed topology: more alive than control", discovery: [7, 8], heldOut: [2, 8] }],
  },
];

// FL-048's inheritance control, the record's "strongest result in E1" (experiments/fl048/README.md).
export const inheritance = {
  title: "Selection acts on inherited variation.",
  body: "Give every newborn the genome a founder would have drawn at its identity, with the default topology and no mutation, instead of its parent's mutated copy; it keeps its parent's lineage. The world then holds about half the organisms, and far more founder lineages survive side by side, because no lineage carries a heritable advantage.",
  figures: [
    { value: "−2,407", label: "organisms alive at the end", note: "discovery; −2,524 held out" },
    { value: "+113.8", label: "founder lineages alive", note: "discovery; +110.9 held out" },
    { value: "0.0078", label: "exact sign-flip p", note: "each, 8 of 8 seeds in both splits" },
  ],
  strips: [
    { measure: "Fewer alive without inheritance", discovery: [8, 8], heldOut: [8, 8] },
    { measure: "More founder lineages alive without inheritance", discovery: [8, 8], heldOut: [8, 8] },
  ] satisfies SeedStrip[],
  scope: "reference-small-world, 4,096 founders, 64 simulated seconds, 8 discovery and 8 held-out seeds, paired against the control.",
};

// The presets (presets/*.toml; README "Planned command surface"). `kind` says how each was chosen:
// a profile's world is declared by its experiment; the search worlds were picked by an outer search whose
// procedure experiments/showcase/README.md states; the two v2 presets are authored Stage-2 presets verified
// across 104 runs in V2-17 (experiments/v2-emergence/README.md), with observed gains and honest residuals stated.
export const presets: {
  file: string;
  model: string;
  founders: number;
  ceiling: number;
  grid: number;
  torus: number;
  dt: string;
  kind: "profile" | "search" | "v2";
  chosen: string;
  body: string;
}[] = [
  {
    file: "reference-small-world",
    model: "field-ecology-v1",
    founders: 4096,
    ceiling: 16384,
    grid: 128,
    torus: 128,
    dt: "1/128",
    kind: "profile",
    chosen: "Reference profile · FL-015",
    body: "Unfed founders starve and fed lineages breed. Its digest is pinned at tick 1,024.",
  },
  {
    file: "interactive-world",
    model: "field-ecology-v1",
    founders: 25000,
    ceiling: 40000,
    grid: 256,
    torus: 256,
    dt: "1/128",
    kind: "profile",
    chosen: "Interactive profile · FL-034 E1",
    body: "Source power chosen by a declared experiment. Populations settle at about 22,000 to 25,000 after an early bottleneck.",
  },
  {
    file: "desktop-world",
    model: "field-ecology-v1",
    founders: 100000,
    ceiling: 160000,
    grid: 512,
    torus: 512,
    dt: "1/128",
    kind: "profile",
    chosen: "Desktop profile · FL-034 E2",
    body: "F16 weights; settles at 91,664 to 99,361 alive. 23 to 24 ticks a second on an M5 Ultra with Metal controllers.",
  },
  {
    file: "showcase-world",
    model: "field-ecology-v1",
    founders: 43816,
    ceiling: 240000,
    grid: 256,
    torus: 256,
    dt: "1/32",
    kind: "search",
    chosen: "Showcase",
    body: "Membranes, rings and crescents, which inherited chemistry makes without the networks.",
  },
  {
    file: "colony-world",
    model: "field-ecology-v1",
    founders: 43816,
    ceiling: 177536,
    grid: 256,
    torus: 256,
    dt: "1/32",
    kind: "search",
    chosen: "Showcase",
    body: "Discrete colonies on a black field, held at a declared population ceiling.",
  },
  {
    file: "tissue-world",
    model: "field-ecology-v1",
    founders: 10954,
    ceiling: 177536,
    grid: 128,
    torus: 128,
    dt: "1/32",
    kind: "search",
    chosen: "Showcase",
    body: "Lineages paint their surroundings with their own chemistry: membranes, voids and mesh walls, 110,000–170,000 strong.",
  },
  {
    file: "contact-world",
    model: "field-ecology-v1-contact",
    founders: 10954,
    ceiling: 177536,
    grid: 128,
    torus: 128,
    dt: "1/32",
    kind: "search",
    chosen: "Showcase · bodies",
    body: "tissue-world with bodies that push apart and pull alike chemistry together: membranes, rings and foam.",
  },
  {
    file: "confluent-world",
    model: "field-ecology-v1-contact",
    founders: 10954,
    ceiling: 262144,
    grid: 320,
    torus: 80,
    dt: "1/32",
    kind: "search",
    chosen: "Showcase · the confluent film",
    body: "contact-world on a field four times finer, fed to its ceiling: domains of labyrinth, foam and packed spheres.",
  },
  {
    file: "morphogenesis-tissue-v2",
    model: "field-ecology-v2 · Bundle A (contact)",
    founders: 10954,
    ceiling: 262144,
    grid: 320,
    torus: 80,
    dt: "1/32",
    kind: "v2",
    chosen: "V2-17 verified · 52 Stage-2 runs",
    body: "Bodies that touch, 14 mechanisms: crowding stops division, polarity orients, tags decide who sticks, kin share food.",
  },
  {
    file: "supercolony-v2",
    model: "field-ecology-v2 · Bundle B (INV-08 pure field)",
    founders: 16384,
    ceiling: 262144,
    grid: 320,
    torus: 80,
    dt: "1/32",
    kind: "v2",
    chosen: "V2-17 verified · 52 Stage-2 runs",
    body: "No forces between organisms, 11 mechanisms: blooming oases, bursting emitters, trails and veins the field carves for itself.",
  },
];

// Plan §22.1 / README "Milestones and gates": evidence gates, not calendar promises.
export const milestones = [
  { id: "M0", name: "Compatible foundation", deliverable: "Runtime, typed arrays, an independent-network batch and a FrankenManim frame in one program", gates: ["G0"] },
  { id: "M1", name: "First living world", deliverable: "4,096 organisms with independent genomes, the ledger, reproduction, mutation and death, live", gates: ["G1", "G2"] },
  { id: "M2", name: "Usable laboratory", deliverable: "A polished 25k profile, inspector, interventions, checkpoint, resume, branches, recording", gates: ["G3", "G5", "G6"] },
  { id: "M3", name: "Qualified acceleration", deliverable: "CPU/Metal parity, persistent buffers, fused population inference, variable topology", gates: ["G4"] },
  { id: "M4", name: "Screenshot-scale world", deliverable: "400k independent networks near 10k parameters on a 1024² field, interactive, honest clocks", gates: ["G7", "G8"] },
  { id: "M5", name: "Scientific and visual release", deliverable: "A seed gallery, ablations, experiment reports, reliable Cinema export", gates: ["G9", "G10"] },
  { id: "M6", name: "Additional model families", deliverable: "field-ecology-v2, 21 mechanisms each switched at founding, in two presets: all 18 of its beads closed, 104 Stage-2 runs observed, residuals kept in experiments/v2-emergence/README.md", gates: [] },
];

export const stack = [
  {
    name: "Asupersync",
    href: "https://github.com/Dicklesworthstone/asupersync",
    role: "Owns the session, cancellation, bounded queues and control lanes. 400,000 organisms never become 400,000 tasks.",
  },
  {
    name: "FrankenTorch",
    href: "https://github.com/Dicklesworthstone/frankentorch",
    role: "Typed tensors, the CPU reference, independent-network population kernels and the Metal gateway.",
  },
  {
    name: "FrankenNumPy",
    href: "https://github.com/Dicklesworthstone/franken_numpy",
    role: "Typed host arrays, layouts and the NumPy-exact Philox random streams every draw is named by.",
  },
  {
    name: "FrankenManim",
    href: "https://github.com/Dicklesworthstone/franken_manim",
    role: "The animation system: cameras, bulk particle and field primitives, overlays, timeline and export.",
  },
  {
    name: "FrankenSQLite",
    href: "https://github.com/Dicklesworthstone/frankensqlite",
    role: "The run catalog, command journal and experiment catalog. Never per-particle rows.",
  },
];

export const commands = [
  {
    comment: "What this machine and build can run, by running it",
    command: "cargo run --release -p flife-cli -- capabilities",
  },
  {
    comment: "1,024 ticks of the 4,096-organism reference world, headless",
    command:
      "cargo run --release -p flife-cli -- run presets/reference-small-world.toml --ticks 1024 --every 128 --metrics-every 128",
  },
  {
    comment: "The same world as PNG frames and an MP4",
    command:
      "cargo run --release -p flife-cli -- render presets/reference-small-world.toml --ticks 2560 --every 4 --width 1080 --height 1080 --out frames --video world.mp4",
  },
  {
    comment: "Live in the native Studio (macOS)",
    command: "cargo run --release -p flife-studio -- presets/reference-small-world.toml --width 960 --height 540",
  },
  {
    comment: "The confluent world in the Studio, from its 10,954 founders to the 262,144 ceiling, drawn as its film draws it",
    command:
      "cargo run --release -p flife-studio -- presets/confluent-world.toml --color lineage-energy --look sphere --no-field",
  },
  {
    comment: "A kept run, a step past its newest durable tick, a control branch and a branch with an energy brush",
    command: [
      "cargo run --release -p flife-cli -- run presets/reference-small-world.toml --ticks 512 --run-dir my-run",
      "cargo run --release -p flife-cli -- step my-run --ticks 64",
      "cargo run --release -p flife-cli -- branch my-run control",
      "cargo run --release -p flife-cli -- branch my-run brushed --inject 640:64,64:256",
    ].join("\n"),
  },
  {
    comment: "Resume all three to tick 1,024 and compare their last records' digests: the control's is the parent's",
    command:
      "for run in my-run control brushed; do cargo run --release -q -p flife-cli -- resume $run --until 1024 | tail -1; done",
  },
  {
    comment: "List the 21 field-ecology-v2 mechanisms, which bundle turns each on, and what each adds to the brain",
    command: "cargo run --release -p flife-cli -- --list-features",
  },
  {
    comment: "Found a v2 world with one mechanism removed, or with every one off (switches are fixed at founding; resume refuses them)",
    command: [
      "cargo run --release -p flife-cli -- validate presets/supercolony-v2.toml",
      "cargo run --release -p flife-cli -- run presets/supercolony-v2.toml --ticks 2048 --disable vascular-field",
      "cargo run --release -p flife-cli -- run presets/morphogenesis-tissue-v2.toml --ticks 2048 --v1-compat",
    ].join("\n"),
  },
  {
    comment: "The supercolony preset: run to tick 2,048 (its digest is Stage-2's 3041ca43…), then draw its veins",
    command: [
      "cargo run --release -p flife-cli -- run presets/supercolony-v2.toml --ticks 2048 --run-dir colony",
      "cargo run --release -p flife-cli -- render --run-dir colony --ticks 3200 --every 4 --width 2160 --height 2160 --field-mapping vascular-conductance --out veins",
    ].join("\n"),
  },
  {
    comment: "Watch the tissue live in the Studio, coloured by polarity (key 9) or memory state (key 8)",
    command: "cargo run --release -p flife-studio -- presets/morphogenesis-tissue-v2.toml --look sphere --no-field",
  },
  {
    comment: "Render 4K at 60 fps (2160 × 2160 square or 3840 × 2160 widescreen) from a kept run, or export a Studio timeline in 4K Cinema mode",
    command: [
      "cargo run --release -p flife-cli -- render --run-dir my-run --ticks 2400 --every 4 --width 2160 --height 2160 --fps 60 --out frames --video world-4k60.mp4",
      "cargo run --release -p flife-studio -- presets/confluent-world.toml --export timeline.ndjson --out cinema-4k60 --export-size 4k --export-fps 60 --export-mp4",
    ].join("\n"),
  },
];

// The flife binary's subcommands (crates/flife-cli/src/main.rs; README "Planned command surface").
export const cliSurface = [
  { name: "--list-features", body: "Print all 21 field-ecology-v2 feature slugs, bundle defaults, INV-08 status and ABI impact" },
  { name: "validate", body: "Range-check a configuration (with optional V2 founding toggles) and report memory admission" },
  { name: "capabilities", body: "Run each controller route against the reference: what this machine can run" },
  { name: "run", body: "A headless run; with --run-dir, durable, journaled and checkpointed" },
  { name: "resume", body: "Continue a run directory from its newest checkpoint that restores (refuses V2 toggles, INV-14)" },
  { name: "step", body: "Advance a run past its newest durable tick, its journal replayed first (refuses V2 toggles, INV-14)" },
  { name: "branch", body: "Control and intervention branches from one checkpoint" },
  { name: "inspect", body: "A run's committed world at a checkpoint: every organism and cell, weights included" },
  { name: "experiment", body: "Seed ensembles, sweeps and ablations against a control, every run cataloged" },
  { name: "analyze", body: "Experiments' effects pooled by condition, refused unless comparable" },
  { name: "discover", body: "A pinned discovery: reference, selection, thumbnail, world export, local package" },
  { name: "transplant", body: "Graft a discovered package into a host world, as a new run that resumes" },
  { name: "render", body: "Frames and MP4 through FrankenManim (including 4K60 and V2 overlays), from a config or kept run" },
];

export const inspiration =
  "Inspired by Fraser Price's Neural-Kimchi-4B. FrankenLife is an original rule set, not affiliated with it and claiming no numerical parity.";
