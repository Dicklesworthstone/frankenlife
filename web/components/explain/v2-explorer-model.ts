// Pure configuration, ABI, validation and minimal CLI flag engine for the
// interactive Modular V2 Feature & CLI Toggle Explorer (`v2-explorer.tsx`).
//
// Every rule mirrors `crates/flife-core/src/v2.rs` exactly (V2Feature::ALL,
// BUNDLE_A_DEFAULTS, BUNDLE_B_DEFAULTS, V2Config::abi, validate_v2, and
// ordered left-to-right CLI overrides). Tested by `v2-explorer.test.ts`.

import { V2_FEATURE_SLUGS, type V2FeatureSlug } from "../../scripts/media-options";

export const BASE_SENSOR_WIDTH = 104;
export const BASE_ACTION_WIDTH = 36;
export const SINGLE_CARRIER_PLANES = 33;
export const DUAL_CARRIER_PLANES = 34;

export type V2VerificationState = "verified" | "in-flight";

export type V2Boundary = "pure-field" | "contact" | "mode-dependent" | "presentation";

export type V2ExplorerFeature = {
  slug: V2FeatureSlug;
  module: string;
  verification: V2VerificationState;
  boundary: V2Boundary;
  /** Matches `V2Feature::requires_contact_unconditionally()` in `crates/flife-core/src/v2.rs`. */
  requiresContact: boolean;
  bundleADefault: boolean;
  bundleBDefault: boolean;
  /** Active in `presets/supercolony-v2.toml` (11-feature showcase closure). */
  bundleBPreset: boolean;
  abiNote: string;
  summary: string;
};

/**
 * All 21 canonical `field-ecology-v2` feature slugs in exact bit order
 * (`crates/flife-core/src/v2.rs`, `V2Feature::ALL`).
 *
 * Verification status reflects the observed branch state (AGENTS.md):
 * - Verified & closed: V2-01..V2-07, V2-09, V2-14, V2-15.
 * - In flight: V2-08, V2-11, V2-12, V2-13, V2-16 (empirical campaigns V2-10 & V2-17 pending).
 */
