# FrankenLife website

The project's page: Next.js 16 as a static export, served by Cloudflare as Workers static assets behind a
small Worker that streams the films from R2 (`wrangler.jsonc`, `worker.ts`). It shows films rendered by `flife render` from committed worlds (the v1 confluent
world, and the same world as field-ecology-v2 with one mechanism on: veins, then polarity), with a readout of each
frame's tick and live organisms taken from the render's own records. The Rust workspace does not
build or read this directory (`.rchignore` keeps it out of remote builds too).

Every word and figure outside the explainers is in `lib/content.ts`. Each one is stated by the
repository (README.md, the plan, the crates' CONTRACT.md files, experiments/fl048 and
experiments/showcase, experiments/v2-emergence) or recorded in a manifest (`lib/films/<id>.json`,
`lib/controls.json`, `lib/stills.json`); `branchObservation` is the
one figure the page observed itself, with its date, host and binary. Change the page's claims when the
repository's change.

## The page's design

The page is six chapters, defined by `chapters` in `lib/content.ts`: the world, why it is different, how
it works, version 2, what it can show, and how to use it. The header's chapter links (a bottom dock below
1,024 px, where the six do not fit beside the name), each
chapter's index and every section's number ("03.2") come from that list, so adding a section there is
enough to number and link it. `components/section.tsx` has the two section layouts: `wide`, where an
explainer gets the full width under its heading, and `split`, a sticky heading beside lists and prose.

The look is a specimen under an instrument, always dark, in the FrankenSuite's green. The film is the
hero's picture, not a decoration. Each chapter wears one colour of the film's lineage palette
(`lib/palette.ts`) as `--chapter`, the way the film colours each founder's descendants. Corner ticks
(`Reticle` in `components/franken.tsx`) frame anything that is a specimen or a figure. Surfaces are
`panel`s; blurred glass is kept for the floating header and dock. Lists are ruled rather than boxed.
`app/globals.css` holds the tokens, the display sizes and the muted text tone, which is raised from
Tailwind's slate-500 to meet WCAG AA contrast (4.5:1) on every panel, not only the ink; SVG labels take
the same grey as `TONE.muted`, and the film's violet takes `VIOLET_TEXT` as words (`lib/palette.ts`).
Something dimmed to show what is not chosen keeps its words at 4.5:1: an arrow can fade, its label only
to 0.7, and a key outside the chosen group loses its colour, not its legibility. A box that scrolls
sideways with nothing focusable in it takes `tabIndex={0}`, so a keyboard can scroll it, and a control's
accessible name contains the words it shows, so a voice command finds it. axe-core 4.14 found no
violations on 2026-11-25 at 320, 390 and 1,440 px, in every tab, view and opened `<details>`; measured
from pixels (axe leaves text over gradients and other text undecided), every line of text met AA at 390
and 1,440 px in 81 states, apart from disabled controls and the keyboard picture's dead keys. Every
control is at least 24 px square on a phone (WCAG 2.5.8; axe ships the rule off, so run it by name):
below 640 px the Studio's keyboard splits in two halves, which is what keeps its keys wide enough.
With WCAG 1.4.12's text spacing forced on (letters 0.12em, words 0.16em, lines 1.5), no text overlaps
or is cut off at 390, 768 or 1,440 px, so a label wraps rather than truncating with an ellipsis, and a
heading's long word may break; at 320 px only the life cycle's "Sense" and "Divide" reach past the ring's
drawing, by 5 and 7 px. Anything that moves by itself for more than five seconds has a control that
stops it (the films, the life cycle, the brain's flowing connections), and stands still under reduced
motion. Whatever pointing shows, the keyboard reaches too (the weight grid steps by arrow keys), and
nothing is said only in a hover title. The focus ring is one unlayered rule in `app/globals.css`, so no
utility removes it (forced colours keep outlines and drop box-shadow rings); a control that fills an
overflow-hidden frame takes `focus-inset`, which draws it inside. Tabbing through every stop at 390 and
1,440 px (239 and 265) and shooting each focused and unfocused, every one changes visibly.

## The explainers

`components/explain/` shows what the project does rather than only saying it. Each figure wears a badge
(`DemoFrame`'s `basis`) that says what it is, so none is mistaken for a run:

| Badge | Meaning | Figures |
|---|---|---|
| The rule, computed here | The model's rule recomputed in the browser on hand-sized numbers, beside its plausible wrong version; it starts at an Appendix C fixture's numbers and says when the answer matches the plan's; also the deterministic `field-ecology-v2` ABI & CLI toggle calculator | `fixture-bench.tsx` (C.1, C.3, C.2, C.9, C.11, C.15, C.5), `v2-explorer.tsx` |
| Derived · arithmetic, not a benchmark | Arithmetic from stated assumptions | `scale.tsx` (README "Why the scale is hard") |
| From the record | Figures an experiment or contract records | inside `weights.tsx` and `scale.tsx` |
| Diagram · not simulation output | How the software is organised | `life-cycle.tsx`, `brain.tsx`, `weights.tsx` (seeded illustrative weights), `tick-pipeline.tsx`, `branches.tsx` (toy digests), `studio-keys.tsx`, the zoom ladder's last two rungs |
| Rendered by flife · not drawn | Pixels `flife render` drew from a committed world; a manifest says how | the film, the zoom ladder's first three rungs (`zoom-ladder.tsx`, the film's poster and social card), `controls-trio.tsx` |
| A toy in your browser · not FrankenLife | A small simulation written for the page, with four of the model's kinds of rule at toy scale; not FrankenLife's code, and nothing it shows is a result | `miniature.tsx` |

Each figure's caption names the plan section, the crate file that implements the rule and, for the
fixtures, the Rust test that checks the same answer. The pure arithmetic is in `fixture-math.ts`,
`scale-math.ts`, `brain-math.ts`, `tick-machine.ts`, `branch-model.ts`, `zoom-ladder-geometry.ts` and
`miniature-engine.ts`, and `bun test` checks it against the plan's stated answers (typed in separately,
never computed by the code under test). The toy world uses only `+ − × ÷` and rounding down, which every
browser computes to the same bit, so a seed replays to the same digest anywhere; its uptake is the same
largest-remainder rule as fixture C.1, and a test holds the two to each other. The
Studio's keys come from `crates/flife-studio/src/controls.rs`; change `studio-keymap.ts` when they do.
Animations stop off screen and never run for a reader who asks for less motion.

