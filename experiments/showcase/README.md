# The showcase search: the parameter sets behind the website's film

This directory records how the showcase presets were chosen; the website's film shows the last of them,
`confluent-world`. It is an outer search, a
procedure outside the simulation that picked parameters for how the result looks. AGENTS.md asks that such a
selection be stated (rule 8; plan §2.4, §7.2, §15.5).

- **Stages 1 and 2** chose `showcase-world`. They are recorded in the search (`search.py`), every sample's score
  (`scores.ndjson`) and the chosen sample's parameters (`s1000.json`).
- **Stage W** continued from it to `colony-world` and `tissue-world`. It is recorded in `match.py`, the
  reference statistics (`reference.json`) and the chosen sample's parameters and ancestry (`colony.json`).
  [Stage W](#stage-w-matching-the-references-colony-world-and-tissue-world) has every step.
- **Stage C** continued from `tissue-world` under a registered model variant with bodies,
  `field-ecology-v1-contact` (plan §5.4), to `contact-world`. It is recorded in
  `scale.py`, its reference statistics (`scale-reference.json`) and the chosen sample's parameters
  (`contact.json`). [Stage C](#stage-c-bodies-that-pack-and-adhere-contact-world) has every step.
- **Stage D** measured that stage C's bodies stack, and registered `field-ecology-v1-contact-inhibition`,
  whose bodies pack. [Stage D](#stage-d-bodies-that-pack-field-ecology-v1-contact-inhibition) has its
  trials; no preset uses it yet.
- **Stage E** continued from `contact-world` on a field four times finer, with food to spare, scattered
  offspring and a smaller torus, to `confluent-world`, the world the website's film shows. It is
  recorded in `confluent.json`. [Stage E](#stage-e-a-finer-field-fed-and-dispersed-confluent-world) has every
  trial.

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

Stage W's film showed this world from tick 1,600, where the search stopped scoring it, to 4,800. It had 401
frames, one every 8 ticks, at 1080 × 1080: raw dots coloured by chemical affinity, without the field. Its
camera was chosen from the run, as before, with each shot's zoom scaled to the 128-unit torus (1.25–2.5). It
ended on the continued run's digest, `556c4973…5758d1`.

## Stage C: bodies that pack and adhere, `contact-world`

### Why

The person who supplied the references sent a second set of 14 screenshots of the same simulation, zoomed in
2.2–26×, and judged the tissue film still not organic enough. Up close, the references' organisms are
shaded spheres of one size that pack without overlapping. They form single-file chains that bound voids,
concentric arcs, foam and reticulated networks with small dark holes, and domains of one saturated colour
with sharp edges and dark gaps between them. Their lit pixels have median OKLab lightness 0.40–0.57.

No rule of `field-ecology-v1` can make that. Its organisms are points: any number share a spot, so a
colony is a cloud, not a packing. Plan §5.4 keeps direct collisions and adhesion out of the baseline and
leaves them to later model variants. Stage C registered one, `field-ecology-v1-contact`, with its two
controls. Plan §5.4 has the rule:

- every organism is a disk of the `[contact]` table's diameter;
- after each tick, overlapping bodies push apart;
- bodies near touching pull together by the square of how alike their heritable affinities are.

It moves positions only, never energy. The search only chose the table's numbers.

### A statistic at the references' scale

match.py measures a frame at one pixel per field cell, the references' wide views. The new screenshots
are close-ups, so `scale.py` first resizes every image until an organism is 2 pixels across: a screenshot
by its zoom, which its overlay shows, and a frame by its bodies' drawn diameter. Then it cuts the image
into match.py's 128-pixel patches and takes the same 14 statistics. Only the eight screenshots at zoom 5 or
less leave whole patches (48 in all). `scale-reference.json` holds their statistics, and none of the images.

**A correction.** The first version of this reference read six of the fourteen overlays' zooms wrongly. Two
close-ups, at 10.4 and 9.8, were measured as wide views at 3.8 and 2.2, and two wide views, at 3.0 and 2.2,
were left out. Four others swapped zooms. So every organism scale but two was wrong. Rebuilt from the
overlays as they read, the references are confluent: median coverage 0.82 ± 0.10 and voids 0.05 ± 0.06,
where the first version had 0.73 ± 0.24 and 0.14 ± 0.23. Every distance below is from the corrected
reference. The first version's distances follow in brackets, because the choices below were made on them.

Each batch below was scored on its last frame, at tick 1,600: 384 × 384 raw dots, coloured by affinity,
drawn at the body's radius. The batches ran under different renderer palettes (the colour change below came
mid-search). `a0` and `ct-d45`, the same world bit for bit, score 1.87 and 1.70 (1.62 and 1.52), so
differences under about 0.2 between batches mean nothing.

### The batches

Every sample is `tissue-world` (`c4-x1`) on seed 42 under `field-ecology-v1-contact`, with the child
placement radius equal to the body diameter.

| Sample | Change | Distance | Live, second half |
|---|---|---:|---|
| `ct-d30` | diameter 0.30, no adhesion | 1.39 (1.07) | 101,037–160,634 |
| `ct-d45` | diameter 0.45, no adhesion | 1.70 (1.52) | 108,303–150,347 |
| `ct-d60` | diameter 0.60, no adhesion | 2.24 (2.05) | 105,052–153,323 |
| `ct-d80` | diameter 0.80, no adhesion | 3.11 (2.77) | 86,853–166,248 |
| `a0` | `ct-d45` as a `[contact]` table: stiffness 0.5, adhesion 0 | 1.87 (1.62) | 108,303–150,347 |
| `a2` | adhesion 0.2, reach 1.5 diameters | 1.73 (1.52) | 104,840–158,690 |
| `a5` | adhesion 0.5 | 1.60 (1.30) | 99,442–159,585 |
| **`a10`** | **adhesion 1.0** | **1.27 (0.69)** | **87,701–159,501** |
| `a5-r2` | adhesion 0.5, reach 2 | 3.31 (1.46) | 67,497–99,156 |
| `a10-r2` | adhesion 1.0, reach 2 | 5.59 (2.25) | 14,531–17,464 |
| `a5-m4` | adhesion 0.5, chemistry mutation ×4 | 1.30 (1.22) | 101,929–154,895 |
| `a10-k1` | adhesion 1.0, stiffness 1 | 1.72 (1.59) | 99,861–149,498 |
| `a10-r125` | adhesion 1.0, reach 1.25 | 1.70 (1.40) | 106,252–151,377 |
| `a10-r175` | adhesion 1.0, reach 1.75 | 2.98 (1.53) | 67,145–124,933 |
| `a10-m4` | adhesion 1.0, chemistry mutation ×4 | 0.98 (0.85) | 92,597–169,733 |
| `a10-d35` | adhesion 1.0, diameter 0.35 | 1.16 (0.77) | 86,145–166,273 |
| `a10-x2` | adhesion 1.0, food ×2, ceiling 260,000 | 2.08 (1.20) | 151,816–211,990 |

By eye, in the same order:

- **Diameter, without adhesion.** At 0.30 the bodies form tubes and rings but rarely pack. At 0.45 they
  pack into tissue with lumens, single-file membranes and small holes. At 0.60 they pack into nearly uniform
  solids, and at 0.80 into hexagonal crystal.
- **Adhesion.** It turns 0.45's smooth bodies into reticulated tissue: beaded strands, rings, foam, and
  colour domains with dark gaps between unlike chemistry. At 1.0 the effect is strongest. With reach 2,
  or 1.75, bodies clump into beads on strings, and the population falls.
- **Stiffness 1** resolves every overlap at once and smooths the tissue again.
- **The rest.** Faster mutation speckles each domain. A diameter of 0.35 makes a finer foam with more holes.
  Double food grows rings, lacy membranes and glandular lobes, but fills less of the torus by tick 1,600.

At adhesion `k_a ≤ 1`, a lone pair closes no further than touching, so `a10` is the strongest pull of that
kind. A person chose `a10` by eye from close-ups drawn as spheres, when it was also the nearest. Against the
corrected reference it is third, behind `a10-m4` and `a10-d35`.

### Colours

The close-ups also showed the renderer's spheres paler than the references: median lit lightness 0.76,
against 0.40–0.57. FrankenManim's true-dot shading mixed most of each dot towards white. The sphere look now
uses little gloss and a deep shadow, so a body keeps its colour, with a dark rim and a soft highlight. The
affinity palette's lightness is 0.46 ± 0.12 at chroma 0.26. A sphere-drawn `a10` frame's lit pixels then
have median lightness 0.47. Both are presentation only.

### Held-out seeds and controls

Seeds the search never saw, scored the same way, and the two controls (seed 42):

| Run | Distance | Live, second half | By eye |
|---|---:|---|---|
| seed 42 (chosen) | 1.27 (0.69) | 87,701–159,501 | reticulated multicoloured tissue, tubes, rings, foam |
| seed 1 | 2.55 (1.20) | 76,023–89,072 | separate colonies: rings, lobes, membranes; more black between |
| seed 2 | 2.78 (1.25) | 68,032–119,654 | the same |
| seed 3 | 2.80 (1.26) | 72,549–104,198 | the same, with large single-file membrane rings |
| `field-ecology-v1-contact-chemistry-only` | 0.80 (0.89) | 177,536 (the ceiling) | a packed, mottled multicoloured haze filling the world: no colonies, rings or membranes |
| `field-ecology-v1-contact-fresh-offspring` | 4.67 (2.60) | 944, dying | sparse scatter |

As with `tissue-world`, the held-out seeds grow the same kinds of structure, but they merge less and leave
more of the torus black. The controls behave as they did without contact:

- **Chemistry-only.** Nothing is emitted, and the bodies pack to the ceiling as a haze. So the colonies,
  rings and membranes come from emission, chosen by the networks; contact only packs them. The statistic
  puts this haze at 0.80, nearer than any sample, which is a limit of the statistic, not a likeness: it
  rewards a frame that is filled.
- **Fresh offspring.** The world dies, so inheritance is necessary.

**The statistic prefers the plain model.** Scored the same way, `tissue-world` is 0.78–1.00 on seed 42 (dot
diameters 0.30–0.45) and 1.82–2.39 on its held-out seeds, closer than `contact-world` (1.27; 2.55–2.80).
Under the first reference the two were level (0.62–0.75 and 0.75–1.09). At 384 pixels a body is 1–2
pixels, and the statistic sees domains, holes and coverage, not packing. So the case for the contact model
rests on the close-ups, judged by eye: bodies drawn at their size, single-file membranes and foam. Stage D
found that those bodies stack rather than pack.

### The contact preset, and the film

`presets/contact-world.toml` is `a10` itself: `tissue-world` with three changes, the model, a child
placement radius of 0.45 and the `[contact]` table. It also keeps `tissue-world`'s raised genome quotas.

- **The quotas never bound.** `flife run presets/contact-world.toml --ticks 1600` ends on the sample's own
  digest, `ce3ed520…51a1f2`, on 16 workers, where the sample ran on 5.
- **The test.** `crates/flife-cli/tests/contact_preset.rs` checks the preset's declared changes and its
  table, and checks it against `contact.json`.

Stage C's film showed this world from tick 1,600, where the search stopped scoring it, to 4,800: 401 frames,
one every 8 ticks, at 1080 × 1080. Each organism is drawn as a lit sphere at its body's size, coloured by
chemical affinity, without the field.

- **The run.** The population falls from 159,501 to 83,045 by about tick 2,050, then recovers to between
  about 92,000 and 165,000. At four frames near tick 4,200 it touches the ceiling, 177,536, where
  `refuse-excess-births-v1` refuses the births that do not fit. The run ends on digest `b76175a7…902bf5`.
- **By eye.** Reticulated multicoloured tissue across the torus; up close, spheres in membranes, tubes,
  interdigitated strands and glands, with domains of one chemistry against dark gaps. The spheres overlap,
  stacked about three deep (stage D).
- **The camera** is chosen from the run by the same rule as before. Its shots are at zooms 3, 8 and 5 on
  the 128-unit torus, where a body is 11, 30 and 19 pixels across, closer than the tissue film's 1.25–2.5,
  so that the bodies show. `web/lib/media.json` records every frame's tick and live count, the camera's
  keys, flife's revision and binary hash, and the encoding.

## Stage D: bodies that pack, `field-ecology-v1-contact-inhibition`

### Why

Measured, `contact-world`'s bodies stack. At tick 1,600:
- **Nearest neighbours.** The median is 0.24 diameters, and 82% of bodies have one nearer than half a diameter.
- **Crowding.** On average 22 bodies lie within one diameter.
- **Coverage.** The disks would cover 155% of the torus and cover 52%.

At zoom 8 the film's "packed spheres" are stacks about three deep. Two causes:
- **The rule's pair potential is not stable.** Its soft core integrates to `(π/24) k D⁴` and its adhesion
  well to about `−0.103 k_a D⁴`. At `k = 0.5, k_a = 1` the total is `−0.038 D⁴`, so a dense clump has lower
  energy than a packing. Stiffness 1 alone moved the median only to 0.29 D.
- **Velocity carries a body into its neighbours.** It decays by only 2% a tick, the median speed is 0.10
  diameters a tick, and a half-diameter overlap is pushed back 0.125 diameters a tick.

Plan §5.4 registers a variant and its two controls. Sweep 1 is the contact rule. `[contact] sweeps − 1`
further sweeps push overlapping bodies apart only. A survivor then loses the share `[contact] inhibition` of
its velocity against its overlaps' push, which never adds speed. Repeating the whole rule in every sweep
made the stacking worse, a median of 0.17 D at eight sweeps, because each sweep descended the same unstable
potential.

### Trials

Every trial is `contact-world` (seed 42) under the variant, run to tick 1,600, with the child placement
radius equal to the diameter, and scored like stage C against the corrected reference. "Nearest" is the
median nearest neighbour in diameters, and "covered" the share of the torus under a disk.

| Trial | Sweeps | Inhibition | Diameter | Adhesion | Live at 1,600 | Nearest | Covered | Distance |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `contact-world` | 1 | — | 0.45 | 1.0 | 159,501 | 0.24 | 0.52 | 1.27 |
| `p8` | 8 | 1 | 0.45 | 1.0 | 160,647 | 0.49 | 0.73 | 1.61 |
| `p8-d35` | 8 | 1 | 0.35 | 1.0 | 145,777 | 0.62 | 0.54 | 1.82 |
| `d35-a0` | 8 | 1 | 0.35 | 0 | 152,622 | 0.62 | 0.54 | 1.65 |
| `c0-d45-a10` | 8 | 0 | 0.45 | 1.0 | 164,611 | 0.49 | 0.75 | 1.51 |
| `c0-d40-a10` | 8 | 0 | 0.40 | 1.0 | 158,619 | 0.51 | 0.63 | 1.44 |
| `c0-d35-a10` | 8 | 0 | 0.35 | 1.0 | 166,747 | 0.53 | 0.55 | 1.46 |
| `c0-d45-a10-s3` | 3 | 0 | 0.45 | 1.0 | 144,696 | 0.47 | 0.62 | 1.74 |
| `c5-d45-a10` | 8 | 0.5 | 0.45 | 1.0 | 172,317 | 0.49 | 0.77 | 1.55 |

At diameter 0.30 the bodies leave more of the torus empty (covered 0.38–0.42, distances 2.00–2.12). At
0.45 the population, which food sets at about 160,000, cannot fit, so overlap remains however many sweeps
run.

By eye:
- **Packing.** At eight sweeps the bodies pack into confluent sheets, hexagonal up close, with sharp
  boundaries between lineages: the references' packed close-ups.
- **Coarser structure.** The structure coarsens into rounded colonies separated by black, and loses the
  single-file membranes, rings and lumens that stacked bodies drew.
- **Motion.** With full inhibition a packed colony freezes; drawn by heading, it is grey. Without
  inhibition it flows in coherent streams.

The statistic agrees on the coarsening. Its worst statistics are voids, and shape at 3.5–8.5 organisms. No
trial is as near as `contact-world` (1.27) or `tissue-world` (0.78–1.00). So no preset uses the variant yet.
The plan's rule and its tests are in place for a search that can grow fine structure out of packed bodies.

## Stage E: a finer field, fed and dispersed, `confluent-world`

### Why

Three measurements put `contact-world` far from the references at every zoom, by eye and by the corrected
statistic.
- **Organisms per field cell.** `contact-world` puts about ten organisms in each of its 128 × 128 field cells
  at tick 1,600. The references show about one, if their field is the 1,000 × 1,000 × 32 tensor the first
  screenshots describe (plan §1.1). Diffusion, sensing and emission act at the scale of cells, so every
  structure the field draws in `contact-world` spans dozens of organisms: blobs, where the references draw
  strands a few organisms wide.
- **A ceiling, not food.** All fourteen references read `pop=999,9xx`, so their worlds sit at a population
  ceiling of a million with food to spare. `contact-world` is limited by food, about 160,000 against a
  ceiling of 177,536, and its organisms crowd the sources.
- **Black.** Its corrected distance, 1.27, is worst on voids and coverage, as is every sample's in stage D.

### Trials

Each trial is `contact-world` or `tissue-world` (seed 42) with every length the organisms and the field act on
divided by a factor, 2 or 4: the grid spacing, the diffusion length (diffusion over the factor squared),
speeds and accelerations, the motion-response limit (over its square, since gradients steepen), the sensing
scales, gradient length and velocity scale, the child placement radius and the body. The motion cost per speed
squared is multiplied by the factor squared, so a move of one cell costs what it did. The torus stays 128
units unless stated, with the same founders, ceiling and food per unit area, so a finer field holds fewer
organisms per cell. The smaller tori keep the 128-unit torus's total food in fewer, stronger sources (30 on 88
units, 25 on 80), so food per unit area rises. In the contact trials children land within one body diameter of
their parent, as in `contact-world`, unless stated. "Occupied" is the share of 2-unit squares holding an
organism. The distance is `scale.py`'s against the corrected reference, from a 1,024-px raw frame of the whole
torus coloured by affinity, at the body diameter (0.25 units without a body). `confluent.json` records every
trial's changes, counts and digest.

| Trial | Change | Tick | Live | Occupied | Distance |
|---|---|---:|---:|---:|---:|
| `t2` | `tissue-world`, field 2× finer | 1,600 | 151,819 | 0.75 | 1.48 |
| `t4` | `tissue-world`, field 4× finer | 1,600 | 81,358 | 0.47 | 2.94 |
| `c4-d30` | `contact-world`, 4× finer, body 0.30 | 1,600 | 90,797 | 0.52 | 3.28 |
| `t4-cap` | `t4`, ceiling 2^18, food ×3 | 2,400 | 158,422 | 0.37 | 3.04 |
| `c4-cap` | 4× finer contact, body 0.175, ceiling and food | 2,400 | 174,755 | 0.37 | 3.56 |
| `t4-cap-wide` | `t4-cap`, sources of σ 15 | 2,400 | 239,476 | 0.52 | 2.02 |
| `c4-d25` | 4× finer contact, body 0.25, ceiling, wide food | 2,400 | 253,909 | 0.54 | 3.08 |
| `i4-d25` | `c4-d25` under contact inhibition, 8 sweeps | 2,400 | 262,144 | 0.65 | 2.75 |
| `i4-d22` | `i4-d25`, body 0.22 | 2,400 | 262,144 | 0.58 | 2.94 |
| `c4-d25-a5` | `c4-d25`, adhesion 0.5 | 2,400 | 260,221 | 0.51 | 2.98 |
| `c4-d25-k1` | `c4-d25`, stiffness 1 | 2,400 | 262,144 | 0.54 | 3.15 |
| `i4-d25-s2` | `i4-d25`, 2 sweeps | 2,400 | 261,440 | 0.51 | 3.08 |
| `c4-d30-cap` | `c4-d25`, body 0.30 | 2,400 | 226,517 | 0.49 | 2.89 |
| `w128-n393k` | `c4-d25`, ceiling 393,216 | 2,400 | 254,507 | 0.54 | 3.08 |
| `w88-n262k` | `c4-d25` on an 88-unit torus | 2,400 | 262,144 | 0.79 | 2.29 |
| `o10-c` | `c4-d25`, children within 1.0 | 2,400 | 235,065 | 0.54 | 2.67 |
| `o25-c` | `c4-d25`, children within 2.5 | 2,400 | 229,563 | 0.63 | 2.50 |
| `o10-t` | `t4-cap-wide`, children within 1.0 | 2,400 | 262,144 | 0.58 | 1.85 |
| `o10-c88` | `w88-n262k`, children within 1.0 | 2,400 | 262,144 | 0.80 | 1.74 |
| `w88-e200` | `w88-n262k`, energy sensor scale 200 | 2,400 | 262,144 | 0.79 | 2.43 |
| `w88-o25` | `w88-n262k`, children within 2.5 (and food 0.05% more, 949,267 against 948,810) | 2,400 | 262,144 | 0.89 | 1.18 |
| `w88-o25-e200` | `w88-o25`, energy sensor scale 200 | 2,400 | 262,144 | 0.92 | 1.23 |
| `w80-o25-e200` | `w88-o25-e200` on an 80-unit torus | 2,400 | 262,144 | 0.99 | 0.76 |
| `w80-o40-e200` | `w80-o25-e200`, children within 4.0 | 2,400 | 262,144 | 1.00 | 0.74 |
| `w80-o25` | `w88-o25` on an 80-unit torus: **`confluent-world`** | 2,400 | 262,144 | 0.99 | 0.74 |

What each step found:
1. **A finer field** (`t2`, `t4`, `c4-d30`) draws single-file membranes, rings and foam, but feeds fewer
   organisms: 81,000–152,000, and most of the torus black.
2. **A ceiling and more food** (`t4-cap`, `c4-cap`, `t4-cap-wide`) reach 2^18 = 262,144, but the colonies do
   not spread. The energy sensor saturates: away from organisms the field holds a median 569 energy per unit
   area against 90 under them, so the sensor reads 0.99 against 0.92 and only a colony's rim feels the food
   beyond it. Two of these 128-unit worlds then fell from the ceiling after tick 2,100, to 158,000 and
   175,000.
3. **Bodies one field cell wide** (`c4-d25`) keep `contact-world`'s look at the finer scale: labyrinths of
   strands two to four bodies thick inside each domain, a single-file rim around it and tubes between
   domains, like r13, r01 and r03. But the bodies stack: median nearest neighbour 0.20 diameters, 25 within
   one diameter, and a disk area of 0.76 of the torus covering 0.23. Packing them (`i4-d25`, the
   contact-inhibition variant at 8 sweeps: median 0.63, 6.8 within one diameter, covering 0.53) or pushing
   harder (`c4-d25-k1`), adhering less (`c4-d25-a5`) or sweeping twice (`i4-d25-s2`) smooths the labyrinths
   into featureless sheets.
4. **Filling the torus.** Colonies hold about 29 organisms per unit area, so 262,144 organisms fill about
   9,000 of a 128-unit torus's 16,384. A higher ceiling (`w128-n393k`) is never reached: food stops it at
   254,000. A smaller torus (`w88-n262k`) fills more, and placing children up to 2.5 units from their
   parent fills most of it (`w88-o25`), seeding colonies into the empty, fed space. On an 80-unit torus the
   tissue is confluent (`w80-o25`, 0.99 occupied) and the distance 0.74, as near as any sample of stages
   W to E (`w80-o40-e200` ties it; `f64-d20`, below, scores 0.73): `contact-world` scores 1.27 and
   `tissue-world` 0.78–1.00. Its worst statistics are now shapes at 2–7
   organisms, not voids. A sensor that stays linear to 200 (`e200`) does not help.

### Held-out seeds

`confluent-world` on seeds 1, 2 and 3 scores 1.57, 1.15, 0.81, against `contact-world`'s 2.55–2.80 and
`tissue-world`'s 1.82–2.39. The variant with the linear sensor scores 1.61, 1.40 on seeds 1 and 2. Seed 1
grows a finer mosaic of lineages (its worst statistic, colour coherence at 8 organisms, 4.9). Seed 2 reaches
the ceiling later, at tick 2,100 against 900, and keeps more black, but grows the most reference-like frame of
any trial by eye: rings and lace with rims, and closed loops like the references' glands.

### Controls

The preset under its model's two controls (plan §5.4), seed 42, to tick 2,400:

| Run | Distance | Live | By eye |
|---|---:|---:|---|
| `confluent-world` | 0.74 | 262,144 | lineage domains of labyrinth, foam and packed spheres |
| `field-ecology-v1-contact-chemistry-only` | 1.44 | 262,144 | a uniform mottled haze of small clumps: no domains, labyrinths or foam |
| `field-ecology-v1-contact-fresh-offspring` | 5.00 | 1,222 | sparse scatter |

So, as in `contact-world`, the domains and their textures come from emission, chosen by the networks, and
inheritance is necessary. Unlike `contact-world`'s, this chemistry-only haze fills the torus and still scores
farther than the preset, so the preset's distance is not only a filled frame.

### By eye

Against the references at zooms showing as many organisms (a reference's zoom over 1.95, since its view of a
million-organism world at zoom 1.95 holds as many as this whole torus):
- **Like the references.** The tissue fills the frame. Coloured by founder lineage, it falls into saturated
  domains of one colour, each with its own texture of worm-like labyrinth, foam or packed spheres, like r04,
  r06, r09 and r13. Close up, packed spheres in mixed clusters resemble r05.
- **Unlike them.** Lineages mix more finely at domain edges, since children land up to 2.5 units away. The
  references' largest structures, glands with a dark moat and a bright rim (r12), white lace networks (r07)
  and moats between domains (r13), are rarer here. Coloured by affinity, every lineage is teal, green or
  olive, where the references' colours are saturated and many.

### The confluent preset, and the film

`presets/confluent-world.toml` is trial `w80-o25` with its name; its run reaches the trial's digest at tick
2,400 (`3f279f51…`). Its header states each of its 22 changes from `contact-world`, and
`crates/flife-cli/tests/confluent_preset.rs` holds it to them and to one factor of four. The film shows it
from tick 2,400, where the trials were scored, to 5,600, coloured by founder lineage. The population stays at
the ceiling throughout.

### Further trials

Seven trials, seed 42 to tick 2,400, each the preset with one or two changes, aimed at its two largest gaps
from the references: no dark moats between domains, and few glands with bright rims. None is clearly nearer
than the preset (`f64-d20` ties it on the statistic and looks hazier), so it stays.

| Trial | Change from `confluent-world` | Distance | Occupied | By eye |
|---|---|---:|---:|---|
| `w80-o25` | none (the preset) | 0.74 | 0.99 | lineage domains of labyrinth, foam and packed spheres |
| `cw-k30` | toxicity sensitivity 0.08 → 0.3 | 0.75 | 0.99 | the same domains; no moats |
| `cw-k80` | toxicity sensitivity 0.08 → 0.8 | 0.75 | 1.00 | the same domains; no moats |
| `cw-o10` | children within 1.0 units, not 2.5 | 1.13 | 0.96 | black gaps return between colonies |
| `cw-o10-k30` | both | 1.14 | 0.97 | as `cw-o10` |
| `f64-d16` | field 512² (one organism per cell), every length the organisms and field act on ÷ 1.6 (body 0.156), children still within 2.5 | 1.06 | 0.99 | dimmer, hazier domains; finer specks |
| `f64-d20` | as `f64-d16`, bodies 0.20 across | 0.73 | 0.99 | as `f64-d16`, with more clumps close up |
| `f64-o10-d16` | as `f64-d16`, children within 1.0 units | 1.34 | 0.96 | hazy, with black gaps |

So raising toxicity does not draw moats here: domains meet without a gap whether toxicity sensitivity is the
preset's 0.08 or ten times that. And the field's last factor of 1.6 towards the references' one organism per
cell does not bring the image nearer; `f64-d20` ties the preset on the statistic and looks hazier.

## What this does not show

- Stages 1–2's statistic rewards fine structure that moves and fills the world. It is not a measure of
  life-likeness, and the final choice was by eye. So was stage W's.
- Stage W's distance compares image statistics, not mechanisms. The references' equations are unknown (plan
  §1.2), and a world whose frames share their spectrum and colour coherence need not work as theirs does.
  The closest sample, `c4-x1`, is still 0.76 standard deviations from the references on average, and its
  held-out seeds 1.1–2.0.
- `contact-world` is a different model from the rest, not a better setting of the same one: its organisms
  are bodies that push and stick (plan §5.4). The references' rules are unknown, and nothing here shows
  they have contact or adhesion; packing spheres is what their close-ups look like.
- Stage C's corrected statistic prefers `tissue-world` to `contact-world`; the eye chose the latter, on
  close-ups whose bodies, stage D found, stack.
- Stage D's variant packs bodies, but no trial of it is as near the references as `tissue-world`,
  `contact-world` or `confluent-world`.
- `confluent-world` is within 0.01 of the nearest sample to the corrected reference on seed 42, and nearer
  than `tissue-world` and `contact-world` on held-out seeds, but it is smaller than the references: an 80-unit torus with a 320 × 320 field and 262,144 organisms, 2.6 to a field
  cell, where the references hold a million. Its scale was matched to theirs by reading their overlays, not by
  any known equation, and it was chosen by eye on one seed. Its 128-unit relatives fell from the ceiling after
  tick 2,100; it holds there to tick 5,600, and nothing here shows it holds longer.
- `colony-world` is discrete colonies on a black field. `tissue-world` is closer to the references' continuous
  tissue on seed 42, less so on held-out seeds. In both, founder lineages narrow to a handful within 150
  simulated seconds, and the colours with them, where the references hold many colours at once.
- Nothing here shows the structure persists past the runs above: 5,600 ticks, the longest, is 175
  simulated seconds.
- Every control above is one seed.
  - In `showcase-world` they show that inheritance is necessary for the shapes and that the controllers'
    decisions are not. They do not show the controllers add nothing elsewhere, such as in which lineages win
    over longer runs.
  - In `colony-world` they show that the controllers' decisions are necessary for the colonies, not which
    decisions.
