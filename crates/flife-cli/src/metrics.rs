//! Incremental metrics and lineage summaries (plan §15.2, §7.7, §16.8;
//! FL-040).
//!
//! A [`Tracker`] starts from one committed population and then folds every
//! published tick's report, so it never scans the population again: per
//! tick it does work in proportion to that tick's births and deaths. It
//! keeps each live organism's stable identity, genome, founder lineage,
//! generation, birth tick and topology, and per lineage its live count,
//! births, deaths and extinction tick. Every death is checked against what
//! it tracked (genome, lineage and generation), every child must carry a
//! fresh identity above every one issued before it, and the live count must
//! equal the report's, so a wrong identity stops the run instead of being
//! summarized.
//!
//! Every `every` committed ticks, and at the end of a run when ticks remain,
//! [`Tracker::sample`] emits one [`Metrics`] record:
//!
//! - **Complete** (every published tick, nothing skipped): the window since
//!   the last record (births, deaths, mutation acceptance, energy flows and
//!   transfers, uptake contention, lifespans of the organisms that died),
//!   the live population (capacity pressure, generations, the architecture
//!   distribution) and the lineages (live and extinct, Shannon entropy and
//!   effective number, inverse Simpson, the largest lineages).
//! - **Sampled**: computed from one render epoch, the immutable compact
//!   projection the session publishes at each committed tick (F32
//!   positions and velocities, and the energy-density texture averaged over
//!   blocks, at most [`flife_render::FIELD_TEXELS_MAX`] texels a side): motility,
//!   dispersion of organisms over a [`BINS`] × [`BINS`] torus grid
//!   (occupied fraction, variance-to-mean ratio, Morisita's index, Moran's
//!   I) and resource concentration (Gini, top-decile share, Moran's I). The
//!   sample's own cost is bounded by the epoch: O(live + texels · log
//!   texels). The epoch is also checked against the tracker: at the same
//!   tick they must list the same identities with the same lineages,
//!   generations and topologies.
//!
//! The scientific content depends only on the configuration, the commands
//! and the ticks; `record_seconds` and `sample_seconds` are wall-clock
//! costs, not scientific state. Every sum is in tick, identity, lineage or
//! cell order, and the entropy uses the host-independent
//! [`numerics::ln`] and [`numerics::exp`], so a record is identical on every
//! qualified host.

use std::collections::BTreeMap;
use std::time::Instant;

use flife_core::CoreError;
use flife_core::ids::{GenomeId, LineageId, OrganismId};
use flife_core::ledger::{Flow, Transfer};
use flife_core::numerics;
use flife_engine::{TickReport, World};
use flife_render::Epoch;
use serde::Serialize;

use crate::observe::Tally;

/// The version of the metrics records and their declaration.
pub const METRICS_SCHEMA: &str = "frankenlife.metrics.v1";

/// Bins a side of the dispersion grid.
pub const BINS: usize = 16;

/// Lineages listed in each record, largest first.
pub const TOP_LINEAGES: usize = 8;

/// What a run's metrics records hold and how each part is computed,
/// declared in its start record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Declaration {
    /// [`METRICS_SCHEMA`].
    pub schema: &'static str,
    /// A record at every multiple of this committed tick, and at the end
    /// when ticks remain.
    pub every: u64,
    /// The complete parts and their rule.
    pub complete: &'static str,
    /// The sampled parts and their rule.
    pub sampled: &'static str,
    /// Bins a side of the dispersion grid.
    pub bins: usize,
    /// Lineages listed per record.
    pub top_lineages: usize,
}