Bun only:

```bash
cd web
bun install
bun run dev          # http://localhost:3000 with the posters and stills in public/media/; no film plays (they are in R2)
bun run preview      # build, then the Worker locally (wrangler dev), with the films from the real R2 bucket
bun run typecheck
bun test             # worker.ts's byte serving and share-image type, the director's camera and card frame,
                     # the controls' manifest against the record, the explainers' arithmetic against the
                     # plan's fixtures, the toy world's determinism
bun run build        # refuses unless public/media/ holds exactly the manifest's files
bun run upload-media # put each film's MP4 from media-r2/ into the R2 bucket frankenlife-media
bun run deploy       # build, upload-media, `wrangler deploy`, then check the live films' bytes (needs `wrangler login`)
```

## The films

The videos, posters and social cards stay out of git, like every FrankenLife recording (AGENTS.md). Each
film has a committed manifest, `lib/films/<id>.json`, written by `bun run media --film <id>`; it names
each file by its SHA-256 and records how it was made: the world (preset, configuration hash, seed, torus,
the starting tick and digest), every frame's tick and live count, the render command, flife's revision and
binary hash, and the ffmpeg arguments. `lib/films/index.ts` lists the films the page shows, in the hero's
order; the first opens the page and the share card is cut from it. The page shows three films: the
confluent world, which opens it and which the explainers are also cut from (the zoom ladder, the controls),
and the same world with one v2 mechanism on, `confluent-veins` (vascular-field) and `confluent-polarity`
(pcp-polarity). A world without a preset names its committed configuration in `lib/content.ts`
(`filmNotes`), and `lib/film-configs.test.ts` checks that configuration against the manifest. To add a film,
list its manifest in `lib/films/index.ts` and move the files of any film it replaces from `public/media` to
`media-retired/`.

