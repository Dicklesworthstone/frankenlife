# Website film worlds: how the v2 film presets were chosen

**Outcome (2026-11-24): the films were taken off the site the day they went up.** In motion the dense
worlds shimmer and are hard to read, nothing like the confluent film, though their single frames
looked full. Every choice below was made from stills 256 ticks apart, so none of them judged motion.
The page then showed the confluent film alone, and the v2 panels lead with the committed presets' own
stills; rounds 3 to 5 below made the two v2 films it now shows beside it. The films' manifests and media are in `web/media-retired/rejected-dense-v2-2026-11-24/`
(not in git; commit 4d76075 has the manifests). A new film should start from a few dozen consecutive
frames rendered small and looked at in motion beside the confluent film's.

The website's tissue, supercolony and veins films showed
`presets/morphogenesis-tissue-v2-dense.toml` and
`presets/supercolony-v2-dense.toml`, chosen in a second round (below) from the
first round's `presets/morphogenesis-tissue-v2-abundant.toml` and
`presets/supercolony-v2-abundant.toml`, which the films of commit ca706d2
showed. The abundant presets are committed v2 presets with only their
`[sources]` changed; the dense presets change a few more keys, each named in
their headers and below. The committed presets stay the evidence. Every
Stage-2 figure on the site (`experiments/v2-emergence/README.md`) is theirs,
and no claim covers the abundant or dense presets.

Each choice was made by eye, outside the world, for a fuller frame. Nothing
chosen feeds back into a run.

# Round 1: abundant (more food)

Each variant below is its base preset (seed 42)
with the `[sources]` keys shown, rendered from tick 0 by `flife render CONFIG
--every 256 --width 1080 --height 1080 --look sphere --color lineage-energy
--no-field` on 2026-11-24, by flife built at 262f067 (the first runs) or at
bbb5773. The two builds agree: the 16 × 48,000, sigma 12 variant reached the
same tick-1,536 digest, 7b72d200…149913, on both. "Alive" is the live count at the
last frame, "peak" the most at any frame.

## Tissue, round 1 (base `morphogenesis-tissue-v2`: 16 sources of power 4,000, sigma 4.2)

| sources | power each | sigma | total power | alive at tick | peak |
|---:|---:|---:|---:|---|---:|
| 16 | 48,000 | 4.2 | 768,000 | 18,541 at 1,536 | 18,541 |
| 16 | 48,000 | 12 | 768,000 | 36 at 1,536 | 10,954 |
| 16 | 192,000 | 4.2 | 3,072,000 | 18,616 at 2,048 | 18,616 |
| 32 | 64,000 | 4.2 | 2,048,000 | 9,454 at 2,048 | 13,347 |
| 48 | 32,000 | 4.2 | 1,536,000 | 3,836 at 2,048 | 11,778 |
| 64 | 32,000 | 4.2 | 2,048,000 | 2,236 at 2,048 | 10,954 |
| 8 | 96,000 | 4.2 | 768,000 | 27,863 at 2,048 | 28,053 |
| 8 | 96,000 | 6 | 768,000 | 36,182 at 2,048 | 36,182 |
| 4 | 192,000 | 8 | 768,000 | 45,861 at 2,048 | 46,968 |
| 4 | 384,000 | 8 | 1,536,000 | 48,262 at 2,048 | 48,262 |
| 4 | 256,000 | 10 | 1,024,000 | 52,302 at 2,048 | 52,302 |
| 3 | 256,000 | 10 | 768,000 | 44,605 at 2,048 | 44,605 |
| **6** | **192,000** | **8** | **1,152,000** | **51,310 at 2,048** | **51,310** |

The committed preset holds 15,446 on average over Stage-2's four seeds at
tick 2,048. Food alone barely moves it: crowding arrest caps how densely the
tissue packs, and its picky chemistry (discrimination threshold 1.16, toxicity
0.28) makes food where several sources overlap mildly toxic, so more sources,
or wider ones, thin it out. A few wide, rich sources let it spread. Six covered
the most of the torus by eye, at about the most cells.