impl Declaration {
    /// The declaration for a record every `every` ticks.
    #[must_use]
    pub const fn new(every: u64) -> Self {
        Self {
            schema: METRICS_SCHEMA,
            every,
            complete: "window, population, lineages: folded from every published tick's report \
                       and carried from the committed population where the tracker started (the \
                       run's start, or the tick a resumed run reached); a window covers the ticks \
                       since the previous record",
            sampled: "sampled: from the render epoch the session published at the record's tick \
                      (F32 positions and velocities; field energy density averaged over texel \
                      blocks), once per record; `lag` is the record's tick minus the epoch's",
            bins: BINS,
            top_lineages: TOP_LINEAGES,
        }
    }
}

/// One metrics record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metrics {
    /// [`crate::RUN_SCHEMA`].
    pub schema: &'static str,
    /// The committed tick it describes.
    pub tick: u64,
    /// What happened since the previous record.
    pub window: Window,
    /// The live population at `tick`.
    pub population: PopulationMetrics,
    /// The lineages at `tick`.
    pub lineages: LineageMetrics,
    /// Statistics of the render epoch at `tick`.
    pub sampled: Sampled,
    /// Wall-clock seconds the sample took. Not scientific state.
    pub sample_seconds: f64,
}

/// The ticks since the previous record, folded from every report.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Window {
    /// The committed tick it opened at.
    pub from: u64,
    /// The committed tick it closed at.
    pub to: u64,
    /// The reports folded: births, deaths by stage, mutation acceptance,
    /// emission, the live range.
    pub tally: Tally,
    /// Viable birth attempts.
    pub attempted_births: u64,
    /// Every flow's total, by its canonical name.
    pub flows: BTreeMap<&'static str, f64>,
    /// Every transfer's credited total, by its canonical name.
    pub transfers: BTreeMap<&'static str, f64>,
    /// `I`, `O` and `D` summed.
    pub injected: f64,
    /// See `injected`.
    pub removed: f64,
    /// See `injected`.
    pub dissipated: f64,
    /// Field and organism energy when the window closed.
    pub holdings: Holdings,
    /// Uptake contention.
    pub uptake: Uptake,
    /// Lifespans of the organisms that died in it.
    pub lifespans: Lifespans,
    /// Lineages that lost their last member in it.
    pub extinctions: u64,
    /// Wall-clock seconds the tracker spent folding its reports. Not
    /// scientific state.
    pub record_seconds: f64,
}

/// Energy held when a window closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Holdings {
    /// In the field.
    pub field: f64,
    /// In live organisms.
    pub organisms: f64,
}

/// Uptake over a window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Uptake {
    /// Energy requested, summed over ticks.
    pub requested: f64,
    /// Energy granted.
    pub granted: f64,
    /// Cells whose demand exceeded their energy, summed over ticks.
    pub contended_cell_ticks: u64,
    /// Cells left with no energy, summed over ticks.
    pub drained_cell_ticks: u64,
    /// The most packets competing for one cell in any tick.
    pub largest_cell_packets: u64,
}

/// Lifespans in ticks: a death's tick minus its organism's birth tick.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Lifespans {
    /// Deaths.
    pub count: u64,
    /// Their lifespans summed.
    pub total: u64,
    /// The shortest.
    pub shortest: Option<u64>,
    /// The longest.
    pub longest: Option<u64>,
    /// Deaths by lifespan: the key is the largest power of two at most the
    /// lifespan, or 0 for a lifespan of 0.
    pub histogram: BTreeMap<u64, u64>,
}

/// The live population.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PopulationMetrics {
    /// Live organisms.
    pub live: u64,
    /// The population's capacity: rows allocated, the ceiling.
    pub capacity: u64,
    /// `live / capacity`.
    pub occupancy: f64,
    /// The next organism identity to issue.
    pub next_identity: u64,
    /// The highest live generation.
    pub max_generation: u32,
    /// The mean live generation.
    pub mean_generation: Option<f64>,
    /// Live organisms by topology index: the architecture distribution.
    pub topologies: BTreeMap<usize, u64>,
}

