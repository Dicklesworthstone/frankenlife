//! FL-040 evidence (plan §15.2, §7.7, §16.8): incremental metrics and
//! lineage summaries with declared sampling, bounded compute and correct
//! stable identities.
//!
//! - The tracker, fed only the published reports, holds exactly the
//!   committed world's live organisms (identity, genome, lineage,
//!   generation, birth tick, topology) at every tick, and its population,
//!   lineage, lifespan and flow figures equal ones computed directly from
//!   the world and the reports.
//! - The sampled statistics equal brute-force computations by other
//!   formulas.
//! - A run declares its metrics, emits them at every multiple and at the
//!   end, its windows partition the run, and its world is the one a run
//!   without metrics reaches.
//! - A resumed run whose session replayed its journal tracks from the state
//!   it replayed to, and agrees with the uninterrupted run.
//! - Reports that break identity are refused.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test helpers and oracles: a failure in one is a test failure, and counts here are \
              far below 2^53"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use flife_cli::metrics::{dispersion, gini, morans_i, top_decile_share};
use flife_cli::{
    ControllerChoice, Injection, METRICS_SCHEMA, Metrics, Persistence, Record, ResumeOptions,
    RunOptions, ScheduledCommand, StoreDurability, Tally, Tracked, Tracker, found, resume,
    run_with,
};
use flife_core::ids::{GenomeId, LineageId, OrganismId};
use flife_core::ledger::{Flow, Transfer};
use flife_core::{CoreError, ValidatedConfig, numerics};
use flife_engine::{Stepped, TickReport, World};
use flife_render::{Epoch, Publisher};
use flife_store::{ChunkStore, Durability};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");
const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn preset() -> ValidatedConfig {
    ValidatedConfig::from_toml(PRESET).expect("the preset is valid")
}

/// Appendix A with `extra` overrides.
fn appendix_a(extra: &[(&str, &str, &str)]) -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for &(section, key, value) in extra {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    lines.join("\n")
}

/// The FL-037 resume tests' 32 × 32 ecology of 48 founders, which breeds
/// within 240 ticks, with `durability`.
fn ecology(durability: &str) -> ValidatedConfig {
    ecology_with(durability, "0.01")
}

/// [`ecology`] with an architecture mutation at `probability` of births
/// (Appendix A: 0.01).
fn ecology_with(durability: &str, probability: &str) -> ValidatedConfig {
    ValidatedConfig::from_toml(&ecology_text(durability, probability))
        .expect("a valid configuration")
}

fn ecology_text(durability: &str, probability: &str) -> String {
    appendix_a(&[
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("controller", "evaluation_interval_ticks", "4"),
        (
            "execution",
            "command_durability",
            &format!("\"{durability}\""),
        ),
        (
            "architecture_mutation",
            "probability_per_birth",
            probability,
        ),
    ])
}

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-metrics")
        .join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    fs::create_dir_all(path.parent().expect("a parent")).expect("creates the scratch directory");
    path
}