Until a film's first frame, the page shows its poster as an AVIF: 1080² on a phone, which plays the 1080²
encode (`poster.small`, about 250 KB), and the film's own size elsewhere (`poster.avif`, about 650 KB),
against the 2160² JPEG's 3 MB, which stays for a browser without AVIF and for the figures that zoom into it
(`buildPosterAvifArgs` in `scripts/media-options.ts`, which `scripts/media.ts` runs). Under Lighthouse's
mobile throttling the JPEG took more than 16 s on a phone, and the page's largest paint waited 10 s for the
video. A picture under the video chooses the poster, since a video's own poster attribute names one file for
every screen, and the video waits for it (`preload="none"` until then): streaming at once, it slowed the
poster by about 1.3 s. Nothing below the hero loads with the page (checked at 390, 768, 1,024, 1,440 and
1,920 px). `loading="lazy"` was not enough for that: Chrome fetches a lazy image well before it is on screen,
and on desktops the zoom ladder's 3 MB JPEG and its 444 KB card came with the page, so the ladder shows the
hero's AVIF until it is on screen and loads them then. An image that is also a CSS background (the controls'
loupe) waits for its lazy `<img>`. Measured on frankenlife.org on 2026-11-26, cold, three or four runs
each: the largest paint (the poster) now comes at 3.8 s on a 390 px phone under that throttling (10.1 s
before), and at 1.1–1.2 s on a 1,024 or 1,440 px desktop over 10 Mbps with 40 ms round trips (3.0–3.1 s
at 1,440 before; 1.5–1.8 s with the AVIF poster while the ladder's JPEG still came with the page).
Safari's and Firefox's engines behave the same, checked on the live site in headless WebKit (Playwright
1.65.1's build 2388) and Firefox 159 (its build 1572) at 390 and 1,440 px:
- with the MP4 held back, the poster shows under the video, which is transparent until its first frame;
- the film plays once its data comes;
- a pause pressed before the poster arrives stays;
- under reduced motion it loads its first frame without playing;
- the ladder swaps to its JPEG once it is on screen.

Playwright's Firefox has no phone emulation, so there 390 px is only a narrow window; the phone poster and
encode were still the ones chosen, since the page picks them by the viewport's size. Firefox did not start
the ladder's lazy image before it neared the screen.

Three v2 films of denser worlds (`presets/morphogenesis-tissue-v2-dense.toml`,
`presets/supercolony-v2-dense.toml`, rendered 2026-11-24) were taken down the same day: in motion they
shimmer and are hard to read, nothing like the confluent film. `experiments/website-films/README.md`
records them. Before rendering a film, render a few dozen consecutive frames small and look at them in
motion beside the confluent film's; run one render at a time, since three 2160 × 2160 renders of 524,288
organisms at once exhausted the machine's memory.

The confluent film (`lib/films/confluent.json`) was made this way:

```bash
# From the repository root: the confluent preset run to tick 2,400, where its trial was scored,
# kept in RUN (about 20 minutes on 14 workers of an M5 Ultra).
cargo build --release -p flife-cli
target/release/flife run presets/confluent-world.toml --ticks 2400 --cpu-workers 14 --run-dir RUN

# From web/: 401 frames from tick 2,400 to 5,600, one every 8 ticks, 1080 × 1080, each organism drawn as a
# lit sphere at its body's size and coloured by founder lineage, without the field. Paths are relative
# to where you run it; keep WORK (several hundred MB of PNGs) outside the repository.
bun run media --film confluent --flife ../target/release/flife --run-dir ../RUN --work WORK \
  --ticks 3200 --every 8 --field none --look sphere --color lineage --zooms 1.6,3,2.2,5,2.6 --cpu-workers 24

# 4K at 60 fps (`bun run media:4k60` sets --ticks 3200 --every 2: 1,601 frames from tick 2,400 to
# 5,600, 26.7 s, 2160 × 2160, CRF 30, one MP4 per film into media-r2/ for R2, and --small 1080
# --small-crf 26, the same frames at 1080 × 1080 for phones; later flags win, so --width 3840
# --height 2160 gives 16:9, with a 1920 × 1080 phone encode). --master-out adds an archival encode at CRF 16:
bun run media:4k60 -- --film confluent --flife ../target/release/flife --run-dir ../RUN --work WORK-4K60 \
  --field none --look sphere --color lineage-energy \
  --zooms 1.6,3,2.2,5,2.6 --master-out ../exports/film-4k60-master.mp4 --cpu-workers 24

# The two v2 films: the confluent world as field-ecology-v2 with one mechanism on, at the engine's
# defaults, from the configurations committed in experiments/website-films/ (its README, round 3,
# says how they were chosen). From the repository root, run each to tick 2,400 (10 to 20 minutes;
# a run or render of this world peaks near 19 GiB, so run one at a time):
target/release/flife run experiments/website-films/confluent-veins.toml --ticks 2400 --cpu-workers 14 --run-dir RUN-VEINS
# Then from web/, the confluent film's recipe. Each was first rendered whole at 1080 and looked at
# shot by shot; the 4K pass took that pass's preview with --preview-from, which gives the same camera:
bun run media:4k60 -- --film confluent-veins --flife ../target/release/flife --run-dir ../RUN-VEINS --work WORK-VEINS \
  --field none --look sphere --color lineage-energy \
  --zooms 1.6,3,2.2,5,2.6 --master-out ../exports/confluent-veins-4k60-master.mp4 --cpu-workers 14
# confluent-polarity: the same, from experiments/website-films/confluent-polarity.toml.

# Widescreen 16:9 4K60 from a fresh field-ecology-v2 preset with V2-16 overlays and founding toggles:
bun run media:4k60 -- --film colony-wide --flife ../target/release/flife --config ../presets/supercolony-v2.toml --work WORK-V2 \
  --width 3840 --height 2160 \
  --field vascular-conductance --look glow --color lineage \
  --disable oases --master-out ../exports/supercolony-4k60-master.mp4
```

Without `--cpu-workers`, both renders run on the preset's `[execution] cpu_workers`, one thread for the
showcase presets. No result depends on the count.

`scripts/media.ts` accepts either `--run-dir DIR` (resuming from its newest checkpoint, matching `flife render --run-dir`)
or `--config PATH` (founding a fresh configuration). Founding-only `field-ecology-v2` toggles (`--disable`, `--enable`,
`--v1-compat`, validated against the 21 canonical slugs plus `all-v2`, `bundle-a`, `bundle-b`) are passed in exact CLI
order with `--config` and refused with `--run-dir` per `INV-14`. Both `--size S` (square) and independent `--width W
--height H` (e.g. `2160 × 2160` or `3840 × 2160`, even integers for 4:2:0) are supported alongside arbitrary positive
`--fps` (including `--fps 60`). Each film is one MP4, `<id>-<hash>.mp4`, encoded at its CRF alone with a
keyframe every second so a seek lands quickly, written to `media-r2/` (outside git) and served from R2 by
`worker.ts`; Cloudflare's 25 MiB limit applies only to static assets, which the films no longer are.
`--small HEIGHT` also encodes the same frames scaled to that height, `<id>-small-<hash>.mp4`, at
`--small-crf` (default `--crf`): the player's `<source media>` gives it to screens at most 640 CSS pixels
wide or tall (`components/film.tsx`, `PHONE`). That is phones either way up, since the 4K film's H.264
level 5.2 is more than some phones decode (the 1080 encode is level 4.2) and an upright phone would
scale most of its pixels away, and short windows, where the hero film is drawn small. At 1080, CRF 30
smeared single bodies that the 4K encode keeps when scaled down; CRF 26 matched it, at about 56% of the
4K film's bytes. `--master-out PATH` (outside `public/media/`) also encodes one archival MP4 from the
same PNG frames.

`presets/confluent-world.toml` was chosen by outer searches and by eye, under the registered contact model;
`../experiments/showcase/README.md` states how (stages C and E).

The camera (`--camera auto`, the default) is chosen from the run itself: `scripts/media.ts` first renders the
same ticks small, as raw dots, and `scripts/director.ts` reads those frames' pixels to find where the organisms
are changing most, then writes the path that `flife render --camera` follows. The film opens on the whole
torus for 35% of its length, so the world can be seen evolving before the first zoom, and closes on it for
at least 10%; in between it eases into the most active region of each shot. Each shot spans 51–102 world
units on any torus of 128 units or more: the zooms are written for a 256-unit torus and scaled to the
run's, never below 1.25. The rule is stated in
`director.ts` and recorded in the manifest with every key. The camera is presentation only, and the script
checks that the preview and the film end on the same world digest. `--camera none` films the whole torus, and
`--camera fixed --zooms Z` holds the torus centre at zoom Z for the whole film, with no preview.
`--field energy|composition|component-C|trail-carrier|vascular-conductance|none`, `--look glow|raw|sphere`
and `--color lineage|lineage-energy|affinity|energy|generation|motion|topology|cell-state|polarity` choose
how the frames are drawn; `--zooms` lists the shots' zooms on the run's torus instead.

The manifest names the commit `--flife` was built from. By default that is HEAD, and `scripts/media.ts`
refuses a dirty `crates/`, `presets/`, `Cargo.toml` or `Cargo.lock`. A binary built earlier is named with
`--revision COMMIT`: an ancestor of HEAD whose `presets/` equal the working tree's, since the configurations
are read from there (`scripts/source-revision.ts`); the working tree's Rust sources may then differ, as
they are not what runs. It renders into `--work`. Run it again with the same `--work` to re-encode the finished
frames without rendering them, for example at another `--crf`. The poster, card and stills are copied into
`public/media/` and the video into `media-r2/`, each under a content-hashed name. The frames
are reproducible on any machine; the video's bytes only for the same frames, arguments, ffmpeg build and
thread count (x264 is deterministic unless told otherwise, but its thread count, by default set from the
machine's cores, shapes the stream), so re-encoding on this machine gives the same names, and another machine
may give new ones. The script never deletes anything, so move the
previous render's files out of `public/media/` before building; `bun run build` names any it finds.

The same configuration, seed and starting checkpoint give the same frames on any machine. The world's
digest at the last frame is in the manifest (`world.to.digest`) and in the render's `end` record.

## The share images

`app/opengraph-image.tsx` (1200 × 630) and `app/twitter-image.tsx` (1200 × 600) are drawn at build time by
`lib/og-card.tsx` with `next/og`. Their picture is the social card: a 1200 × 630 crop of the middle frame of
the film's closest shot (`cardFrame` in `scripts/director.ts`; the manifest's `social.frame`), darkened
beside the words. The organism count and the caption's world and tick come from the manifest. The build
fetches the card's fonts (Inter, JetBrains Mono) from Google Fonts, as `next/font` does, and encodes the
card as a JPEG (about 180 KB for this film) with ffmpeg, since a PNG of the frame is over 2 MB, more than
some link previewers fetch. Next.js links both from the page's metadata with a hash, so `app/layout.tsx`
names no image itself. The hash is of the route's source, not the picture, so a new film keeps the same
URL: a platform that cached the old card shows it until its own refresh tool fetches it again.

## Cloudflare

`bun run deploy` uploads `out/` as the Worker's static assets, served at https://frankenlife.org, and each
film's MP4 and its phone encode to the R2 bucket `frankenlife-media` (binding `MEDIA`, keys
`media/<id>-<hash>.mp4` and `media/<id>-small-<hash>.mp4`), skipping a video the site already serves. `worker.ts` serves `/media/*.mp4` from R2: the whole file, or for a Range
request only the bytes asked for as a 206 (Safari plays no video without it), sized from the manifests; a
name R2 does not hold gets the site's 404 page, uncached, and a film whose size in R2 differs from its
manifest's, or an R2 failure, a 503. `bun run upload-media --check` then checks each live film: whole (its
size, and R2's ETag against the MD5 of the copy in `media-r2/`) and its first and last bytes, asking again
for a minute if an edge is slow to answer. Uploads go through `wrangler r2 object put`, one PUT of at most
300 MiB per film; the build refuses a larger film (`scripts/media-verify.ts`). The static export
writes the share images without an extension, so the Worker gives them their type, `image/jpeg`
(`lib/share.ts`). It also sends www.frankenlife.org to frankenlife.org; every other request goes to the
static assets unchanged. Both names are custom domains on the Worker
(`wrangler.jsonc`), so `wrangler deploy` attaches them and their certificates.
`public/_headers` makes the media and Next's hashed build output immutable. The site's address is
`siteConfig.url` in `lib/content.ts`; Open Graph and the sitemap use it.

## The controls

`components/explain/controls-trio.tsx` shows the film's world beside its model's two controls (plan §5.4)
at tick 2,400, the tick the showcase scored: `field-ecology-v1-contact-chemistry-only`, whose networks run
and are charged but whose actions decode from an all-zero output, and
`field-ecology-v1-contact-fresh-offspring`, whose newborns get a founder's genome instead of their
parent's. Each is `presets/confluent-world.toml` with only its `model` (and `name`) changed, run from
tick 0 inside `flife render` and drawn as the film is: at the film's square size (1080 × 1080, or
2160 × 2160 with `--size 2160` / `bun run controls:4k`), the whole torus, no field, the sphere
look, lineage lit by energy. `lib/controls.json` is their committed manifest; the stills live in
`public/media/` beside the film, outside git, and `bun run build` checks them the same way.

