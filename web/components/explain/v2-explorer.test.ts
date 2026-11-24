// Known-answer unit tests for the Modular V2 Feature & CLI Toggle Explorer
// (`v2-explorer-model.ts`), cross-checked directly against `crates/flife-core/src/v2.rs`
// constants, ABI widths, INV-08/V2-12 dependency rules, and CLI flag round-trips.

import { describe, expect, test } from "bun:test";

import { v2BuildStatus, v2Features } from "@/lib/content";
import { V2_FEATURE_SLUGS, type V2FeatureSlug } from "../../scripts/media-options";
import {
  BUNDLE_A_DEFAULTS,
  BUNDLE_B_DEFAULTS,
  BUNDLE_B_PRESET_ACTIVE,
  V2_EXPLORER_FEATURES,
  V2_PRESETS,
  applyV2CliFlags,
  buildMinimalV2CliFlags,
  computeV2Abi,
  formatV2CliCommand,
  nextRadioIndex,
  validateV2Selection,
} from "./v2-explorer-model";

describe("V2 explorer canonical registry parity", () => {
  test("lists all 21 V2Feature slugs in exact bit order matching media-options and content.ts", () => {
    expect(V2_EXPLORER_FEATURES).toHaveLength(21);
    expect(V2_EXPLORER_FEATURES.map((f) => f.slug)).toEqual([...V2_FEATURE_SLUGS]);
    expect(v2Features.map((f) => f.slug)).toEqual([...V2_FEATURE_SLUGS]);
    expect(BUNDLE_A_DEFAULTS).toHaveLength(14);
    expect(BUNDLE_B_DEFAULTS).toHaveLength(13);
    expect(BUNDLE_B_PRESET_ACTIVE).toHaveLength(11);
    expect(v2Features.filter((f) => f.bundleA).map((f) => f.slug)).toEqual([...BUNDLE_A_DEFAULTS]);
    expect(v2Features.filter((f) => f.bundleB).map((f) => f.slug)).toEqual([...BUNDLE_B_DEFAULTS]);
  });

  test("maps honest observed verification states (verified vs in-flight)", () => {
    const verifiedModules = new Set(["V2-02", "V2-03", "V2-04", "V2-05", "V2-06", "V2-07", "V2-09", "V2-14", "V2-15"]);
    const inFlightModules = new Set(["V2-08", "V2-11", "V2-12", "V2-13", "V2-16"]);
    for (const f of V2_EXPLORER_FEATURES) {
      if (verifiedModules.has(f.module)) {
        expect(f.verification).toBe("verified");
      } else {
        expect(inFlightModules.has(f.module)).toBe(true);
        expect(f.verification).toBe("in-flight");
      }
    }
  });
});