fn step(world: &mut World) -> Box<TickReport> {
    match world.step().expect("the tick runs") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

/// What the committed world holds, organism by organism, as the tracker
/// should.
fn committed(world: &World) -> Vec<Tracked> {
    let population = world.population();
    population
        .iter()
        .map(|organism| Tracked {
            id: organism.id,
            genome: organism.genome,
            lineage: organism.lineage,
            generation: organism.generation.get(),
            birth_tick: organism.birth_tick.get(),
            topology: population
                .genome(organism.id)
                .expect("a genome")
                .topology()
                .index(),
        })
        .collect()
}

fn real(n: u64) -> f64 {
    n as f64
}

/// Lineage figures from the world alone: live members by lineage, and
/// births and deaths by lineage from the reports' identities resolved
/// against the world.
#[derive(Default)]
struct LineageOracle {
    seen: BTreeSet<u64>,
    births: BTreeMap<u64, u64>,
    deaths: BTreeMap<u64, u64>,
    max_generation: BTreeMap<u64, u32>,
}

impl LineageOracle {
    fn found(world: &World) -> Self {
        let mut oracle = Self::default();
        for organism in world.population().iter() {
            oracle.seen.insert(organism.lineage.get());
            let top = oracle
                .max_generation
                .entry(organism.lineage.get())
                .or_insert(0);
            *top = (*top).max(organism.generation.get());
        }
        oracle
    }

    fn fold(&mut self, report: &TickReport, world: &World) {
        for birth in &report.births {
            let child = world
                .population()
                .get(birth.child)
                .expect("the child lives");
            let lineage = child.lineage.get();
            self.seen.insert(lineage);
            *self.births.entry(lineage).or_insert(0) += 1;
            let top = self.max_generation.entry(lineage).or_insert(0);
            *top = (*top).max(child.generation.get());
        }
        for death in &report.deaths {
            *self.deaths.entry(death.lineage.get()).or_insert(0) += 1;
        }
    }

    fn check(&self, metrics: &Metrics, world: &World) {
        let mut live: BTreeMap<u64, u64> = BTreeMap::new();
        for organism in world.population().iter() {
            *live.entry(organism.lineage.get()).or_insert(0) += 1;
        }
        let total: u64 = live.values().sum();
        let lineages = &metrics.lineages;
        assert_eq!(lineages.alive, live.len() as u64);
        assert_eq!(lineages.extinct, (self.seen.len() - live.len()) as u64);
        let mut entropy = 0.0;
        let mut concentration = 0.0;
        for &n in live.values() {
            let share = real(n) / real(total);
            entropy -= share * numerics::ln(share).expect("a share's log");
            concentration += share * share;
        }
        assert_eq!(lineages.entropy, Some(entropy));
        assert_eq!(lineages.effective, numerics::exp(entropy));
        assert_eq!(lineages.inverse_simpson, Some(1.0 / concentration));
        let mut rows: Vec<(u64, u64)> = live.iter().map(|(&l, &n)| (l, n)).collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        assert_eq!(lineages.dominant_share, Some(real(rows[0].1) / real(total)));
        let largest: Vec<_> = lineages
            .largest
            .iter()
            .map(|row| {
                (
                    (row.lineage, row.live),
                    (row.births, row.deaths, row.max_generation),
                )
            })
            .collect();
        let expected: Vec<_> = rows
            .iter()
            .take(flife_cli::metrics::TOP_LINEAGES)
            .map(|&(lineage, n)| {
                (
                    (lineage, n),
                    (
                        self.births.get(&lineage).copied().unwrap_or(0),
                        self.deaths.get(&lineage).copied().unwrap_or(0),
                        self.max_generation[&lineage],
                    ),
                )
            })
            .collect();
        assert_eq!(largest, expected);
    }
}

/// Population figures from the world alone.
fn check_population(metrics: &Metrics, world: &World) {
    let population = world.population();
    let mut topologies = BTreeMap::new();
    let mut generations = 0_u64;
    let mut max_generation = 0;
    for organism in population.iter() {
        let topology = population
            .genome(organism.id)
            .expect("a genome")
            .topology()
            .index();
        *topologies.entry(topology).or_insert(0_u64) += 1;
        generations += u64::from(organism.generation.get());
        max_generation = max_generation.max(organism.generation.get());
    }
    let live = population.len() as u64;
    let p = &metrics.population;
    assert_eq!(p.live, live);
    assert_eq!(p.capacity, population.capacity() as u64);
    assert_eq!(p.occupancy, real(live) / real(population.capacity() as u64));
    assert_eq!(
        Some(p.next_identity),
        world.identities().organisms.next_raw()
    );
    assert_eq!(p.max_generation, max_generation);
    assert_eq!(p.mean_generation, Some(real(generations) / real(live)));
    assert_eq!(p.topologies, topologies);
}

/// Lifespans of `report`'s deaths, from the birth ticks in `before`, the
/// world it advanced from.
fn lifespans(report: &TickReport, before: &BTreeMap<OrganismId, u64>) -> Vec<u64> {
    report
        .deaths
        .iter()
        .map(|death| death.tick.get() - before[&death.id])
        .collect()
}

/// The Gini coefficient by the Lorenz curve: `1 + 1/n − (2/n) Σ L_i`.
fn gini_by_lorenz(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let total: f64 = sorted.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let n = sorted.len() as f64;
    let mut cumulative = 0.0;
    let mut area = 0.0;
    for value in &sorted {
        cumulative += value;
        area += cumulative / total;
    }
    Some(1.0 + 1.0 / n - 2.0 * area / n)
}

/// The Gini coefficient by mean absolute difference, `O(n²)`.
fn gini_by_differences(values: &[f64]) -> Option<f64> {
    let n = values.len() as f64;
    let total: f64 = values.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut differences = 0.0;
    for a in values {
        for b in values {
            differences += (a - b).abs();
        }
    }
    Some(differences / (2.0 * n * total))
}

/// Moran's I from an explicit list of rook neighbour pairs on the torus,
/// `(n / W) Σ w_ij z_i z_j / Σ z_i²`.
fn morans_i_by_pairs(values: &[f64], width: usize, height: usize) -> Option<f64> {
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    let mut pairs = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            for (dx, dy) in [(1, 0), (width - 1, 0), (0, 1), (0, height - 1)] {
                pairs.push((i, ((y + dy) % height) * width + (x + dx) % width));
            }
        }
    }
    let variance: f64 = values.iter().map(|v| (v - mean) * (v - mean)).sum();
    if variance == 0.0 {
        return None;
    }
    let cross: f64 = pairs
        .iter()
        .map(|&(i, j)| (values[i] - mean) * (values[j] - mean))
        .sum();
    Some(n as f64 / pairs.len() as f64 * cross / variance)
}

fn close(a: Option<f64>, b: Option<f64>, what: &str) {
    match (a, b) {
        (Some(a), Some(b)) => assert!(
            (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0),
            "{what}: {a} vs {b}"
        ),
        (a, b) => assert_eq!(a, b, "{what}"),
    }
}

