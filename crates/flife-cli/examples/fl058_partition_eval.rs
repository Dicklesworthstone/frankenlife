//! Empirical FL-058 single-world multi-device partitioning evaluation
//! (plan §20.4, §24.2, ADR-11).
//!
//! Measures real stepped FrankenLife worlds and real field grids:
//! 1. Seam-crossing uptake packets, emission deposits, post-motion organism &
//!    full-genome migration traffic, and spatial load imbalance on stepped
//!    `reference-small-world` (128×128, 4k founders, 512 ticks) and
//!    `desktop-world` (512×512, 100k F16 founders on Metal, 32 ticks).
//! 2. Exact `0 ULP` partitioned field transport scaling (`1×1`, `2×1`, `2×2`,
//!    `4×2`) vs global `transport_on` on `512×512×33` and `1024×1024×33`.
//! 3. Communication budget across 5 real interconnects (`10GbE TCP`,
//!    `80Gbps TB5`, `100GbE RDMA`, `400GbE IB NDR`, `NVLink/UltraFusion`).
//!
//! Run in release mode:
//!   cargo run --release --locked -p flife-cli --example fl058_partition_eval

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    reason = "empirical measurement tool"
)]

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::time::Instant;

use flife_backend_metal::MetalBackend;
use flife_cli::found;
use flife_core::ValidatedConfig;
use flife_core::ids::OrganismId;
use flife_field::{
    BufferLimits, CommBudgetInput, FIELD_PLANES, FieldGrid, FieldState, Grants, HaloMode,
    MigratingOrganism, PartitionedOrganism, RequestRule, ResidentTileField, TilePartition,
    TransportPlan, TransportScratch, demands, estimate_comm_budget, migrate_organisms,
    resolve_partitioned_uptake, resolve_uptake, transport_on, transport_partitioned,
};

const REF_PRESET: &str = include_str!("../../../presets/reference-small-world.toml");
const DESKTOP_PRESET: &str = include_str!("../../../presets/desktop-world.toml");

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(f64::NAN)
}

