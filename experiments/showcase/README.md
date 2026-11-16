# The showcase search: the parameter set behind `presets/showcase-world.toml`

This directory records how the world the website's film shows was chosen. It is an outer search, a procedure
outside the simulation that picked parameters for how the result looks. AGENTS.md asks that such a selection be
stated (rule 8; plan §2.4, §7.2, §15.5). It holds the search (`search.py`), every sample's score
(`scores.ndjson`) and the chosen sample's parameters (`s1000.json`). Frames and per-run records stay outside
git.

## Why

The Desktop and Interactive presets were calibrated for viability: a population inside a declared band. They
do that, but at the film's scale they show diffuse scatter and Gaussian blobs at the sources. Nothing in them
looks like the reference screenshots' membranes, filaments and cells. Three things looked responsible:

- Eating leaves holes in the field, so organisms effectively avoid one another.
- Sources whose 32-channel compositions are nearly orthogonal leave food edible only near their centres.
- Emission is too weak to structure the field.

A hand sweep tried the newly registered `field-ecology-v1-ungated-motion` (motion not gated by uptake) on the
Desktop parameters, and it alone made no structure. The question then became where in the existing model's
parameter space organism-like structure appears, if anywhere. The search answered that question. No rule was
changed for it.

## What the search did

Stage 1: 300 configurations, `s0000`–`s0299`.

- **Sampling.** Sample *i* draws from `random.Random(i)`: the model (`field-ecology-v1` or
  `field-ecology-v1-ungated-motion`, evenly) and 26 parameters, on log or linear ranges given in `sample()`. The
  parameters are field diffusion and decay; source count, width, path speed and total supply; seven chemistry
  and motion constants; emission; basal and motion costs; reproduction; death recycling; and founder density.
- **Runs.** Each is `presets/desktop-world.toml` with those values on a 128 × 128 world at Δt = 1/32 s, seed
  42. It runs 1,600 ticks, rendered every 40 ticks as raw dots without the field. The population ceiling is 8
  founders' worth, at most 48,000.
- **Scoring.** The second half of the frames is scored:
  - **fine**: power at wavelengths of 4–16 cells over the shot-noise level;
  - **motion**: 1 minus the correlation of frames two apart;
  - **coverage**: the share of the world the organisms light.

  `search.py` defines each one exactly.
- **Ranking.** The key is fine × min(1, motion / 0.3) × min(1, coverage / 0.5).

Stage 2: 110 perturbations, `s1000`–`s1109`, each of one of 8 parents: `s0192`, `s0178`, `s0154`, `s0051`,
`s0008`, `s0259`, `s0231` and `s0139`. A person chose those parents by eye from stage 1's 14 best by fine ×
motion. Coverage was not in that stage-1 ranking because its first version read the background colour as
organisms; it was fixed before stage 2. The person skipped stage 1's best (`s0043`, a sparse scatter at 6%
coverage) and five others whose frames looked like noise or fixed clusters. Each perturbation multiplies every
number by a factor in [0.7, 1.4]. It also multiplies the source count by 1, 2, 4 or 8 and divides each source's
power by the same factor. Stage 2 was stopped by hand once 110 of the planned 150 had started. The 8 still
running then have no score.

The choice: a person looked at the last frames of the best-ranked stage-2 samples, and at `s1000` and `s1063`
over time, against the reference screenshots. They chose `s1000`, 13th of the 402 scored. The six above it with
the highest fine scores (`s1047`, `s1001`, `s1022`, `s1065`, `s1004`, `s1076`) all descend from `s0154`, and
they are dense round clusters sitting on their sources: their fine power is the clusters' edges. `s1000`
descends from `s0178` with 8× its sources at an eighth of the power each. It makes membranes, rings, crescents
and colonies with sparse interiors that fill the world: coverage 0.57, against 0.32–0.41 for the six. The other
six above it are `s0178` itself and five of its descendants, which look like it but fill less of the world:
coverage 0.31–0.44.

| | Stage 1 | Stage 2 |
|---|---:|---:|
| Samples | 300 | 110 |
| Reached the last tick | 212 | 94 |
| Paused at the population ceiling (scored on the frames before) | 21 | 5 |
| Dying (under a tenth of the founders, or under 200, at the end) | 67 | 3 |
| Interrupted, no score | 0 | 8 |
| `field-ecology-v1` / `-ungated-motion` | 151 / 149 | 37 / 73 |

The 20 best by the key split evenly between the two models. The chosen world runs `field-ecology-v1`, the
baseline model.