/// The sampled statistics of `epoch` against brute-force computations by
/// other formulas.
fn check_sampled(metrics: &Metrics, epoch: &Epoch) {
    let sampled = &metrics.sampled;
    assert_eq!(sampled.source_tick, metrics.tick);
    assert_eq!(sampled.lag, 0);
    // Motility.
    let speeds: Vec<f64> = epoch
        .velocities()
        .iter()
        .map(|v| (f64::from(v[0]).powi(2) + f64::from(v[1]).powi(2)).sqrt())
        .collect();
    let n = speeds.len() as f64;
    close(
        sampled.motility.mean_speed,
        Some(speeds.iter().sum::<f64>() / n),
        "mean speed",
    );
    close(
        sampled.motility.top_speed,
        speeds.iter().copied().reduce(f64::max),
        "top speed",
    );
    // Dispersion over the bins, counted naively.
    let bins = flife_cli::metrics::BINS;
    let extent = epoch.extent();
    let mut counts = vec![0_u64; bins * bins];
    for position in epoch.positions() {
        let x = ((f64::from(position[0]) / extent[0]) * bins as f64).floor() as usize;
        let y = ((f64::from(position[1]) / extent[1]) * bins as f64).floor() as usize;
        counts[y.min(bins - 1) * bins + x.min(bins - 1)] += 1;
    }
    let total: u64 = counts.iter().sum();
    assert_eq!(total, epoch.len() as u64);
    let q = counts.len() as f64;
    let mean = total as f64 / q;
    let variance = counts
        .iter()
        .map(|&c| (c as f64 - mean).powi(2))
        .sum::<f64>()
        / (q - 1.0);
    let d = &sampled.dispersion;
    close(
        d.variance_to_mean,
        Some(variance / mean),
        "variance to mean",
    );
    let pairs: u64 = counts.iter().map(|&c| c * c.saturating_sub(1)).sum();
    close(
        d.morisita,
        Some(q * pairs as f64 / (total as f64 * (total as f64 - 1.0))),
        "morisita",
    );
    assert_eq!(
        d.occupied,
        counts.iter().filter(|&&c| c > 0).count() as f64 / q
    );
    let values: Vec<f64> = counts.iter().map(|&c| c as f64).collect();
    close(
        d.morans_i,
        morans_i_by_pairs(&values, bins, bins),
        "organism moran",
    );
    // Resources.
    let texture = epoch.field();
    let texels = texture.texels().map(|t| t as usize);
    let values: Vec<f64> = texture.values().iter().map(|&v| f64::from(v)).collect();
    let r = &sampled.resources;
    assert_eq!(r.texels, texture.texels());
    close(r.gini, gini_by_lorenz(&values), "field gini");
    close(
        r.morans_i,
        morans_i_by_pairs(&values, texels[0], texels[1]),
        "field moran",
    );
    let mut sorted = values.clone();
    sorted.sort_by(f64::total_cmp);
    let top = sorted.len().div_ceil(10);
    let sum: f64 = sorted.iter().sum();
    close(
        r.top_decile_share,
        Some(sorted.iter().rev().take(top).sum::<f64>() / sum),
        "top decile",
    );
}

/// Power-of-two lifespan buckets, by repeated doubling.
fn histogram(spans: &[u64]) -> BTreeMap<u64, u64> {
    let mut histogram = BTreeMap::new();
    for &span in spans {
        let mut bucket = 0;
        if span > 0 {
            bucket = 1;
            while bucket * 2 <= span {
                bucket *= 2;
            }
        }
        *histogram.entry(bucket).or_insert(0) += 1;
    }
    histogram
}

/// A multi-tick window built from the reports themselves, summed in tick
/// order.
#[derive(Default)]
struct ExpectedWindow {
    from: u64,
    tally: Tally,
    attempted: u64,
    flows: BTreeMap<&'static str, f64>,
    transfers: BTreeMap<&'static str, f64>,
    injected: f64,
    removed: f64,
    dissipated: f64,
    requested: f64,
    granted: f64,
    contended: u64,
    drained: u64,
    largest: u64,
    spans: Vec<u64>,
    extinctions: u64,
}

impl ExpectedWindow {
    fn fold(&mut self, report: &TickReport, spans: &[u64], extinctions: u64) {
        self.tally.record(report);
        self.attempted += report.attempted as u64;
        for flow in Flow::ALL {
            *self.flows.entry(flow.name()).or_insert(0.0) += report.closure.flow(flow);
        }
        for transfer in Transfer::ALL {
            *self.transfers.entry(transfer.name()).or_insert(0.0) +=
                report.closure.credited(transfer);
        }
        self.injected += report.closure.injected();
        self.removed += report.closure.removed();
        self.dissipated += report.closure.dissipated();
        self.requested += report.uptake.requested;
        self.granted += report.uptake.granted;
        self.contended += report.uptake.contended_cells as u64;
        self.drained += report.uptake.drained_cells as u64;
        self.largest = self.largest.max(report.uptake.largest_cell_packets as u64);
        self.spans.extend_from_slice(spans);
        self.extinctions += extinctions;
    }

