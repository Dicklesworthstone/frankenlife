// The scale arithmetic of README.md "Why the scale is hard" and plan §19.2,
// §19.5. Derived from stated assumptions; nothing here is a measurement.

/** The operating profiles' populations (README "Operating profiles"; Targets, not results). */
export const PROFILES = [
  { name: "Reference", population: 4_096 },
  { name: "Interactive", population: 25_000 },
  { name: "Desktop", population: 100_000 },
  { name: "Target full", population: 400_000 },
] as const;

/** README's table rows: full-population sweeps per second. */
export const SWEEP_ROWS = [15, 30, 60, 128] as const;

export type Precision = "f16" | "f32";

export const BYTES_PER_WEIGHT: Record<Precision, number> = { f16: 2, f32: 4 };

export type Scale = {
  /** Independent parameters: population × mean parameters. No two organisms share one. */
  parameters: number;
  /** The weight bank in bytes. */
  bankBytes: number;
  /** One multiply-accumulate per parameter per evaluation (biases counted, as the README does). */
  macsPerSecond: number;
  /** Every sweep streams the whole bank: there is almost no weight reuse. */
  streamBytesPerSecond: number;
};

export function scale(population: number, meanParameters: number, sweepsPerSecond: number, precision: Precision): Scale {
  const parameters = population * meanParameters;
  const bankBytes = parameters * BYTES_PER_WEIGHT[precision];
  return {
    parameters,
    bankBytes,
    macsPerSecond: parameters * sweepsPerSecond,
    streamBytesPerSecond: bankBytes * sweepsPerSecond,
  };
}

/**
 * Plan §19.5: the starter's minimum mandatory cost per organism at unit
 * sensing scale, `0.005 + 0.001 + P × 128 × 2e−9` energy units per
 * simulated second (basal, sensing, and the controller evaluated 128 times a
 * simulated second at Δt = 1/128). Before movement, emission, birth and field
 * losses.
 */
export function mandatoryCost(meanParameters: number): number {
  return 0.005 + 0.001 + meanParameters * 128 * 2e-9;
}

/** The starter profile's eight sources: 8 × 80 energy per simulated second (plan §19.5). */
export const STARTER_SOURCE_POWER = 640;

/** The most organisms of this size the starter's sources could sustain with perfect uptake and no other losses. */
export function sustainable(meanParameters: number): number {
  return STARTER_SOURCE_POWER / mandatoryCost(meanParameters);
}

const GIB = 2 ** 30;

/** `3,995,200,000` → "3.995 billion"; small counts in full. */
export function count(value: number, significant = 4): string {
  const units: [number, string][] = [
    [1e12, "trillion"],
    [1e9, "billion"],
    [1e6, "million"],
  ];
  for (const [size, name] of units) {
    if (value >= size) return `${trim(value / size, significant)} ${name}`;
  }
  return Math.round(value).toLocaleString("en-US");
}

/** Bytes as GiB (or MiB below one GiB), the README's unit for the bank. */
export function gibibytes(bytes: number): string {
  if (bytes >= GIB) return `${trim(bytes / GIB, 3)} GiB`;
  return `${trim(bytes / 2 ** 20, 3)} MiB`;
}

/** Bytes per second in decimal units, as the README's table gives them. */
export function rate(bytesPerSecond: number): string {
  if (bytesPerSecond >= 1e12) return `${trim(bytesPerSecond / 1e12, 3)} TB/s`;
  if (bytesPerSecond >= 1e9) return `${trim(bytesPerSecond / 1e9, 3)} GB/s`;
  return `${trim(bytesPerSecond / 1e6, 3)} MB/s`;
}

/** `value` to `significant` figures, without trailing zeros or exponent notation. */
export function trim(value: number, significant: number): string {
  if (value === 0) return "0";
  const digits = Math.max(0, significant - 1 - Math.floor(Math.log10(Math.abs(value))));
  const rounded = Number(value.toFixed(Math.min(digits, 20)));
  return rounded.toLocaleString("en-US", { maximumFractionDigits: digits });
}
