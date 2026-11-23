// Keep studio-keymap.ts synchronized with crates/flife-studio/src/controls.rs
// (KEY_DIGITS 1..9, V2-16 cell-state/polarity keys 8 & 9, and C field cycling).

import { describe, expect, test } from "bun:test";

import { GROUPS, KEYS, ROWS } from "./studio-keymap";

describe("studio-keymap", () => {
  test("defines all nine colour-mapping digit keys 1..9 matching controls.rs KEY_DIGITS", () => {
    for (const digit of ["1", "2", "3", "4", "5", "6", "7", "8", "9"]) {
      const entry = KEYS[digit];
      expect(entry).toBeDefined();
      expect(entry?.group).toBe("look");
    }
    expect(KEYS["8"]?.v2).toBe(true);
    expect(KEYS["8"]?.action).toContain("cell-state-v1");
    expect(KEYS["8"]?.action).toContain("v2-visual-mappings");
    expect(KEYS["9"]?.v2).toBe(true);
    expect(KEYS["9"]?.action).toContain("polarity-v1");
    expect(KEYS["9"]?.action).toContain("v2-visual-mappings");
  });

  test("C field cycling documents both V2-16 field overlays under v2-visual-mappings", () => {
    const c = KEYS.C;
    expect(c).toBeDefined();
    expect(c?.group).toBe("field");
    expect(c?.v2).toBe(true);
    expect(c?.action).toContain("trail-carrier");
    expect(c?.action).toContain("vascular-conductance");
    expect(c?.action).toContain("v2-visual-mappings");
  });

  test("every live key appears on the keyboard drawing and belongs to a known group", () => {
    const drawn = new Set(ROWS.flat().map((cap) => cap.label));
    const groupIds = new Set(GROUPS.map((group) => group.id));
    for (const [label, entry] of Object.entries(KEYS)) {
      expect(drawn.has(label)).toBe(true);
      expect(groupIds.has(entry.group)).toBe(true);
    }
  });
});