    fn check(&self, metrics: &Metrics) {
        let window = &metrics.window;
        assert_eq!((window.from, window.to), (self.from, metrics.tick));
        assert_eq!(window.tally, self.tally);
        assert_eq!(window.attempted_births, self.attempted);
        assert_eq!(window.flows, self.flows);
        assert_eq!(window.transfers, self.transfers);
        assert_eq!(
            (window.injected, window.removed, window.dissipated),
            (self.injected, self.removed, self.dissipated)
        );
        let uptake = &window.uptake;
        assert_eq!(
            (uptake.requested, uptake.granted),
            (self.requested, self.granted)
        );
        assert_eq!(
            (
                uptake.contended_cell_ticks,
                uptake.drained_cell_ticks,
                uptake.largest_cell_packets
            ),
            (self.contended, self.drained, self.largest)
        );
        let lifespans = &window.lifespans;
        assert_eq!(lifespans.count, self.spans.len() as u64);
        assert_eq!(lifespans.total, self.spans.iter().sum::<u64>());
        assert_eq!(lifespans.shortest, self.spans.iter().copied().min());
        assert_eq!(lifespans.longest, self.spans.iter().copied().max());
        assert_eq!(lifespans.histogram, histogram(&self.spans));
        assert_eq!(window.extinctions, self.extinctions);
    }
}

/// The coarse tracker's interval in the oracle.
const COARSE: u64 = 64;

/// What an oracle run exercised.
struct Exercised {
    descendant_deaths: usize,
    extinctions: u64,
    topology_changes: u64,
}

/// The oracle: `config`'s world stepped directly for `ticks`, the tracker
/// fed only its reports and sampled at every tick, compared with the world
/// at every tick; and a second tracker sampled every [`COARSE`] ticks,
/// whose windows are compared with sums over the reports.
fn oracle(config: &ValidatedConfig, ticks: u64) -> Exercised {
    let mut world = found(config).expect("founds");
    let mut tracker = Tracker::new(&world, 1).expect("a tracker");
    let mut coarse = Tracker::new(&world, COARSE).expect("a tracker");
    let mut expected = ExpectedWindow::default();
    assert_eq!(tracker.organisms().collect::<Vec<_>>(), committed(&world));
    let mut publisher = Publisher::new(&world, 3, || {}).expect("a publisher");
    let viewer = publisher.feed().viewer(1).expect("a viewer");
    let mut lineages = LineageOracle::found(&world);
    let (mut descendant_deaths, mut extinctions, mut topology_changes) = (0, 0, 0);
    for _ in 0..ticks {
        let before: BTreeMap<OrganismId, u64> = world
            .population()
            .iter()
            .map(|organism| (organism.id, organism.birth_tick.get()))
            .collect();
        let report = step(&mut world);
        tracker.record(&report).expect("the tracker follows");
        coarse.record(&report).expect("the coarse tracker follows");
        assert_eq!(
            coarse.due(report.tick.get()),
            report.tick.get().is_multiple_of(COARSE)
        );
        assert_eq!(
            tracker.organisms().collect::<Vec<_>>(),
            committed(&world),
            "tick {}",
            report.tick.get()
        );
        lineages.fold(&report, &world);
        publisher.offer(&world).expect("publishes");
        let epoch = viewer.latest().expect("a lease");
        let metrics = tracker.sample(&epoch).expect("samples");
        assert_eq!(metrics.tick, report.tick.get());
        // The window is this one tick.
        let window = &metrics.window;
        assert_eq!((window.from, window.to), (report.from.get(), metrics.tick));
        let mut tally = Tally::default();
        tally.record(&report);
        assert_eq!(window.tally, tally);
        for flow in Flow::ALL {
            assert_eq!(window.flows[flow.name()], report.closure.flow(flow));
        }
        for transfer in Transfer::ALL {
            assert_eq!(
                window.transfers[transfer.name()],
                report.closure.credited(transfer)
            );
        }
        assert_eq!(
            window.holdings.field + window.holdings.organisms,
            report.closure.closing().total()
        );
        assert_eq!(window.attempted_births, report.attempted as u64);
        assert_eq!(window.injected, report.closure.injected());
        assert_eq!(window.removed, report.closure.removed());
        assert_eq!(window.dissipated, report.closure.dissipated());
        assert_eq!(window.uptake.requested, report.uptake.requested);
        assert_eq!(window.uptake.granted, report.uptake.granted);
        assert_eq!(
            window.uptake.contended_cell_ticks,
            report.uptake.contended_cells as u64
        );
        assert_eq!(
            window.uptake.drained_cell_ticks,
            report.uptake.drained_cells as u64
        );
        assert_eq!(
            window.uptake.largest_cell_packets,
            report.uptake.largest_cell_packets as u64
        );
        let spans = lifespans(&report, &before);
        assert_eq!(window.lifespans.count, spans.len() as u64);
        assert_eq!(window.lifespans.total, spans.iter().sum::<u64>());
        assert_eq!(window.lifespans.shortest, spans.iter().copied().min());
        assert_eq!(window.lifespans.longest, spans.iter().copied().max());
        assert_eq!(window.lifespans.histogram, histogram(&spans));
        expected.fold(&report, &spans, window.extinctions);
        if coarse.due(metrics.tick) {
            let sampled = coarse.sample(&epoch).expect("samples");
            expected.check(&sampled);
            assert_eq!(sampled.population, metrics.population);
            assert_eq!(sampled.lineages, metrics.lineages);
            assert_eq!(sampled.sampled, metrics.sampled);
            expected = ExpectedWindow {
                from: metrics.tick,
                ..ExpectedWindow::default()
            };
        }
        descendant_deaths += report
            .deaths
            .iter()
            .filter(|death| before[&death.id] > 0)
            .count();
        extinctions += window.extinctions;
        topology_changes += tally.topology_changes;
        check_population(&metrics, &world);
        lineages.check(&metrics, &world);
        if metrics.tick.is_multiple_of(160) {
            check_sampled(&metrics, &epoch);
        }
    }
    eprintln!(
        "{ticks} ticks: {} live, {descendant_deaths} deaths of organisms born in the run, \
         {extinctions} extinctions, {topology_changes} topology changes",
        world.population().len()
    );
    Exercised {
        descendant_deaths,
        extinctions,
        topology_changes,
    }
}

