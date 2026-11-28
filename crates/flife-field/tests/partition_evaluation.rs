//! FL-058 evidence (plan §20.4, §24.2, ADR-11): verifies `0 ULP` bit-for-bit
//! correctness of spatially partitioned halo transport, boundary-consistent
//! demand/grant uptake (`INV-04`, Appendix C.1 ties across 4-way tile corners),
//! boundary deposit reduction (`INV-03`), atomic post-motion organism & genome
//! migration (`INV-07`), and communication budget accounting.

#![forbid(unsafe_code)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    reason = "integration tests: unexpected failure is a test failure"
)]

use std::collections::BTreeSet;

use flife_core::TimeStep;
use flife_core::ids::OrganismId;
use flife_field::{
    BufferLimits, CommBudgetInput, Demand, Deposits, FIELD_PLANES, FieldGrid, FieldState, Grants,
    HaloMode, MigratingOrganism, PartitionedDeposit, PartitionedOrganism, RequestRule,
    ResidentTileField, TilePartition, TransportPlan, TransportScratch, apply_deposits,
    apply_partitioned_deposits, demands, estimate_comm_budget, migrate_organisms,
    resolve_partitioned_uptake, resolve_uptake, transport_on, transport_partitioned,
};

/// Seed a deterministic heterogeneous 33-plane field (energy + valid chemical
/// moments inside the cone `||m|| <= e`) with sharp peaks on tile seams and
/// periodic wrap boundaries.
fn seeded_field(grid: FieldGrid) -> FieldState {
    let mut field = FieldState::zeroed(grid, &BufferLimits::host()).expect("zeroed field");
    let gx = grid.cells_x() as usize;
    let gy = grid.cells_y() as usize;
    let cells = gx * gy;
    let data = field.as_mut_slice();
    for y in 0..gy {
        for x in 0..gx {
            let idx = y * gx + x;
            let hash = ((x as u64)
                .wrapping_mul(6364136223846793005)
                .wrapping_add((y as u64).wrapping_mul(1442695040888963407))
                .wrapping_add(0x9E37_79B9_7F4A_7C15))
                >> 32;
            let e = ((hash % 1000) as f32) * 0.01 + 0.05;
            data[idx] = e;
            // Put nonzero chemical moments along channels 0..32 strictly inside the cone.
            let scale = e * 0.125;
            for ch in 0..32_usize {
                let sign = if ((hash >> (ch % 24)) & 1) == 0 {
                    1.0_f32
                } else {
                    -1.0_f32
                };
                data[(1 + ch) * cells + idx] = sign * scale * (1.0 / 32.0);
            }
        }
    }
    field
}