/// Lineage diversity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineageMetrics {
    /// Lineages with a live member.
    pub alive: u64,
    /// Lineages tracked whose last member died.
    pub extinct: u64,
    /// `H = −Σ p ln p` over live lineage shares, in nats.
    pub entropy: Option<f64>,
    /// `exp(H)`: the number of equally sized lineages with that entropy.
    pub effective: Option<f64>,
    /// `1 / Σ p²`.
    pub inverse_simpson: Option<f64>,
    /// The largest lineage's share.
    pub dominant_share: Option<f64>,
    /// The largest live lineages, largest first, ties by lineage.
    pub largest: Vec<LineageRow>,
}

/// One lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LineageRow {
    /// Its founder lineage identity.
    pub lineage: u64,
    /// Live members.
    pub live: u64,
    /// Births into it since the tracker started.
    pub births: u64,
    /// Deaths in it since the tracker started.
    pub deaths: u64,
    /// The highest generation it has reached.
    pub max_generation: u32,
}

/// Statistics of one render epoch.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sampled {
    /// The render epoch.
    pub epoch: u64,
    /// Its committed tick.
    pub source_tick: u64,
    /// The record's tick minus `source_tick`.
    pub lag: u64,
    /// Motility.
    pub motility: Motility,
    /// Organisms over the bin grid.
    pub dispersion: Dispersion,
    /// The energy-density texture.
    pub resources: Resources,
}

/// Speeds, from the epoch's F32 velocities.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Motility {
    /// The mean speed, world units per simulated second.
    pub mean_speed: Option<f64>,
    /// The highest.
    pub top_speed: Option<f64>,
    /// The world's speed limit.
    pub speed_limit: f64,
    /// The fraction of organisms with a nonzero velocity.
    pub moving: Option<f64>,
}

/// Organisms counted over a `bins × bins` grid on the torus.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Dispersion {
    /// Bins a side.
    pub bins: usize,
    /// The fraction of bins holding an organism.
    pub occupied: f64,
    /// The sample variance of the bin counts over their mean: 1 for a
    /// Poisson scatter, above 1 for clumping.
    pub variance_to_mean: Option<f64>,
    /// Morisita's index `Q Σ n(n−1) / (N(N−1))`: 1 for random, above 1 for
    /// clumped.
    pub morisita: Option<f64>,
    /// Moran's I of the bin counts, rook neighbours on the torus.
    pub morans_i: Option<f64>,
}

/// The field's energy density, from the epoch's texture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Resources {
    /// Texels `[x, y]`.
    pub texels: [u32; 2],
    /// Cells per texel `[x, y]`.
    pub block: [u32; 2],
    /// The mean texel density.
    pub mean_density: f64,
    /// The highest.
    pub max_density: f64,
    /// The Gini coefficient of the texel densities.
    pub gini: Option<f64>,
    /// The share of the summed density in the densest tenth of texels.
    pub top_decile_share: Option<f64>,
    /// Moran's I of the texel densities, rook neighbours on the torus.
    pub morans_i: Option<f64>,
}

/// What the tracker holds of one live organism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracked {
    /// Its stable identity.
    pub id: OrganismId,
    /// Its genome.
    pub genome: GenomeId,
    /// Its founder lineage.
    pub lineage: LineageId,
    /// Its generation.
    pub generation: u32,
    /// The committed tick it was born at; 0 for a founder.
    pub birth_tick: u64,
    /// Its topology index.
    pub topology: usize,
}

/// What the tracker keeps of one live organism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Member {
    genome: GenomeId,
    lineage: LineageId,
    generation: u32,
    birth_tick: u64,
    topology: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Lineage {
    live: u64,
    births: u64,
    deaths: u64,
    max_generation: u32,
}

/// Incremental metrics over a run's published reports.
#[derive(Debug, Clone)]
pub struct Tracker {
    every: u64,
    tick: u64,
    next: u64,
    capacity: usize,
    live: BTreeMap<OrganismId, Member>,
    lineages: BTreeMap<LineageId, Lineage>,
    topologies: BTreeMap<usize, u64>,
    generations: BTreeMap<u32, u64>,
    window: Window,
    flows: [f64; Flow::ALL.len()],
    transfers: [f64; Transfer::ALL.len()],
}