/// The oracle over the ecology with architecture mutation at a quarter of
/// births, where children change topology, organisms born in the run die
/// and lineages go extinct; and over the 4,096-founder preset's first 320
/// ticks, where founders die in thousands.
#[test]
fn the_tracker_holds_the_committed_world_at_every_tick() {
    let ecology = oracle(&ecology_with("volatile-in-memory", "0.25"), 1_200);
    assert!(
        ecology.descendant_deaths > 0,
        "organisms born in the run die"
    );
    assert!(ecology.extinctions > 0, "lineages go extinct");
    assert!(ecology.topology_changes > 0, "children change topology");
    let preset = oracle(&preset(), 320);
    assert!(preset.extinctions > 1_000, "founder lineages die out");
}

/// The sampled statistics on known patterns, and against the brute-force
/// formulas on deterministic pseudo-random values.
#[test]
fn the_spatial_statistics_are_the_textbook_ones() {
    // A checkerboard is perfectly negatively autocorrelated, stripes along
    // x are uncorrelated (two like and two unlike neighbours), a constant
    // has no variance.
    let checkerboard: Vec<f64> = (0..64).map(|i| f64::from((i % 8 + i / 8) % 2)).collect();
    assert_eq!(morans_i(&checkerboard, [8, 8]), Some(-1.0));
    let stripes: Vec<f64> = (0..64).map(|i| f64::from(i % 2)).collect();
    assert_eq!(morans_i(&stripes, [8, 8]), Some(0.0));
    assert_eq!(morans_i(&[3.0; 64], [8, 8]), None);
    // One unit among n: Gini (n−1)/n, all of it in the top decile.
    let mut lone = vec![0.0; 50];
    lone[17] = 1.0;
    assert_eq!(gini(&lone), Some(49.0 / 50.0));
    assert_eq!(top_decile_share(&lone), Some(1.0));
    assert_eq!(gini(&[2.0; 10]), Some(0.0));
    assert_eq!(gini(&[0.0; 10]), None);
    // Pseudo-random values against the other formulas.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1_u64 << 53) as f64
    };
    for (width, height) in [(3, 3), (5, 7), (16, 16), (31, 9)] {
        let values: Vec<f64> = (0..width * height).map(|_| next().powi(3)).collect();
        close(
            morans_i(&values, [width, height]),
            morans_i_by_pairs(&values, width, height),
            "moran",
        );
        close(gini(&values), gini_by_differences(&values), "gini");
        close(gini(&values), gini_by_lorenz(&values), "gini");
    }
    // All organisms in one bin: Morisita Q; one per bin: 0, and no
    // variance; a position at the extent wraps to the first bin.
    let extent = [32.0, 32.0];
    let clumped = vec![[1.0_f32, 1.0]; 40];
    let d = dispersion(&clumped, extent, 4);
    assert_eq!(d.morisita, Some(16.0));
    assert_eq!(d.occupied, 1.0 / 16.0);
    let even: Vec<[f32; 2]> = (0..16)
        .map(|i| [4.0 + 8.0 * (i % 4) as f32, 4.0 + 8.0 * (i / 4) as f32])
        .collect();
    let d = dispersion(&even, extent, 4);
    assert_eq!(d.morisita, Some(0.0));
    assert_eq!(d.variance_to_mean, Some(0.0));
    assert_eq!(d.occupied, 1.0);
    assert_eq!(d.morans_i, None);
    let edge = dispersion(&[[32.0, 0.0], [0.0, 0.0]], extent, 4);
    assert_eq!(edge.occupied, 1.0 / 16.0);
    assert_eq!(edge.morisita, Some(16.0));
}

fn collect(records: &Arc<Mutex<Vec<Record>>>) -> Vec<Record> {
    records.lock().expect("records").clone()
}