/// Halo-exchanged five-point transport + decay matches global `transport_on`
/// `0 ULP` bit-for-bit across `1x1`, `2x1`, `1x2`, `2x2`, `4x2`, uneven
/// non-divisible grids (`17x13`), multi-substep diffusion (`substeps > 1`),
/// AND both `HaloMode::PerSubstep` and `HaloMode::WideTick`!
#[test]
fn halo_transport_matches_global_field_bit_for_bit() {
    let step = TimeStep::new(1, 32).expect("dt");

    for (gx, gy, diffusion, configs) in [
        (
            16_u32,
            16_u32,
            0.25_f64, // 1 substep
            vec![(1, 1), (2, 1), (1, 2), (2, 2), (4, 2)],
        ),
        (
            17_u32,
            13_u32,
            24.0_f64, // multi-substep diffusion (r_x = 0.75 -> substeps >= 3) + uneven tile splits!
            vec![(1, 1), (2, 2), (3, 2), (4, 3)],
        ),
    ] {
        let grid = FieldGrid::new(gx, gy, f64::from(gx), f64::from(gy)).expect("grid");
        let plan = TransportPlan::new(&grid, step, diffusion, 0.15).expect("plan");
        if gx == 17 {
            assert!(
                plan.substeps() >= 2,
                "expected multi-substep diffusion for uneven test"
            );
        }
        let current = seeded_field(grid);
        let mut ref_next = FieldState::zeroed(grid, &BufferLimits::host()).expect("ref next");
        let mut scratch = TransportScratch::new(&grid, &BufferLimits::host()).expect("scratch");
        let ref_report = transport_on(
            &plan,
            &current,
            &mut ref_next,
            std::slice::from_mut(&mut scratch),
        )
        .expect("ref transport");

        for (tx, ty) in configs {
            let partition = TilePartition::new(grid, tx, ty).expect("partition");
            for mode in [HaloMode::PerSubstep, HaloMode::WideTick] {
                for workers in [1_usize, 4_usize] {
                    let mut part_next =
                        FieldState::zeroed(grid, &BufferLimits::host()).expect("part next");
                    let (part_report, stats) = transport_partitioned(
                        &partition,
                        &plan,
                        &current,
                        &mut part_next,
                        mode,
                        workers,
                    )
                    .expect("partitioned transport");

                    assert_eq!(
                        part_report, ref_report,
                        "TransportReport mismatch on {gx}x{gy} ({tx}x{ty}, {mode:?}, w={workers})"
                    );
                    let ref_bits: Vec<u32> =
                        ref_next.as_slice().iter().map(|v| v.to_bits()).collect();
                    let part_bits: Vec<u32> =
                        part_next.as_slice().iter().map(|v| v.to_bits()).collect();
                    assert_eq!(
                        part_bits, ref_bits,
                        "0-ULP field mismatch on {gx}x{gy} ({tx}x{ty}, {mode:?}, w={workers})"
                    );
                    if tx * ty > 1 {
                        assert!(stats.remote_halo_cells > 0);
                        assert_eq!(
                            stats.remote_halo_bytes,
                            stats.remote_halo_cells * u64::from(FIELD_PLANES) * 4
                        );
                    } else {
                        assert_eq!(stats.remote_halo_cells, 0);
                        assert_eq!(stats.remote_halo_bytes, 0);
                    }
                }
            }
        }
    }

    // Multi-tick `ResidentTileField::transport_step` must decay ALL planes
    // (including plane 0 energy!) and stay 0-ULP bit-exact over 3 ticks!
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let plan = TransportPlan::new(&grid, step, 0.25, 0.2).expect("plan");
    let mut ref_a = seeded_field(grid);
    let mut ref_b = FieldState::zeroed(grid, &BufferLimits::host()).expect("ref_b");
    let mut scratch = TransportScratch::new(&grid, &BufferLimits::host()).expect("scratch");
    let part = TilePartition::new(grid, 2, 2).expect("2x2");
    let mut resident = ResidentTileField::new(&part, FIELD_PLANES as usize, 1).expect("resident");
    resident.load_from(&ref_a);
    for _ in 0..3 {
        transport_on(
            &plan,
            &ref_a,
            &mut ref_b,
            std::slice::from_mut(&mut scratch),
        )
        .expect("ref tick");
        std::mem::swap(&mut ref_a, &mut ref_b);
        resident
            .transport_step(&part, &plan, HaloMode::PerSubstep, 4)
            .expect("resident tick");
    }
    let mut got = FieldState::zeroed(grid, &BufferLimits::host()).expect("got");
    resident.store_into(&mut got);
    let ref_bits: Vec<u32> = ref_a.as_slice().iter().map(|v| v.to_bits()).collect();
    let got_bits: Vec<u32> = got.as_slice().iter().map(|v| v.to_bits()).collect();
    assert_eq!(
        got_bits, ref_bits,
        "3-tick ResidentTileField must match 3-tick transport_on 0-ULP bit-for-bit across all planes"
    );
}