describe("V2 ABI and field-plane known-answer fixtures (crates/flife-core/src/v2.rs)", () => {
  const byId = (id: (typeof V2_PRESETS)[number]["id"]) => V2_PRESETS.find((p) => p.id === id)!;

  test("Bundle A (presets/morphogenesis-tissue-v2.toml): 108 inputs, 40 actions, 44 raw outputs, 33 planes, 4-aligned", () => {
    const p = byId("bundle-a");
    const active = new Set(p.targetEnabled);
    const abi = computeV2Abi(active, p);
    expect(abi.sensorWidth).toBe(106); // 104 + 1 hippo + 1 notch
    expect(abi.recurrentStateWidth).toBe(2);
    expect(abi.inputWidth).toBe(108);
    expect(abi.actionWidth).toBe(40); // 36 + 4 so2-motor
    expect(abi.rawOutputWidth).toBe(44); // 40 + 2*2
    expect(abi.fieldPlanes).toBe(33);
    expect(abi.metalF16Aligned).toBe(true);

    const validation = validateV2Selection(active, p.hasContact, p);
    expect(validation.inv08PureField).toBe(false);
    expect(validation.issues).toEqual([]);
    expect(buildMinimalV2CliFlags(new Set(p.baseEnabled), active)).toEqual([]);
  });

  test("Bundle B (presets/supercolony-v2.toml): 108 inputs, 40 actions, 40 raw outputs, 34 planes, pure INV-08", () => {
    const p = byId("bundle-b");
    const active = new Set(p.targetEnabled);
    const abi = computeV2Abi(active, p);
    expect(abi.sensorWidth).toBe(108); // 104 + 4 dual-carrier trails
    expect(abi.recurrentStateWidth).toBe(0);
    expect(abi.inputWidth).toBe(108);
    expect(abi.actionWidth).toBe(40); // 36 + 4 so2-motor
    expect(abi.rawOutputWidth).toBe(40);
    expect(abi.fieldPlanes).toBe(34);
    expect(abi.metalF16Aligned).toBe(true);

    const validation = validateV2Selection(active, p.hasContact, p);
    expect(validation.inv08PureField).toBe(true);
    expect(validation.issues).toEqual([]);
    expect(buildMinimalV2CliFlags(new Set(p.baseEnabled), active)).toEqual([]);
  });

  test("All V2 (21 slugs): 112 inputs, 43 actions, 47 raw outputs, 34 planes, --enable all-v2", () => {
    const p = byId("all-v2");
    const active = new Set(p.targetEnabled);
    const abi = computeV2Abi(active, p);
    expect(abi.sensorWidth).toBe(110); // 104 + 4 trails + 1 hippo + 1 notch
    expect(abi.recurrentStateWidth).toBe(2);
    expect(abi.inputWidth).toBe(112);
    expect(abi.actionWidth).toBe(43); // 36 + 4 motor + 2 budding + 1 emt-gate
    expect(abi.rawOutputWidth).toBe(47); // 43 + 4
    expect(abi.fieldPlanes).toBe(34);
    expect(abi.metalF16Aligned).toBe(false);
    expect(buildMinimalV2CliFlags(new Set(p.baseEnabled), active)).toEqual(["--enable", "all-v2"]);
  });

  test("V1 Compat (0 slugs): 104 inputs, 36 actions, 36 raw outputs, 33 planes, --v1-compat", () => {
    const p = byId("v1-compat");
    const active = new Set(p.targetEnabled);
    const abi = computeV2Abi(active, p);
    expect(abi.inputWidth).toBe(104);
    expect(abi.actionWidth).toBe(36);
    expect(abi.rawOutputWidth).toBe(36);
    expect(abi.fieldPlanes).toBe(33);
    expect(abi.metalF16Aligned).toBe(true);
    expect(buildMinimalV2CliFlags(new Set(p.baseEnabled), active)).toEqual(["--v1-compat"]);
  });

  test("neural wake pump mode adds +1 action when wake-offset is active", () => {
    const abi = computeV2Abi(new Set(["wake-offset", "so2-motor"]), {
      hippoExposeSensor: false,
      hippoLocalNeighbors: false,
      stateWidth: 0,
      notchNeighborMean: false,
      wakeNeuralPump: true,
    });
    expect(abi.actionWidth).toBe(41); // 36 + 4 + 1
  });
});

describe("V2 dependency & INV-08 refusal rules", () => {
  test("refuses notch-delta without epigenetic-recurrence (V2-12)", () => {
    const active = new Set<V2FeatureSlug>(BUNDLE_A_DEFAULTS);
    active.delete("epigenetic-recurrence");
    const v = validateV2Selection(active, true, { hippoLocalNeighbors: true, stateWidth: 2 });
    expect(v.issues).toHaveLength(1);
    expect(v.issues[0]?.code).toBe("notch-requires-recurrence");
    expect(v.issues[0]?.slug).toBe("notch-delta");

    active.delete("notch-delta");
    expect(validateV2Selection(active, true, { hippoLocalNeighbors: true, stateWidth: 2 }).issues).toEqual([]);
  });

  test("refuses contact-coupled modules on pure-field Bundle B without [contact] (INV-08)", () => {
    const active = new Set<V2FeatureSlug>(BUNDLE_B_PRESET_ACTIVE);
    active.add("trophallaxis");
    active.add("pcp-polarity");
    const v = validateV2Selection(active, false, { hippoLocalNeighbors: false, stateWidth: 2 });
    expect(v.inv08PureField).toBe(false);
    expect(v.issues.map((i) => i.slug)).toEqual(["pcp-polarity", "trophallaxis"]);
    expect(v.issues.every((i) => i.code === "contact-required")).toBe(true);
  });
});