export const V2_EXPLORER_FEATURES: readonly V2ExplorerFeature[] = [
  {
    slug: "oases",
    module: "V2-02",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: false,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none",
    summary: "Phase-staggered ephemeral oasis resource blooms on the periodic field.",
  },
  {
    slug: "senescence",
    module: "V2-03",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none",
    summary: "Gompertz metabolic maintenance ramp with organism age.",
  },
  {
    slug: "surplus-emission",
    module: "V2-04",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none",
    summary: "Surplus-gated emission floor so starving organisms cannot spend down to zero.",
  },
  {
    slug: "excitable-relay",
    module: "V2-04",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: false,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none",
    summary: "Vesicle-readiness refractory pulse relay for propagating chemical waves.",
  },
  {
    slug: "wake-offset",
    module: "V2-04",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "+1 action (neural pump mode)",
    summary: "Wake-velocity or polarized-pump emission offset behind the emitter.",
  },
  {
    slug: "so2-motor",
    module: "V2-05",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "+4 actions",
    summary: "4-output SO(2)-equivariant neural chemotaxis, chiral contour and propulsion.",
  },
  {
    slug: "so2-budding",
    module: "V2-06",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: false,
    bundleBDefault: true,
    bundleBPreset: false,
    abiNote: "+2 actions",
    summary: "2-output SO(2)-equivariant oriented offspring budding and dispersal.",
  },
  {
    slug: "hippo-crowding",
    module: "V2-07",
    verification: "verified",
    boundary: "mode-dependent",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "+1 sensor (if exposed)",
    summary: "Hippo-YAP local crowding birth arrest (pure-field occupancy or contact neighbours).",
  },
  {
    slug: "pcp-polarity",
    module: "V2-08",
    verification: "in-flight",
    boundary: "contact",
    requiresContact: true,
    bundleADefault: true,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "none",
    summary: "Persistent apical-basal planar cell polarity, wedge curvature and lateral mitosis.",
  },
  {
    slug: "emt-gate",
    module: "V2-08",
    verification: "in-flight",
    boundary: "contact",
    requiresContact: true,
    bundleADefault: false,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "+1 action",
    summary: "Neural epithelial-to-mesenchymal transition gate modulating tissue adhesion.",
  },
  {
    slug: "tight-junctions",
    module: "V2-09",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none",
    summary: "Passive symmetric flux-form diffusion barrier modulated by local cell density.",
  },
  {
    slug: "lysis-recycling",
    module: "V2-11",
    verification: "in-flight",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none",
    summary: "Viability-floor corpse lysis recycling energy and negated affinity back to the field.",
  },
  {
    slug: "symplectic-waste",
    module: "V2-11",
    verification: "in-flight",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: false,
    abiNote: "none",
    summary: "Obligate symplectic metabolic byproduct deposited to the field under the ledger.",
  },
  {
    slug: "epigenetic-recurrence",
    module: "V2-12",
    verification: "in-flight",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "+S sensors, +2S raw outputs",
    summary: "Heritable recurrent cell-state vector driving reversible fate differentiation.",
  },
  {
    slug: "notch-delta",
    module: "V2-12",
    verification: "in-flight",
    boundary: "contact",
    requiresContact: true,
    bundleADefault: true,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "+1 sensor (neighbor-mean)",
    summary: "Juxtacrine Notch-Delta lateral inhibition coupling adjacent cell states.",
  },
  {
    slug: "trophallaxis",
    module: "V2-13",
    verification: "in-flight",
    boundary: "contact",
    requiresContact: true,
    bundleADefault: true,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "none",
    summary: "Passive exact-quantum kin energy sharing across contacting tissue neighbours.",
  },
  {
    slug: "cadherin-4d",
    module: "V2-13",
    verification: "in-flight",
    boundary: "contact",
    requiresContact: true,
    bundleADefault: true,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "none",
    summary: "Decoupled 4-D heritable cadherin adhesion tag sorting tissues independently of diet.",
  },
  {
    slug: "velocity-alignment",
    module: "V2-13",
    verification: "in-flight",
    boundary: "contact",
    requiresContact: true,
    bundleADefault: false,
    bundleBDefault: false,
    bundleBPreset: false,
    abiNote: "none",
    summary: "Dissipative kin velocity alignment across local tissue neighbours.",
  },
  {
    slug: "vascular-field",
    module: "V2-14",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: false,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none",
    summary: "4-neighbour Murray/Physarum flux-reinforced vascular pipe conductances.",
  },
  {
    slug: "dual-carrier-trails",
    module: "V2-15",
    verification: "verified",
    boundary: "pure-field",
    requiresContact: false,
    bundleADefault: false,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "+4 sensors, 34 planes",
    summary: "34-plane two-carrier field separating metabolic food from persistent pheromone trails.",
  },
  {
    slug: "v2-visual-mappings",
    module: "V2-16",
    verification: "in-flight",
    boundary: "presentation",
    requiresContact: false,
    bundleADefault: true,
    bundleBDefault: true,
    bundleBPreset: true,
    abiNote: "none (INV-09 read-only)",
    summary: "Pure read-only cell-state, polarity, trail-carrier and vascular-conductance overlays.",
  },
];

export const BUNDLE_A_DEFAULTS: readonly V2FeatureSlug[] = V2_EXPLORER_FEATURES.filter((f) => f.bundleADefault).map(
  (f) => f.slug,
);
export const BUNDLE_B_DEFAULTS: readonly V2FeatureSlug[] = V2_EXPLORER_FEATURES.filter((f) => f.bundleBDefault).map(
  (f) => f.slug,
);
export const BUNDLE_B_PRESET_ACTIVE: readonly V2FeatureSlug[] = V2_EXPLORER_FEATURES.filter((f) => f.bundleBPreset).map(
  (f) => f.slug,
);

export type V2PresetId = "bundle-a" | "bundle-b" | "all-v2" | "v1-compat";

export type V2PresetSpec = {
  id: V2PresetId;
  label: string;
  shortLabel: string;
  presetPath: string;
  hasContact: boolean;
  /** Features enabled by `presetPath` on disk before CLI overrides. */
  baseEnabled: readonly V2FeatureSlug[];
  /** Target features activated when clicking this preset button. */
  targetEnabled: readonly V2FeatureSlug[];
  /** Preset's `[v2.crowding] expose_sensor` setting. */
  hippoExposeSensor: boolean;
  /** Preset's `[v2.crowding] mode`. */
  hippoLocalNeighbors: boolean;
  /** Preset's `[v2.differentiation] state_width` when recurrence is enabled. */
  stateWidth: number;
  /** Preset's `[v2.differentiation.notch_delta] mode == "sensor-neighbor-mean-v1"`. */
  notchNeighborMean: boolean;
  /** Preset's `[v2.emission.offset]` neural-output pump mode. */
  wakeNeuralPump: boolean;
  note: string;
};