impl Tracker {
    /// A tracker starting from `world`'s committed population, emitting a
    /// record every `every` ticks.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for `every` 0;
    /// [`CoreError::StateConflict`] for an organism without a genome.
    pub fn new(world: &World, every: u64) -> Result<Self, CoreError> {
        if every == 0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "metrics interval",
                detail: "a record every 0 ticks".into(),
            });
        }
        let mut tracker = Self {
            every,
            tick: 0,
            next: 0,
            capacity: 0,
            live: BTreeMap::new(),
            lineages: BTreeMap::new(),
            topologies: BTreeMap::new(),
            generations: BTreeMap::new(),
            window: Window::default(),
            flows: [0.0; Flow::ALL.len()],
            transfers: [0.0; Transfer::ALL.len()],
        };
        tracker.refound(world)?;
        Ok(tracker)
    }

    /// Start again from `world`'s committed population: what a resumed run
    /// does when its session replayed ticks the tracker never saw. Lineage
    /// births and deaths count from here; the open window restarts.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for an organism without a genome.
    pub fn refound(&mut self, world: &World) -> Result<(), CoreError> {
        let population = world.population();
        self.tick = world.tick().get();
        self.next = world.identities().organisms.next_raw().unwrap_or(u64::MAX);
        self.capacity = population.capacity();
        self.live.clear();
        self.lineages.clear();
        self.topologies.clear();
        self.generations.clear();
        for organism in population.iter() {
            let genome = population
                .genome(organism.id)
                .ok_or_else(|| conflict(format!("{} has no genome", organism.id)))?;
            let member = Member {
                genome: organism.genome,
                lineage: organism.lineage,
                generation: organism.generation.get(),
                birth_tick: organism.birth_tick.get(),
                topology: genome.topology().index(),
            };
            self.live.insert(organism.id, member);
            self.admit(member);
        }
        self.open_window();
        Ok(())
    }

    /// The committed tick it reflects.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }

    /// The live organisms it tracks, in identity order.
    pub fn organisms(&self) -> impl Iterator<Item = Tracked> + '_ {
        self.live.iter().map(|(&id, member)| Tracked {
            id,
            genome: member.genome,
            lineage: member.lineage,
            generation: member.generation,
            birth_tick: member.birth_tick,
            topology: member.topology,
        })
    }

    /// Whether a record is due at committed tick `tick`.
    #[must_use]
    pub const fn due(&self, tick: u64) -> bool {
        tick.is_multiple_of(self.every)
    }

    /// Ticks folded since the last record.
    #[must_use]
    pub const fn pending(&self) -> u64 {
        self.window.tally.ticks
    }

    /// Fold one published tick's report: its births (each child takes its
    /// parent's lineage, the next generation, and the parent's topology or
    /// the one its mutation changed to), then its deaths.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`], leaving the tracker unusable, when the
    /// report does not advance from the tracker's tick; a child's parent is
    /// not alive, its identity is not above every one issued before, or its
    /// topology change starts elsewhere than its parent's; a death is not
    /// of a live organism or disagrees with its genome, lineage or
    /// generation; or the live count differs from the report's.
    pub fn record(&mut self, report: &TickReport) -> Result<(), CoreError> {
        let started = Instant::now();
        if report.from.get() != self.tick {
            return Err(conflict(format!(
                "the tracker is at tick {}, and the report advances from tick {}",
                self.tick,
                report.from.get()
            )));
        }
        let tick = report.tick.get();
        for birth in &report.births {
            let parent = *self.live.get(&birth.parent).ok_or_else(|| {
                conflict(format!(
                    "{}'s parent {} is not alive",
                    birth.child, birth.parent
                ))
            })?;
            if birth.child.get() < self.next {
                return Err(conflict(format!(
                    "{} is not above every identity issued before it (next {})",
                    birth.child, self.next
                )));
            }
            let topology = match &birth.record.topology {
                Some(change) if change.from.index() != parent.topology => {
                    return Err(conflict(format!(
                        "{}'s topology change starts at {}, its parent's is {}",
                        birth.child,
                        change.from.index(),
                        parent.topology
                    )));
                }
                Some(change) => change.to.index(),
                None => parent.topology,
            };
            let member = Member {
                genome: birth.genome,
                lineage: parent.lineage,
                generation: parent.generation.saturating_add(1),
                birth_tick: tick,
                topology,
            };
            self.next = birth.child.get().saturating_add(1);
            self.live.insert(birth.child, member);
            self.admit(member);
            self.lineages.entry(member.lineage).or_default().births += 1;
        }
        for death in &report.deaths {
            let member = self
                .live
                .remove(&death.id)
                .ok_or_else(|| conflict(format!("{} died but is not alive", death.id)))?;
            if member.genome != death.genome
                || member.lineage != death.lineage
                || member.generation != death.generation.get()
            {
                return Err(conflict(format!(
                    "{} died as genome {}, lineage {}, generation {}; tracked as {}, {}, {}",
                    death.id,
                    death.genome,
                    death.lineage,
                    death.generation.get(),
                    member.genome,
                    member.lineage,
                    member.generation
                )));
            }
            let lifespan = death
                .tick
                .get()
                .checked_sub(member.birth_tick)
                .ok_or_else(|| {
                    conflict(format!(
                        "{} died at tick {} before its birth at {}",
                        death.id,
                        death.tick.get(),
                        member.birth_tick
                    ))
                })?;
            self.window.lifespans.add(lifespan);
            if self.release(member) {
                self.window.extinctions += 1;
            }
        }
        if self.live.len() != report.live {
            return Err(conflict(format!(
                "{} organisms tracked at tick {tick}, {} reported",
                self.live.len(),
                report.live
            )));
        }
        self.fold(report);
        self.tick = tick;
        self.window.to = tick;
        self.window.record_seconds += started.elapsed().as_secs_f64();
        Ok(())
    }

    /// The record at the tracker's tick, its sampled part from `epoch`,
    /// then a new window.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when `epoch` shows the tracker's tick
    /// but not its live identities, lineages, generations and topologies,
    /// or shows a later tick.
    pub fn sample(&mut self, epoch: &Epoch) -> Result<Metrics, CoreError> {
        let started = Instant::now();
        let source_tick = epoch.tick().get();
        let lag = self.tick.checked_sub(source_tick).ok_or_else(|| {
            conflict(format!(
                "the render epoch shows tick {source_tick}, after the tracker's {}",
                self.tick
            ))
        })?;
        if lag == 0 {
            self.check(epoch)?;
        }
        let sampled = Sampled {
            epoch: epoch.id().get(),
            source_tick,
            lag,
            motility: motility(epoch),
            dispersion: dispersion(epoch.positions(), epoch.extent(), BINS),
            resources: resources(epoch),
        };
        let mut window = std::mem::take(&mut self.window);
        window.flows = Flow::ALL
            .iter()
            .zip(self.flows)
            .map(|(flow, total)| (flow.name(), total))
            .collect();
        window.transfers = Transfer::ALL
            .iter()
            .zip(self.transfers)
            .map(|(transfer, total)| (transfer.name(), total))
            .collect();
        let metrics = Metrics {
            schema: crate::run::RUN_SCHEMA,
            tick: self.tick,
            window,
            population: self.population(),
            lineages: self.lineage_metrics(),
            sampled,
            sample_seconds: started.elapsed().as_secs_f64(),
        };
        self.open_window();
        Ok(metrics)
    }

    fn open_window(&mut self) {
        self.window = Window {
            from: self.tick,
            to: self.tick,
            ..Window::default()
        };
        self.flows = [0.0; Flow::ALL.len()];
        self.transfers = [0.0; Transfer::ALL.len()];
    }

    fn admit(&mut self, member: Member) {
        let lineage = self.lineages.entry(member.lineage).or_default();
        lineage.live += 1;
        lineage.max_generation = lineage.max_generation.max(member.generation);
        *self.topologies.entry(member.topology).or_insert(0) += 1;
        *self.generations.entry(member.generation).or_insert(0) += 1;
    }

    /// Count `member` out; whether its lineage just lost its last member.
    fn release(&mut self, member: Member) -> bool {
        decrement(&mut self.topologies, member.topology);
        decrement(&mut self.generations, member.generation);
        let lineage = self.lineages.entry(member.lineage).or_default();
        lineage.live = lineage.live.saturating_sub(1);
        lineage.deaths += 1;
        lineage.live == 0
    }

    fn fold(&mut self, report: &TickReport) {
        let window = &mut self.window;
        window.tally.record(report);
        window.attempted_births += count(report.attempted);
        let closure = &report.closure;
        for (total, flow) in self.flows.iter_mut().zip(Flow::ALL) {
            *total += closure.flow(flow);
        }
        for (total, transfer) in self.transfers.iter_mut().zip(Transfer::ALL) {
            *total += closure.credited(transfer);
        }
        window.injected += closure.injected();
        window.removed += closure.removed();
        window.dissipated += closure.dissipated();
        window.holdings = Holdings {
            field: closure.closing().field(),
            organisms: closure.closing().organisms(),
        };
        let uptake = &report.uptake;
        window.uptake.requested += uptake.requested;
        window.uptake.granted += uptake.granted;
        window.uptake.contended_cell_ticks += count(uptake.contended_cells);
        window.uptake.drained_cell_ticks += count(uptake.drained_cells);
        window.uptake.largest_cell_packets = window
            .uptake
            .largest_cell_packets
            .max(count(uptake.largest_cell_packets));
    }

    /// The epoch at the tracker's tick lists exactly the tracked organisms.
    fn check(&self, epoch: &Epoch) -> Result<(), CoreError> {
        if epoch.len() != self.live.len() {
            return Err(conflict(format!(
                "the render epoch at tick {} shows {} organisms, the tracker {}",
                self.tick,
                epoch.len(),
                self.live.len()
            )));
        }
        let rows = epoch
            .ids()
            .iter()
            .zip(epoch.lineages())
            .zip(epoch.generations())
            .zip(epoch.topologies());
        for ((id, member), (((&shown, &lineage), &generation), &topology)) in
            self.live.iter().zip(rows)
        {
            if id.get() != shown
                || member.lineage.get() != lineage
                || member.generation != generation
                || member.topology != usize::from(topology)
            {
                return Err(conflict(format!(
                    "the render epoch at tick {} shows organism {shown} (lineage {lineage}, \
                     generation {generation}, topology {topology}) where the tracker has {id} \
                     ({}, {}, {})",
                    self.tick, member.lineage, member.generation, member.topology
                )));
            }
        }
        Ok(())
    }

    fn population(&self) -> PopulationMetrics {
        let live = count(self.live.len());
        let generation_sum = self
            .generations
            .iter()
            .fold(0_u64, |sum, (&generation, &n)| {
                sum.saturating_add(u64::from(generation).saturating_mul(n))
            });
        PopulationMetrics {
            live,
            capacity: count(self.capacity),
            occupancy: ratio(live, count(self.capacity)).unwrap_or(0.0),
            next_identity: self.next,
            max_generation: self.generations.keys().next_back().copied().unwrap_or(0),
            mean_generation: ratio(generation_sum, live),
            topologies: self.topologies.clone(),
        }
    }

    fn lineage_metrics(&self) -> LineageMetrics {
        let live = count(self.live.len());
        let mut entropy = 0.0;
        let mut concentration = 0.0;
        let mut alive = 0;
        let mut rows = Vec::new();
        for (&id, lineage) in &self.lineages {
            if lineage.live == 0 {
                continue;
            }
            alive += 1;
            if let Some(share) = ratio(lineage.live, live) {
                entropy -= share * numerics::ln(share).unwrap_or(0.0);
                concentration += share * share;
            }
            rows.push(LineageRow {
                lineage: id.get(),
                live: lineage.live,
                births: lineage.births,
                deaths: lineage.deaths,
                max_generation: lineage.max_generation,
            });
        }
        rows.sort_by(|a, b| b.live.cmp(&a.live).then(a.lineage.cmp(&b.lineage)));
        let dominant_share = rows.first().and_then(|row| ratio(row.live, live));
        rows.truncate(TOP_LINEAGES);
        let populated = live > 0;
        LineageMetrics {
            alive,
            extinct: count(self.lineages.len()) - alive,
            entropy: populated.then_some(entropy),
            effective: populated.then(|| numerics::exp(entropy)).flatten(),
            inverse_simpson: populated.then(|| 1.0 / concentration),
            dominant_share,
            largest: rows,
        }
    }
}