## Supercolony, round 1 (base `supercolony-v2`: 16 oases of power 3,200)

| power each | total power | alive at tick | peak |
|---:|---:|---|---:|
| 9,600 | 153,600 | 55,021 at 2,048 | 136,541 |
| **16,000** | **256,000** | **119,984 at 2,048** | **245,116** |
| 38,400 | 614,400 | 258,891 at 512 (stopped: at the ceiling) | 258,891 |

The committed preset holds 21,308 on average at tick 2,048. The oases set
their own widths (`[v2.oases]` sigma 2.6 to 4.8), so `[sources]
sigma_world_units` does not change this world (a run with sigma 8 matched the
38,400 run exactly). Five times the food boomed to 245,116 near tick 768,
below the ceiling of 262,144, then held between about 100,000 and 150,000.
Twelve times reached the ceiling by tick 512, where births are refused, which
is externally imposed selection (AGENTS.md), so it was stopped.

# Round 2: dense (more organisms on the frame)

The abundant films still left most of each frame black: about 33,000 to
50,000 tissue cells and 64,000 to 181,000 supercolony organisms, against the
confluent film's 262,144 bodies tiling its torus. Each variant below is an
abundant preset with the keys shown changed, rendered as in round 1 (2,048
ticks or until stopped, 1080², a frame every 256 ticks) on 2026-11-24 by flife
built at bbb5773. The variants with a ceiling of 1,048,576, all but the
160 × 160 supercolony, also raise `[execution]` maximum_logical_genome_bytes to
64 GiB, maximum_allocated_genome_bytes to 72 GiB and maximum_total_bytes to 128
GiB, so that the 16 GiB genome quota (about 830,000 organisms of today's genome
size) could not bind before the ceiling; the most any reached was 761,630. "Stopped" runs were
ended once their frames had answered the question.

## Tissue, round 2 (base `morphogenesis-tissue-v2-abundant`: gate 3.25, 6 × 192,000)

Crowding arrest stops division at 3.25 neighbours within 0.375 world units,
which caps the tissue near 7 cells per square world unit, about 47,000 on the
80 × 80 torus: more food cannot add cells. Two ways round it were tried.

| change | ceiling | alive at tick | note |
|---|---:|---|---|
| torus 160 × 160 (grid 640), 24 sources of 192,000, 43,816 founders | 524,288 | 94,303 at 1,024 (stopped) | at full view each cell is a speck |
| torus 160 × 160, 16 sources of 256,000, sigma 10, 43,816 founders | 524,288 | 91,420 at 1,024 (stopped) | the same |
| gate 12, sources of 768,000 | 1,048,576 | 152,933 at 1,280 (stopped) | packed, gaps between sources |
| **gate 24, sources of 1,536,000** | 1,048,576 | **336,582 at 1,536** (stopped) | **chosen** |

The chosen variant is `morphogenesis-tissue-v2-dense` with the ceiling at
524,288, which it never reaches, so its live counts equal the variant's
(61,582 at tick 512, 258,101 at 1,024). The committed presets' ceiling of
262,144 would bind: in the film the tissue passes it at tick 1,044 and stays
above it in 595 of 801 frames, at most 340,097. It packs into lineage patches
over most of the torus, the way the confluent world does.

## Supercolony, round 2 (base `supercolony-v2-abundant`: 16 oases of 16,000)

The supercolony's organisms are points: no force pushes them apart, so more
food makes taller piles on the richest oases rather than a fuller torus.