export const V2_PRESETS: readonly V2PresetSpec[] = [
  {
    id: "bundle-a",
    label: "Bundle A · Tissue",
    shortLabel: "Bundle A",
    presetPath: "presets/morphogenesis-tissue-v2.toml",
    hasContact: true,
    baseEnabled: BUNDLE_A_DEFAULTS,
    targetEnabled: BUNDLE_A_DEFAULTS,
    hippoExposeSensor: true,
    hippoLocalNeighbors: true,
    stateWidth: 2,
    notchNeighborMean: true,
    wakeNeuralPump: false,
    note: "14 defaults on [contact] (inv08_pure_field = false), Metal F16 4-aligned (108→40, 44 raw, 33 planes).",
  },
  {
    id: "bundle-b",
    label: "Bundle B · Super-Colony",
    shortLabel: "Bundle B",
    presetPath: "presets/supercolony-v2.toml",
    hasContact: false,
    baseEnabled: BUNDLE_B_PRESET_ACTIVE,
    targetEnabled: BUNDLE_B_PRESET_ACTIVE,
    hippoExposeSensor: false,
    hippoLocalNeighbors: false,
    stateWidth: 2,
    notchNeighborMean: false,
    wakeNeuralPump: false,
    note: "11 active pure-field features (strict INV-08, no [contact]), Metal F16 4-aligned (108→40, 34 planes).",
  },
  {
    id: "all-v2",
    label: "All V2 · 21 slugs",
    shortLabel: "All V2",
    presetPath: "presets/morphogenesis-tissue-v2.toml",
    hasContact: true,
    baseEnabled: BUNDLE_A_DEFAULTS,
    targetEnabled: V2_FEATURE_SLUGS,
    hippoExposeSensor: true,
    hippoLocalNeighbors: true,
    stateWidth: 2,
    notchNeighborMean: true,
    wakeNeuralPump: false,
    note: "Every registered V2 slug enabled over [contact] via --enable all-v2 (112→43, 47 raw, 34 planes).",
  },
  {
    id: "v1-compat",
    label: "V1 Compat · 0 slugs",
    shortLabel: "V1 Compat",
    presetPath: "presets/morphogenesis-tissue-v2.toml",
    hasContact: true,
    baseEnabled: BUNDLE_A_DEFAULTS,
    targetEnabled: [],
    hippoExposeSensor: true,
    hippoLocalNeighbors: true,
    stateWidth: 2,
    notchNeighborMean: true,
    wakeNeuralPump: false,
    note: "All V2 features disabled via --v1-compat: collapses cleanly to 104→36, 33 field planes.",
  },
];

export type V2AbiOptions = {
  hippoExposeSensor: boolean;
  hippoLocalNeighbors: boolean;
  stateWidth: number;
  notchNeighborMean: boolean;
  wakeNeuralPump: boolean;
};

export type V2AbiResult = {
  sensorWidth: number;
  recurrentStateWidth: number;
  inputWidth: number;
  actionWidth: number;
  rawOutputWidth: number;
  fieldPlanes: 33 | 34;
  metalF16Aligned: boolean;
  sensorDelta: string[];
  actionDelta: string[];
};

