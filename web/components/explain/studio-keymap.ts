// The native Studio's keys, as crates/flife-studio/src/controls.rs reads
// them (by virtual key code) and crates/flife-studio/CONTRACT.md tabulates
// them; brush amounts from crates/flife-studio/src/brush.rs (BRUSH_ENERGY 64,
// BRUSH_RADIUS_POINTS 12, BRUSH_INTRODUCTION 8, ENERGY_STEPS ±6 halvings,
// INTRODUCTION_STEPS ±3) and README.md. Only keys that exist are listed.

export type KeyGroup = "run" | "look" | "field" | "camera" | "tools";

export const GROUPS: { id: KeyGroup; name: string; color: string; summary: string }[] = [
  { id: "run", name: "Run", color: "#4ade80", summary: "Run, pause, step and close the session." },
  { id: "look", name: "Look", color: "#F15BB5", summary: "Colour mappings and looks; the picture only." },
  { id: "field", name: "Field", color: "#67e8f9", summary: "What the field under the organisms shows; the picture only." },
  { id: "camera", name: "Camera and selection", color: "#FFB627", summary: "Reset, track, dump a controller; observer state only." },
  { id: "tools", name: "Brushes and measuring", color: "#FF6F59", summary: "Brushes the session applies at its next open boundary; the measuring tool changes nothing." },
];

export type StudioKey = { group: KeyGroup; name: string; action: string };

/** By the cap's label on the keyboard drawing. */
export const KEYS: Record<string, StudioKey> = {
  Esc: { group: "run", name: "Esc (or ⌘W)", action: "Close the Studio, once. ⌘W closes too." },
  Space: { group: "run", name: "Space", action: "Run or pause the world. The Studio opens running unless started with --paused." },
  "→": { group: "run", name: "→", action: "While paused, step one tick; held, it steps again." },
  ".": { group: "run", name: ". (period)", action: "While paused, step one tick, as → does." },

  "1": { group: "look", name: "1", action: "Colour by founder lineage." },
  "2": { group: "look", name: "2", action: "Colour by energy, dark to bright." },
  "3": { group: "look", name: "3", action: "Colour by generation, on a logarithmic scale." },
  "4": { group: "look", name: "4", action: "Colour by motion: hue for heading, strength for speed." },
  "5": { group: "look", name: "5", action: "Colour by topology: the organism's catalog network." },
  "6": { group: "look", name: "6", action: "Colour by chemical affinity: like chemistry, like colour; grey for none." },
  "7": {
    group: "look",
    name: "7",
    action: "Lineage lit by energy, as the film is: black with none, white with about 1.8 times what a birth needs.",
  },
  R: { group: "look", name: "R", action: "The next look: glow, then raw (no halos, linear grey field), then sphere." },
  D: { group: "look", name: "D", action: "Density per screen bin instead of dots, and back." },

  F: { group: "field", name: "F", action: "Show or hide the energy field the organisms feed on." },
  C: {
    group: "field",
    name: "C",
    action: "The field's energy, then its chemical composition (hue for chemistry), then its chosen component, in turn.",
  },
  "[": { group: "field", name: "[", action: "Show the field's chosen chemical channel; again, the previous of its 32." },
  "]": { group: "field", name: "]", action: "Show the field's chosen chemical channel; again, the next of its 32." },

  "0": { group: "camera", name: "0", action: "The whole torus again: zoom 1, centred." },
  T: {
    group: "camera",
    name: "T",
    action: "Track the selected organism, the camera keeping it centred; when it dies, its heir. Again to stop.",
  },
  W: {
    group: "camera",
    name: "W",
    action: "Dump the selected organism's whole controller as a weights record, read at the committed boundary.",
  },

  E: { group: "tools", name: "E", action: "Arm the energy brush: a click adds 64 units of declared energy to the field cell under it." },
  X: {
    group: "tools",
    name: "X",
    action: "Arm the removal brush: a click removes the organisms within 12 points; a drag draws the disk to remove.",
  },
  M: {
    group: "tools",
    name: "M",
    action: "Arm the material brush: 64 units whose chemistry is the selected organism's affinity, the material it converts best.",
  },
  I: { group: "tools", name: "I", action: "Arm the introduction brush: a click introduces 8 founder-drawn organisms, each its own lineage." },
  G: {
    group: "tools",
    name: "G",
    action: "Arm the measuring tool: a click or drag reports a disk's organisms, energy, lineages and field. Nothing changes.",
  },
  "-": { group: "tools", name: "-", action: "Halve the armed brush's amount (down to 1 energy) or count (down to 1 organism)." },
  "=": { group: "tools", name: "=", action: "Double the armed brush's amount (up to 4,096 energy) or count (up to 64 organisms)." },
  "/": {
    group: "tools",
    name: "/",
    action: "With the removal brush armed, what it removes: recycled as a death's, dissipated, or taken out of the world, in turn.",
  },
};

/** One cap on the drawing: its label and its width in half-key columns of a 30-column row. */
export type Cap = { label: string; span: number };

const cap = (label: string, span = 2): Cap => ({ label, span });
const caps = (labels: string) => [...labels].map((label) => cap(label));

/** A compact Mac keyboard: only the caps above are live; the rest are drawn dim for orientation. */
export const ROWS: Cap[][] = [
  [cap("Esc"), ...caps("1234567890"), cap("-"), cap("=")],
  [cap("Tab", 3), ...caps("QWERTYUIOP"), cap("["), cap("]")],
  [cap("Caps", 4), ...caps("ASDFGHJKL;'")],
  [cap("Shift", 5), ...caps("ZXCVBNM,./")],
  [cap("", 3), cap("⌘", 3), cap("Space", 12), cap("⌘", 3), cap("", 2), cap("←"), cap("↓"), cap("→")],
];

/** The pointer's gestures (controls.rs's table). */
export const GESTURES = [
  { gesture: "Scroll", action: "Zoom about the pointer; up zooms in." },
  { gesture: "Drag", action: "Pan, across the torus's seams; with removal or measuring armed, draw the disk." },
  { gesture: "Click", action: "Select the organism under the pointer: its energy, age, motion, inputs and hidden activations, read back from the session. With a brush armed, brush there." },
] as const;
