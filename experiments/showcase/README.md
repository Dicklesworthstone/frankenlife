# The showcase search: the parameter sets behind `presets/showcase-world.toml` and `presets/colony-world.toml`

This directory records how the worlds the website's film has shown were chosen. It is an outer search, a
procedure outside the simulation that picked parameters for how the result looks. AGENTS.md asks that such a
selection be stated (rule 8; plan §2.4, §7.2, §15.5).

- **Stages 1 and 2** chose `showcase-world`. They are recorded in the search (`search.py`), every sample's score
  (`scores.ndjson`) and the chosen sample's parameters (`s1000.json`).
- **Stage W** continued from it to `colony-world`, the film's world now. It is recorded in `match.py`, the
  reference statistics (`reference.json`) and the chosen sample's parameters and ancestry (`colony.json`).
  [Stage W](#stage-w-matching-the-references-colony-world-and-tissue-world) has every step.

Frames and per-run records stay outside git.

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

## Stage W: matching the references, `colony-world` and `tissue-world`

### Why

Shown the film, the person who supplied the reference screenshots judged it still nothing like them. Measured
against them, `showcase-world` at tick 4,800 is a famine world:

- It holds 35% of its ceiling. The median life is about 8 simulated seconds, and 98% of deaths are starvation.
  The references' overlays show 299,892 to 299,998 organisms across all six screenshots, so their world
  appears to live at a ceiling of about 300,000.
- `flife inspect` read every organism's heritable motion response χ (33 coefficients: χ_e for energy, χ_m for
  the 32 composition channels):
  - Every organism climbs energy (χ_e > 0), at about a quarter of |χ|.
  - Within a lineage, emission directions agree (cosine 0.75–0.95), but kin are not drawn to kin's emission
    (cosine of χ_m with it −0.32 to +0.09).
  - Emission is 2.5% of the field's input, too little to shape anything.

### A capacity policy, and a statistic

- **The capacity policy.** Under `pause-before-commit`, the research policy, a world that reaches its ceiling
  pauses. The plan's exhibition policy (§7.5) instead refuses births beyond the ceiling in a stable,
  identity-keyed order and counts them. It is now registered as `refuse-excess-births-v1` (c3353bf): a
  refused birth takes no slot, identity or charge, and the parent keeps its energy. It is part of an
  experiment's identity, like every capacity policy, and changes no ecological rule.
- **The statistic.** `match.py` measures 14 statistics of a frame reduced to one pixel per field cell:
  - coverage and voids;
  - contrast;
  - the shape of the luminance spectrum in 8 bands from 2 to 64 cells;
  - colour coherence at 8, 16 and 32 cells.

  `reference.json` holds their mean and spread over 168 patches of the four zoom-1 screenshots. A sample's
  distance is the root mean square of its statistics' differences from that mean, in units of the spread.
  The screenshots stay out of git. The first version scored random noise as close, so it was restated: the
  spectrum's shape rather than its level, and colour coherence.

### The batches

Every sample is a 128 × 128 world at Δt = 1/32 s, seed 42, run 1,600 ticks and scored on the second half of its
frames (raw dots coloured by affinity, every 40 ticks). Scores here are the final statistic's; the first two
batches were rescored with it (`match.py rescore`).

| Batch | Samples | What | Scored | Dying | INV-03 | Closest |
|---|---:|---|---:|---:|---:|---|
| `sample_wide` | 51 | random, ceiling binding, broad supply, reserves up to 40 | 28 | 23 | 0 | `s0005` 1.55 (collapsing) |
| `sample_signal` | 26 | random, emission-dominated | 5 | 21 | 0 | 3.37 |
| fed sweep of `s1000` | 12 | supply ×1/2/4, ceiling 1 or 2 per cell, reserve ×1/4 | 8 | 4 | 0 | `x1-c2-r1` 1.39 |
| round 1, `s3000`–`s3039` | 40 | perturbations of `x1-c2-r1`, `x2-c2-r1` | 21 | 17 | 2 | `s3030` 1.05, `s3004` 1.27 |
| round 2, `s3100`–`s3139` | 40 | perturbations of `s3004`, `s3015`, `s3027`, `s3000` | 23 | 7 | 12 | `s3123` 1.12, `s3104` 1.37 |
| trail sweep of `s3104` | 32 | emission, sensor saturation, diffusion, decay, source width | 32 | 0 | 0 | `e4-r8-d1-k8-w0` 1.01 |
| trail sweep 2 | 24 | more emission, sensor scale, decay, toxicity | 8 | 16 | 0 | its parent's 1.01 |

