# Website film worlds: how the v2 film presets were chosen

The website's tissue, supercolony and veins films show
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
bbb5773. Their ticks agree: the 16 × 48,000, sigma 12 variant reached the same
tick-1,536 digest, 7b72d200…149913, on both. "Alive" is the live count at the
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
524,288, which it never approaches: its live counts equal the variant's
(61,582 at tick 512, 258,101 at 1,024). It packs into lineage patches over
most of the torus, the way the confluent world does.

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
tick 512 and returns to it whenever its oases bloom: in the film's 3,200 ticks it
falls to 321,553 between blooms and is at the ceiling in 123 of 801 frames. At the ceiling births are refused by
refuse-excess-births-v1, which is externally imposed selection (AGENTS.md).
Round 1 stopped
the run that reached its ceiling; round 2 accepts it, as the confluent world
does (`presets/confluent-world.toml`, fed to its ceiling of 262,144), and the
site says so beside the films, as it does for the confluent film.