/// Globally consistent boundary demand/grant uptake matches global
/// `demands` + `resolve_uptake` `0 ULP` bit-for-bit — including contended
/// organisms straddling horizontal seams, vertical seams, periodic wrap edges,
/// AND a 4-way tile corner with exact Appendix C.1 largest-remainder ties!
#[test]
fn boundary_uptake_grants_match_global_bit_for_bit_including_four_way_corners() {
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let field = seeded_field(grid);
    let step = TimeStep::new(1, 32).expect("dt");
    let rule = RequestRule {
        step,
        capacity_per_second: 8.0,
        half_saturation_density: 1.5,
    };

    // Place organisms directly on tile seams (x=8.0, y=8.0), periodic seam
    // (x=0.0, y=0.0 -> touches cells 15 and 0!), and a 4-way corner cluster at
    // (7.95, 7.95), (8.05, 7.95), (7.95, 8.05), (8.05, 8.05) with colliding
    // requests to force cross-device C.1 tie-breaking!
    let mut positions = vec![
        [7.95, 7.95],
        [8.05, 7.95],
        [7.95, 8.05],
        [8.05, 8.05],
        [8.0, 8.0],
        [0.01, 0.01],
        [15.99, 15.99],
        [15.99, 0.01],
        [0.01, 15.99],
        [4.0, 8.0],
        [8.0, 12.0],
        [3.3, 11.7],
    ];
    for i in 0..40_u32 {
        positions.push([
            ((i as f64) * 3.71).rem_euclid(16.0),
            ((i as f64) * 5.13).rem_euclid(16.0),
        ]);
    }

    // Include both rule-driven organisms and explicit high-contention tie pairs.
    let mut global_demands: Vec<Demand> = Vec::new();
    let mut part_orgs: Vec<PartitionedOrganism> = Vec::new();

    // Test 1: Rule-driven saturating requests read strictly from 1-cell tile halos.
    let ids: Vec<OrganismId> = (1..=positions.len() as u64).map(OrganismId::new).collect();
    let mut ref_demands = Vec::new();
    demands(
        &field,
        &rule,
        ids.iter().copied().zip(positions.iter().copied()),
        &mut ref_demands,
    )
    .expect("ref demands");
    let mut ref_grants = Grants::new();
    let ref_summary = resolve_uptake(&field, &ref_demands, &mut ref_grants).expect("ref uptake");

    for (&id, &pos) in ids.iter().zip(&positions) {
        part_orgs.push(PartitionedOrganism {
            organism: id,
            position: pos,
            explicit_request: None,
        });
    }

    for (tx, ty) in [(1, 1), (2, 1), (1, 2), (2, 2), (4, 2), (4, 4)] {
        let partition = TilePartition::new(grid, tx, ty).expect("partition");
        let res = resolve_partitioned_uptake(&partition, &field, Some(&rule), &part_orgs)
            .expect("partitioned uptake");
        assert_eq!(res.summary, ref_summary, "summary mismatch on {tx}x{ty}");
        assert_eq!(
            res.debits,
            ref_grants.debits(),
            "debit mismatch on {tx}x{ty}"
        );
        assert_eq!(
            res.packets,
            ref_grants.packets(),
            "packet grant mismatch on {tx}x{ty}"
        );
        assert_eq!(
            res.totals,
            ref_grants.totals(),
            "organism grant totals mismatch on {tx}x{ty}"
        );
        // INV-04: every cell debit strictly <= energy_before.
        for d in &res.debits {
            assert!(d.granted <= f64::from(d.energy_before));
            assert!(d.energy_after >= 0.0);
        }
        if tx * ty > 1 {
            assert!(
                res.comm.boundary_demand_packets > 0,
                "expected cross-seam demand packets on {tx}x{ty}"
            );
            assert_eq!(
                res.comm.boundary_demand_packets,
                res.comm.boundary_grant_packets
            );
        }
    }

    // Test 2: Extreme 4-way corner contention with identical requests forcing
    // exact C.1 tie-breaking by ascending `OrganismId` across four distinct tiles.
    for (idx, pos) in [
        [7.99, 7.99],
        [8.01, 7.99],
        [7.99, 8.01],
        [8.01, 8.01],
        [7.98, 8.02],
        [8.02, 7.98],
    ]
    .into_iter()
    .enumerate()
    {
        // Deliberately push in reverse ID order to confirm tile routing is order-independent.
        let id = OrganismId::new(100 - idx as u64);
        global_demands.push(Demand {
            organism: id,
            position: pos,
            amount: 50.0,
        });
    }
    let mut ref_tie_grants = Grants::new();
    let ref_tie_sum =
        resolve_uptake(&field, &global_demands, &mut ref_tie_grants).expect("ref tie uptake");
    let part_tie_orgs: Vec<PartitionedOrganism> = global_demands
        .iter()
        .map(|d| PartitionedOrganism {
            organism: d.organism,
            position: d.position,
            explicit_request: Some(d.amount),
        })
        .collect();
    let part_2x2 = TilePartition::new(grid, 2, 2).expect("2x2");
    let part_tie =
        resolve_partitioned_uptake(&part_2x2, &field, None, &part_tie_orgs).expect("part tie");
    assert_eq!(part_tie.summary, ref_tie_sum);
    assert_eq!(part_tie.debits, ref_tie_grants.debits());
    assert_eq!(part_tie.packets, ref_tie_grants.packets());
    assert_eq!(part_tie.totals, ref_tie_grants.totals());
    assert!(part_tie.summary.contended_cells > 0);

    // Duplicate `OrganismId` must be refused identically to global `resolve_uptake`.
    let dup_orgs = vec![part_tie_orgs[0], part_tie_orgs[0]];
    assert!(resolve_partitioned_uptake(&part_2x2, &field, None, &dup_orgs).is_err());
}