## Provenance

- `search.py` regenerates every sample's configuration from its index (stage 2 also from its parents'
  `params.json`). Checked against the configurations that ran:
  - all 110 stage-2 configurations are byte-identical;
  - 224 of 300 stage-1 configurations are identical;
  - the other 76 differ only in `population.maximum_count`. They ran first under a lower ceiling, 3 founders'
    worth, and never reached it. The ceiling only pauses a run that reaches it, so it changes nothing else.
- `scores.ndjson` is `search.py record`: every sample rescored from its frames with the final `score()`, one
  line each.
- Binary: `flife` built at 36d83da, the revision that registered `field-ecology-v1-ungated-motion` and added
  `flife render --camera`. Every sample used CPU controllers with 4 workers, several samples at a time on an
  M5 Ultra. No result depends on the route or the worker count.

## The preset, and held-out seeds

`presets/showcase-world.toml` is `s1000` on 4× the area: 256 × 256, 64 sources, 43,816 founders. Its own
ceiling is 240,000, with genome and memory budgets to match. Its header lists each change from the Desktop
preset, and `crates/flife-cli/tests/showcase_preset.rs` checks those changes and checks the preset against
`s1000.json`.

`flife run presets/showcase-world.toml --ticks 4800` ends on digest `bb49c89c…9e3527`. That is the digest of the
same parameters run earlier under the search's scale-up ceiling with 24 workers. The film's run directory is
this run.

The search ran seed 42 only. Seeds it never saw:

| Seed | World | Ticks | Live, second half | Highest | End digest | By eye |
|---:|---|---:|---|---:|---|---|
| 1 | 128², `s1000` | 3,200 | 21,329–44,826 | | `3f3b7faa…` | membranes, colonies, bands |
| 2 | 128², `s1000` | 3,200 | 19,860–40,111 | | `4de4e28f…` | membranes, colonies |
| 3 | 128², `s1000` | 3,200 | 18,403–42,630 | | `95b92077…` | membranes, colonies, rings |
| 4 | 128², `s1000` | 3,200 | 21,040–44,589 | | `86df8170…` | membranes, colonies |
| 1 | the preset | 4,800 | 54,027–83,204 | 145,464 | `b8bec629…` | rings, cells, membranes |
| 2 | the preset | 4,800 | 58,499–111,017 | 130,814 | `348555ec…` | rings, cells, membranes |
| 3 | the preset | 4,800 | 57,671–91,289 | 147,035 | `0bfbcb27…` | rings, cells, membranes |
| 42 | the preset | 4,800 | 66,405–103,917 | 131,678 | `bb49c89c…` | rings, cells, membranes |

Every run reached its last tick, none came within 90,000 of the ceiling, and each population cycles rather
than settling. "By eye" means one person looking at frames. It is not the G5 review against the references,
which has not been run.

## Two controls on the preset

These runs use the preset at seed 42 for 4,800 ticks, changing only `model`, with frames every 96 ticks. They are
one run each.

| Model | Live, second half | Highest | End digest | By eye |
|---|---|---:|---|---|
| `field-ecology-v1` (the preset) | 66,405–103,917 | 131,678 | `bb49c89c…` | rings, cells, membranes |
| `field-ecology-v1-chemistry-only` | 75,695–135,297 | 165,601 | `5126f564…` | the same kinds of rings, cells and membranes |
| `field-ecology-v1-fresh-offspring` | 766–3,796 | 43,816 | `ae5cf41d…` | diffuse scatter, no structure; dwindling |

- **Chemistry-only.** Every controller is evaluated and charged, but its actions are decoded from zeros, so no
  decision depends on what an organism senses. The shapes still form.
- **Fresh offspring.** Each newborn's genome, including its chemical affinity and motion response, is drawn as a
  founder's instead of inherited. No shapes form, and the population falls from 43,816 to 766.

So in this world the shapes come from inherited chemistry: what each lineage eats, and how it climbs the field
towards food. They do not come from its neural network's decisions. A lineage whose chemistry suits the food
near it grows into a feeding front. That front eats a void into the field and becomes a membrane around it.

## What this does not show

- The search's statistic rewards fine structure that moves and fills the world. It is not a measure of
  life-likeness, and the final choice was by eye.
- Nothing here shows the structure persists past the runs above: 4,800 ticks is 150 simulated seconds.
- The controls above are one seed each. They show that inheritance is necessary for the shapes and that the
  controllers' decisions are not. They do not show the controllers add nothing elsewhere, such as in which
  lineages win over longer runs.
