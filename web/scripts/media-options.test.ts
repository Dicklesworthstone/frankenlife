import { describe, expect, test } from "bun:test";
import { resolve } from "node:path";

import { v2Features } from "../lib/content";
import {
  ASSET_LIMIT,
  MEDIA_COLORS,
  SOCIAL_CROP_VF,
  V2_FEATURE_SLUGS,
  buildMasterEncodeArgs,
  buildWebEncodeArgs,
  cloudflareCeilingKbps,
  isInsideDirectory,
  isValidFieldChoice,
  parseControlsArgs,
  parseMediaArgs,
} from "./media-options";

const PUBLIC_MEDIA = resolve("/tmp/frankenlife-web/public/media");

describe("parseMediaArgs", () => {
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