```bash
# From web/; WORK outside the repository. About 13 minutes for each full world on 8 workers of an
# M5 Ultra (two at once), under a minute for the inheritance control.
bun scripts/controls.ts --flife ../target/release/flife --work WORK --cpu-workers 8

# Matching 4K (2160 × 2160) stills beside a 2160 × 2160 `media:4k60` film (reuses the 4K poster byte for byte):
bun run controls:4k -- --flife ../target/release/flife --work WORK-4K --cpu-workers 8
```

The script refuses unless each world's digest and live count at tick 2,400 equal the ones
`experiments/showcase/confluent.json` records for the trial it repeats (`w80-o25`, `cw-chem`,
`cw-fresh`), and the full world's equals the film's starting digest: each still is the recorded run,
drawn. The film's world, drawn and encoded as the poster was, is the poster byte for byte, so the
manifest names the poster rather than a copy. The record's distances and the README's words for each
frame ("### Controls" in `experiments/showcase/README.md`) are copied with their source.
`scripts/controls.test.ts` checks the manifest against the record.

## The v2 stills

The Version 2 chapter shows ten stills, all 2160 × 2160, drawn at tick 2,048, the tick the V2-17 Stage-2
campaign scored (`experiments/v2-emergence/README.md`). `scripts/stills.ts` renders them and writes their
committed manifest, `lib/stills.json`:

- `v2-controls`: each v2 preset as committed beside two controls, the same preset with every v2 mechanism
  off (`--v1-compat`) and with inheritance removed (model `field-ecology-v2-fresh-offspring`), the whole
  torus, glow look, founder lineage lit by energy over the field's energy.
- `v2-views`: the four read-only v2 views, polarity and cell state of the tissue (zoomed 3× on islands
  chosen by eye from the whole-torus still; the script records each centre), trail carrier and vascular
  conductance of the supercolony (the whole torus).

```bash
# From the repository root; WORK outside the repository. About a minute a world on 12 workers.
bun web/scripts/stills.ts --flife target/release/flife --work WORK --cpu-workers 12 \
  --stage2 PATH/stage2_summary_FINAL_104.json
```

Each world runs from tick 0 inside `flife render`. With `--stage2` (the campaign's summary, kept outside
git), the script refuses unless each world's digest and live count equal those the summary records for
the same preset, condition and seed 42, and the manifest says so. `--only ID,ID` renders some stills
again and keeps the others' entries; `--revision` is as for `scripts/media.ts`.