- **`sample_wide`** was stopped by hand at 51 of 158. Fed worlds became a uniform, static gas of dots, and hungry
  ones famine rings.
- **`sample_signal`** was stopped at 26 of 96. Random controllers spend about half their maximum, so the
  founders bled their energy into the field and died.
- **The fed sweep** binds the ceiling under `refuse-excess-births-v1`. Its four reserve ×4 variants died
  because their founders' initial energy, 2.0, is below the raised attempt threshold.
- **Each perturbation** multiplies every number by a factor in [0.7, 1.4], the ceiling included. It multiplies
  the source count by 1, 2, 4 or 8 at the same total power.
- **Round 2's parents** were the four closest when 34 of round 1's 40 had finished. `s3030`, the closest of all
  round 1, finished later and was never a parent.
- **INV-03.** Fourteen local samples stopped on the energy ledger's cumulative residual check (bead
  `flife-ze4`).
  - Twelve had already collapsed to 200 organisms or fewer.
  - The other two, `s3109` and `s3122`, were declining (1,275 and 1,900 of 10,954 founders). Both were scored
    from their frames before the stop, at 3.48 and 3.10, so they count in round 2's 23 scored as well.

  No viable world was lost to it.
- **`s3104`.** Two of round 2's samples, `s3104` and `s3107`, were run on held-out seeds 1–3 and both held
  their ceilings. `s3104` was taken forward by eye because it filled the world and moved: coverage 0.84–0.86 and
  motion 0.34–0.41 on those seeds, against `s3107`'s 0.56–0.70 and 0.10–0.11.
  - Meanwhile, `x1-c2-r1` on 256 × 256 had collapsed by tick 9,600 to a few lineages' rings around eaten voids.
  - On `s3104` the controls repeated stages 1–2's finding. With controllers zeroed, shapes still form.
    With fresh offspring genomes, the world is a diffuse scatter.
- **The trail sweep** is a factorial over five changes:
  - emission rate ×1, or ×4 at transfer efficiency 0.95;
  - the sensors' energy density scale 1 or 8 (less saturated energy readings in a rich field);
  - diffusion ×1 or ×0.3;
  - decay ×1 or ×8, with source power ×2.75 at ×8;
  - sources 3× wider and 4× slower, or not.

  The eight farthest all had the wider sources, which make a nearly uniform world. Strong emission with
  decay ×8 and scale 8 was closest.
- **Trail sweep 2** found that every variant with more emission (×2 or ×4 again) died for the same reason as
  `sample_signal`. None of its survivors came closer than its parent.

### The choice

A person chose `e4-r8-d1-k8-w0`, the closest of the 125 scored (of 225 run), by eye, against the references. Its lineages
grow into single-colour colonies with membranes and textured interiors: territories of one colour, as in the
references, though still discrete bodies on black where the references are a nearly continuous tissue.

Seeds the search never saw, at 128²:

| Seed | Distance | Live, second half | By eye |
|---:|---:|---|---|
| 42 (chosen) | 1.01 | 44,384 (the ceiling) | single-colour colonies, membranes |
| 1 | 2.18 | 44,210–44,384 | the same |
| 2 | 1.19 | 44,384 | the same |
| 3 | 2.32 | 44,321–44,384 | the same |

Two controls, seed 42, one run each:

| Model | Distance | End | By eye |
|---|---:|---|---|
| `field-ecology-v1-chemistry-only` | 2.04 | 44,384 | tangled multicoloured filaments filling the world; no single-colour colonies |
| `field-ecology-v1-fresh-offspring` | — | 937, dying | sparse scatter |

Unlike in `showcase-world`, the controllers' decisions are needed here.

- **Chemistry-only** decodes every action from zeros. That leaves the emission gate open (σ(0) = 0.5 is above
  the threshold 0.355) but the emission direction zero, so nothing is emitted. Without it the colonies do not
  form.