fn measure_world_seam_traffic(
    label: &str,
    toml: &str,
    warmup_ticks: u64,
    sample_ticks: u64,
    use_metal: bool,
    verify_global_uptake_parity: bool,
) -> serde_json::Value {
    let config = ValidatedConfig::from_toml(toml).expect("valid preset");
    let mut world = found(&config).expect("found world");
    let cpus = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
        .min(32);
    world
        .set_cpu_workers(NonZeroUsize::new(cpus).expect("non-zero"))
        .expect("set workers");
    if use_metal && let Ok(metal) = MetalBackend::open() {
        world.set_controller_backend(Some(Box::new(metal)));
    }

    for _ in 0..warmup_ticks {
        world.step().expect("warmup step");
    }

    let grid = *world.field().grid();
    let layouts: [(u32, u32); 3] = [(2, 1), (2, 2), (4, 2)];
    let partitions: Vec<TilePartition> = layouts
        .iter()
        .map(|&(tx, ty)| TilePartition::new(grid, tx, ty).expect("partition"))
        .collect();

    // Track each organism's previous tile across consecutive stepped ticks to
    // measure real post-motion migration traffic and exact genome wire bytes.
    let mut prev_pos: BTreeMap<OrganismId, [f64; 2]> = world
        .population()
        .iter()
        .map(|org| (org.id, org.position))
        .collect();

    let mut mig_orgs_sum = [0_u64; 3];
    let mut mig_bytes_sum = [0_u64; 3];
    let mut imbalance_max = [1.0_f64; 3];
    let mut emitted_sum = 0_u64;
    let mut live_sum = 0_u64;
    let mut mean_genome_bytes = 0_u64;

    for _ in 0..sample_ticks {
        let flife_engine::Stepped::Published(report) = world.step().expect("sample step") else {
            panic!("world paused during sample");
        };
        emitted_sum += report.emitted as u64;
        live_sum += report.live as u64;

        let mut next_pos: BTreeMap<OrganismId, [f64; 2]> = BTreeMap::new();
        let mut total_genome_bytes = 0_u64;
        let mut genome_count = 0_u64;

        // Build per-layout tile buckets seeded at previous-tick tiles, with
        // current-tick post-motion positions, then invoke `migrate_organisms`.
        let mut buckets: Vec<Vec<Vec<MigratingOrganism<()>>>> = partitions
            .iter()
            .map(|p| vec![Vec::new(); p.tile_count()])
            .collect();

        for (org, genome_opt) in world.population().iter_with_genomes() {
            let g_bytes = genome_opt.map_or(0, flife_genome::Genome::logical_bytes);
            total_genome_bytes += g_bytes;
            genome_count += 1;
            next_pos.insert(org.id, org.position);

            let prev = prev_pos.get(&org.id).copied().unwrap_or(org.position);
            for (l_idx, part) in partitions.iter().enumerate() {
                let prev_tile = part.tile_of_position(prev).expect("prev tile");
                buckets[l_idx][prev_tile].push(MigratingOrganism {
                    id: org.id,
                    position: org.position,
                    genome_bytes: g_bytes,
                    payload: (),
                });
            }
        }
        if let Some(mean) = total_genome_bytes.checked_div(genome_count) {
            mean_genome_bytes = mean;
        }
        for (l_idx, part) in partitions.iter().enumerate() {
            let stats = migrate_organisms(part, &mut buckets[l_idx]).expect("migrate");
            mig_orgs_sum[l_idx] += stats.migrated_organisms as u64;
            mig_bytes_sum[l_idx] += stats.migration_bytes;
            imbalance_max[l_idx] = imbalance_max[l_idx].max(stats.load_imbalance_ratio);
        }
        prev_pos = next_pos;
    }

    // Measure exact boundary uptake packets on the live stepped snapshot.
    let chem = &config.config().chemistry;
    let rule = RequestRule {
        step: config.report().timestep,
        capacity_per_second: chem.uptake_capacity_per_second,
        half_saturation_density: chem.uptake_half_saturation_density,
    };
    let live_orgs: Vec<PartitionedOrganism> = world
        .population()
        .iter()
        .map(|org| PartitionedOrganism {
            organism: org.id,
            position: org.position,
            explicit_request: None,
        })
        .collect();

    let ref_grants_opt = if verify_global_uptake_parity {
        let mut ref_demands = Vec::with_capacity(live_orgs.len());
        demands(
            world.field(),
            &rule,
            live_orgs.iter().map(|o| (o.organism, o.position)),
            &mut ref_demands,
        )
        .expect("ref demands");
        let mut grants = Grants::new();
        let summary = resolve_uptake(world.field(), &ref_demands, &mut grants).expect("ref uptake");
        Some((summary, grants))
    } else {
        None
    };

    let mut per_layout = Vec::new();
    let mean_live = (live_sum as f64) / (sample_ticks as f64);
    let mean_emitted = (emitted_sum as f64) / (sample_ticks as f64);
    let emit_fraction = if mean_live > 0.0 {
        mean_emitted / mean_live
    } else {
        0.0
    };

    for (l_idx, (&(tx, ty), part)) in layouts.iter().zip(&partitions).enumerate() {
        let uptake =
            resolve_partitioned_uptake(part, world.field(), Some(&rule), &live_orgs).expect("up");
        let mut uptake_bit_exact = None;
        if let Some((ref_summary, ref_grants)) = &ref_grants_opt {
            let exact = uptake.summary == *ref_summary
                && uptake.debits == ref_grants.debits()
                && uptake.packets == ref_grants.packets()
                && uptake.totals == ref_grants.totals();
            assert!(
                exact,
                "live world partitioned uptake parity failed on {tx}x{ty}"
            );
            uptake_bit_exact = Some(exact);
        }

        let mean_mig_orgs = (mig_orgs_sum[l_idx] as f64) / (sample_ticks as f64);
        let mean_mig_bytes = (mig_bytes_sum[l_idx] as f64) / (sample_ticks as f64);
        // Boundary deposit packets occur at the exact same grid-cic-v1 seam
        // crossing rate weighted by the live emitting fraction.
        let est_boundary_deposit_packets =
            ((uptake.comm.boundary_demand_packets as f64) * emit_fraction).round() as u64;

        let input = CommBudgetInput {
            planes: FIELD_PLANES,
            substeps: 1,
            halo_mode: HaloMode::PerSubstep,
            pyramid_levels: 4,
            boundary_demand_packets_per_tick: uptake.comm.boundary_demand_packets,
            boundary_deposit_packets_per_tick: est_boundary_deposit_packets,
            migrated_organisms_per_tick: mean_mig_orgs.round() as u64,
            mean_genome_bytes,
        };
        let (breakdown, interconnects) = estimate_comm_budget(part, input);

        per_layout.push(serde_json::json!({
            "layout": format!("{tx}x{ty}"),
            "devices": part.tile_count(),
            "uptake_bit_exact_with_global": uptake_bit_exact,
            "boundary_demand_packets": uptake.comm.boundary_demand_packets,
            "est_boundary_deposit_packets": est_boundary_deposit_packets,
            "mean_migrated_organisms_per_tick": mean_mig_orgs,
            "mean_migration_bytes_per_tick": mean_mig_bytes,
            "peak_load_imbalance_ratio": imbalance_max[l_idx],
            "comm_total_bytes_per_tick": breakdown.total_bytes_per_tick,
            "comm_per_device_bytes_per_tick": breakdown.per_device_bytes_per_tick,
            "comm_transport_halo_bytes": breakdown.transport_halo_bytes,
            "comm_pyramid_halo_bytes": breakdown.pyramid_halo_bytes,
            "comm_uptake_boundary_bytes": breakdown.uptake_boundary_bytes,
            "comm_deposit_boundary_bytes": breakdown.deposit_boundary_bytes,
            "comm_migration_bytes": breakdown.migration_bytes,
            "barriers_per_tick": breakdown.barriers_per_tick,
            "interconnects_ms": interconnects.iter().map(|e| serde_json::json!({
                "name": e.profile.name,
                "transfer_ms": e.transfer_ms,
                "barrier_ms": e.barrier_ms,
                "total_comm_ms": e.total_comm_ms,
                "pct_of_30hz_tick": e.fraction_of_30hz_tick * 100.0,
            })).collect::<Vec<_>>(),
        }));
    }

    serde_json::json!({
        "preset": label,
        "grid": format!("{}x{}", grid.cells_x(), grid.cells_y()),
        "warmup_ticks": warmup_ticks,
        "sample_ticks": sample_ticks,
        "final_tick": world.tick().get(),
        "final_live": world.population().len(),
        "mean_live": mean_live,
        "mean_emitted_per_tick": mean_emitted,
        "mean_genome_bytes": mean_genome_bytes,
        "layouts": per_layout,
    })
}

