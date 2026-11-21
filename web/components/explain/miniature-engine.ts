// A toy world for the page: FrankenLife's kinds of rules at toy scale, in
// TypeScript, so a visitor can watch organisms with their own networks feed
// from one field, pay for everything, divide and die. It is not FrankenLife's
// code and nothing it does is a FrankenLife result (components/explain/miniature.tsx
// says so on its face).
//
// What it borrows, each in miniature:
// - two-pass uptake with whole quanta by largest remainder (plan §5.2, fixture C.1);
// - an exact energy ledger (INV-03): injected = field + bodies + dissipated, every tick;
// - birth as a transaction (fixture C.5): pay the cost, split what is left;
// - each organism its own weights (INV-13), changed only by inherited mutation;
// - determinism: a seeded world, commands recorded with their tick, a digest.
//
// Determinism across browsers: the step uses only +, −, ×, ÷, floor, sqrt and
// 32-bit integer arithmetic, which IEEE 754 and ECMAScript fix exactly. No
// Math.exp, Math.tanh or Math.sin (their last bit differs between engines):
// the activation, the sources' paths and the Gaussian draws are polynomials.

/** The torus is SIZE × SIZE cells; a cell is one unit. */
export const SIZE = 96;
export const CELLS = SIZE * SIZE;
/** The most organisms alive at once: a birth past it is refused, and counted. */
export const CAPACITY = 2048;

/** The network: INPUTS → HIDDEN (tanh) → OUTPUTS, with output biases. */
export const INPUTS = 8;
export const HIDDEN = 8;
export const OUTPUTS = 4;
export const PARAMETERS = INPUTS * HIDDEN + HIDDEN * OUTPUTS + OUTPUTS;

export const INPUT_NAMES = [
  "food here",
  "food → east",
  "food → south",
  "food far east",
  "food far south",
  "own energy",
  "crowding",
  "bias",
] as const;
export const OUTPUT_NAMES = ["move east", "move south", "eat", "divide"] as const;

export const LINEAGES = 12;

/** The world's constants, all stated here and in the page's caption. */
export const RULES = {
  foundersPerLineage: 20,
  founderEnergy: 120,
  /** A lineage's founders start within this many cells of its home, on each axis. */
  founderSpread: 8,
  /** Cells per tick at full output, on each axis. */
  speed: 0.4,
  /** The most quanta one organism asks its cell for in a tick. */
  bite: 10,
  /** Paid every tick, dissipated. */
  upkeep: 1,
  /** Paid for motion: up to this many quanta at full speed on both axes, dissipated. */
  moveCost: 1,
  /** Energy needed to divide, and the birth's cost, dissipated. */
  divideAt: 48,
  divideCost: 4,
  /** Ticks an organism can live; at the end its energy returns to its cell. */
  maxAge: 1500,
  /** Founder lineages: every weight of the forager's genome moved by this times a unit draw. */
  founderVariation: 0.25,
  /** Mutation: every weight of a child moves by this times a unit draw. */
  mutation: 0.07,
  sources: 4,
  /** Quanta each source injects per tick, over a smooth bump of this radius. */
  sourcePower: 270,
  sourceRadius: 11,
  /** Per tick, each cell sends 1/32 of its energy to each of its four neighbours. */
  diffusion: 1 / 32,
  /** Per tick, a cell loses 1/256 of its energy as heat. */
  decay: 1 / 512,
  /** Quanta in every cell at tick 0, counted as injected. */
  initialField: 10,
  /** Quanta a brush adds to one cell. */
  brush: 600,
  /** Bodies a cell has room for: a move into a full cell is refused, and a child needs room. */
  perCell: 2,
  /** Field energy that reads as 1 on a sensor. */
  foodScale: 24,
} as const;

export type Brains = "own" | "shared";
export type Children = "inherit" | "fresh";
export type Mode = { brains: Brains; children: Children };
export const DEFAULT_MODE: Mode = { brains: "own", children: "inherit" };

/** A researcher's brush: energy added to one cell, applied at the start of `tick`. */
export type Brush = { tick: number; x: number; y: number; amount: number };

export type Ledger = {
  injected: number;
  field: number;
  bodies: number;
  dissipated: number;
  /** injected − (field + bodies + dissipated): zero, every tick. */
  residual: number;
};

