# Website film worlds: how the two abundant v2 presets were chosen

The website's tissue, supercolony and veins films show
`presets/morphogenesis-tissue-v2-abundant.toml` and
`presets/supercolony-v2-abundant.toml`. Each is a committed v2 preset with only
its `[sources]` changed: more food, so that the films show far more organisms.
The committed presets stay the evidence. Every Stage-2 figure on the site
(`experiments/v2-emergence/README.md`) is theirs, and no claim covers the
abundant presets.

The choice was made by eye, outside the world, for a fuller frame. Nothing
chosen feeds back into a run. Each variant below is its base preset (seed 42)
with the `[sources]` keys shown, rendered from tick 0 by `flife render CONFIG
--every 256 --width 1080 --height 1080 --look sphere --color lineage-energy
--no-field` on 2026-11-24, by flife built at 262f067 (the first runs) or at
bbb5773. Their ticks agree: the 16 × 48,000, sigma 12 variant reached the same
tick-1,536 digest, 7b72d200…149913, on both. "Alive" is the live count at the
last frame, "peak" the most at any frame.

## Tissue (base `morphogenesis-tissue-v2`: 16 sources of power 4,000, sigma 4.2)

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

## Supercolony (base `supercolony-v2`: 16 oases of power 3,200)

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
