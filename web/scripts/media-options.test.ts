import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { v2Features } from "../lib/content";
import { type ControlsManifest, type FilmManifest, collectMediaFiles } from "../lib/media";
import type { CameraKey } from "./director";
import {
  ASSET_LIMIT,
  MEDIA_COLORS,
  SOCIAL_CROP_VF,
  V2_FEATURE_SLUGS,
  buildMasterEncodeArgs,
  buildSegmentEncodeArgs,
  buildWebEncodeArgs,
  cloudflareCeilingKbps,
  findCameraCutFrames,
  isInsideDirectory,
  isValidFieldChoice,
  parseControlsArgs,
  parseMediaArgs,
  planSegmentRanges,
  splitSegmentRange,
} from "./media-options";
import { validateSegmentsManifest, verifyMediaDirectory } from "./media-verify";

const PUBLIC_MEDIA = resolve("/tmp/frankenlife-web/public/media");

describe("parseMediaArgs", () => {
  test("--camera fixed holds the one zoom --zooms gives, and refuses any other count", () => {
    const base = ["--flife", "flife", "--run-dir", "../runs/veins", "--work", "/tmp/work-fixed", "--camera", "fixed"];
    const opts = parseMediaArgs([...base, "--zooms", "1.03"], PUBLIC_MEDIA);
    expect(opts.cameraMode).toBe("fixed");
    expect(opts.zooms).toEqual([1.03]);
    expect(() => parseMediaArgs(base, PUBLIC_MEDIA)).toThrow("exactly one zoom");
    expect(() => parseMediaArgs([...base, "--zooms", "1.03,2"], PUBLIC_MEDIA)).toThrow("exactly one zoom");
  });

  test("parses square 4K60 via --size 2160 --fps 60 and archival --master-out", () => {
    const opts = parseMediaArgs(
      [
        "--flife",
        "../target/release/flife",
        "--run-dir",
        "../runs/confluent",
        "--work",
        "/tmp/work-4k60",
        "--size",
        "2160",
        "--fps",
        "60",
        "--crf",
        "24",
        "--master-out",
        "/tmp/masters/film-4k60.mp4",
        "--master-crf",
        "15",
        "--color",
        "cell-state",
        "--field",
        "trail-carrier",
        "--look",
        "sphere",
      ],
      PUBLIC_MEDIA,
    );
    expect(opts.width).toBe(2160);
    expect(opts.height).toBe(2160);
    expect(opts.fps).toBe(60);
    expect(opts.crf).toBe(24);
    expect(opts.masterOut).toBe(resolve("/tmp/masters/film-4k60.mp4"));
    expect(opts.masterCrf).toBe(15);
    expect(opts.color).toBe("cell-state");
    expect(opts.field).toBe("trail-carrier");
    expect(opts.look).toBe("sphere");
    expect(opts.featureFlags).toEqual([]);
  });

  test("parses 16:9 4K60 via --width 3840 --height 2160 --fps 60 and ordered V2 toggles on --config", () => {
    const opts = parseMediaArgs(
      [
        "--flife",
        "../target/release/flife",
        "--config",
        "../presets/supercolony-v2.toml",
        "--work",
        "/tmp/work-v2",
        "--width",
        "3840",
        "--height",
        "2160",
        "--fps",
        "60",
        "--color",
        "polarity",
        "--field",
        "vascular-conductance",
        "--v1-compat",
        "--enable",
        "vascular-field,dual-carrier-trails",
        "--disable",
        "oases",
      ],
      PUBLIC_MEDIA,
    );
    expect(opts.width).toBe(3840);
    expect(opts.height).toBe(2160);
    expect(opts.fps).toBe(60);
    expect(opts.runDir).toBeNull();
    expect(opts.config).toBe(resolve("../presets/supercolony-v2.toml"));
    expect(opts.color).toBe("polarity");
    expect(opts.field).toBe("vascular-conductance");
    expect(opts.featureFlags).toEqual([
      "--v1-compat",
      "--enable",
      "vascular-field,dual-carrier-trails",
      "--disable",
      "oases",
    ]);
  });

  test("refuses V2 feature toggles with --run-dir (INV-14)", () => {
    for (const toggle of [["--v1-compat"], ["--disable", "oases"], ["--enable", "bundle-b"]]) {
      expect(() =>
        parseMediaArgs(
          ["--flife", "flife", "--run-dir", "run", "--work", "work", ...toggle],
          PUBLIC_MEDIA,
        ),
      ).toThrow(/INV-14/);
    }
  });

  test("refuses unknown V2 feature slugs, odd dimensions, bad fields and master inside public/media", () => {
    expect(() =>
      parseMediaArgs(["--flife", "flife", "--config", "c.toml", "--work", "w", "--disable", "not-a-feature"]),
    ).toThrow(/unknown v2 feature/);

    expect(() =>
      parseMediaArgs(["--flife", "flife", "--run-dir", "r", "--work", "w", "--width", "2161", "--height", "2160"]),
    ).toThrow(/must be even/);

    expect(() =>
      parseMediaArgs(["--flife", "flife", "--run-dir", "r", "--work", "w", "--size", "1079"]),
    ).toThrow(/must be even/);

    expect(() =>
      parseMediaArgs(["--flife", "flife", "--run-dir", "r", "--work", "w", "--field", "component-32"]),
    ).toThrow(/--field/);

    expect(() =>
      parseMediaArgs(
        [
          "--flife",
          "flife",
          "--run-dir",
          "r",
          "--work",
          "w",
          "--master-out",
          `${PUBLIC_MEDIA}/master.mp4`,
        ],
        PUBLIC_MEDIA,
      ),
    ).toThrow(/outside public\/media/);

    expect(() =>
      parseMediaArgs(["--flife", "flife", "--run-dir", "r", "--work", "w", "--master-crf", "16"]),
    ).toThrow(/--master-crf requires --master-out/);

    expect(() => parseMediaArgs(["--flife", "flife", "--work", "w"])).toThrow(/exactly one of --run-dir/);
    expect(() =>
      parseMediaArgs(["--flife", "flife", "--run-dir", "r", "--config", "c.toml", "--work", "w"]),
    ).toThrow(/exactly one of --run-dir/);
  });

  test("accepts all 9 colour mappings and component-0..31 field mappings", () => {
    expect(MEDIA_COLORS).toContain("cell-state");
    expect(MEDIA_COLORS).toContain("polarity");
    expect(MEDIA_COLORS).toHaveLength(9);
    expect(isValidFieldChoice("component-0")).toBe(true);
    expect(isValidFieldChoice("component-31")).toBe(true);
    expect(isValidFieldChoice("component-07")).toBe(false);
    expect(isValidFieldChoice("component-32")).toBe(false);
    expect(isValidFieldChoice("trail-carrier")).toBe(true);
    expect(isValidFieldChoice("vascular-conductance")).toBe(true);
  });
});