/** What one organism senses now and would do with it. */
export type Activity = { inputs: number[]; hidden: number[]; outputs: number[] };

// ---------------------------------------------------------------- arithmetic

/** tanh to within 0.03, from a rational function: monotone, exactly ±1 beyond ±3. */
export function tanhToy(x: number): number {
  if (x >= 3) return 1;
  if (x <= -3) return -1;
  const x2 = x * x;
  return (x * (27 + x2)) / (27 + 9 * x2);
}

export const sigmoidToy = (x: number) => 0.5 + 0.5 * tanhToy(x / 2);

const TWO_PI = 6.283185307179586;
const PI = 3.141592653589793;
const HALF_PI = 1.5707963267948966;

/** sin from a polynomial on [−π/2, π/2] (error below 4e-6): the sources' paths only. */
export function sinToy(x: number): number {
  let t = x - TWO_PI * Math.floor((x + PI) / TWO_PI);
  if (t > HALF_PI) t = PI - t;
  else if (t < -HALF_PI) t = -PI - t;
  const t2 = t * t;
  return t * (1 - (t2 / 6) * (1 - (t2 / 20) * (1 - (t2 / 42) * (1 - t2 / 72))));
}

/** mulberry32: a 32-bit PRNG, the same numbers in every engine. */
export function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const ROOT_THREE = Math.sqrt(3);

/** A unit draw, roughly Gaussian (Irwin–Hall, four uniforms), from arithmetic alone. */
function gaussian(next: () => number): number {
  return (next() + next() + next() + next() - 2) * ROOT_THREE;
}

/** A 32-bit integer hash of two integers: the field's stochastic rounding. */
function hash2(a: number, b: number): number {
  let h = Math.imul(a ^ 0x9e3779b9, 0x85ebca6b) ^ Math.imul(b + 0x632be5ab, 0xc2b2ae35);
  h ^= h >>> 16;
  h = Math.imul(h, 0x7feb352d);
  h ^= h >>> 15;
  h = Math.imul(h, 0x846ca68b);
  return (h ^ (h >>> 16)) >>> 0;
}

/** floor(e·rate + u) with u uniform in [0, 1) from the hash: rate·e on average, an integer always. */
function stochastic(e: number, rate: number, h: number): number {
  return Math.floor(e * rate + (h & 0xffff) / 65536);
}

/**
 * The uptake rule on whole quanta (plan §5.2, `cell-quantum-largest-remainder-v1`):
 * `requests` in identity order, `cell` the energy held. Uncontended, everyone
 * gets what they asked; contended, each gets ⌊q·e/A⌋ and the leftover quanta go
 * to the largest remainders, ties to the lower identity. Writes into `grants`.
 */
export function grantQuanta(cell: number, requests: ArrayLike<number>, count: number, grants: Int32Array | number[]): number {
  let demand = 0;
  for (let i = 0; i < count; i++) demand += requests[i] ?? 0;
  if (demand <= cell) {
    for (let i = 0; i < count; i++) grants[i] = requests[i] ?? 0;
    return demand;
  }
  let given = 0;
  for (let i = 0; i < count; i++) {
    const share = Math.floor(((requests[i] ?? 0) * cell) / demand);
    grants[i] = share;
    given += share;
  }
  let left = cell - given;
  while (left > 0) {
    let best = -1;
    let bestRemainder = -1;
    for (let i = 0; i < count; i++) {
      // q·e − g·A: the remainder in units of 1/A. A leftover quantum makes it negative, so none gets two.
      const remainder = (requests[i] ?? 0) * cell - (grants[i] ?? 0) * demand;
      if (remainder > bestRemainder) {
        best = i;
        bestRemainder = remainder;
      }
    }
    if (best < 0) break;
    grants[best] = (grants[best] ?? 0) + 1;
    left -= 1;
  }
  return cell - left;
}

// ------------------------------------------------------------------ genomes

/** Where weight (input k → hidden h), (hidden h → output o) and output o's bias sit in a genome. */
export const inputWeight = (k: number, h: number) => k * HIDDEN + h;
export const outputWeight = (h: number, o: number) => INPUTS * HIDDEN + h * OUTPUTS + o;
export const outputBias = (o: number) => INPUTS * HIDDEN + HIDDEN * OUTPUTS + o;

