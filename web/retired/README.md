# Retired

Code the site no longer uses, kept rather than deleted (AGENTS.md, rule 1). Nothing here is built, typechecked
or tested (tsconfig.json excludes this folder).

- `film-segments.ts`: the timeline that stitched a film from several MP4 segments of at most 25 MiB, Cloudflare's
  static-asset limit, with two stacked `<video>` elements. Retired on 2026-11-25, when each film became one MP4
  served from R2 (worker.ts). Its type `VideoSegment` was removed from lib/media.ts at the same time.