/// Boundary-consistent emission deposit routing reproduces global
/// `Deposits` + `apply_deposits` `0 ULP` bit-for-bit across tile seams.
#[test]
fn boundary_deposit_reduction_matches_global_bit_for_bit() {
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let base_field = seeded_field(grid);

    let mut dir_a = [0.0_f64; 32];
    dir_a[0] = 0.6;
    dir_a[1] = -0.8;
    let mut dir_b = [0.0_f64; 32];
    dir_b[0] = -0.6;
    dir_b[2] = 0.8;

    let part_deps = vec![
        PartitionedDeposit {
            organism: OrganismId::new(42),
            position: [7.99, 7.99],
            paid: 1.5,
            offered: 1.425,
            direction: dir_a,
        },
        PartitionedDeposit {
            organism: OrganismId::new(7),
            position: [8.01, 8.01],
            paid: 2.0,
            offered: 1.9,
            direction: dir_b,
        },
        PartitionedDeposit {
            organism: OrganismId::new(19),
            position: [15.99, 0.01],
            paid: 0.75,
            offered: 0.7125,
            direction: dir_a,
        },
        PartitionedDeposit {
            organism: OrganismId::new(3),
            position: [0.01, 15.99],
            paid: 1.25,
            offered: 1.1875,
            direction: dir_b,
        },
    ];

    // Reference unpartitioned reduction.
    let mut ref_field = base_field.clone();
    let mut ref_deposits = Deposits::with_capacity(&grid, 16, &BufferLimits::host()).expect("dep");
    for dep in &part_deps {
        ref_deposits
            .push(
                &grid,
                dep.organism,
                dep.position,
                dep.paid,
                dep.offered,
                &dep.direction,
            )
            .expect("push");
    }
    let ref_summary = apply_deposits(&mut ref_field, &mut ref_deposits).expect("apply ref");

    for (tx, ty) in [(1, 1), (2, 2), (4, 2)] {
        let partition = TilePartition::new(grid, tx, ty).expect("partition");
        let mut part_field = base_field.clone();
        let (part_summary, comm) =
            apply_partitioned_deposits(&partition, &mut part_field, &part_deps)
                .expect("apply partitioned");
        assert_eq!(
            part_summary, ref_summary,
            "deposit summary mismatch {tx}x{ty}"
        );
        let ref_bits: Vec<u32> = ref_field.as_slice().iter().map(|v| v.to_bits()).collect();
        let part_bits: Vec<u32> = part_field.as_slice().iter().map(|v| v.to_bits()).collect();
        assert_eq!(part_bits, ref_bits, "0-ULP deposit mismatch on {tx}x{ty}");
        if tx * ty > 1 {
            assert!(comm.boundary_packets > 0);
            assert!(comm.boundary_bytes > 0);
        }
    }

    // Out-of-unit-ball chemical direction must be refused without mutating `field`.
    let mut bad_dir = [0.0_f64; 32];
    bad_dir[0] = 1.1;
    let bad_dep = PartitionedDeposit {
        organism: OrganismId::new(999),
        position: [7.99, 7.99],
        paid: 1.0,
        offered: 0.9,
        direction: bad_dir,
    };
    let mut intact = base_field.clone();
    let p2x2 = TilePartition::new(grid, 2, 2).expect("2x2");
    assert!(apply_partitioned_deposits(&p2x2, &mut intact, &[bad_dep]).is_err());
    assert_eq!(intact.as_slice(), base_field.as_slice());
}