/**
 * The genome founders are drawn around: a plain forager that climbs food
 * gradients, eats, and divides when it holds enough. Every founder lineage
 * starts from it with every weight moved at random (RULES.founderVariation),
 * so lineages differ from the first tick; selection does the rest.
 */
export const FORAGER: Float64Array = (() => {
  const g = new Float64Array(PARAMETERS);
  g[inputWeight(1, 0)] = 2.5; // food → east  → hidden 0
  g[inputWeight(3, 0)] = 1.5; // food far east
  g[inputWeight(2, 1)] = 2.5; // food → south → hidden 1
  g[inputWeight(4, 1)] = 1.5; // food far south
  g[inputWeight(0, 2)] = 1.5; // food here → hidden 2
  g[inputWeight(5, 3)] = 3; // own energy → hidden 3
  g[inputWeight(7, 3)] = -2.5; // bias
  g[inputWeight(6, 4)] = 1.5; // crowding → hidden 4
  g[outputWeight(0, 0)] = 2; // hidden 0 → move east
  g[outputWeight(1, 1)] = 2; // hidden 1 → move south
  g[outputWeight(2, 2)] = 1; // hidden 2 → eat
  g[outputWeight(3, 3)] = 2.5; // hidden 3 → divide
  g[outputBias(2)] = 1;
  g[outputBias(3)] = 0.5;
  return g;
})();

// --------------------------------------------------------------------- world

const SLOTS = CAPACITY * 2;
const REACH = 4;

export class World {
  readonly seed: number;
  readonly mode: Mode;
  /** The brushes, in the order made; each applies at the start of its tick. */
  readonly commands: Brush[];

  tick = 0;
  /** Slots in use: the living, in identity order. */
  count = 0;
  nextIdentity = 0;
  births = 0;
  deaths = 0;
  refused = 0;
  /** Births that found no room near the parent (every nearby cell full). */
  crowded = 0;
  /** The last tick's flows: quanta injected (sources, brushes) and dissipated. */
  lastIn = 0;
  lastOut = 0;
  /** The last tick's births as [parent identity, child identity] pairs. */
  lastBirths: number[] = [];

  injected = 0;
  dissipated = 0;

  field = new Int32Array(CELLS);
  private spare = new Int32Array(CELLS);

  identity = new Int32Array(SLOTS);
  x = new Float64Array(SLOTS);
  y = new Float64Array(SLOTS);
  energy = new Int32Array(SLOTS);
  age = new Int32Array(SLOTS);
  lineage = new Int32Array(SLOTS);
  generation = new Int32Array(SLOTS);
  weights = new Float64Array(SLOTS * PARAMETERS);

  private alive = new Uint8Array(SLOTS);
  private request = new Int32Array(SLOTS);
  private motion = new Float64Array(SLOTS * 2);
  private divide = new Uint8Array(SLOTS);
  private cellOf = new Int32Array(SLOTS);
  private occupancy = new Int32Array(CELLS);
  private bucketStart = new Int32Array(CELLS + 1);
  private bucketOrder = new Int32Array(SLOTS);
  private bucketFill = new Int32Array(CELLS);
  private scratchRequests = new Int32Array(SLOTS);
  private scratchGrants = new Int32Array(SLOTS);
  private input = new Float64Array(INPUTS);
  private hiddenScratch = new Float64Array(HIDDEN);
  private outputScratch = new Float64Array(OUTPUTS);

  /** The sources' bump: integer quanta at each offset, summing exactly to sourcePower. */
  private static bump: { dx: number; dy: number; q: number }[] | null = null;
  private sourcePhase: number[] = [];
  private draw: () => number;
  private shared: Float64Array | null = null;