impl Lifespans {
    fn add(&mut self, lifespan: u64) {
        self.count += 1;
        self.total = self.total.saturating_add(lifespan);
        self.shortest = Some(self.shortest.map_or(lifespan, |s| s.min(lifespan)));
        self.longest = Some(self.longest.map_or(lifespan, |l| l.max(lifespan)));
        let bucket = if lifespan == 0 {
            0
        } else {
            1_u64 << (63 - lifespan.leading_zeros())
        };
        *self.histogram.entry(bucket).or_insert(0) += 1;
    }
}

fn decrement<K: Ord>(counts: &mut BTreeMap<K, u64>, key: K) {
    if let Some(n) = counts.get_mut(&key) {
        *n -= 1;
        if *n == 0 {
            counts.remove(&key);
        }
    }
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "track run metrics",
        detail,
    }
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "organism and cell counts are far below 2^53"
)]
fn ratio(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "organism and cell counts are far below 2^53"
)]
fn real(n: usize) -> f64 {
    n as f64
}

/// Speeds from the epoch's velocities, summed in identity order.
fn motility(epoch: &Epoch) -> Motility {
    let mut sum = 0.0_f64;
    let mut top = 0.0_f64;
    let mut moving = 0_u64;
    for velocity in epoch.velocities() {
        let [x, y] = velocity.map(f64::from);
        let speed = x.hypot(y);
        sum += speed;
        top = top.max(speed);
        moving += u64::from(speed > 0.0);
    }
    let n = epoch.len();
    Motility {
        mean_speed: (n > 0).then(|| sum / real(n)),
        top_speed: (n > 0).then_some(top),
        speed_limit: epoch.maximum_speed(),
        moving: ratio(moving, count(n)),
    }
}