fn metrics_of(records: &[Record]) -> Vec<Metrics> {
    records
        .iter()
        .filter_map(|record| match record {
            Record::Metrics(metrics) => Some((**metrics).clone()),
            _ => None,
        })
        .collect()
}

fn run_ecology(ticks: u64, metrics_every: u64) -> (flife_cli::Finished, Vec<Record>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let finished = run_with(
        &ecology("volatile-in-memory"),
        RunOptions {
            ticks,
            every: 0,
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
            metrics_every,
        },
        &[],
        None,
        move |record| sink.lock().expect("records").push(record.clone()),
    )
    .expect("the run runs");
    (finished, collect(&records))
}

/// A run declares its metrics in its start record, emits one at every
/// multiple of the interval and one at the end for the remainder; the
/// windows partition the run, so their births, deaths and lifespans add up
/// to the run's tally; the last one agrees with the end survey; and the
/// world is the one a run without metrics reaches.
#[test]
fn a_run_declares_its_metrics_and_its_windows_partition_it() {
    let (finished, records) = run_ecology(400, 96);
    let Record::Start { metrics, .. } = &records[0] else {
        panic!("a start record first: {:?}", records[0]);
    };
    let declaration = metrics.as_ref().expect("declared");
    assert_eq!(declaration.schema, METRICS_SCHEMA);
    assert_eq!(declaration.every, 96);
    let start = serde_json::to_value(&records[0]).expect("serializes");
    assert!(start["metrics"]["complete"].is_string());
    assert!(start["metrics"]["sampled"].is_string());
    let all = metrics_of(&records);
    let ticks: Vec<u64> = all.iter().map(|m| m.tick).collect();
    assert_eq!(ticks, [96, 192, 288, 384, 400]);
    let mut from = 0;
    let (mut births, mut deaths, mut rejected, mut spans) = (0, 0, 0, 0);
    for metrics in &all {
        assert_eq!(metrics.window.from, from);
        assert_eq!(metrics.window.to, metrics.tick);
        assert_eq!(metrics.window.tally.ticks, metrics.tick - from);
        assert_eq!(metrics.sampled.source_tick, metrics.tick);
        assert_eq!(metrics.sampled.lag, 0);
        births += metrics.window.tally.births;
        deaths += metrics.window.tally.deaths;
        rejected += metrics.window.tally.rejected_births;
        spans += metrics.window.lifespans.count;
        from = metrics.tick;
        let line =
            serde_json::to_value(Record::Metrics(Box::new(metrics.clone()))).expect("serializes");
        assert_eq!(line["record"], "metrics");
        assert_eq!(line["tick"], metrics.tick);
    }
    assert!(finished.tally.births > 0 && finished.tally.deaths > 0);
    assert_eq!(births, finished.tally.births);
    assert_eq!(deaths, finished.tally.deaths);
    assert_eq!(spans, finished.tally.deaths);
    assert_eq!(rejected, finished.tally.rejected_births);
    let last = all.last().expect("a record");
    let survey = &finished.survey;
    assert_eq!(last.population.live, survey.live as u64);
    assert_eq!(last.population.max_generation, survey.max_generation);
    assert_eq!(last.lineages.alive, survey.lineages_alive as u64);
    let topologies: BTreeMap<usize, u64> = survey
        .topologies
        .iter()
        .map(|(&t, &n)| (t, n as u64))
        .collect();
    assert_eq!(last.population.topologies, topologies);
    assert_eq!(last.population.next_identity, survey.identities_issued);

    let (plain, records) = run_ecology(400, 0);
    let Record::Start { metrics, .. } = &records[0] else {
        panic!("a start record first");
    };
    assert_eq!(*metrics, None);
    let start = serde_json::to_value(&records[0]).expect("serializes");
    assert!(start.get("metrics").is_none(), "{start}");
    assert!(metrics_of(&records).is_empty());
    assert_eq!(plain.closed.world.digest(), finished.closed.world.digest());
}