  constructor(seed: number, mode: Mode = DEFAULT_MODE, commands: readonly Brush[] = []) {
    this.seed = seed >>> 0;
    this.mode = mode;
    this.commands = commands.map((command) => ({ ...command }));
    this.draw = random(this.seed ^ 0x5eed1e55);
    for (let s = 0; s < RULES.sources; s++) {
      this.sourcePhase.push(this.draw() * TWO_PI, this.draw() * TWO_PI, 0.6 + this.draw() * 0.8, 0.6 + this.draw() * 0.8);
    }
    // Founders: each lineage a random genome, its founders near copies of it, scattered over the torus.
    const genome = new Float64Array(PARAMETERS);
    for (let lineage = 0; lineage < LINEAGES; lineage++) {
      this.drawGenome(genome);
      const homeX = this.draw() * SIZE;
      const homeY = this.draw() * SIZE;
      for (let k = 0; k < RULES.foundersPerLineage; k++) {
        const slot = this.count++;
        this.identity[slot] = this.nextIdentity++;
        this.x[slot] = wrap(homeX + (this.draw() - 0.5) * 2 * RULES.founderSpread);
        this.y[slot] = wrap(homeY + (this.draw() - 0.5) * 2 * RULES.founderSpread);
        this.energy[slot] = RULES.founderEnergy;
        this.lineage[slot] = lineage;
        this.generation[slot] = 0;
        this.alive[slot] = 1;
        const base = slot * PARAMETERS;
        for (let p = 0; p < PARAMETERS; p++) this.weights[base + p] = (genome[p] ?? 0) + RULES.mutation * gaussian(this.draw);
      }
    }
    if (mode.brains === "shared") {
      // One network for everyone: founder 0's.
      this.shared = this.weights.slice(0, PARAMETERS);
      for (let slot = 1; slot < this.count; slot++) this.weights.set(this.shared, slot * PARAMETERS);
    }
    // The bodies' and the field's first energy was put into the world: it counts as injected.
    this.field.fill(RULES.initialField);
    this.injected = this.count * RULES.founderEnergy + CELLS * RULES.initialField;
  }

  /** A fresh genome: weights in roughly [−1.7, 1.7], an eater's bias on the eat output. */
  /** A fresh genome: the forager's, every weight moved by a random amount. */
  private drawGenome(into: Float64Array): void {
    for (let p = 0; p < PARAMETERS; p++) into[p] = (FORAGER[p] ?? 0) + RULES.founderVariation * gaussian(this.draw);
  }

  private static sourceBump(): { dx: number; dy: number; q: number }[] {
    if (World.bump) return World.bump;
    const r = RULES.sourceRadius;
    const cells: { dx: number; dy: number; w: number }[] = [];
    let total = 0;
    for (let dy = -r; dy <= r; dy++) {
      for (let dx = -r; dx <= r; dx++) {
        const d2 = (dx * dx + dy * dy) / (r * r);
        if (d2 >= 1) continue;
        const w = (1 - d2) * (1 - d2) * (1 - d2);
        cells.push({ dx, dy, w });
        total += w;
      }
    }
    // Whole quanta by largest remainder, ties to the earlier offset: exactly sourcePower.
    const shares = cells.map((cell, index) => {
      const ideal = (cell.w / total) * RULES.sourcePower;
      return { ...cell, index, q: Math.floor(ideal), rest: ideal - Math.floor(ideal) };
    });
    let left = RULES.sourcePower - shares.reduce((sum, share) => sum + share.q, 0);
    for (const share of [...shares].sort((a, b) => b.rest - a.rest || a.index - b.index)) {
      if (left <= 0) break;
      share.q += 1;
      left -= 1;
    }
    World.bump = shares.filter((share) => share.q > 0).map(({ dx, dy, q }) => ({ dx, dy, q }));
    return World.bump;
  }

  /** Where source `s` is at tick `t`: smooth drifting paths across the torus. */
  sourceAt(s: number, t = this.tick): [number, number] {
    const base = s * 4;
    const phaseX = this.sourcePhase[base] ?? 0;
    const phaseY = this.sourcePhase[base + 1] ?? 0;
    const rateX = this.sourcePhase[base + 2] ?? 1;
    const rateY = this.sourcePhase[base + 3] ?? 1;
    const cx = SIZE * ((s + 0.5) / RULES.sources) + SIZE * 0.22 * sinToy((t / 2400) * TWO_PI * rateX + phaseX);
    const cy = SIZE * 0.5 + SIZE * 0.3 * sinToy((t / 3100) * TWO_PI * rateY + phaseY);
    return [cx - SIZE * Math.floor(cx / SIZE), cy - SIZE * Math.floor(cy / SIZE)];
  }

  /** Record a brush at this tick (it applies when the tick is computed) and return it. */
  brush(x: number, y: number, amount: number = RULES.brush): Brush {
    const command = { tick: this.tick, x: wrapCell(x), y: wrapCell(y), amount };
    this.commands.push(command);
    return command;
  }