- **What emission does.** In the last 160 ticks of the 128² run, organisms deposited 181,246 energy as
  emission against 353,874 taken up: they return about half of what they eat. Emission is about a tenth of
  the field's input (the sources gave 1,581,130), against 2.5% in `showcase-world`. Every organism's gate is
  open, and its spend is unregulated (σ ≈ 0.50, the median). But the direction its network chooses is a
  lineage signature: kin agree at cosine 0.79–0.99 in each of the eight lineages examined, of the 27 with
  200 or more members.
- **The mechanism.** Each lineage paints the field around it with its own composition, and organisms whose
  affinity differs eat that composition less efficiently. This is niche construction.
- **The limits.** Chemistry-only zeroes every decision, not only emission's direction, so the controls show
  the controllers are necessary. Emission is the likely route, not shown to be the only one.

### The colony preset

`presets/colony-world.toml` is `e4-r8-d1-k8-w0` on 4× the area: 256 × 256, 256 sources, 43,816 founders and a
ceiling of 177,536 (2.71 per cell, as in the sample). Its genome quotas are raised so the worst case fits, so no
hidden genome-size limit selects. Its header lists each change from the Desktop preset, and
`crates/flife-cli/tests/colony_preset.rs` checks those changes and checks the preset against `colony.json`.

`flife run presets/colony-world.toml --ticks 4800` ends on digest `7b4752e7…f95aea`, with 24 workers on CPU
controllers in about 25 minutes. A film continuing it from tick 4,800 to 8,000 was rendered but not published:
the denser world below replaced it.

- **Population.** It fell from 43,816 founders to 27,604 as the founders that could not feed died. It reached
  the ceiling by tick 960, and every progress record after that shows it there except one, 165,286 at tick
  1,920. Over the run, 2,605,491 births were committed and 3,715,950 refused at the ceiling.
- **Emission.** It deposited 15.7 million energy in all.
- **Lineages.** Founder lineages fell to 10 alive by tick 4,320, with an effective number of 4.5 at 4,800, as
  colonies took over. The largest holds 89,563 of the 177,536. Affinity keeps mutating within a lineage, so
  one founder's descendants still differ in colour.
- **By eye**, at ticks 2,880, 3,840 and 4,800: single-colour colonies 30–60 cells across with membranes.
  Some have a foam-like inner mesh, and black field lies between them.
- **The ledger.** Its cumulative residual ends at 5.8 × 10⁻⁹ of the initial energy plus turnover.

This is one seed at this scale; the held-out seeds above are at 128².

### A denser world: `tissue-world`

`colony-world`'s colonies sit on a black field, where the references are a nearly continuous tissue. One more
sweep of `e4-r8-d1-k8-w0` tried its ceiling at ×1, ×2 and ×4 and its food supply at ×1, ×2 and ×4 (8 new
samples, all scored):

| Sample | Ceiling | Supply | Distance | Live, second half | Coverage | Motion |
|---|---:|---:|---:|---|---:|---:|
| `c4-x1` | 177,536 | ×1 | 0.76 | 90,081–150,103 | 0.79 | 0.43 |
| `c2-x1` | 88,768 | ×1 | 0.92 | 88,768 (the ceiling) | 0.76 | 0.30 |
| `e4-r8-d1-k8-w0` | 44,384 | ×1 | 1.01 | 44,384 (the ceiling) | 0.68 | 0.20 |
| `c4-x2` | 177,536 | ×2 | 1.18 | 143,708–177,536 | 0.75 | 0.27 |
| `c2-x2` | 88,768 | ×2 | 1.39 | the ceiling | 0.67 | 0.17 |
| `c1-x2` | 44,384 | ×2 | 1.49 | the ceiling | 0.62 | 0.12 |
| `c4-x4` | 177,536 | ×4 | 1.60 | the ceiling | 0.67 | 0.16 |
| `c1-x4` | 44,384 | ×4 | 1.66 | the ceiling | 0.57 | 0.10 |
| `c2-x4` | 88,768 | ×4 | 1.67 | the ceiling | 0.62 | 0.10 |

- **Ceilings.** `c4-x1` is the closest of the 133 Stage W samples scored. At ×4 the ceiling stops binding:
  food holds the population at 5–9 organisms a cell. The colonies grow into one another and fill most of the
  world, with voids, membranes and mesh walls between them.