describe("encoding helpers", () => {
  test("cloudflareCeilingKbps matches the committed manifest's 401-frame 30fps cap and scales cleanly to 60fps", () => {
    expect(cloudflareCeilingKbps(401, 30)).toBe(14120);
    // 600 frames at 60 fps = 10 s: 0.9 * 25 MiB * 8 / 10 / 1000 = 18874 kbps
    expect(cloudflareCeilingKbps(600, 60)).toBe(18874);
    expect(cloudflareCeilingKbps(1200, 60)).toBe(9437);
    expect((cloudflareCeilingKbps(1200, 60) * 1000 * 20) / 8).toBeLessThan(ASSET_LIMIT);
  });

  test("web encode caps bitrate while archival master encode is uncapped with identical BT.709 colour chain", () => {
    const webArgs = buildWebEncodeArgs(24, 600, 60);
    const masterArgs = buildMasterEncodeArgs(16);
    expect(webArgs).toContain("-maxrate");
    expect(webArgs).toContain("18874k");
    expect(masterArgs).not.toContain("-maxrate");
    expect(masterArgs).not.toContain("-bufsize");
    expect(masterArgs[masterArgs.indexOf("-crf") + 1]).toBe("16");
    expect(masterArgs[masterArgs.indexOf("-vf") + 1]).toBe(webArgs[webArgs.indexOf("-vf") + 1]);
    expect(SOCIAL_CROP_VF).toContain("force_original_aspect_ratio=increase");
    expect(isInsideDirectory(`${PUBLIC_MEDIA}/nested/a.mp4`, PUBLIC_MEDIA)).toBe(true);
    expect(isInsideDirectory("/tmp/exports/master.mp4", PUBLIC_MEDIA)).toBe(false);
  });
});