  /** The cell under (x, y) on the torus. */
  static cell(x: number, y: number): number {
    return wrapCell(y) * SIZE + wrapCell(x);
  }

  /** Advance one tick, in a fixed order. */
  step(): void {
    const t = this.tick;
    const injectedBefore = this.injected;
    const dissipatedBefore = this.dissipated;
    // 1. Commands due at this boundary.
    for (const command of this.commands) {
      if (command.tick !== t) continue;
      add(this.field, World.cell(command.x, command.y), command.amount);
      this.injected += command.amount;
    }
    // 2. Sources inject, then the field diffuses and loses a little heat.
    const bump = World.sourceBump();
    for (let s = 0; s < RULES.sources; s++) {
      const [cx, cy] = this.sourceAt(s, t);
      const ix = Math.floor(cx);
      const iy = Math.floor(cy);
      for (const { dx, dy, q } of bump) {
        add(this.field, World.cell(ix + dx, iy + dy), q);
        this.injected += q;
      }
    }
    this.diffuse(t);
    // 3. Sense and decide, everyone from the same field.
    this.occupancy.fill(0);
    for (let i = 0; i < this.count; i++) {
      const cell = World.cell(this.x[i] ?? 0, this.y[i] ?? 0);
      this.cellOf[i] = cell;
      this.occupancy[cell] = (this.occupancy[cell] ?? 0) + 1;
    }
    for (let i = 0; i < this.count; i++) this.decide(i);
    // 4. Uptake: two passes per cell, whole quanta, identity order.
    this.uptake();
    // 5. Pay upkeep and motion; who cannot pay dies, and what it held returns to its cell.
    for (let i = 0; i < this.count; i++) {
      const vx = this.motion[i * 2] ?? 0;
      const vy = this.motion[i * 2 + 1] ?? 0;
      const cost = RULES.upkeep + Math.floor((RULES.moveCost * (vx * vx + vy * vy)) / (RULES.speed * RULES.speed));
      const e = this.energy[i] ?? 0;
      if (e < cost) {
        this.die(i);
        continue;
      }
      this.energy[i] = e - cost;
      this.dissipated += cost;
      // 6. Move, on the torus, unless the cell ahead already holds RULES.perCell bodies.
      const nx = wrap((this.x[i] ?? 0) + vx);
      const ny = wrap((this.y[i] ?? 0) + vy);
      const from = this.cellOf[i] ?? 0;
      const to = World.cell(nx, ny);
      if (to === from || (this.occupancy[to] ?? 0) < RULES.perCell) {
        this.x[i] = nx;
        this.y[i] = ny;
        this.cellOf[i] = to;
        this.occupancy[from] = (this.occupancy[from] ?? 0) - 1;
        this.occupancy[to] = (this.occupancy[to] ?? 0) + 1;
      }
      this.age[i] = (this.age[i] ?? 0) + 1;
      if ((this.age[i] ?? 0) >= RULES.maxAge) this.die(i);
    }
    // 7. Births, in identity order: a transaction each, or nothing.
    this.lastBirths.length = 0;
    let living = 0;
    for (let i = 0; i < this.count; i++) living += this.alive[i] ?? 0;
    const parents = this.count;
    for (let i = 0; i < parents; i++) {
      if (!this.alive[i] || !this.divide[i] || (this.energy[i] ?? 0) < RULES.divideAt) continue;
      if (living >= CAPACITY) {
        this.refused += 1;
        continue;
      }
      // A child needs room: up to four tries at a cell near the parent with space in it.
      let placed = false;
      for (let attempt = 0; attempt < 4 && !placed; attempt++) {
        const cx = wrap((this.x[i] ?? 0) + (this.draw() - 0.5) * 3);
        const cy = wrap((this.y[i] ?? 0) + (this.draw() - 0.5) * 3);
        if ((this.occupancy[World.cell(cx, cy)] ?? 0) >= RULES.perCell) continue;
        this.bear(i, cx, cy);
        placed = true;
      }
      if (!placed) {
        this.crowded += 1;
        continue;
      }
      living += 1;
    }
    // 8. Compact: the living stay in identity order (children have the newest identities).
    this.compact();
    this.lastIn = this.injected - injectedBefore;
    this.lastOut = this.dissipated - dissipatedBefore;
    this.tick = t + 1;
  }