| change | ceiling | alive at tick | note |
|---|---:|---|---|
| oases sigma 10 to 16, power 38,400, child offset 2.5 | 524,288 | 56,609 at 1,792 (stopped) | crashed to 1,768 at tick 256, regrew as one band |
| the same, with source path speed 6.41 | 524,288 | 0 from tick 512 | extinct |
| torus 160 × 160 (grid 640), 64 oases of 38,400, sigma 10 to 16, child offset 2.5, 65,536 founders | 1,048,576 | 92,806 at 768 (stopped) | specks at full view |
| power 38,400 | 1,048,576 | 258,891 at 512 (stopped) | round 1's last run, past its old ceiling |
| power 76,800 | 1,048,576 | 399,605 at 512 (stopped) | piles, half the frame black |
| power 38,400, child offset 2.5 | 1,048,576 | 761,630 at 768 (stopped) | piles, half the frame black |
| 48 oases of 12,800 | 524,288 | 201,388 at 768 (stopped) | an archipelago, mostly black |
| 32 oases of 19,200, child offset 2.5 | 524,288 | 376,529 at 768 (stopped) | most of the frame |
| 64 oases of 19,200, child offset 2.5 | 524,288 | 148,921 at 512 (stopped) | many colonies, more black |
| **48 oases of 25,600, child offset 2.5** | 524,288 | **263,982 at 512** (stopped) | **chosen**: the fullest frame |

The chosen variant is `supercolony-v2-dense`, total power 1,228,800, 24 times
the committed preset's 51,200. It reaches its ceiling of 524,288 soon after
tick 512 and returns to it whenever its oases bloom: in the film's 3,200 ticks
it falls to 321,553 between blooms and is at the ceiling in 123 of 801 frames.
At the ceiling births are refused by refuse-excess-births-v1, which is
externally imposed selection (AGENTS.md). Round 1 stopped the run that reached
its ceiling; round 2 accepts it, as the confluent world does
(`presets/confluent-world.toml`, fed to its ceiling of 262,144), and the site
says so beside the films, as it does for the confluent film.

Both films start from a tick chosen by eye once the world fills the frame:
1,024 for the tissue and 768 for the supercolony. The frames that chose the
supercolony variant were its tick-512 frames, the fullest of the variants'.

# Round 3: one v2 switch on the confluent world (previews only, 2026-11-24)

After the dense films came down, each candidate was the confluent world itself
(`presets/confluent-world.toml` with `model = "field-ecology-v2"`), founded with
`--disable all-v2 --enable SWITCH` and the engine's default parameters for that
switch, run to tick 2,400 and drawn as the film is (`--look sphere --no-field
--color lineage-energy`, 1,080 px, a frame every 4 ticks) for 33 consecutive
frames. With every switch off (`--v1-compat`) the world's tick-2,400 digest is
the v1 film's own, `3f279f51…`, so each difference is that one switch's. Runs
and renders went one process at a time, each run 10 to 19 minutes and each peak
18.7 to 19.5 GiB.

Motion is the share of pixels whose brightness moves by more than 32 of 255
between consecutive frames, at 540 px. On that measure the v1 film scores 7.0%
and the rejected tissue film 10.8%; the number alone did not predict what looked
bad, so every preview was also judged by eye, still and in motion.

| switch | digest at 2,400 | motion | by eye |
|---|---|---:|---|
| none (`--v1-compat`) | 3f279f51… | 8.3% | the film's world; a little darker at 1,080 px |
| `vascular-field` | 684e4559… | 8.2% | v1's labyrinths sharper and more numerous: the best |
| `oases` | e9627874… | 10.9% | big rounded colonies between black voids; v1's shapes kept inside |
| `pcp-polarity` | 49a1ef81… | 7.9% | v1's layout, shapes stretched into bands and tubes |
| `cadherin-4d` | cd6ae289… | 2.8% | lineages merge into smooth single-colour sheets, close to the rejected look |