/** Exact mirror of `V2Config::abi` in `crates/flife-core/src/v2.rs`. */
export function computeV2Abi(enabled: ReadonlySet<V2FeatureSlug>, opts: V2AbiOptions): V2AbiResult {
  const dualCarrier = enabled.has("dual-carrier-trails");
  const hippoSensor = enabled.has("hippo-crowding") && opts.hippoExposeSensor;
  const notchSensor = enabled.has("notch-delta") && opts.notchNeighborMean;

  const sensorDelta: string[] = [];
  if (dualCarrier) sensorDelta.push("+4 trails");
  if (hippoSensor) sensorDelta.push("+1 hippo");
  if (notchSensor) sensorDelta.push("+1 notch");

  const sensorWidth =
    BASE_SENSOR_WIDTH + (dualCarrier ? 4 : 0) + (hippoSensor ? 1 : 0) + (notchSensor ? 1 : 0);

  const recurrentStateWidth = enabled.has("epigenetic-recurrence") ? Math.max(0, Math.floor(opts.stateWidth)) : 0;
  if (recurrentStateWidth > 0) sensorDelta.push(`+${recurrentStateWidth} recurrent (S=${recurrentStateWidth})`);

  const so2Motor = enabled.has("so2-motor");
  const so2Budding = enabled.has("so2-budding");
  const emtGate = enabled.has("emt-gate");
  const wakePump = enabled.has("wake-offset") && opts.wakeNeuralPump;

  const actionDelta: string[] = [];
  if (so2Motor) actionDelta.push("+4 so2-motor");
  if (so2Budding) actionDelta.push("+2 so2-budding");
  if (emtGate) actionDelta.push("+1 emt-gate");
  if (wakePump) actionDelta.push("+1 wake-pump");

  const actionWidth =
    BASE_ACTION_WIDTH + (so2Motor ? 4 : 0) + (so2Budding ? 2 : 0) + (emtGate ? 1 : 0) + (wakePump ? 1 : 0);
  const inputWidth = sensorWidth + recurrentStateWidth;
  const rawOutputWidth = actionWidth + 2 * recurrentStateWidth;
  const fieldPlanes: 33 | 34 = dualCarrier ? DUAL_CARRIER_PLANES : SINGLE_CARRIER_PLANES;

  return {
    sensorWidth,
    recurrentStateWidth,
    inputWidth,
    actionWidth,
    rawOutputWidth,
    fieldPlanes,
    metalF16Aligned: inputWidth % 4 === 0 && rawOutputWidth % 4 === 0,
    sensorDelta,
    actionDelta,
  };
}

export type V2ValidationIssue = {
  code: "contact-required" | "notch-requires-recurrence";
  slug: V2FeatureSlug;
  detail: string;
};

export type V2BoundarySummary = {
  inv08PureField: boolean;
  anyContactActive: boolean;
  issues: V2ValidationIssue[];
};

/** Exact mirror of `validate_v2` dependency & INV-08 rules in `crates/flife-core/src/v2.rs`. */
export function validateV2Selection(
  enabled: ReadonlySet<V2FeatureSlug>,
  hasContact: boolean,
  opts: Pick<V2AbiOptions, "hippoLocalNeighbors" | "stateWidth">,
): V2BoundarySummary {
  const issues: V2ValidationIssue[] = [];
  let anyContactActive = false;

  for (const feature of V2_EXPLORER_FEATURES) {
    if (!enabled.has(feature.slug)) continue;
    if (feature.requiresContact) {
      anyContactActive = true;
      if (!hasContact) {
        issues.push({
          code: "contact-required",
          slug: feature.slug,
          detail: `\`${feature.slug}\` requires a [contact] table (violates INV-08 pure-field)`,
        });
      }
    }
  }

  if (enabled.has("hippo-crowding") && opts.hippoLocalNeighbors) {
    anyContactActive = true;
    if (!hasContact) {
      issues.push({
        code: "contact-required",
        slug: "hippo-crowding",
        detail: "`hippo-crowding` in local-neighbors-v1 mode requires [contact]; use cell-occupancy-v1 for INV-08",
      });
    }
  }

  if (enabled.has("notch-delta") && (!enabled.has("epigenetic-recurrence") || opts.stateWidth < 1)) {
    issues.push({
      code: "notch-requires-recurrence",
      slug: "notch-delta",
      detail: "`notch-delta` requires `epigenetic-recurrence` enabled with state_width >= 1",
    });
  }

  return {
    inv08PureField: !hasContact && !anyContactActive,
    anyContactActive,
    issues,
  };
}

const SLUG_ORDER = new Map<V2FeatureSlug, number>(V2_FEATURE_SLUGS.map((slug, idx) => [slug, idx]));

function sortCanonical(slugs: Iterable<V2FeatureSlug>): V2FeatureSlug[] {
  return [...slugs].sort((a, b) => (SLUG_ORDER.get(a) ?? 99) - (SLUG_ORDER.get(b) ?? 99));
}