  private diffuse(t: number): void {
    const from = this.field;
    const to = this.spare;
    to.set(from);
    for (let cell = 0; cell < CELLS; cell++) {
      const e = from[cell] ?? 0;
      if (e < 4) continue;
      const share = Math.min(Math.floor(e / 4), stochastic(e, RULES.diffusion, hash2(cell, t)));
      if (share === 0) continue;
      const cx = cell % SIZE;
      const cy = (cell - cx) / SIZE;
      to[cell] = (to[cell] ?? 0) - 4 * share;
      add(to, cy * SIZE + ((cx + 1) % SIZE), share);
      add(to, cy * SIZE + ((cx + SIZE - 1) % SIZE), share);
      add(to, ((cy + 1) % SIZE) * SIZE + cx, share);
      add(to, ((cy + SIZE - 1) % SIZE) * SIZE + cx, share);
    }
    for (let cell = 0; cell < CELLS; cell++) {
      const e = to[cell] ?? 0;
      if (e === 0) continue;
      const loss = Math.min(e, stochastic(e, RULES.decay, hash2(cell ^ 0x3c6ef372, t)));
      to[cell] = e - loss;
      this.dissipated += loss;
    }
    this.field = to;
    this.spare = from;
  }

  /** Fill `this.input` with what organism `i` senses now. */
  private sense(i: number): void {
    const fx = wrapCell(this.x[i] ?? 0);
    const fy = wrapCell(this.y[i] ?? 0);
    const at = (dx: number, dy: number) => this.field[((fy + dy + SIZE) % SIZE) * SIZE + ((fx + dx + SIZE) % SIZE)] ?? 0;
    const scale = RULES.foodScale;
    const input = this.input;
    input[0] = Math.min(4, at(0, 0) / scale);
    input[1] = (at(1, 0) - at(-1, 0)) / (2 * scale);
    input[2] = (at(0, 1) - at(0, -1)) / (2 * scale);
    input[3] = (at(REACH, 0) - at(-REACH, 0)) / (2 * scale);
    input[4] = (at(0, REACH) - at(0, -REACH)) / (2 * scale);
    input[5] = (this.energy[i] ?? 0) / RULES.divideAt;
    input[6] = (this.occupancy[fy * SIZE + fx] ?? 0) / 3;
    input[7] = 1;
    for (let k = 1; k <= 4; k++) input[k] = Math.max(-4, Math.min(4, input[k] ?? 0));
  }

  /** Run organism `i`'s own network on `this.input`. */
  private forward(i: number): void {
    const w = this.weights;
    const base = i * PARAMETERS;
    const input = this.input;
    const hidden = this.hiddenScratch;
    for (let h = 0; h < HIDDEN; h++) {
      let sum = 0;
      for (let k = 0; k < INPUTS; k++) sum += (w[base + k * HIDDEN + h] ?? 0) * (input[k] ?? 0);
      hidden[h] = tanhToy(sum);
    }
    const outBase = base + INPUTS * HIDDEN;
    const output = this.outputScratch;
    for (let o = 0; o < OUTPUTS; o++) {
      let sum = w[outBase + HIDDEN * OUTPUTS + o] ?? 0;
      for (let h = 0; h < HIDDEN; h++) sum += (w[outBase + h * OUTPUTS + o] ?? 0) * (hidden[h] ?? 0);
      output[o] = sum;
    }
  }

  private decide(i: number): void {
    this.sense(i);
    this.forward(i);
    const output = this.outputScratch;
    this.motion[i * 2] = RULES.speed * tanhToy(output[0] ?? 0);
    this.motion[i * 2 + 1] = RULES.speed * tanhToy(output[1] ?? 0);
    this.request[i] = Math.floor(RULES.bite * sigmoidToy(output[2] ?? 0));
    this.divide[i] = (output[3] ?? 0) > 0 ? 1 : 0;
  }