Every world sat at its ceiling of 262,144 in all 33 frames, as the confluent
film does. The veins world drawn with `--look glow --field-mapping
vascular-conductance` (founded again with `v2-visual-mappings`, since render
refuses switches on a run dir; same digest) shows a nearly uniform pale field
with no network, so its veins read better through the organisms. These are
visual judgements on one seed and a 128-tick window, not measured effects. Round 4
records the two films made from them.


# Round 4: the veins and polarity films (2026-11-24)

The user chose veins (`vascular-field`) for a film and polarity
(`pcp-polarity`) for a second. One more preview first tried veins with blooming
food (`--enable vascular-field --enable oases`; digest `c460f4bc…` at tick
2,400, motion 9.8%). It looked like the oases world, big rounded colonies
between voids, and the veins' labyrinths were lost, so it was not filmed.

Each film's world is committed here as a configuration:
`confluent-veins.toml` and `confluent-polarity.toml`. Each is the run
directory's own `config.toml` (config_hash `93b03ef6…` and `01eb3276…`) under a
header that says how it was founded. Run from tick 0, each file reaches its
film's first digest at tick 2,400, as the site's "Make it yourself" says
(veins in 1,327 s, polarity in 1,068 s, on 14 workers, 2026-11-25).

Each film was made in three passes, one process at a time, each peaking near
19 GiB:

1. The camera's preview: the whole torus at 512 px, 801 frames.
2. The whole film at 1,080 px along the camera that the site's rule chose from
   that preview, with the confluent film's zooms. At the middle of every shot
   four consecutive frames were looked at, whole and at 1:1, with their
   differences, beside the confluent film's. The motion was measured too.
3. Only then, the film at 4K60 (`media:4k60 --preview-from`, the same camera).

Motion in each shot of the 1,080 px pass:

| shot | confluent film | veins | polarity |
|---|---:|---:|---:|
| whole torus, opening | 6.9% | 8.2% | 7.9% |
| ×1.6 | 11.1% | 19.8% | 13.9% |
| ×3 | 13.8% | 17.6% | 14.4% |
| ×2.2 | 16.9% | 25.3% | 18.8% |
| whole torus, closing | 8.2% | 10.5% | 9.5% |

The confluent film's figures come from its web video, which reads about a
sixth lower than PNG frames of the same world: 7.0% against 8.3% for
`--v1-compat`. The veins camera drifts 4.4 and 5.6 units through its ×1.6 and
×2.2 shots, where the confluent film's drifts 1.25 and 1.9 and polarity's
1.25. That pan adds motion the world does not make. By eye, both films keep
legible shapes from frame to frame in every shot.

| film | digest at 2,400 | digest at 5,600 | alive | 4K pass | web files |
|---|---|---|---|---|---|
| `confluent-veins` | `684e4559…` | `2a620955…` | 262,144 in all 801 frames | 1,802 s, peak 19.2 GiB | 6 segments, 102.2 MiB, CRF 30 |
| `confluent-polarity` | `49a1ef81…` | `d4ce6e6a…` | 262,144 in all 801 frames | 1,820 s, peak 19.2 GiB | 6 segments, 104.3 MiB, CRF 30 |

The veins film's first web encode used `media:4k60`'s own CRF 19: 26 segments,
370.5 MiB, about 233 Mbit/s, three times the confluent film's 70. Those files
are in `web/media-retired/`. The film was encoded again at CRF 30 from the same
frames.