/// Organisms counted over a `bins × bins` grid of the `extent` torus.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "organism and bin counts are far below 2^53"
)]
pub fn dispersion(positions: &[[f32; 2]], extent: [f64; 2], bins: usize) -> Dispersion {
    let mut counts = vec![0_u64; bins * bins];
    for position in positions {
        let [x, y] = [0, 1].map(|axis| bin(f64::from(position[axis]), extent[axis], bins));
        counts[y * bins + x] += 1;
    }
    let cells = counts.len();
    let total: u64 = counts.iter().sum();
    let occupied = counts.iter().filter(|&&n| n > 0).count();
    let mean = ratio(total, count(cells)).unwrap_or(0.0);
    let squares: f64 = counts
        .iter()
        .map(|&n| {
            let deviation = n as f64 - mean;
            deviation * deviation
        })
        .sum();
    let variance_to_mean = (cells > 1 && mean > 0.0).then(|| squares / real(cells - 1) / mean);
    let pairs: u64 = counts.iter().map(|&n| n * n.saturating_sub(1)).sum();
    let morisita =
        (total > 1).then(|| real(cells) * pairs as f64 / (total as f64 * (total - 1) as f64));
    let values: Vec<f64> = counts.iter().map(|&n| n as f64).collect();
    Dispersion {
        bins,
        occupied: ratio(count(occupied), count(cells)).unwrap_or(0.0),
        variance_to_mean,
        morisita,
        morans_i: morans_i(&values, [bins, bins]),
    }
}