/**
 * Compute the minimal valid founding CLI flag sequence (`--v1-compat`, `--disable`,
 * `--enable`, aliases `all-v2`, `bundle-a`, `bundle-b`) transforming `baseEnabled`
 * into `targetEnabled` in exact left-to-right `flife` override order.
 */
export function buildMinimalV2CliFlags(
  baseEnabled: ReadonlySet<V2FeatureSlug>,
  targetEnabled: ReadonlySet<V2FeatureSlug>,
): string[] {
  const same =
    baseEnabled.size === targetEnabled.size && [...baseEnabled].every((slug) => targetEnabled.has(slug));
  if (same) return [];

  if (targetEnabled.size === 0) return ["--v1-compat"];

  const candidates: string[][] = [];

  // Candidate 1: direct delta from the preset's materialized defaults.
  const toDisable = sortCanonical([...baseEnabled].filter((slug) => !targetEnabled.has(slug)));
  const toEnable = sortCanonical([...targetEnabled].filter((slug) => !baseEnabled.has(slug)));
  const delta: string[] = [];
  if (toDisable.length > 0) delta.push("--disable", toDisable.join(","));
  if (toEnable.length > 0) delta.push("--enable", toEnable.join(","));
  candidates.push(delta);

  // Candidate 2: start from --v1-compat then enable the kept slugs.
  const kept = sortCanonical(targetEnabled);
  candidates.push(["--v1-compat", "--enable", kept.join(",")]);

  // Candidate 3: start from --enable all-v2 then disable omitted slugs.
  const omitted = sortCanonical(V2_FEATURE_SLUGS.filter((slug) => !targetEnabled.has(slug)));
  if (omitted.length === 0) {
    candidates.push(["--enable", "all-v2"]);
  } else {
    candidates.push(["--enable", "all-v2", "--disable", omitted.join(",")]);
  }

  // Pick shortest rendered flag string (tie-break: fewer flag tokens, then direct delta first).
  candidates.sort((a, b) => {
    const lenDiff = a.join(" ").length - b.join(" ").length;
    if (lenDiff !== 0) return lenDiff;
    return a.length - b.length;
  });
  return candidates[0] ?? delta;
}

/** Replay left-to-right CLI feature override flags onto `base` (for round-trip testing). */
export function applyV2CliFlags(
  base: ReadonlySet<V2FeatureSlug>,
  flags: readonly string[],
): Set<V2FeatureSlug> {
  const expand = (token: string): readonly V2FeatureSlug[] => {
    if (token === "all-v2") return V2_FEATURE_SLUGS;
    if (token === "bundle-a") return BUNDLE_A_DEFAULTS;
    if (token === "bundle-b") return BUNDLE_B_DEFAULTS;
    if (SLUG_ORDER.has(token as V2FeatureSlug)) return [token as V2FeatureSlug];
    throw new Error(`unknown v2 feature token "${token}"`);
  };
  const next = new Set<V2FeatureSlug>(base);
  for (let i = 0; i < flags.length; i += 1) {
    const flag = flags[i];
    if (flag === "--v1-compat") {
      next.clear();
    } else if (flag === "--disable" || flag === "--enable") {
      const raw = flags[i + 1];
      if (!raw) throw new Error(`${flag} missing value`);
      i += 1;
      for (const token of raw.split(",")) {
        for (const slug of expand(token)) {
          if (flag === "--disable") next.delete(slug);
          else next.add(slug);
        }
      }
    } else {
      throw new Error(`unexpected flag "${String(flag)}"`);
    }
  }
  return next;
}

/** Format the full copy-ready `flife` CLI command. */
export function formatV2CliCommand(
  subcommand: "run" | "validate",
  presetPath: string,
  flags: readonly string[],
): string {
  const base =
    subcommand === "run"
      ? ["flife", "run", presetPath, "--ticks", "1024"]
      : ["flife", "validate", presetPath];
  return [...base, ...flags].join(" ");
}

/** WAI-ARIA radiogroup keyboard navigation helper (Arrow keys, Home, End). */
export function nextRadioIndex(current: number, length: number, key: string): number | null {
  if (length <= 0) return null;
  if (key === "ArrowRight" || key === "ArrowDown") return (current + 1) % length;
  if (key === "ArrowLeft" || key === "ArrowUp") return (current - 1 + length) % length;
  if (key === "Home") return 0;
  if (key === "End") return length - 1;
  return null;
}