Both films are on the site beside the confluent film. The record panel names
each film's configuration. The Version 2 chapter's presets each point to the
film of one of their mechanisms: polarity is in the tissue preset, veins in
the supercolony. The films run each mechanism at the engine's defaults, not at
the presets' settings, and the site says so. Polarity's
`initial_polarity_strength_max` is 0.3 in the film and 0.88 in
`morphogenesis-tissue-v2.toml`. The veins' `flux_half_saturation` is 0.1 in
the film and 0.25 in `supercolony-v2.toml`, and its `maximum_diffusion` is
0.298717 (ten times this world's diffusion) against 0.3. Every other value
of the two tables is the same. The films stay visual choices on one seed, and
the site says so. What each mechanism does, measured, is in the Stage-2 campaign
(`experiments/v2-emergence/README.md`).


# Round 5: longer on the whole torus, one MP4 each, and a phone encode (2026-11-25)

The user found that the films left the whole torus too soon to see what it was
doing, and asked that each film be one file in Cloudflare R2 instead of
segments under the 25 MiB static-asset limit. All three films were made again
from their committed worlds at a frame every 2 ticks instead of 4: 1,601 frames
from tick 2,400 to 5,600, 26.7 s at 60 fps. The camera's pacing is now fixed in
`web/scripts/director.ts` (`FILM_PACING`): 35% of the film on the whole torus
before it moves (560 frames, 9.3 s), 5% for each move, 16% for each held shot
and at least 10% on the whole torus at the end. Two shots fit, so each film
closes on the whole torus for the rest: 288 frames, 4.8 s, 18%; with its
opening that is 14.1 s of the film's 26.7 s. Round 4's films held the whole torus
for 64 frames, about 1 s, and then three shots (×1.6, ×3, ×2.2); each film now
holds two (×1.6 and ×3). The site's tests hold every film to at least a third
of its length, and 9 s, on the whole torus first.

The worlds and their digests are round 4's; only the frames per tick and the
camera changed. Each film was one `media:4k60` process, one at a time:

| film | digest at 5,600 | alive | 4K pass | film | phone encode | archival master |
|---|---|---|---|---:|---:|---:|
| `confluent` | `7fa0cc68…` | 262,144 in all 1,601 frames | 1,600 s, peak 19.9 GiB | 185.9 MiB | 104.6 MiB | 1,045.8 MiB |
| `confluent-veins` | `2a620955…` | 262,144 in all 1,601 frames | 1,820 s, peak 19.1 GiB | 175.4 MiB | 99.1 MiB | 970.3 MiB |
| `confluent-polarity` | `d4ce6e6a…` | 262,144 in all 1,601 frames | 1,847 s, peak 19.4 GiB | 174.3 MiB | 97.5 MiB | 1,009.4 MiB |

Each film is one MP4 at CRF 30, 55 to 58 Mbit/s, that `web/worker.ts` serves
from R2 by byte range. That is H.264 High at level 5.2, which not every phone
decodes, and on an upright phone most of its pixels are scaled away. So
`--small 1080` also encodes the same frames at 1,080 × 1,080 (level 4.2, about
32 Mbit/s), which screens at most 640 CSS pixels wide or tall play instead:
phones either way up, and short windows, where the page draws the film small
(464 device pixels across at 1366 × 640). A phone held sideways draws the film
about 2,500 device pixels across, so there the phone encode is softer than the
4K film; it is chosen for decoding and download.

The phone encode's CRF was chosen on a 3 s clip of the confluent film (frames
1,050 to 1,229, its ×3 shot), compared at 1:1 against the 4K encode scaled to
1,080:

| encode of the clip | bytes | by eye at 1,080 |
|---|---:|---|
| 4K, CRF 30 (the film) | 17,378,404 | the reference: single bodies distinct |
| 1,080, CRF 30 | 5,873,044 | bodies smeared together in the dense colonies |
| 1,080, CRF 28 | 7,827,153 | still softer than the reference |
| 1,080, CRF 26 | 10,289,213 | the same as the reference |
| 1,080, CRF 23 | 15,201,932 | no better than 26 |

The first phone encodes, at CRF 30 (54 MiB for the confluent film), were
replaced by CRF 26 (`--small-crf 26`, now in `media:4k60`). Those first
encodes, the round-4 films' 801-frame single MP4s and their share cards are in
`web/media-retired/films-801-and-phone-crf30-2026-11-25/`. Each 4K encode
reran byte for byte when the phone encodes were added: x264's output is the
same for the same frames, arguments, build and thread count.