/// The bin of coordinate `x` on an axis of length `extent`, folded onto
/// the torus.
fn bin(x: f64, extent: f64, bins: usize) -> usize {
    let at = (x / extent).rem_euclid(1.0) * real(bins);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "`at` is in [0, bins]: flooring it is the bin"
    )]
    let index = at.floor() as usize;
    index.min(bins - 1)
}

/// Moran's I of a row-major `size[0] × size[1]` grid with rook neighbours
/// on the torus: `(1/4) Σ_i Σ_{j∈N(i)} z_i z_j / Σ z_i²`, every cell having
/// four neighbours (`W = 4n`). `None` when the values do not vary.
#[must_use]
pub fn morans_i(values: &[f64], size: [usize; 2]) -> Option<f64> {
    let [width, height] = size;
    let n = values.len();
    if n == 0 || n != width * height {
        return None;
    }
    let mean = values.iter().sum::<f64>() / real(n);
    let deviations: Vec<f64> = values.iter().map(|value| value - mean).collect();
    let variance: f64 = deviations.iter().map(|z| z * z).sum();
    if variance <= 0.0 {
        return None;
    }
    let mut cross = 0.0;
    for y in 0..height {
        for x in 0..width {
            let z = deviations[y * width + x];
            let right = deviations[y * width + (x + 1) % width];
            let left = deviations[y * width + (x + width - 1) % width];
            let down = deviations[((y + 1) % height) * width + x];
            let up = deviations[((y + height - 1) % height) * width + x];
            cross += z * (right + left + down + up);
        }
    }
    Some(cross / 4.0 / variance)
}

