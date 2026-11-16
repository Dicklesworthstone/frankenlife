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
bun test             # worker.ts's byte serving
bun run build        # refuses unless public/media/ holds exactly the manifest's files
bun run deploy       # build, then `wrangler deploy` (needs `wrangler login`)
```

## The film

The video, poster and social card stay out of git, like every FrankenLife recording (AGENTS.md). The
committed `lib/media.json` names each file by its SHA-256 and records how it was made: the world (preset,
configuration hash, seed, the starting tick and digest), every frame's tick and live count, the render
command, flife's revision and binary hash, and the ffmpeg arguments. A fresh checkout renders them again:

```bash
# From the repository root: the Desktop preset run to tick 8,192, kept in RUN (about a quarter
# of an hour at the 9.8–10.2 ticks a second the README measured on 16 workers of an M5 Ultra).
cargo build --release -p flife-cli
target/release/flife run presets/desktop-world.toml --ticks 8192 --run-dir RUN

# From web/: 301 frames from tick 8,192 to 17,792, one every 32 ticks, 1080 × 1080, at CRF 24
# (the defaults). Paths are relative to where you run it; keep WORK (about 700 MB of PNGs) outside the repository.
bun run media --flife ../target/release/flife --run-dir ../RUN --work WORK
```

`scripts/media.ts` refuses a dirty `crates/`, `presets/`, `Cargo.toml` or `Cargo.lock`, since the
manifest names the revision. It renders into `--work`. Run it again with the same `--work` to re-encode
the finished frames without rendering them, for example at another `--crf` when the video exceeds
Cloudflare's 25 MiB limit for a static asset. Each output is copied into `public/media/` under a
content-hashed name. The script never deletes anything, so move the previous render's files out of
`public/media/` before building; `bun run build` names any it finds.

The same configuration, seed and starting checkpoint give the same frames on any machine. The world's
digest at the last frame is in the manifest (`world.to.digest`) and in the render's `end` record.

## Cloudflare

`bun run deploy` uploads `out/` as the Worker's static assets. Static assets answer a Range request with
the whole file and a 200, and Safari plays no video from such a server, so `/media/*` runs `worker.ts`
first: it streams only the bytes asked for as a 206, sized from `lib/media.json` (an asset's response
from the binding has no Content-Length). Every other path is served as a plain static asset.
`public/_headers` makes the media and Next's hashed build output immutable. The site's address is
`siteConfig.url` in `lib/content.ts`; Open Graph and the sitemap use it.