fn measure_transport_scaling(cells: u32, rounds: usize) -> serde_json::Value {
    let grid = FieldGrid::new(cells, cells, f64::from(cells), f64::from(cells)).expect("grid");
    let step = flife_core::TimeStep::new(1, 128).expect("dt");
    let plan = TransportPlan::new(&grid, step, 0.15, 0.01).expect("plan");

    let mut current = FieldState::zeroed(grid, &BufferLimits::host()).expect("current");
    let n = (cells as usize) * (cells as usize);
    let data = current.as_mut_slice();
    for i in 0..n {
        let e = (((i.wrapping_mul(2654435761)) >> 20) as f32) * 0.001 + 0.05;
        data[i] = e;
        data[n + i] = e * 0.25;
        data[2 * n + i] = -e * 0.125;
    }

    let mut ref_next = FieldState::zeroed(grid, &BufferLimits::host()).expect("ref_next");
    let mut global_timings = Vec::new();
    for workers in [1_usize, 4, 8, 32] {
        let mut scratches: Vec<TransportScratch> = (0..workers)
            .map(|_| TransportScratch::new(&grid, &BufferLimits::host()).expect("scratch"))
            .collect();
        // Warmup
        transport_on(&plan, &current, &mut ref_next, &mut scratches).expect("warmup");
        let mut ms = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let t0 = Instant::now();
            transport_on(&plan, &current, &mut ref_next, &mut scratches).expect("transport_on");
            ms.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        global_timings.push(serde_json::json!({
            "workers": workers,
            "median_ms": median(ms),
        }));
    }

    let mut part_timings = Vec::new();
    for (tx, ty) in [(1_u32, 1_u32), (2, 1), (2, 2), (4, 2)] {
        let partition = TilePartition::new(grid, tx, ty).expect("partition");
        let mut part_next = FieldState::zeroed(grid, &BufferLimits::host()).expect("part_next");
        let workers = partition.tile_count();
        let (_, stats) = transport_partitioned(
            &partition,
            &plan,
            &current,
            &mut part_next,
            HaloMode::PerSubstep,
            workers,
        )
        .expect("warmup part");
        let bit_exact = part_next
            .as_slice()
            .iter()
            .zip(ref_next.as_slice())
            .all(|(a, b)| a.to_bits() == b.to_bits());
        assert!(
            bit_exact,
            "0-ULP transport parity failed on {cells}x{cells} {tx}x{ty}"
        );

        let mut resident =
            ResidentTileField::new(&partition, FIELD_PLANES as usize, 1).expect("resident");
        resident.load_from(&current);
        let mut ms = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let t0 = Instant::now();
            resident
                .transport_step(&partition, &plan, HaloMode::PerSubstep, workers)
                .expect("resident step");
            ms.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        part_timings.push(serde_json::json!({
            "layout": format!("{tx}x{ty}"),
            "tiles": partition.tile_count(),
            "workers": workers,
            "bit_exact_0ulp": bit_exact,
            "remote_halo_bytes": stats.remote_halo_bytes,
            "resident_step_median_ms": median(ms),
        }));
    }

    serde_json::json!({
        "grid": format!("{cells}x{cells}"),
        "planes": FIELD_PLANES,
        "substeps": plan.substeps(),
        "rounds": rounds,
        "global_transport_on_ms": global_timings,
        "partitioned_shared_mem_ms": part_timings,
    })
}