/// The Gini coefficient of nonnegative `values`:
/// `Σ_i (2i − n − 1) x_(i) / (n Σ x)` over the ascending order, `i` from 1.
/// `None` when they sum to zero.
#[must_use]
pub fn gini(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let total: f64 = sorted.iter().sum();
    if n == 0 || total <= 0.0 {
        return None;
    }
    let weighted: f64 = sorted
        .iter()
        .enumerate()
        .map(|(index, value)| (2.0 * real(index + 1) - real(n) - 1.0) * value)
        .sum();
    Some(weighted / (real(n) * total))
}

/// The share of the sum of nonnegative `values` held by the largest tenth
/// of them (rounded up). `None` when they sum to zero.
#[must_use]
pub fn top_decile_share(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let total: f64 = sorted.iter().sum();
    if sorted.is_empty() || total <= 0.0 {
        return None;
    }
    let top: f64 = sorted.iter().take(sorted.len().div_ceil(10)).sum();
    Some(top / total)
}

fn resources(epoch: &Epoch) -> Resources {
    let texture = epoch.field();
    let values: Vec<f64> = texture.values().iter().map(|&v| f64::from(v)).collect();
    let texels = texture.texels();
    let n = values.len();
    let mean_density = if n == 0 {
        0.0
    } else {
        values.iter().sum::<f64>() / real(n)
    };
    let max_density = values.iter().copied().fold(0.0, f64::max);
    let size = texels.map(|t| usize::try_from(t).unwrap_or(0));
    Resources {
        texels,
        block: texture.block(),
        mean_density,
        max_density,
        gini: gini(&values),
        top_decile_share: top_decile_share(&values),
        morans_i: morans_i(&values, size),
    }
}