- **More food.** Extra supply filled every ceiling and slowed the world.
- **The choice.** A person chose `c4-x1` by eye against the references.

Seeds the search never saw, at 128², and the two controls (seed 42):

| Run | Distance | Live, second half | By eye |
|---|---:|---|---|
| seed 42 (chosen) | 0.76 | 90,081–150,103 | continuous multicoloured tissue, membranes, mesh |
| seed 1 | 1.96 | 74,277–90,490 | colonies, some merged into tissue; more black between |
| seed 2 | 1.12 | 79,121–115,662 | the same |
| seed 3 | 2.04 | 60,766–91,840 | the same |
| `field-ecology-v1-chemistry-only` | 1.57 | 177,536 (the ceiling) | a featureless multicoloured haze filling the world |
| `field-ecology-v1-fresh-offspring` | — | 937, dying | sparse scatter |

**The seeds are the weak point.** On all three held-out seeds the world grows the same single-colour colonies,
but they merge less and leave more of the field black than on seed 42. The seeds' coverage is 0.68–0.73,
against 0.62–0.65 for `e4-r8-d1-k8-w0` on the same seeds, and each seed's distance is a little lower than
that sample's. So the ceiling's effect generalizes; the degree of continuity on seed 42 is partly that seed.

Without emission (chemistry-only), nothing ever leaves an organism: the population fills its ceiling with a
haze and no structure. With fresh offspring the world dies, as before.

### The tissue preset, and the film

`presets/tissue-world.toml` is `c4-x1` itself, at its own 128 × 128 scale: 64 sources, 10,954 founders, a
ceiling of 177,536. Its only difference from the sample is the genome quotas, raised so the worst case fits.

- **The quotas never bound.** `flife run presets/tissue-world.toml --ticks 1600` ends on the sample's own
  digest, `a93e9dd2…a37e02`.
- **The test.** `crates/flife-cli/tests/tissue_preset.rs` checks the preset's declared changes and checks it
  against `tissue.json`.
- **The run, continued to tick 4,800** (digest `556c4973…5758d1`).
  - The population moves between 110,000 and 163,000 at the 480-tick records after tick 1,600, with a low of
    82,139 and a high of 171,856.
  - It never reached its ceiling, so `refuse-excess-births-v1` refused no birth in the whole run.
  - Founder lineages fall from 98 at tick 1,600 to 8 at 4,800.
  - By eye: dense tissue with membranes, voids and mesh walls throughout. The colours narrow towards two or
    three as lineages consolidate.

The website's film is this world from tick 1,600, where the search stopped scoring it, to 4,800. It has 401
frames, one every 8 ticks, at 1080 × 1080: raw dots coloured by chemical affinity, without the field. Its camera
is chosen from the run, as before, with each shot's zoom scaled to the 128-unit torus (1.25–2.5). It ends on
the continued run's digest, `556c4973…5758d1`. `web/lib/media.json` records every frame's tick and live count,
the camera's keys, flife's revision and binary hash, and the encoding.

## What this does not show

- Stages 1–2's statistic rewards fine structure that moves and fills the world. It is not a measure of
  life-likeness, and the final choice was by eye. So was stage W's.
- Stage W's distance compares image statistics, not mechanisms. The references' equations are unknown (plan
  §1.2), and a world whose frames share their spectrum and colour coherence need not work as theirs does.
  The closest sample, `c4-x1`, is still 0.76 standard deviations from the references on average, and its
  held-out seeds 1.1–2.0.
- `colony-world` is discrete colonies on a black field. `tissue-world` is closer to the references' continuous
  tissue on seed 42, less so on held-out seeds. In both, founder lineages narrow to a handful within 150
  simulated seconds, and the colours with them, where the references hold many colours at once.
- Nothing here shows the structure persists past the runs above: 4,800 ticks is 150 simulated seconds.
- Every control above is one seed.
  - In `showcase-world` they show that inheritance is necessary for the shapes and that the controllers'
    decisions are not. They do not show the controllers add nothing elsewhere, such as in which lineages win
    over longer runs.
  - In `colony-world` they show that the controllers' decisions are necessary for the colonies, not which
    decisions.