describe("minimal CLI override synthesis & exact round-trip", () => {
  test("emits shortest equivalent CLI flags and round-trips identically on every test case", () => {
    const bundleABase = new Set<V2FeatureSlug>(BUNDLE_A_DEFAULTS);
    const bundleBBase = new Set<V2FeatureSlug>(BUNDLE_B_PRESET_ACTIVE);

    // 1. Single disable on Bundle A.
    const noPcp = new Set(bundleABase);
    noPcp.delete("pcp-polarity");
    const f1 = buildMinimalV2CliFlags(bundleABase, noPcp);
    expect(f1).toEqual(["--disable", "pcp-polarity"]);
    expect(applyV2CliFlags(bundleABase, f1)).toEqual(noPcp);

    // 2. Keeping only 1 feature on Bundle A uses shorter `--v1-compat --enable oases`.
    const onlyOases = new Set<V2FeatureSlug>(["oases"]);
    const f2 = buildMinimalV2CliFlags(bundleABase, onlyOases);
    expect(f2).toEqual(["--v1-compat", "--enable", "oases"]);
    expect(applyV2CliFlags(bundleABase, f2)).toEqual(onlyOases);

    // 3. Enabling 20 of 21 features uses shorter `--enable all-v2 --disable velocity-alignment`.
    const almostAll = new Set<V2FeatureSlug>(V2_FEATURE_SLUGS);
    almostAll.delete("velocity-alignment");
    const f3 = buildMinimalV2CliFlags(bundleBBase, almostAll);
    expect(f3).toEqual(["--enable", "all-v2", "--disable", "velocity-alignment"]);
    expect(applyV2CliFlags(bundleBBase, f3)).toEqual(almostAll);

    // 4. Enabling Bundle B's 2 extra defaults (`so2-budding,symplectic-waste`) on supercolony-v2.
    const fullB = new Set<V2FeatureSlug>(BUNDLE_B_DEFAULTS);
    const f4 = buildMinimalV2CliFlags(bundleBBase, fullB);
    expect(f4).toEqual(["--enable", "so2-budding,symplectic-waste"]);
    expect(applyV2CliFlags(bundleBBase, f4)).toEqual(fullB);

    // 5. Formatting run & validate commands cleanly.
    expect(formatV2CliCommand("validate", "presets/supercolony-v2.toml", f4)).toBe(
      "flife validate presets/supercolony-v2.toml --enable so2-budding,symplectic-waste",
    );
    expect(formatV2CliCommand("run", "presets/morphogenesis-tissue-v2.toml", ["--v1-compat"])).toBe(
      "flife run presets/morphogenesis-tissue-v2.toml --ticks 1024 --v1-compat",
    );
  });

  test("keeps content.ts v2BuildStatus synced with the 10/18 closed V2 beads + FL-043 and supports radiogroup arrow navigation", () => {
    const landed = v2BuildStatus.find((row) => row.tone === "built")?.detail ?? "";
    const inFlight = v2BuildStatus.find((row) => row.tone === "partial")?.detail ?? "";
    const open = v2BuildStatus.find((row) => row.tone === "open")?.detail ?? "";

    for (const closed of ["V2-01", "V2-02", "V2-03", "V2-04", "V2-05", "V2-06", "V2-07", "V2-09", "V2-14", "V2-15", "FL-043"]) {
      expect(landed).toContain(closed);
    }
    for (const active of ["V2-08", "V2-11", "V2-12", "V2-13", "V2-16"]) {
      expect(inFlight).toContain(active);
    }
    expect(open).toContain("V2-10");
    expect(open).toContain("V2-17");

    // WAI-ARIA radiogroup keyboard wrapping (ArrowRight/Left/Up/Down, Home, End).
    expect(nextRadioIndex(0, 4, "ArrowRight")).toBe(1);
    expect(nextRadioIndex(3, 4, "ArrowRight")).toBe(0);
    expect(nextRadioIndex(0, 4, "ArrowLeft")).toBe(3);
    expect(nextRadioIndex(2, 4, "ArrowUp")).toBe(1);
    expect(nextRadioIndex(2, 4, "ArrowDown")).toBe(3);
    expect(nextRadioIndex(2, 4, "Home")).toBe(0);
    expect(nextRadioIndex(1, 4, "End")).toBe(3);
    expect(nextRadioIndex(1, 4, "Tab")).toBeNull();
  });
});
