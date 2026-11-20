# FrankenLife website

The project's page: Next.js 16 as a static export, served by Cloudflare as an assets-only Worker
(`wrangler.jsonc`). It shows a film rendered by `flife render` from a committed world, with a readout of
each frame's tick and live organisms taken from the render's own records. The Rust workspace does not
build or read this directory (`.rchignore` keeps it out of remote builds too).

Every word and figure is in `lib/content.ts`. Each one is stated by the repository (README.md, the
crates' CONTRACT.md files, experiments/fl048) or recorded in the film's manifest (`lib/media.json`).
Change the page's claims when the repository's change.

Bun only:

```bash
cd web
bun install
bun run dev          # http://localhost:3000, once the media are in public/media/
bun run typecheck
bun test             # worker.ts's byte serving, and the director's camera path and card frame
bun run build        # refuses unless public/media/ holds exactly the manifest's files
bun run deploy       # build, then `wrangler deploy` (needs `wrangler login`)
```

## The film

The video, poster and social card stay out of git, like every FrankenLife recording (AGENTS.md). The
committed `lib/media.json` names each file by its SHA-256 and records how it was made: the world (preset,
configuration hash, seed, the starting tick and digest), every frame's tick and live count, the render
command, flife's revision and binary hash, and the ffmpeg arguments. A fresh checkout renders them again:

```bash
# From the repository root: the confluent preset run to tick 2,400, where its trial was scored,
# kept in RUN (about 20 minutes on 14 workers of an M5 Ultra).
cargo build --release -p flife-cli
target/release/flife run presets/confluent-world.toml --ticks 2400 --cpu-workers 14 --run-dir RUN

# From web/: 401 frames from tick 2,400 to 5,600, one every 8 ticks, 1080 × 1080, each organism drawn as a
# lit sphere at its body's size and coloured by founder lineage, without the field. Paths are relative
# to where you run it; keep WORK (several hundred MB of PNGs) outside the repository.
bun run media --flife ../target/release/flife --run-dir ../RUN --work WORK \
  --ticks 3200 --every 8 --field none --look sphere --color lineage --zooms 1.6,3,2.2,5,2.6 --cpu-workers 24
```

Without `--cpu-workers`, both renders run on the preset's `[execution] cpu_workers`, one thread for the
showcase presets. No result depends on the count.

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
checks that the preview and the film end on the same world digest. `--camera none` films the whole torus.
`--field energy|composition|none` and `--look glow|raw|sphere` choose how the frames are drawn;
`--zooms` lists the shots' zooms on the run's torus instead. The film's shots, at 1.6, 3 and 2.2, show as many
organisms as the reference screenshots' views at zooms 3–6: a screenshot's zoom over 1.95.

`scripts/media.ts` refuses a dirty `crates/`, `presets/`, `Cargo.toml` or `Cargo.lock`, since the
manifest names the revision. It renders into `--work`. Run it again with the same `--work` to re-encode
the finished frames without rendering them, for example at another `--crf` when the video exceeds
Cloudflare's 25 MiB limit for a static asset. Each output is copied into `public/media/` under a
content-hashed name. The script never deletes anything, so move the previous render's files out of
`public/media/` before building; `bun run build` names any it finds.

The same configuration, seed and starting checkpoint give the same frames on any machine. The world's
digest at the last frame is in the manifest (`world.to.digest`) and in the render's `end` record.

## The share images

`app/opengraph-image.tsx` (1200 × 630) and `app/twitter-image.tsx` (1200 × 600) are drawn at build time
by `lib/og-card.tsx` with `next/og`. Their picture is the social card: a 1200 × 630 crop of the middle
frame of the film's closest shot (`cardFrame` in `scripts/director.ts`; the manifest's `social.frame`),
darkened beside the words. The organism count and the caption's world and tick come from the manifest.
The build fetches the card's fonts (Inter, JetBrains Mono) from Google Fonts, as `next/font` does, and
encodes the card as a JPEG of about 250 KB with ffmpeg, since a PNG of the frame is over 2 MB, more than
some link previewers fetch. Next.js links both from the page's metadata with a content hash, so
`app/layout.tsx` names no image itself.

## Cloudflare

`bun run deploy` uploads `out/` as the Worker's static assets, served at https://frankenlife.org.
Static assets answer a Range request with the whole file and a 200, and Safari plays no video from such
a server, so `worker.ts` runs first, and for `/media/*` it streams only the bytes asked for as a 206, sized
from `lib/media.json` (an asset's response from the binding has no Content-Length). The static export
writes the share images without an extension, so the Worker gives them their type, `image/jpeg`
(`lib/share.ts`). It also sends www.frankenlife.org to frankenlife.org; every other request goes to the
static assets unchanged. Both names are custom domains on the Worker
(`wrangler.jsonc`), so `wrangler deploy` attaches them and their certificates.
`public/_headers` makes the media and Next's hashed build output immutable. The site's address is
`siteConfig.url` in `lib/content.ts`; Open Graph and the sitemap use it.