/// A write-ahead run stopped at tick 152 whose newest checkpoint is damaged
/// resumes from tick 147 and replays its journal past it, so the session
/// starts at a tick the tracker never saw. The tracker starts again from a
/// checkpoint of that state, and its records then agree with the
/// uninterrupted run's: every population, lineage-share and sampled figure,
/// and every window that starts after the resume.
#[test]
fn a_resumed_run_tracks_from_the_state_its_journal_replayed_to() {
    let config = ecology("write-ahead-before-apply");
    let injections: Vec<ScheduledCommand> =
        ["3:5,6:2.0", "66:9,2:3.0", "150:20,11:4.0", "230:1,30:1.0"]
            .iter()
            .map(|text| ScheduledCommand::from(text.parse::<Injection>().expect("an injection")))
            .collect();
    let persistence = |run_dir: &Path| {
        let mut persistence = Persistence::new(run_dir, &config);
        persistence.checkpoint_every = 7;
        persistence.durability = StoreDurability::ProcessCrash;
        persistence
    };
    let options = |ticks, metrics_every| RunOptions {
        ticks,
        every: 0,
        cpu_workers: None,
        controllers: ControllerChoice::Cpu,
        metrics_every,
    };
    let collect_run = |run_dir: &Path, ticks, metrics_every| {
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&records);
        run_with(
            &config,
            options(ticks, metrics_every),
            &injections,
            Some(&persistence(run_dir)),
            move |record| sink.lock().expect("records").push(record.clone()),
        )
        .expect("the run runs");
        collect(&records)
    };

    let reference = metrics_of(&collect_run(&scratch("uninterrupted"), 240, 16));
    let run_dir = scratch("damaged-newest");
    let first = collect_run(&run_dir, 152, 0);
    let newest = first
        .iter()
        .find_map(|record| match record {
            Record::Checkpoint {
                tick: 152,
                sequence,
                ..
            } => Some(*sequence),
            _ => None,
        })
        .expect("a checkpoint at 152");
    let store =
        ChunkStore::open(run_dir.join("store"), Durability::ProcessCrash).expect("the store opens");
    fs::write(store.manifest_path(newest), b"damaged").expect("damages the manifest");
    drop(store);

    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    resume(
        &run_dir,
        &ResumeOptions {
            checkpoint_mode: None,
            retained_bytes: None,
            until: 240,
            every: 0,
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
            checkpoint_every: Some(7),
            durability: StoreDurability::ProcessCrash,
            metrics_every: 16,
        },
        move |record| sink.lock().expect("records").push(record.clone()),
    )
    .expect("resumes");
    let records = collect(&records);
    let resumed_at = records
        .iter()
        .find_map(|record| match record {
            Record::Resume {
                checkpoint, tick, ..
            } => {
                assert_eq!(checkpoint.map(|at| at.tick), Some(147));
                Some(*tick)
            }
            _ => None,
        })
        .expect("a resume record");
    assert!(
        resumed_at >= 150,
        "the journal replayed past 147: {resumed_at}"
    );
    let resumed = metrics_of(&records);
    assert_eq!(
        resumed[0].window.from, resumed_at,
        "the tracker restarted there"
    );
    let ticks: Vec<u64> = resumed.iter().map(|m| m.tick).collect();
    assert_eq!(ticks, [160, 176, 192, 208, 224, 240]);
    for metrics in &resumed {
        let expected = reference
            .iter()
            .find(|m| m.tick == metrics.tick)
            .expect("the reference has the tick");
        assert_eq!(metrics.population, expected.population);
        let lineages = &metrics.lineages;
        assert_eq!(lineages.alive, expected.lineages.alive);
        assert_eq!(lineages.entropy, expected.lineages.entropy);
        assert_eq!(lineages.inverse_simpson, expected.lineages.inverse_simpson);
        assert_eq!(lineages.dominant_share, expected.lineages.dominant_share);
        let shares = |m: &Metrics| -> Vec<(u64, u64)> {
            m.lineages
                .largest
                .iter()
                .map(|row| (row.lineage, row.live))
                .collect()
        };
        assert_eq!(shares(metrics), shares(expected));
        assert_eq!(metrics.sampled.motility, expected.sampled.motility);
        assert_eq!(metrics.sampled.dispersion, expected.sampled.dispersion);
        assert_eq!(metrics.sampled.resources, expected.sampled.resources);
        if metrics.window.from > resumed_at {
            let (mut window, mut reference_window) =
                (metrics.window.clone(), expected.window.clone());
            window.record_seconds = 0.0;
            reference_window.record_seconds = 0.0;
            assert_eq!(window, reference_window, "tick {}", metrics.tick);
        }
    }
}

/// Reports that break identity are refused: one that does not advance
/// from the tracker's tick, a child of an unknown parent or with an
/// identity already issued, a death of an unknown organism or under
/// another genome, lineage or generation, and a live count that differs.
#[test]
fn the_tracker_refuses_reports_that_break_identity() {
    let mut world = found(&ecology_with("volatile-in-memory", "0.25")).expect("founds");
    let mut tracker = Tracker::new(&world, 1).expect("a tracker");
    let mut topology_refused = false;
    let report = loop {
        let report = step(&mut world);
        if !topology_refused
            && let Some(at) = report
                .births
                .iter()
                .position(|birth| birth.record.topology.is_some())
        {
            refused(
                &tracker,
                &report,
                (
                    "a topology change from another topology",
                    "topology change starts at",
                ),
                &|r: &mut TickReport| {
                    let change = r.births[at].record.topology.as_mut().expect("a change");
                    change.from = change.to;
                },
            );
            topology_refused = true;
        }
        if topology_refused && !report.births.is_empty() && !report.deaths.is_empty() {
            break report;
        }
        tracker.record(&report).expect("follows");
        assert!(world.tick().get() < 3_000, "a tick with births and deaths");
    };
    let refused = |what: (&str, &str), tamper: &dyn Fn(&mut TickReport)| {
        refused(&tracker, &report, what, tamper);
    };
    let stranger = OrganismId::new(u64::MAX - 7);
    let live: BTreeSet<OrganismId> = tracker.organisms().map(|o| o.id).collect();
    let elder = *live.first().expect("a live organism");
    // An identity issued to an organism that has since died: only the
    // order of identities shows it was issued before.
    let dead = (0..report.births[0].child.get())
        .map(OrganismId::new)
        .find(|id| !live.contains(id))
        .expect("an organism has died");
    let issued = "not above every identity issued";
    refused(("a report from another tick", "advances from tick"), &|r| {
        r.from = flife_core::Tick::new(r.from.get() + 1);
    });
    refused(("an unknown parent", "is not alive"), &|r| {
        r.births[0].parent = stranger;
    });
    refused(("the identity of a live organism", issued), &|r| {
        r.births[0].child = elder;
    });
    refused(("the identity of a dead organism", issued), &|r| {
        r.births[0].child = dead;
    });
    refused(("an unknown death", "died but is not alive"), &|r| {
        r.deaths[0].id = stranger;
    });
    refused(("another genome", "died as genome"), &|r| {
        r.deaths[0].genome = GenomeId::new(r.deaths[0].genome.get() + 1);
    });
    refused(("another lineage", "died as genome"), &|r| {
        r.deaths[0].lineage = LineageId::new(r.deaths[0].lineage.get() + 1);
    });
    refused(("another generation", "died as genome"), &|r| {
        r.deaths[0].generation = flife_core::ids::Generation::new(r.deaths[0].generation.get() + 1);
    });
    refused(("another live count", "organisms tracked at tick"), &|r| {
        r.live += 1
    });
    // The untampered report is followed.
    tracker.record(&report).expect("follows");
    assert_eq!(tracker.organisms().collect::<Vec<_>>(), committed(&world));
}