  private uptake(): void {
    const start = this.bucketStart;
    start.fill(0);
    for (let i = 0; i < this.count; i++) start[(this.cellOf[i] ?? 0) + 1] = (start[(this.cellOf[i] ?? 0) + 1] ?? 0) + 1;
    for (let c = 0; c < CELLS; c++) start[c + 1] = (start[c + 1] ?? 0) + (start[c] ?? 0);
    const fill = this.bucketFill;
    fill.fill(0);
    for (let i = 0; i < this.count; i++) {
      const cell = this.cellOf[i] ?? 0;
      this.bucketOrder[(start[cell] ?? 0) + (fill[cell] ?? 0)] = i;
      fill[cell] = (fill[cell] ?? 0) + 1;
    }
    const requests = this.scratchRequests;
    const grants = this.scratchGrants;
    for (let c = 0; c < CELLS; c++) {
      const from = start[c] ?? 0;
      const n = (start[c + 1] ?? 0) - from;
      if (n === 0) continue;
      for (let k = 0; k < n; k++) requests[k] = this.request[this.bucketOrder[from + k] ?? 0] ?? 0;
      const granted = grantQuanta(this.field[c] ?? 0, requests, n, grants);
      this.field[c] = (this.field[c] ?? 0) - granted;
      for (let k = 0; k < n; k++) {
        const i = this.bucketOrder[from + k] ?? 0;
        this.energy[i] = (this.energy[i] ?? 0) + (grants[k] ?? 0);
      }
    }
  }

  private die(i: number): void {
    if (!this.alive[i]) return;
    this.alive[i] = 0;
    const cell = World.cell(this.x[i] ?? 0, this.y[i] ?? 0);
    add(this.field, cell, this.energy[i] ?? 0);
    this.occupancy[cell] = (this.occupancy[cell] ?? 0) - 1;
    this.energy[i] = 0;
    this.deaths += 1;
  }

  private bear(parent: number, x: number, y: number): void {
    const remaining = (this.energy[parent] ?? 0) - RULES.divideCost;
    this.dissipated += RULES.divideCost;
    const child = this.count++;
    const given = Math.floor(remaining / 2);
    this.energy[parent] = remaining - given;
    this.energy[child] = given;
    this.identity[child] = this.nextIdentity++;
    this.x[child] = x;
    this.y[child] = y;
    const cell = World.cell(x, y);
    this.cellOf[child] = cell;
    this.occupancy[cell] = (this.occupancy[cell] ?? 0) + 1;
    this.age[child] = 0;
    this.lineage[child] = this.lineage[parent] ?? 0;
    this.generation[child] = (this.generation[parent] ?? 0) + 1;
    this.alive[child] = 1;
    this.divide[child] = 0;
    const from = parent * PARAMETERS;
    const to = child * PARAMETERS;
    if (this.shared) {
      this.weights.set(this.shared, to);
    } else if (this.mode.children === "fresh") {
      const genome = new Float64Array(PARAMETERS);
      this.drawGenome(genome);
      this.weights.set(genome, to);
    } else {
      for (let p = 0; p < PARAMETERS; p++) this.weights[to + p] = (this.weights[from + p] ?? 0) + RULES.mutation * gaussian(this.draw);
    }
    this.births += 1;
    this.lastBirths.push(this.identity[parent] ?? 0, this.identity[child] ?? 0);
  }

  private compact(): void {
    let write = 0;
    for (let read = 0; read < this.count; read++) {
      if (!this.alive[read]) continue;
      if (write !== read) {
        this.identity[write] = this.identity[read] ?? 0;
        this.x[write] = this.x[read] ?? 0;
        this.y[write] = this.y[read] ?? 0;
        this.energy[write] = this.energy[read] ?? 0;
        this.age[write] = this.age[read] ?? 0;
        this.lineage[write] = this.lineage[read] ?? 0;
        this.generation[write] = this.generation[read] ?? 0;
        this.alive[write] = 1;
        this.weights.copyWithin(write * PARAMETERS, read * PARAMETERS, (read + 1) * PARAMETERS);
      }
      write += 1;
    }
    for (let i = write; i < this.count; i++) this.alive[i] = 0;
    this.count = write;
  }

  // ------------------------------------------------------------- reading

  ledger(): Ledger {
    let field = 0;
    for (let c = 0; c < CELLS; c++) field += this.field[c] ?? 0;
    let bodies = 0;
    for (let i = 0; i < this.count; i++) bodies += this.energy[i] ?? 0;
    return { injected: this.injected, field, bodies, dissipated: this.dissipated, residual: this.injected - field - bodies - this.dissipated };
  }