describe("parseControlsArgs", () => {
  test("defaults --size to 1080 and supports 2160 for 4K stills", () => {
    expect(parseControlsArgs(["--flife", "flife", "--work", "w"]).size).toBe(1080);
    const fourK = parseControlsArgs(["--flife", "flife", "--work", "w", "--size", "2160", "--parallel", "3"]);
    expect(fourK.size).toBe(2160);
    expect(fourK.parallel).toBe(3);
    expect(() => parseControlsArgs(["--flife", "flife", "--work", "w", "--size", "2159"])).toThrow(/must be even/);
  });
});

describe("V2 feature slug registry parity", () => {
  test("web/lib/content.ts lists the exact 21 canonical V2Feature slugs in canonical bit order", () => {
    expect(V2_FEATURE_SLUGS).toHaveLength(21);
    expect(v2Features.map((entry) => entry.slug)).toEqual([...V2_FEATURE_SLUGS]);
  });
});

describe("multi-segment 4K60 splitting and manifest verification", () => {
  const ticks = Array.from({ length: 600 }, (_, f) => 2400 + f * 4);
  const cameraKeys: CameraKey[] = [
    { tick: 2400, center: [40, 40], zoom: 1 },
    { tick: 2592, center: [40, 40], zoom: 1 }, // frame 48: opening ends -> transition 1
    { tick: 2760, center: [20, 30], zoom: 2.5 }, // frame 90: shot 1 hold
    { tick: 3160, center: [22, 31], zoom: 2.5 }, // frame 190: shot 1 ends -> transition 2
    { tick: 3320, center: [60, 50], zoom: 4.0 }, // frame 230: shot 2 hold
    { tick: 3920, center: [58, 52], zoom: 4.0 }, // frame 380: shot 2 ends -> transition 3
    { tick: 4080, center: [35, 65], zoom: 2.2 }, // frame 420: shot 3 hold
    { tick: 4600, center: [37, 63], zoom: 2.2 }, // frame 550: shot 3 ends -> closing
    { tick: 4796, center: [40, 40], zoom: 1 }, // frame 599
  ];

  test("parses --segments auto|N and --segment-max-mib cleanly and rejects unsafe caps", () => {
    const auto = parseMediaArgs(["--flife", "flife", "--run-dir", "run", "--work", "w"]);
    expect(auto.segments).toBe("auto");
    expect(auto.segmentMaxBytes).toBe(22 * 1024 * 1024);

    const fixed = parseMediaArgs([
      "--flife",
      "flife",
      "--run-dir",
      "run",
      "--work",
      "w",
      "--segments",
      "3",
      "--segment-max-mib",
      "20",
    ]);
    expect(fixed.segments).toBe(3);
    expect(fixed.segmentMaxBytes).toBe(20 * 1024 * 1024);

    expect(() =>
      parseMediaArgs(["--flife", "flife", "--run-dir", "run", "--work", "w", "--segment-max-mib", "26"]),
    ).toThrow(/--segment-max-mib/);

    expect(() =>
      parseMediaArgs(["--flife", "flife", "--run-dir", "run", "--work", "w", "--segments", "0"]),
    ).toThrow(/--segments/);

    // Package script `media:4k60` prepends true-60fps defaults (--ticks 3200 --every 4 -> 801 frames = 13.35s);
    // later CLI flags override cleanly.
    const media4k60Defaults = parseMediaArgs([
      "--ticks",
      "3200",
      "--every",
      "4",
      "--width",
      "2160",
      "--height",
      "2160",
      "--fps",
      "60",
      "--crf",
      "19",
      "--segments",
      "auto",
      "--segment-max-mib",
      "22",
      "--flife",
      "flife",
      "--run-dir",
      "run",
      "--work",
      "w",
    ]);
    expect(media4k60Defaults.ticks).toBe(3200);
    expect(media4k60Defaults.every).toBe(4);
    expect(media4k60Defaults.width).toBe(2160);
    expect(media4k60Defaults.height).toBe(2160);
    expect(media4k60Defaults.fps).toBe(60);
    expect(media4k60Defaults.ticks / media4k60Defaults.every + 1).toBe(801);

    const overridden = parseMediaArgs([
      "--ticks",
      "3200",
      "--every",
      "4",
      "--width",
      "2160",
      "--height",
      "2160",
      "--fps",
      "60",
      "--crf",
      "19",
      "--segments",
      "auto",
      "--segment-max-mib",
      "22",
      "--flife",
      "flife",
      "--run-dir",
      "run",
      "--work",
      "w",
      "--width",
      "3840",
      "--crf",
      "18",
    ]);
    expect(overridden.width).toBe(3840);
    expect(overridden.height).toBe(2160);
    expect(overridden.fps).toBe(60);
    expect(overridden.crf).toBe(18);
    expect(overridden.segments).toBe("auto");
    expect(overridden.segmentMaxBytes).toBe(22 * 1024 * 1024);
  });

  test("plans 3, 4, and 5 camera-cut segments cleanly across 801 true-60fps frames (ticks 2,400..5,600)", () => {
    const ticks801 = Array.from({ length: 801 }, (_, f) => 2400 + f * 4);
    const keys801: CameraKey[] = [
      { tick: 2400, center: [40, 40], zoom: 1 },
      { tick: 2656, center: [40, 40], zoom: 1 }, // frame 64
      { tick: 2880, center: [20, 30], zoom: 1.6 }, // frame 120
      { tick: 3392, center: [22, 31], zoom: 1.6 }, // frame 248
      { tick: 3616, center: [28, 67], zoom: 3 }, // frame 304
      { tick: 4128, center: [28, 68], zoom: 3 }, // frame 432
      { tick: 4352, center: [55, 45], zoom: 2.2 }, // frame 488
      { tick: 4864, center: [54, 46], zoom: 2.2 }, // frame 616
      { tick: 5088, center: [40, 40], zoom: 1 }, // frame 672
      { tick: 5600, center: [40, 40], zoom: 1 }, // frame 800
    ];
    expect(findCameraCutFrames(ticks801, keys801)).toEqual([64, 120, 248, 304, 432, 488, 616, 672]);

    for (const count of [3, 4, 5] as const) {
      const ranges = planSegmentRanges(801, ticks801, keys801, count);
      expect(ranges).toHaveLength(count);
      let cursor = 0;
      for (const r of ranges) {
        expect(r.startFrame).toBe(cursor);
        expect(r.frames).toBeGreaterThan(0);
        cursor += r.frames;
      }
      expect(cursor).toBe(801);
    }
    // 4 segments snap directly to the three shot ends (frames 248, 432, 616).
    expect(planSegmentRanges(801, ticks801, keys801, 4)).toEqual([
      { startFrame: 0, frames: 248 },
      { startFrame: 248, frames: 184 },
      { startFrame: 432, frames: 184 },
      { startFrame: 616, frames: 185 },
    ]);
  });

  test("snaps segment boundaries to natural camera cuts and falls back evenly without a camera", () => {
    const cuts = findCameraCutFrames(ticks, cameraKeys);
    expect(cuts).toEqual([48, 90, 190, 230, 380, 420, 550]);

    // Splitting 600 frames into 3 targets ideal cuts 200 and 400 -> snaps to camera cuts 190 and 380!
    const snapped = planSegmentRanges(600, ticks, cameraKeys, 3);
    expect(snapped).toEqual([
      { startFrame: 0, frames: 190 },
      { startFrame: 190, frames: 190 },
      { startFrame: 380, frames: 220 },
    ]);

    // Without camera keys, splits evenly into 3 × 200.
    const even = planSegmentRanges(600, ticks, null, 3);
    expect(even).toEqual([
      { startFrame: 0, frames: 200 },
      { startFrame: 200, frames: 200 },
      { startFrame: 400, frames: 200 },
    ]);

    // Recursive subdivision snaps to an interior camera cut when inside the middle 50%, else midpoint.
    const [left, right] = splitSegmentRange({ startFrame: 0, frames: 380 }, ticks, cameraKeys);
    expect(left).toEqual({ startFrame: 0, frames: 190 });
    expect(right).toEqual({ startFrame: 190, frames: 190 });
    expect(splitSegmentRange({ startFrame: 0, frames: 100 }, ticks, null)).toEqual([
      { startFrame: 0, frames: 50 },
      { startFrame: 50, frames: 50 },
    ]);
  });

  test("buildSegmentEncodeArgs uses closed 1-second GOP with optional fallback maxrate", () => {
    const clean = buildSegmentEncodeArgs(19, 60);
    expect(clean).toContain("-g");
    expect(clean[clean.indexOf("-g") + 1]).toBe("60");
    expect(clean).toContain("-sc_threshold");
    expect(clean).not.toContain("-maxrate");

    const capped = buildSegmentEncodeArgs(19, 60, 12000);
    expect(capped).toContain("-maxrate");
    expect(capped).toContain("12000k");
  });

  test("verifyMediaDirectory and collectMediaFiles validate every segment byte-for-byte and catch defects", () => {
    const temp = mkdtempSync(join(tmpdir(), "flife-media-verify-"));
    try {
      const mediaDir = join(temp, "media");
      mkdirSync(mediaDir, { recursive: true });
      const writeAsset = (name: string, payload: string) => {
        const buf = Buffer.from(payload);
        const hash = createHash("sha256").update(buf).digest("hex");
        writeFileSync(join(mediaDir, name), buf);
        return { src: `/media/${name}`, sha256: hash, bytes: buf.length };
      };
      const seg0 = writeAsset("film-seg00-111111111111.mp4", "segment-zero-bytes");
      const seg1 = writeAsset("film-seg01-222222222222.mp4", "segment-one-bytes");
      const poster = writeAsset("poster-333333333333.jpg", "poster");
      const social = writeAsset("social-444444444444.jpg", "social");

      const filmManifest: FilmManifest = {
        schema: "frankenlife.website-media.v1",
        id: "film",
        video: {
          ...seg0,
          width: 2160,
          height: 2160,
          fps: 60,
          frames: 10,
          encode: buildSegmentEncodeArgs(19, 60),
          segments: [
            { ...seg0, start_frame: 0, frames: 4, start_tick: 100, end_tick: 112 },
            { ...seg1, start_frame: 4, frames: 6, start_tick: 116, end_tick: 136 },
          ],
        },
        poster: { ...poster, frame: 0 },
        social: { ...social, width: 1200, height: 630, frame: 5 },
        world: {
          preset: "presets/confluent-world.toml",
          name: "confluent-world",
          config_hash: "0".repeat(64),
          seed: 42,
          founders: 10954,
          grid: [320, 320],
          torus: [80, 80],
          timestep: [1, 32],
          model: "field-ecology-v1-contact",
          from: { tick: 100, digest: "a".repeat(64) },
          ticks: [100, 104, 108, 112, 116, 120, 124, 128, 132, 136],
          live: Array(10).fill(1000),
          every: 4,
          to: { tick: 136, digest: "b".repeat(64) },
        },
        render: {
          command: "flife render --run-dir RUN --out FRAMES",
          mapping: "lineage",
          meaning: "founder lineage",
          look: "sphere",
          field_mapping: "none",
          controllers: "metal",
          revision: "d6352a0",
          flife_sha256: "c".repeat(64),
        },
      };
      const chemStill = writeAsset("controls-chemistry-only-555555555555.jpg", "chem-4k-jpeg");
      const freshStill = writeAsset("controls-fresh-offspring-666666666666.jpg", "fresh-4k-jpeg");
      const controls4k = {
        stills: [
          { ...poster, id: "confluent", poster: true, width: 2160, height: 2160 },
          { ...chemStill, id: "chemistry-only", poster: false, width: 2160, height: 2160 },
          { ...freshStill, id: "fresh-offspring", poster: false, width: 2160, height: 2160 },
        ],
      } as unknown as ControlsManifest;

      // Worker & check-media collect EVERY segment + shared 4K poster (deduplicated once!) + social + 2 control stills.
      const collected = collectMediaFiles([filmManifest], [controls4k]);
      expect(collected.map((f) => f.src)).toEqual([
        seg0.src,
        seg1.src,
        poster.src,
        social.src,
        chemStill.src,
        freshStill.src,
      ]);
      expect(verifyMediaDirectory([filmManifest], [controls4k], temp)).toEqual([]);

      // Defect 1: frame gap in segments caught by validateSegmentsManifest.
      const gapped: FilmManifest = {
        ...filmManifest,
        video: {
          ...filmManifest.video,
          segments: [
            { ...seg0, start_frame: 0, frames: 4, start_tick: 100, end_tick: 112 },
            { ...seg1, start_frame: 5, frames: 5, start_tick: 120, end_tick: 136 },
          ],
        },
      };
      expect(validateSegmentsManifest(gapped).some((e) => e.includes("starts at frame 5, expected 4"))).toBe(true);

      // Defect 2: oversized segment > ASSET_LIMIT refused.
      const oversized: FilmManifest = {
        ...filmManifest,
        video: {
          ...filmManifest.video,
          segments: [
            { ...seg0, start_frame: 0, frames: 4, start_tick: 100, end_tick: 112 },
            { ...seg1, bytes: ASSET_LIMIT + 1, start_frame: 4, frames: 6, start_tick: 116, end_tick: 136 },
          ],
        },
      };
      expect(validateSegmentsManifest(oversized).some((e) => e.includes("asset limit"))).toBe(true);

      // A second film shares public/media/: its files are named by its own manifest, not strays.
      const tissue = writeAsset("tissue-777777777777.mp4", "tissue-film");
      const tissuePoster = writeAsset("tissue-poster-888888888888.jpg", "tissue-poster");
      const tissueSocial = writeAsset("tissue-social-999999999999.jpg", "tissue-social");
      const second: FilmManifest = {
        ...filmManifest,
        id: "tissue",
        video: { ...filmManifest.video, ...tissue, segments: undefined },
        poster: { ...tissuePoster, frame: 0 },
        social: { ...tissueSocial, width: 1200, height: 630, frame: 5 },
      };
      expect(verifyMediaDirectory([filmManifest], [controls4k], temp).some((e) => e.includes("tissue-777777777777.mp4"))).toBe(true);
      expect(verifyMediaDirectory([filmManifest, second], [controls4k], temp)).toEqual([]);
      expect(collectMediaFiles([filmManifest, second], [controls4k]).map((f) => f.src)).toContain(tissue.src);
      // Two films may not share an id: each names lib/films/<id>.json and prefixes its files.
      expect(
        verifyMediaDirectory([filmManifest, { ...second, id: "film" }], [controls4k], temp).some((e) =>
          e.includes("two films have the id film"),
        ),
      ).toBe(true);

      // Defect 3: unlisted stray file in public/media/ caught.
      writeFileSync(join(mediaDir, "stray-old.mp4"), "junk");
      expect(verifyMediaDirectory([filmManifest], [controls4k], temp).some((e) => e.includes("stray-old.mp4"))).toBe(true);
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  });
});