/// `tracker` refuses `report` once `tamper`ed, as a metrics state conflict
/// whose detail names the check: `what` is the case and that fragment.
fn refused(
    tracker: &Tracker,
    report: &TickReport,
    (what, check): (&str, &str),
    tamper: &dyn Fn(&mut TickReport),
) {
    let mut tampered = report.clone();
    tamper(&mut tampered);
    let error = tracker.clone().record(&tampered).expect_err(what);
    assert_refused(&error, what, check);
}

fn assert_refused(error: &CoreError, what: &str, check: &str) {
    assert!(
        matches!(
            error,
            CoreError::StateConflict {
                operation: "track run metrics",
                detail,
            } if detail.contains(check)
        ),
        "{what}: refused by `{check}`, not {error:?}"
    );
    eprintln!("{what}: {error}");
}

/// A sample is checked against its render epoch: an epoch of another world
/// at the tracker's tick (other organisms) and an epoch ahead of the
/// tracker are refused; an epoch behind it is summarized with its lag,
/// unchecked.
#[test]
fn a_sample_refuses_an_epoch_of_another_world() {
    let text = ecology_text("volatile-in-memory", "0.01");
    assert_eq!(text.matches("seed = 42").count(), 1);
    let config = ValidatedConfig::from_toml(&text).expect("valid");
    let other =
        ValidatedConfig::from_toml(&text.replacen("seed = 42", "seed = 7", 1)).expect("valid");
    let epoch_of = |world: &World| {
        let publisher = Publisher::new(world, 3, || {}).expect("a publisher");
        let viewer = publisher.feed().viewer(1).expect("a viewer");
        let epoch = viewer.latest().expect("a lease");
        (*epoch).clone()
    };
    let refusal = |tracker: &Tracker, epoch: &Epoch, (what, check): (&str, &str)| {
        let error = tracker.clone().sample(epoch).expect_err(what);
        assert_refused(&error, what, check);
    };
    let mut world = found(&config).expect("founds");
    let mut tracker = Tracker::new(&world, 1).expect("a tracker");
    refusal(
        &tracker,
        &epoch_of(&found(&preset()).expect("founds")),
        ("the preset's founders", "organisms, the tracker"),
    );
    let founding = epoch_of(&world);
    let mut seven = found(&other).expect("founds");
    let rows = |world: &World| -> Vec<(u64, u64, u32)> {
        world
            .population()
            .iter()
            .map(|o| (o.id.get(), o.lineage.get(), o.generation.get()))
            .collect()
    };
    // The first tick past the founding where the two worlds hold as many
    // organisms but not the same ones: refused row by row, not by count.
    let mut tick = 0;
    loop {
        tracker.record(&step(&mut world)).expect("follows");
        let _ = step(&mut seven);
        tick += 1;
        if world.population().len() == seven.population().len() && rows(&seven) != rows(&world) {
            break;
        }
        assert!(tick < 2_000, "the two worlds once hold as many organisms");
    }
    refusal(
        &tracker,
        &epoch_of(&seven),
        (
            "as many organisms at the same tick, not the same ones",
            "where the tracker has",
        ),
    );
    let _ = step(&mut seven);
    refusal(
        &tracker,
        &epoch_of(&seven),
        ("an epoch ahead", "after the tracker's"),
    );
    let behind = tracker.clone().sample(&founding).expect("summarized");
    assert_eq!(
        (behind.tick, behind.sampled.source_tick, behind.sampled.lag),
        (tick, 0, tick)
    );
    tracker.sample(&epoch_of(&world)).expect("its own epoch");
}