  /** Living organisms per lineage. */
  lineageCounts(into: Int32Array = new Int32Array(LINEAGES)): Int32Array {
    into.fill(0);
    for (let i = 0; i < this.count; i++) into[this.lineage[i] ?? 0] = (into[this.lineage[i] ?? 0] ?? 0) + 1;
    return into;
  }

  /** The slot of the organism with this identity, or −1 (slots are in identity order). */
  find(identity: number): number {
    let lo = 0;
    let hi = this.count - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      const id = this.identity[mid] ?? 0;
      if (id === identity) return mid;
      if (id < identity) lo = mid + 1;
      else hi = mid - 1;
    }
    return -1;
  }

  /** The organism nearest (x, y) on the torus within `radius` cells, or −1. */
  nearest(x: number, y: number, radius = 2.5): number {
    let best = -1;
    let bestDistance = radius * radius;
    for (let i = 0; i < this.count; i++) {
      const dx = minimum((this.x[i] ?? 0) - x);
      const dy = minimum((this.y[i] ?? 0) - y);
      const d = dx * dx + dy * dy;
      if (d < bestDistance) {
        bestDistance = d;
        best = i;
      }
    }
    return best;
  }

  /** What organism `i` senses now and what its own network makes of it. Reads only. */
  activity(i: number): Activity {
    this.occupancy.fill(0);
    for (let k = 0; k < this.count; k++) {
      const cell = World.cell(this.x[k] ?? 0, this.y[k] ?? 0);
      this.occupancy[cell] = (this.occupancy[cell] ?? 0) + 1;
    }
    this.sense(i);
    this.forward(i);
    return {
      inputs: Array.from(this.input),
      hidden: Array.from(this.hiddenScratch),
      outputs: Array.from(this.outputScratch),
    };
  }

  /** A 64-bit digest of the whole state (tick, field, every organism and its weights), as 16 hex digits. */
  digest(): string {
    let a = 0x811c9dc5 ^ this.seed;
    let b = 0x01000193 ^ Math.imul(this.seed, 0x9e3779b1);
    const mix = (word: number) => {
      a = Math.imul(a ^ word, 0x01000193);
      b = Math.imul((b ^ word) + 0x7f4a7c15, 0x85ebca77);
      b ^= b >>> 13;
    };
    const bits = new Float64Array(1);
    const words = new Uint32Array(bits.buffer);
    const mixFloat = (value: number) => {
      bits[0] = value;
      mix(words[0] ?? 0);
      mix(words[1] ?? 0);
    };
    mix(this.tick);
    mix(this.count);
    mix(this.mode.brains === "own" ? 1 : 2);
    mix(this.mode.children === "inherit" ? 1 : 2);
    for (let c = 0; c < CELLS; c++) mix(this.field[c] ?? 0);
    for (let i = 0; i < this.count; i++) {
      mix(this.identity[i] ?? 0);
      mix(this.energy[i] ?? 0);
      mix(this.age[i] ?? 0);
      mix(this.lineage[i] ?? 0);
      mix(this.generation[i] ?? 0);
      mixFloat(this.x[i] ?? 0);
      mixFloat(this.y[i] ?? 0);
      const base = i * PARAMETERS;
      for (let p = 0; p < PARAMETERS; p++) mixFloat(this.weights[base + p] ?? 0);
    }
    const hex = (n: number) => (n >>> 0).toString(16).padStart(8, "0");
    return hex(a) + hex(b);
  }

  /** The organism's own weights (a copy). */
  weightsOf(i: number): Float64Array {
    return this.weights.slice(i * PARAMETERS, (i + 1) * PARAMETERS);
  }
}

function add(array: Int32Array, index: number, amount: number): void {
  array[index] = (array[index] ?? 0) + amount;
}

function wrap(v: number): number {
  return v - SIZE * Math.floor(v / SIZE);
}

function wrapCell(v: number): number {
  const c = Math.floor(v) % SIZE;
  return c < 0 ? c + SIZE : c;
}

function minimum(d: number): number {
  return d - SIZE * Math.floor(d / SIZE + 0.5);
}

/** The same world from its seed, mode and brushes, run to `tick`. */
export function replay(seed: number, mode: Mode, commands: readonly Brush[], tick: number): World {
  const world = new World(seed, mode, commands);
  while (world.tick < tick) world.step();
  return world;
}
