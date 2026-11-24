# FrankenLife website

The project's page: Next.js 16 as a static export, served by Cloudflare as an assets-only Worker
(`wrangler.jsonc`). It shows films rendered by `flife render` from committed worlds (the two
field-ecology-v2 presets fed more, the supercolony's veins, and the v1 confluent world), with a readout of each
frame's tick and live organisms taken from the render's own records. The Rust workspace does not
build or read this directory (`.rchignore` keeps it out of remote builds too).

Every word and figure outside the explainers is in `lib/content.ts`. Each one is stated by the
repository (README.md, the plan, the crates' CONTRACT.md files, experiments/fl048 and
experiments/showcase, experiments/v2-emergence) or recorded in a manifest (`lib/films/<id>.json`,
`lib/controls.json`, `lib/stills.json`); `branchObservation` is the
one figure the page observed itself, with its date, host and binary. Change the page's claims when the
repository's change.

## The page's design

The page is five chapters, defined by `chapters` in `lib/content.ts`: the world, why it is different, how
it works, what it can show, and how to use it. The header's chapter links (a bottom dock on phones), each
chapter's index and every section's number ("03.2") come from that list, so adding a section there is
enough to number and link it. `components/section.tsx` has the two section layouts: `wide`, where an
explainer gets the full width under its heading, and `split`, a sticky heading beside lists and prose.

The look is a specimen under an instrument, always dark, in the FrankenSuite's green. The film is the
hero's picture, not a decoration. Each chapter wears one colour of the film's lineage palette
(`lib/palette.ts`) as `--chapter`, the way the film colours each founder's descendants. Corner ticks
(`Reticle` in `components/franken.tsx`) frame anything that is a specimen or a figure. Surfaces are
`panel`s; blurred glass is kept for the floating header and dock. Lists are ruled rather than boxed.
`app/globals.css` holds the tokens, the display sizes and the muted text tone, which is raised from
Tailwind's slate-500 to meet WCAG AA contrast on the ink.

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
bun run dev          # http://localhost:3000, once the media are in public/media/
bun run typecheck
bun test             # worker.ts's byte serving and share-image type, the director's camera and card frame,
                     # the controls' manifest against the record, the explainers' arithmetic against the
                     # plan's fixtures, the toy world's determinism
bun run build        # refuses unless public/media/ holds exactly the manifest's files
bun run deploy       # build, then `wrangler deploy` (needs `wrangler login`)
```

## The films

The videos, posters and social cards stay out of git, like every FrankenLife recording (AGENTS.md). Each
film has a committed manifest, `lib/films/<id>.json`, written by `bun run media --film <id>`; it names
each file by its SHA-256 and records how it was made: the world (preset, configuration hash, seed, torus,
the starting tick and digest), every frame's tick and live count, the render command, flife's revision and
binary hash, and the ffmpeg arguments. `lib/films/index.ts` lists the films the page shows, in the hero's
order; the first opens the page. The confluent film keeps the explainers cut from it (the zoom ladder, the
controls); the share card is cut from the tissue film.

The v2 films, all 801 frames at 60 fps, split into segments of at most 22 MiB: the tissue and supercolony
films at 2160 × 2160, the veins film at 1440 × 1440. The tissue's packed frames barely compress: at CRF 20 its
film came to 338 MB, so it is CRF 26, which halves that with every sphere still distinct at full size; the
other two are CRF 20 (153 MB and 79 MB). `scripts/media.ts` encodes a finished `--work` again, without
simulating, when only `--crf` changes.
They show denser worlds built from the committed presets (`presets/morphogenesis-tissue-v2-dense.toml`,
`presets/supercolony-v2-dense.toml`): more food, crowding arrest at 24 neighbours for the tissue, 48
oases with farther-landing children for the supercolony, and for both twice the presets' population ceiling,
524,288, which the supercolony reaches whenever its oases bloom. `experiments/website-films/README.md`
records how they were chosen, and each preset's header names every key it changes. The page says "every
other parameter is the preset's", so `bun run build` runs `scripts/check-film-words.ts` after check-media:
it refuses to build unless the words beside the films name every key the film presets change, with the
presets' own numbers.

```bash
# From the repository root: each world, seed 42, kept in a run directory. The supercolony run is
# made twice so that its two films can render at once (a render locks its run directory); both
# reach the same digest.
target/release/flife run presets/morphogenesis-tissue-v2-dense.toml --ticks 1024 --checkpoint-every 0 --run-dir RUN-A
target/release/flife run presets/supercolony-v2-dense.toml --ticks 768 --checkpoint-every 0 --run-dir RUN-B
target/release/flife run presets/supercolony-v2-dense.toml --ticks 768 --checkpoint-every 0 --run-dir RUN-B2

# From web/: 3,200 ticks, a frame every 4, founder lineage lit by energy, as spheres with no field.
# Pass --cpu-workers: without it flife render uses the configuration's single worker.
bun run media -- --film tissue --flife ../target/release/flife --run-dir ../RUN-A --work WORK-A \
  --ticks 3200 --every 4 --size 2160 --fps 60 --crf 26 --look sphere --color lineage-energy --field none \
  --zooms 1.6,3,2.2 --cpu-workers 12
bun run media -- --film supercolony --flife ../target/release/flife --run-dir ../RUN-B --work WORK-B \
  --ticks 3200 --every 4 --size 2160 --fps 60 --crf 20 --look sphere --color lineage-energy --field none \
  --zooms 1.5,2.2,1.8 --cpu-workers 12
# The supercolony again, glowing over its field coloured by vascular conductance, at 1440 × 1440 and held
# at zoom 1.03: at the ceiling of 524,288 the whole torus needs 2^20 + 1 draw rows (a halo and a dot per
# organism, and the field), one more than flife-manim allows (bead flife-ovd); the zoom culls the rim.
bun run media -- --film veins --flife ../target/release/flife --run-dir ../RUN-B2 --work WORK-VEINS \
  --ticks 3200 --every 4 --size 1440 --fps 60 --crf 20 --look glow --color lineage-energy \
  --field vascular-conductance --camera fixed --zooms 1.03 --cpu-workers 12
```

A 3,200-tick film has room for three shots, one zoom each. Then list each new manifest in
`lib/films/index.ts` and move the files of the films it replaces from `public/media` to
`media-retired/`. Each film starts at a tick chosen by eye once its world fills the frame: tick 1,024 for the
tissue (about 258,000 cells) and 768 for the supercolony (at its ceiling). The looks are the confluent
film's, chosen by eye from 4K test frames. The page says so beside the films.

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

# Turnkey true 60 fps 4K re-render of confluent-world (`bun run media:4k60` sets --ticks 3200
# --every 4 -> 801 distinct frames from tick 2,400 to 5,600 = 13.35 s at true 60 fps, matching
# the 30 fps film's 13.37 s pacing with zero frame duplication, 2160 × 2160, CRF 19,
# --segments auto, --segment-max-mib 22 so every segment stays <= 25 MiB for Cloudflare Workers,
# snapped at camera cuts, plus one unconstrained archival 4K60 master outside public/media/;
# pass --width 3840 --height 2160 to override to 16:9 4K60):
bun run media:4k60 -- --film confluent --flife ../target/release/flife --run-dir ../RUN --work WORK-4K60 \
  --field none --look sphere --color lineage-energy \
  --zooms 1.6,3,2.2,5,2.6 --master-out ../exports/film-4k60-master.mp4 --cpu-workers 24

# Widescreen 16:9 4K60 from a fresh field-ecology-v2 preset with V2-16 overlays and founding toggles:
bun run media:4k60 -- --film colony-wide --flife ../target/release/flife --config ../presets/supercolony-v2.toml --work WORK-V2 \
  --width 3840 --height 2160 --ticks 3200 --every 4 \
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
`--fps` (including `--fps 60`). With `--segments auto` (the default) or `--segments N` (`N >= 2`), any film whose full-CRF
encode exceeds `--segment-max-mib` (default `22` MiB, strictly below Cloudflare's `25 MiB` static asset limit) is split
at natural camera shot boundaries into closed-GOP segments (`<id>-seg00-<hash>.mp4`, …) — recursively subdivided if any
individual shot still exceeds the cap — and played seamlessly through `<Film>`'s dual-`<video>` preloader with a single
unified scrubber, tick readout, and Cloudflare Worker 206 Range serving. Pass `--segments 1` to force a single bitrate-capped
web MP4, and `--master-out PATH` (outside `public/media/`) to also encode one unconstrained archival 4K60 MP4 from the
exact same PNG frames.

`presets/confluent-world.toml` was chosen by outer searches against image statistics of the reference
screenshots and by eye, under the registered contact model, and `../experiments/showcase/README.md` states
how (stages C and E). The site says so beside the film.

The camera (`--camera auto`, the default) is chosen from the run itself: `scripts/media.ts` first renders the
same ticks small, as raw dots, and `scripts/director.ts` reads those frames' pixels to find where the organisms
are changing most, then writes the path that `flife render --camera` follows. The film opens and closes on the
whole torus and, in between, eases into the most active region of each shot. Each shot spans 51–102 world
units on any torus of 128 units or more: the zooms are written for a 256-unit torus and scaled to the
run's, never below 1.25. The rule is stated in
`director.ts` and recorded in the manifest with every key. The camera is presentation only, and the script
checks that the preview and the film end on the same world digest. `--camera none` films the whole torus, and
`--camera fixed --zooms Z` holds the torus centre at zoom Z for the whole film, with no preview.
`--field energy|composition|component-C|trail-carrier|vascular-conductance|none`, `--look glow|raw|sphere`
and `--color lineage|lineage-energy|affinity|energy|generation|motion|topology|cell-state|polarity` choose
how the frames are drawn; `--zooms` lists the shots' zooms on the run's torus instead. The film's shots, at 1.6, 3 and 2.2, show as many
organisms as the reference screenshots' views at zooms 3–6: a screenshot's zoom over 1.95.

The manifest names the commit `--flife` was built from. By default that is HEAD, and `scripts/media.ts`
refuses a dirty `crates/`, `presets/`, `Cargo.toml` or `Cargo.lock`. A binary built earlier is named with
`--revision COMMIT`: an ancestor of HEAD whose `presets/` equal the working tree's, since the configurations
are read from there (`scripts/source-revision.ts`); the working tree's Rust sources may then differ, as
they are not what runs. It renders into `--work`. Run it again with the same `--work` to re-encode the finished
frames without rendering them, for example at another `--crf` when the video exceeds Cloudflare's 25 MiB
limit for a static asset. Each output is copied into `public/media/` under a content-hashed name. The frames
are reproducible, the video's bytes are not: x264 on several threads encodes the same frames differently
each time, so a re-encode gives the video a new name. The script never deletes anything, so move the
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

`bun run deploy` uploads `out/` as the Worker's static assets, served at https://frankenlife.org.
Static assets answer a Range request with the whole file and a 200, and Safari plays no video from such
a server, so `worker.ts` runs first, and for `/media/*` it streams only the bytes asked for as a 206, sized
from the manifests, `lib/films/*.json`, `lib/controls.json` and `lib/stills.json` (an asset's response from the binding has
no Content-Length). The static export
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
