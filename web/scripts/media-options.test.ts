import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { v2Features } from "../lib/content";
import { type ControlsManifest, type FilmManifest, collectStaticMedia, videosOf } from "../lib/media";
import {
  ASSET_LIMIT,
  MEDIA_COLORS,
  POSTER_AVIF_CRF,
  SOCIAL_CROP_VF,
  V2_FEATURE_SLUGS,
  buildMasterEncodeArgs,
  buildPosterAvifArgs,
  buildWebEncodeArgs,
  isInsideDirectory,
  isValidFieldChoice,
  parseControlsArgs,
  parseMediaArgs,
} from "./media-options";
import {
  UPLOAD_LIMIT,
  etagAgainstMd5,
  filmVideoFiles,
  siteOrigin,
  verifyFilmVideos,
  verifyMediaDirectory,
  videoName,
} from "./media-verify";

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
  test("the web encode is set by its CRF alone, uncapped, with a keyframe every second", () => {
    const webArgs = buildWebEncodeArgs(30, 60);
    expect(webArgs).not.toContain("-maxrate");
    expect(webArgs).not.toContain("-bufsize");
    expect(webArgs[webArgs.indexOf("-crf") + 1]).toBe("30");
    expect(webArgs[webArgs.indexOf("-g") + 1]).toBe("60");
    expect(webArgs[webArgs.indexOf("-keyint_min") + 1]).toBe("60");
    expect(webArgs[webArgs.indexOf("-sc_threshold") + 1]).toBe("0");
    expect(webArgs[webArgs.indexOf("-movflags") + 1]).toBe("+faststart");
    expect(buildWebEncodeArgs(24, 30)[buildWebEncodeArgs(24, 30).indexOf("-g") + 1]).toBe("30");
    expect(() => buildWebEncodeArgs(30, 0)).toThrow(/fps/);
  });

  test("the archival master encode is uncapped with the web encode's BT.709 colour chain", () => {
    const webArgs = buildWebEncodeArgs(24, 60);
    const masterArgs = buildMasterEncodeArgs(16);
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

describe("one film, one file, served from R2", () => {
  test("media:4k60's defaults make a 1,601-frame film at CRF 30 with a 1080 phone encode; later flags win", () => {
    // package.json's `media:4k60` prepends these; media.ts takes the last value of a repeated flag.
    const script: string = JSON.parse(readFileSync(join(import.meta.dir, "..", "package.json"), "utf8")).scripts["media:4k60"];
    expect(script.startsWith("bun scripts/media.ts ")).toBe(true);
    const defaults = script.split(" ").slice(2);
    const media4k60 = parseMediaArgs([...defaults, "--flife", "flife", "--run-dir", "run", "--work", "w"]);
    expect(media4k60.ticks / media4k60.every + 1).toBe(1601);
    expect([media4k60.width, media4k60.height, media4k60.fps, media4k60.crf]).toEqual([2160, 2160, 60, 30]);
    expect(media4k60.small).toEqual([1080, 1080]);
    expect(media4k60.smallCrf).toBe(26);
    const overridden = parseMediaArgs([
      ...defaults,
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
    expect(overridden.crf).toBe(18);
  });

  test("--small keeps the film's aspect at an even width, and must be an even height below the film's", () => {
    const base = ["--flife", "flife", "--run-dir", "run", "--work", "w", "--width", "3840", "--height", "2160"];
    expect(parseMediaArgs(base).small).toBeNull();
    expect(parseMediaArgs([...base, "--small", "0"]).small).toBeNull();
    expect(parseMediaArgs([...base, "--small", "1080"]).small).toEqual([1920, 1080]);
    expect(parseMediaArgs([...base, "--small", "720"]).small).toEqual([1280, 720]);
    for (const bad of ["1081", "2160", "4320", "-2", "x"]) expect(() => parseMediaArgs([...base, "--small", bad])).toThrow(/--small/);
    // Its CRF is the film's unless given, and only with --small.
    expect(parseMediaArgs([...base, "--crf", "30", "--small", "1080"]).smallCrf).toBe(30);
    expect(parseMediaArgs([...base, "--small", "1080", "--small-crf", "26"]).smallCrf).toBe(26);
    expect(() => parseMediaArgs([...base, "--small-crf", "26"])).toThrow(/--small-crf requires --small/);
    expect(() => parseMediaArgs([...base, "--small", "1080", "--small-crf", "52"])).toThrow(/--small-crf/);
  });

  test("the phone encode is the film's encode with the frames scaled first, in the same filter", () => {
    const film = buildWebEncodeArgs(30, 60);
    const phone = buildWebEncodeArgs(30, 60, [1080, 1080]);
    expect(phone.length).toBe(film.length);
    const vf = film.indexOf("-vf") + 1;
    expect(phone[vf]).toBe(film[vf]!.replace("scale=", "scale=w=1080:h=1080:flags=lanczos:"));
    expect(phone.filter((_, at) => at !== vf)).toEqual(film.filter((_, at) => at !== vf));
  });

  test("a poster AVIF is one AV1 frame at the size asked, keeping the JPEG's own colour", () => {
    const args = buildPosterAvifArgs([1080, 1080]);
    const value = (flag: string) => args[args.indexOf(flag) + 1];
    // Scaled without converting range or matrix: the JPEG's full-range BT.601 samples, tagged as such.
    expect(value("-vf")).toBe("scale=w=1080:h=1080:flags=lanczos:in_range=full:out_range=full,format=yuv420p");
    expect(value("-colorspace")).toBe("bt470bg");
    expect(value("-color_range")).toBe("pc");
    expect(value("-frames:v")).toBe("1");
    expect(value("-c:v")).toBe("libsvtav1");
    expect(value("-crf")).toBe(String(POSTER_AVIF_CRF));
    // The film's size and the phone's differ only in the scale.
    const full = buildPosterAvifArgs([2160, 2160]);
    expect(full[1]).toContain("scale=w=2160:h=2160:");
    expect(full.slice(2)).toEqual(args.slice(2));
    expect(buildPosterAvifArgs([1080, 608])[1]).toContain("scale=w=1080:h=608:");
  });

  test("refuses the retired segment flags", () => {
    for (const flag of [["--segments", "auto"], ["--segment-max-mib", "22"]]) {
      expect(() => parseMediaArgs(["--flife", "flife", "--run-dir", "run", "--work", "w", ...flag])).toThrow(/unknown option/);
    }
  });

  test("verifies the static media byte for byte in public/media/ and each film's one video in media-r2/", () => {
    const temp = mkdtempSync(join(tmpdir(), "flife-media-verify-"));
    try {
      const mediaDir = join(temp, "media");
      const store = join(temp, "media-r2");
      mkdirSync(mediaDir, { recursive: true });
      mkdirSync(store, { recursive: true });
      const hashed = (payload: string) => {
        const buf = Buffer.from(payload);
        return { buf, hash: createHash("sha256").update(buf).digest("hex") };
      };
      const writeAsset = (name: string, payload: string) => {
        const { buf, hash } = hashed(payload);
        writeFileSync(join(mediaDir, name), buf);
        return { src: `/media/${name}`, sha256: hash, bytes: buf.length };
      };
      /** A film's video as media.ts names it, written into the R2 store. */
      const writeVideo = (id: string, payload: string) => {
        const { buf, hash } = hashed(payload);
        const name = `${id}-${hash.slice(0, 12)}.mp4`;
        writeFileSync(join(store, name), buf);
        return { src: `/media/${name}`, sha256: hash, bytes: buf.length };
      };
      const video = writeVideo("film", "the-whole-film");
      const poster = writeAsset("poster-333333333333.jpg", "poster");
      const social = writeAsset("social-444444444444.jpg", "social");

      const filmManifest: FilmManifest = {
        schema: "frankenlife.website-media.v1",
        id: "film",
        video: {
          ...video,
          width: 2160,
          height: 2160,
          fps: 60,
          frames: 10,
          encode: buildWebEncodeArgs(30, 60),
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

      // The static media: the poster (shared with a control still, once), the card and two control stills; no video.
      const collected = collectStaticMedia([filmManifest], [controls4k]);
      expect(collected.map((f) => f.src)).toEqual([poster.src, social.src, chemStill.src, freshStill.src]);
      expect(verifyMediaDirectory([filmManifest], [controls4k], temp)).toEqual([]);
      expect(videoName(filmManifest)).toBe(video.src.replace("/media/", ""));
      expect(verifyFilmVideos([filmManifest], store)).toEqual([]);

      // A video named other than <id>-<hash>.mp4 is refused: R2's key and the Worker's path come from the name.
      const misnamed: FilmManifest = { ...filmManifest, video: { ...filmManifest.video, src: "/media/film-seg00-111111111111.mp4" } };
      expect(verifyMediaDirectory([misnamed], [controls4k], temp).some((e) => e.includes("not /media/film-"))).toBe(true);

      // A local video that is not the manifest's bytes, or is missing, is caught.
      const otherBytes: FilmManifest = { ...filmManifest, video: { ...filmManifest.video, bytes: filmManifest.video.bytes + 1 } };
      expect(verifyFilmVideos([otherBytes], store).some((e) => e.includes("not the file its manifest names"))).toBe(true);
      expect(verifyFilmVideos([filmManifest], join(temp, "nowhere")).some((e) => e.includes("is missing"))).toBe(true);
      // The build's check (scripts/check-media.ts): an absent copy is fine, it is in R2; a wrong one is not.
      expect(verifyFilmVideos([filmManifest], join(temp, "nowhere"), true)).toEqual([]);
      expect(verifyFilmVideos([otherBytes], store, true).some((e) => e.includes("not the file its manifest names"))).toBe(true);

      // A static file over Cloudflare's 25 MiB static-asset limit is refused; one at it is not.
      const hugeCard: FilmManifest = { ...filmManifest, social: { ...filmManifest.social, bytes: ASSET_LIMIT + 1 } };
      expect(verifyMediaDirectory([hugeCard], [controls4k], temp).some((e) => e.includes("asset limit"))).toBe(true);
      const fullCard: FilmManifest = { ...filmManifest, social: { ...filmManifest.social, bytes: ASSET_LIMIT } };
      expect(verifyMediaDirectory([fullCard], [controls4k], temp).some((e) => e.includes("asset limit"))).toBe(false);

      // A film too large for one upload to R2 is refused before the deploy reaches it; one at the limit is not.
      const atLimit = (bytes: number): FilmManifest => ({ ...filmManifest, video: { ...filmManifest.video, bytes } });
      expect(verifyMediaDirectory([atLimit(UPLOAD_LIMIT + 1)], [controls4k], temp).some((e) => e.includes("one upload to R2"))).toBe(true);
      expect(verifyMediaDirectory([atLimit(UPLOAD_LIMIT)], [controls4k], temp).some((e) => e.includes("one upload to R2"))).toBe(false);

      // A second film shares public/media/: its poster and card are named by its own manifest, not strays.
      const tissue = writeVideo("tissue", "tissue-film");
      const tissuePoster = writeAsset("tissue-poster-888888888888.jpg", "tissue-poster");
      const tissueSocial = writeAsset("tissue-social-999999999999.jpg", "tissue-social");
      const second: FilmManifest = {
        ...filmManifest,
        id: "tissue",
        video: { ...filmManifest.video, ...tissue },
        poster: { ...tissuePoster, frame: 0 },
        social: { ...tissueSocial, width: 1200, height: 630, frame: 5 },
      };
      expect(verifyMediaDirectory([filmManifest], [controls4k], temp).some((e) => e.includes("tissue-poster-888888888888.jpg"))).toBe(true);
      expect(verifyMediaDirectory([filmManifest, second], [controls4k], temp)).toEqual([]);
      expect(verifyFilmVideos([filmManifest, second], store)).toEqual([]);
      expect(collectStaticMedia([filmManifest, second], [controls4k]).map((f) => f.src)).toContain(tissuePoster.src);
      // Two films may not share an id: each names lib/films/<id>.json and prefixes its files.
      expect(
        verifyMediaDirectory([filmManifest, { ...second, id: "film" }], [controls4k], temp).some((e) =>
          e.includes("two films have the id film"),
        ),
      ).toBe(true);

      // A phone encode is a second video of the film, named <id>-small-<hash>.mp4, checked in media-r2/ like it.
      const phone = writeVideo("film-small", "the-phone-film");
      const withPhone: FilmManifest = {
        ...filmManifest,
        video: { ...filmManifest.video, small: { ...phone, width: 1080, height: 1080, encode: buildWebEncodeArgs(30, 60, [1080, 1080]) } },
      };
      expect(filmVideoFiles(withPhone).map((video) => video.name)).toEqual([
        video.src.replace("/media/", ""),
        phone.src.replace("/media/", ""),
      ]);
      expect(videosOf(withPhone).map((file) => file.src)).toEqual([video.src, phone.src]);
      expect(verifyMediaDirectory([withPhone, second], [controls4k], temp)).toEqual([]);
      expect(verifyFilmVideos([withPhone, second], store)).toEqual([]);
      const misnamedPhone: FilmManifest = {
        ...withPhone,
        video: { ...withPhone.video, small: { ...withPhone.video.small!, src: "/media/film-phone-111111111111.mp4" } },
      };
      expect(verifyMediaDirectory([misnamedPhone], [controls4k], temp).some((e) => e.includes("not /media/film-small-"))).toBe(true);
      rmSync(join(store, phone.src.replace("/media/", "")));
      expect(verifyFilmVideos([withPhone], store).some((e) => e.includes("is missing"))).toBe(true);

      // The poster AVIFs (poster.avif, poster.small) are their film's static files: strays until its manifest names
      // them, served between the poster and the card, and caught when their bytes are not the manifest's.
      const tissueAvif = writeAsset("tissue-poster-666666666666.avif", "tissue-poster-avif");
      const tissueSmall = writeAsset("tissue-poster-small-777777777777.avif", "tissue-poster-small");
      const fullPoster = { ...tissueAvif, width: 2160, height: 2160, encode: buildPosterAvifArgs([2160, 2160]) };
      const smallPoster = { ...tissueSmall, width: 1080, height: 1080, encode: buildPosterAvifArgs([1080, 1080]) };
      const strays = verifyMediaDirectory([filmManifest, second], [controls4k], temp);
      expect(strays.some((e) => e.includes("tissue-poster-666666666666.avif"))).toBe(true);
      expect(strays.some((e) => e.includes("tissue-poster-small-777777777777.avif"))).toBe(true);
      const withSmall: FilmManifest = { ...second, poster: { ...second.poster, avif: fullPoster, small: smallPoster } };
      expect(verifyMediaDirectory([filmManifest, withSmall], [controls4k], temp)).toEqual([]);
      expect(collectStaticMedia([withSmall], []).map((f) => f.src)).toEqual([
        tissuePoster.src,
        tissueAvif.src,
        tissueSmall.src,
        tissueSocial.src,
      ]);
      const otherSmall: FilmManifest = { ...withSmall, poster: { ...withSmall.poster, small: { ...smallPoster, bytes: smallPoster.bytes + 1 } } };
      const wrong = verifyMediaDirectory([filmManifest, otherSmall], [controls4k], temp);
      expect(wrong.length).toBe(1);
      expect(wrong[0]).toContain("tissue-poster-small-777777777777.avif");

      // Defect 3: unlisted stray file in public/media/ caught.
      writeFileSync(join(mediaDir, "stray-old.mp4"), "junk");
      expect(verifyMediaDirectory([filmManifest], [controls4k], temp).some((e) => e.includes("stray-old.mp4"))).toBe(true);
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  });
});

describe("upload-media's checks", () => {
  test("--site is an http(s) origin, its trailing slash dropped so paths join as /media/…", () => {
    expect(siteOrigin("https://frankenlife.org/")).toBe("https://frankenlife.org");
    expect(siteOrigin("http://localhost:8787")).toBe("http://localhost:8787");
    for (const bad of [undefined, "", "frankenlife.org", "ftp://frankenlife.org"]) expect(siteOrigin(bad)).toBeNull();
  });

  test("R2's ETag is compared with a film's MD5 only when it is one (a single PUT's)", () => {
    const md5 = "0123456789abcdef0123456789abcdef";
    expect(etagAgainstMd5(`"${md5}"`, md5)).toBe("same");
    expect(etagAgainstMd5(`W/"${md5.toUpperCase()}"`, md5)).toBe("same");
    expect(etagAgainstMd5(`"${"f".repeat(32)}"`, md5)).toBe("different");
    // An ETag of a multipart upload, or none, says nothing about the bytes.
    expect(etagAgainstMd5(`"${md5}-12"`, md5)).toBe("unknown");
    expect(etagAgainstMd5(null, md5)).toBe("unknown");
  });
});