fn main() {
    let ref_eval =
        measure_world_seam_traffic("reference-small-world", REF_PRESET, 384, 128, false, true);
    let desktop_eval =
        measure_world_seam_traffic("desktop-world", DESKTOP_PRESET, 64, 32, true, false);

    // Project 400k screenshot-world (1024×1024 = 2× linear seam length at same
    // areal density as desktop-world 512×512 = 100k -> 2× seam traffic per seam,
    // derived explicitly from desktop-world's measured seam rates!).
    let desktop_layouts = desktop_eval["layouts"].as_array().expect("layouts");
    let grid_1024 = FieldGrid::new(1024, 1024, 1024.0, 1024.0).expect("1024x1024");
    let mut screenshot_projected = Vec::new();
    for (idx, &(tx, ty)) in [(2_u32, 1_u32), (2, 2), (4, 2)].iter().enumerate() {
        let dl = &desktop_layouts[idx];
        // At fixed areal population density (100k / 512^2 == 400k / 1024^2),
        // total tile seam length doubles (2×), so cross-seam packets and
        // migrating organisms scale by 2× linearly with seam perimeter!
        let demand_400k = dl["boundary_demand_packets"].as_u64().expect("dem") * 2;
        let dep_400k = dl["est_boundary_deposit_packets"].as_u64().expect("dep") * 2;
        let mig_400k = (dl["mean_migrated_organisms_per_tick"]
            .as_f64()
            .expect("mig")
            * 2.0)
            .round() as u64;
        let part = TilePartition::new(grid_1024, tx, ty).expect("1024 part");
        let input = CommBudgetInput {
            planes: FIELD_PLANES,
            substeps: 1,
            halo_mode: HaloMode::PerSubstep,
            pyramid_levels: 5,
            boundary_demand_packets_per_tick: demand_400k,
            boundary_deposit_packets_per_tick: dep_400k,
            migrated_organisms_per_tick: mig_400k,
            mean_genome_bytes: 9_988 * 2,
        };
        let (b, est) = estimate_comm_budget(&part, input);
        screenshot_projected.push(serde_json::json!({
            "layout": format!("{tx}x{ty}"),
            "devices": b.devices,
            "scaled_boundary_demand_packets": demand_400k,
            "scaled_boundary_deposit_packets": dep_400k,
            "scaled_migrated_organisms_per_tick": mig_400k,
            "comm_total_bytes_per_tick": b.total_bytes_per_tick,
            "comm_per_device_bytes_per_tick": b.per_device_bytes_per_tick,
            "comm_transport_halo_bytes": b.transport_halo_bytes,
            "comm_pyramid_halo_bytes": b.pyramid_halo_bytes,
            "comm_uptake_boundary_bytes": b.uptake_boundary_bytes,
            "comm_deposit_boundary_bytes": b.deposit_boundary_bytes,
            "comm_migration_bytes": b.migration_bytes,
            "barriers_per_tick": b.barriers_per_tick,
            "interconnects_ms": est.iter().map(|e| serde_json::json!({
                "name": e.profile.name,
                "transfer_ms": e.transfer_ms,
                "barrier_ms": e.barrier_ms,
                "total_comm_ms": e.total_comm_ms,
                "pct_of_30hz_tick": e.fraction_of_30hz_tick * 100.0,
            })).collect::<Vec<_>>(),
        }));
    }

    let transport_512 = measure_transport_scaling(512, 7);
    let transport_1024 = measure_transport_scaling(1024, 5);

    let report = serde_json::json!({
        "reference_small_world": ref_eval,
        "desktop_world": desktop_eval,
        "screenshot_world_400k_derived_from_desktop_seam_density": screenshot_projected,
        "transport_scaling_512": transport_512,
        "transport_scaling_1024": transport_1024,
    });
    println!("{}", serde_json::to_string_pretty(&report).expect("json"));
}
