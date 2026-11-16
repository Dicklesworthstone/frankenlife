// Every word and number on the site. Each figure here is one the repository
// itself states (README.md, the crates' CONTRACT.md files, experiments/fl048)
// or one the film's manifest records (lib/media.json); none is a target
// dressed as a result.

export const siteConfig = {
  name: "FrankenLife",
  // Where Cloudflare serves the site: wrangler.jsonc's Worker on the account's workers.dev subdomain.
  url: "https://frankenlife.jeffrey-emanuel.workers.dev",
  title: "FrankenLife — artificial life you can watch, replay and check",
  description:
    "A real-time artificial-life laboratory in memory-safe Rust. Organisms with their own heritable neural networks feed, signal, move, split and die through one shared energy field, and every committed tick is deterministic to the bit.",
  github: "https://github.com/Dicklesworthstone/frankenlife",
  plan: "https://github.com/Dicklesworthstone/frankenlife/blob/main/COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md",
  readme: "https://github.com/Dicklesworthstone/frankenlife#readme",
  fl048: "https://github.com/Dicklesworthstone/frankenlife/blob/main/experiments/fl048/README.md",
  license: "https://github.com/Dicklesworthstone/frankenlife/blob/main/LICENSE",
};

export const navItems = [
  { href: "#film", label: "The film" },
  { href: "#world", label: "The world" },
  { href: "#tick", label: "One tick" },
  { href: "#status", label: "Status" },
  { href: "#science", label: "Science" },
  { href: "#try", label: "Try it" },
] as const;

export type Stat = { value: string; label: string; note: string };

export const heroStats: Stat[] = [
  {
    value: "9,988",
    label: "parameters per brain",
    note: "The default network, 104 → 64 → 32 → 36. No two organisms share one.",
  },
  {
    value: "104 → 36",
    label: "senses to decisions",
    note: "Field energy, 32 chemical channels and their gradients in; reproduction, emission and spending out.",
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

export const principles = [
  {
    title: "Every organism has its own weights.",
    body: "Organisms with the same topology share a kernel and a memory layout, never a parameter.",
  },
  {
    title: "Organisms meet only through the field.",
    body: "No pairwise attraction tables, collision forces or global reward functions in the baseline.",
  },
  {
    title: "Signals cannot counterfeit food.",
    body: "A cell holds spendable energy e and a 32-component chemical signature m with ‖m‖ ≤ e. Opposite signals can cancel each other's chemistry without creating or destroying energy.",
  },
  {
    title: "Nothing is trained by gradient.",
    body: "Weights start random and change only through inherited mutation.",
  },
  {
    title: "The ledger always balances.",
    body: "Energy in the organisms plus energy in the field changes only by injection, removal, dissipation and a declared numerical residual, every tick.",
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
    body: "36 outputs: a reproduction hazard, a child's energy share, an emission gate, a 32-dimensional emission direction and a spending rate. No motor output.",
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
    body: "Zero energy after any stage is death, checked after every stage. Part of the residue returns to the field.",
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

export const claims = [
  {
    claim: "Rules",
    meaning: "The simulation obeys its own rules",
    evidence:
      "Invariants INV-01–INV-15 and the energy ledger; the field, ledger and population checks run on every tick before it is published",
  },
  {
    claim: "Implementation",
    meaning: "The code reproduces those rules",
    evidence: "Hand-computed fixtures, a separate scalar oracle, and CPU/Metal parity bit for bit",
  },
  {
    claim: "Visual",
    meaning: "The images have the intended character",
    evidence: "Human review against the references, raw diagnostic views beside the aesthetic ones (G5)",
  },
  {
    claim: "Ecological",
    meaning: "Populations show measurable evolutionary phenomena",
    evidence: "Ablations, seed ensembles and held-out seeds, failed and extinct runs kept (G9)",
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
    state: "open",
    detail: "Needs a human review against the references. Not yet run.",
  },
  {
    id: "G6",
    name: "30 fps with p95 input latency ≤ 100 ms",
    state: "partial",
    detail: "Offscreen only: 30.00 fps at 1080p (p95 51.6 ms) and 29.79 at 4K (p95 59.9 ms). Not yet measured in a window.",
  },
  {
    id: "G7",
    name: "400,000 organisms at 30 ticks/s",
    state: "open",
    detail: "Not reached. The 100,000-organism Desktop world runs at about 10 ticks/s on an M5 Ultra.",
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

export const hypotheses: { id: string; name: string; verdict: Verdict; label: string; result: string }[] = [
  {
    id: "H1",
    name: "Persistent heterogeneous colonies",
    verdict: "met",
    label: "Met · a low bar",
    result: "Every run that reached tick 8,192 kept clustered, spatially autocorrelated colonies: 32 of 32 and 16 of 16.",
  },
  {
    id: "H2",
    name: "Controllers add behaviour beyond chemistry",
    verdict: "not",
    label: "Not supported",
    result: "The chemistry-only ablation also stops emission, and removing emission alone has the same effects. The decisions' own effect is inconsistent.",
  },
  {
    id: "H3",
    name: "Paid emission supports niche construction",
    verdict: "narrow",
    label: "Met narrowly",
    result: "Emission evens the field (Gini lower on 6 of 8 seeds, held out 7 of 8) but leaves colonies' clustering unchanged: environment modification, not demonstrated niche construction.",
  },
  {
    id: "H4",
    name: "Visual structures are functional",
    verdict: "narrow",
    label: "Rule met · not established",
    result: "The declared rule was met on one measure, but post hoc that effect is mostly an emptied disk refilling. A functional boundary is not established.",
  },
  {
    id: "H5",
    name: "Architecture variation affects ecology",
    verdict: "not",
    label: "Not supported",
    result: "Final population and births reverse sign between the discovery and held-out seeds.",
  },
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
    comment: "A kept run, then a step past its newest durable tick",
    command:
      "cargo run --release -p flife-cli -- run presets/reference-small-world.toml --ticks 512 --run-dir my-run && cargo run --release -p flife-cli -- step my-run --ticks 64",
  },
];

export const inspiration =
  "The visual and mechanical target comes from screenshots of Fraser Price's Neural-Kimchi-4B and its description. FrankenLife is an original, explicit rule set (FieldEcologyV1) built from the described primitives. It is not affiliated with that project and claims no numerical parity with it.";