/// Atomic post-motion organism + full-genome migration preserves every
/// organism, `INV-07` identity uniqueness, periodic seam wrapping, exact
/// genome wire accounting, and refuses duplicate identities.
#[test]
fn atomic_organism_and_genome_migration_preserves_identities_and_accounts_wire_bytes() {
    let grid = FieldGrid::new(32, 32, 32.0, 32.0).expect("grid");
    let partition = TilePartition::new(grid, 2, 2).expect("2x2");
    let genome_bytes = 9_988 * 2; // F16 canonical genome (~19.98 KB)

    // Start 4 organisms on tile 0 ([0..16) x [0..16)) and move 3 of them across
    // x, y, and periodic negative seams into tiles 1, 2, and 3!
    let mut tiles: Vec<Vec<MigratingOrganism<u64>>> = vec![Vec::new(); 4];
    tiles[0] = vec![
        MigratingOrganism {
            id: OrganismId::new(1),
            position: [5.0, 5.0], // stays on tile 0
            genome_bytes,
            payload: 101,
        },
        MigratingOrganism {
            id: OrganismId::new(2),
            position: [16.5, 5.0], // crosses east -> tile 1
            genome_bytes,
            payload: 202,
        },
        MigratingOrganism {
            id: OrganismId::new(3),
            position: [5.0, 17.2], // crosses north -> tile 2
            genome_bytes,
            payload: 303,
        },
        MigratingOrganism {
            id: OrganismId::new(4),
            position: [-0.25, -0.5], // periodic wrap -> (31.75, 31.5) -> tile 3!
            genome_bytes,
            payload: 404,
        },
    ];

    let stats = migrate_organisms(&partition, &mut tiles).expect("migrate");
    assert_eq!(stats.total_organisms, 4);
    assert_eq!(stats.migrated_organisms, 3);
    assert_eq!(stats.migration_bytes, 3 * (256 + genome_bytes));
    assert_eq!(stats.max_tile_organisms, 1);
    assert_eq!(stats.min_tile_organisms, 1);
    assert!((stats.load_imbalance_ratio - 1.0).abs() < 1e-12);

    // Every tile now owns exactly the organism whose wrapped position lies in it.
    let mut seen = BTreeSet::new();
    for (t, bucket) in tiles.iter().enumerate() {
        assert_eq!(bucket.len(), 1);
        assert_eq!(
            partition.tile_of_position(bucket[0].position).expect("pos"),
            t
        );
        assert!(seen.insert(bucket[0].id));
    }
    assert_eq!(seen.len(), 4);
    assert_eq!(tiles[3][0].id, OrganismId::new(4));
    assert_eq!(tiles[3][0].payload, 404);

    // Duplicate `OrganismId` across tiles must be refused (`INV-07`) AND must
    // leave `tiles` completely intact (verify-before-mutate atomicity).
    tiles[1].push(MigratingOrganism {
        id: OrganismId::new(1),
        position: [20.0, 5.0],
        genome_bytes,
        payload: 999,
    });
    let before_err = tiles.clone();
    assert!(migrate_organisms(&partition, &mut tiles).is_err());
    assert_eq!(
        tiles, before_err,
        "failed migration must not lose or reorder organisms"
    );
}

/// Communication budget estimator computes sensible monotonic scaling across
/// `1x1`, `2x2`, and `4x2` partitions on the 400k Screenshot-scale grid.
#[test]
fn communication_budget_accounts_for_all_phases_at_screenshot_scale() {
    let grid = FieldGrid::new(1024, 1024, 1024.0, 1024.0).expect("1024x1024");
    let input = CommBudgetInput {
        planes: FIELD_PLANES,
        substeps: 1,
        halo_mode: HaloMode::PerSubstep,
        pyramid_levels: 5,
        boundary_demand_packets_per_tick: 12_000,
        boundary_deposit_packets_per_tick: 2_500,
        migrated_organisms_per_tick: 180,
        mean_genome_bytes: 9_988 * 2,
    };

    let p1 = TilePartition::new(grid, 1, 1).expect("1x1");
    let (b1, _) = estimate_comm_budget(&p1, input);
    assert_eq!(b1.total_bytes_per_tick, 0);
    assert_eq!(b1.barriers_per_tick, 0);

    let p4 = TilePartition::new(grid, 2, 2).expect("2x2");
    let (b4, est4) = estimate_comm_budget(&p4, input);
    assert_eq!(b4.devices, 4);
    assert!(b4.transport_halo_bytes > 0);
    assert!(b4.pyramid_halo_bytes > 0);
    assert!(b4.uptake_boundary_bytes > 0);
    assert!(b4.deposit_boundary_bytes > 0);
    assert!(b4.migration_bytes > 0);
    assert_eq!(
        b4.total_bytes_per_tick,
        b4.transport_halo_bytes
            + b4.pyramid_halo_bytes
            + b4.uptake_boundary_bytes
            + b4.deposit_boundary_bytes
            + b4.migration_bytes
    );
    assert_eq!(est4.len(), 5);
    // Faster interconnects must strictly reduce estimated communication ms.
    assert!(est4[4].total_comm_ms < est4[0].total_comm_ms);

    // Ultra-coarse level clamp: a 4x4 grid split 4x2 (each tile owns 2 cells)
    // with halo=2 can never report more remote cells than exist outside the tile.
    let tiny = FieldGrid::new(4, 4, 4.0, 4.0).expect("4x4");
    let tiny_part = TilePartition::new(tiny, 4, 2).expect("4x2");
    for t in 0..tiny_part.tile_count() {
        let owned = tiny_part.bounds(t).expect("bounds").cell_count() as u64;
        assert!(
            tiny_part.halo_cells_per_tile(t, 2) <= tiny.cell_count() - owned,
            "tile {t} halo cells exceeded total remote cells on tiny coarse grid"
        );
    }
}
