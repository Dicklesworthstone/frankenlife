//! FL-027 evidence (plan §9.4, §11.2, §11.3, §23.1): bounded cell bins and
//! the contribution reductions that use them. Covers hotspot correctness,
//! scratch limits, and deterministic ordering.
//!
//! The oracles are independent of `Bins`: a stable sort of item indices by
//! bucket, and a sort of each demand's footprint packets by (cell, organism).
//! That sort is what uptake and deposits used before FL-027.

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_field::pyramid::PLANES;
use flife_field::state::CHANNELS;
use flife_field::{
    Bins, BufferLimits, Cell, Demand, DepositSummary, Deposits, ExchangeKernel, ExchangeScratch,
    ExchangeStencil, FieldGrid, FieldState, Grants, RawSample, SensorPyramid, SensorQuery,
    apply_deposit_streams_to_carrier_on, apply_deposits, apply_deposits_on,
    apply_deposits_to_carrier_on, apply_grants, apply_grants_on, resolve_uptake,
    resolve_uptake_on_workers,
};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn grid(cells_x: u32, cells_y: u32) -> FieldGrid {
    FieldGrid::new(cells_x, cells_y, f64::from(cells_x), f64::from(cells_y)).expect("grid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn field(grid: FieldGrid, seed: u64) -> FieldState {
    let mut state = FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate");
    let mut next = lcg(seed);
    for y in 0..grid.cells_y() {
        for x in 0..grid.cells_x() {
            let energy = (next() * 4.0) as f32;
            let mut moments = [0.0_f32; CHANNELS];
            moments[(x as usize + y as usize) % CHANNELS] = energy * 0.5;
            state
                .set_cell(x, y, &Cell { energy, moments })
                .expect("cell inside the grid");
        }
    }
    state
}

/// A fixed linear congruential generator in `[0, 1)`.
fn lcg(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(state >> 40).unwrap_or(0)) / 16_777_216.0
    }
}

/// A permutation of `0..n` from `seed` (Fisher–Yates).
fn shuffled(n: usize, seed: u64) -> Vec<usize> {
    let mut next = lcg(seed);
    let mut order: Vec<usize> = (0..n).collect();
    for at in (1..n).rev() {
        let pick = ((next() * (at + 1) as f64) as usize).min(at);
        order.swap(at, pick);
    }
    order
}

/// `n` demands in a clustered population: a quarter in one hotspot cell,
/// a quarter in a 3×3 crowd, the rest spread, each at a random position
/// within its region, with identities scattered over `0..4n`.
fn clustered(grid: &FieldGrid, n: usize, seed: u64) -> Vec<Demand> {
    let mut next = lcg(seed);
    let identities = shuffled(4 * n, seed ^ 0x5eed);
    let (width, height) = (f64::from(grid.cells_x()), f64::from(grid.cells_y()));
    (0..n)
        .map(|at| {
            let position = match at % 4 {
                0 => [7.5, 5.5],
                1 => [20.0 + 3.0 * next(), 9.0 + 3.0 * next()],
                _ => [width * next(), height * next()],
            };
            Demand {
                organism: OrganismId::new(identities[at] as u64 + 1),
                position,
                amount: if at % 17 == 0 { 0.0 } else { next() * 0.01 },
            }
        })
        .collect()
}

/// The packets uptake must produce, by the pre-FL-027 rule: every positive
/// footprint share, sorted by (cell, organism).
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn oracle_packets(grid: &FieldGrid, demands: &[Demand]) -> Vec<(usize, OrganismId, f64)> {
    let mut packets = Vec::new();
    for demand in demands {
        let stencil = ExchangeStencil::at(grid, demand.position).expect("stencil");
        for (&cell, &weight) in stencil.cells().iter().zip(stencil.weights()) {
            if demand.amount * weight > 0.0 {
                packets.push((cell, demand.organism, demand.amount * weight));
            }
        }
    }
    packets.sort_by_key(|&(cell, organism, _)| (cell, organism));
    packets
}

/// One deposit to stage: organism, position, paid, offered, direction.
type Staged = (OrganismId, [f64; 2], f64, f64, [f64; CHANNELS]);

fn planes(state: &FieldState) -> Vec<Vec<u32>> {
    let mut planes = vec![state.energy().iter().map(|v| v.to_bits()).collect()];
    for channel in 0..u32::try_from(CHANNELS).unwrap_or(0) {
        let plane = state.moment(channel).unwrap_or(&[]);
        planes.push(plane.iter().map(|v| v.to_bits()).collect());
    }
    planes
}

#[test]
fn bins_are_a_stable_sort_by_bucket_whatever_the_clustering() -> Result<(), CoreError> {
    let mut bins = Bins::with_capacity("test bins", 1_000, 50_000, &BufferLimits::host())?;
    let allocated = bins.allocated();
    let mut next = lcg(7);
    let spread: Vec<usize> = (0..50_000).map(|_| (next() * 1_000.0) as usize).collect();
    let hotspot = vec![417; 50_000];
    let mut mixed = spread.clone();
    for slot in mixed.iter_mut().step_by(3) {
        *slot = 999;
    }
    for (name, keys, buckets) in [
        ("spread", &spread, 1_000),
        ("one hotspot", &hotspot, 1_000),
        ("a third in the last bucket", &mixed, 1_000),
        ("empty", &Vec::new(), 1_000),
        ("one bucket", &vec![0; 10], 1),
    ] {
        bins.group(buckets, keys.len(), |at| keys[at])?;
        // The oracle: item indices, stably sorted by bucket.
        let mut expected: Vec<u32> = (0..keys.len() as u32).collect();
        expected.sort_by_key(|&at| keys[at as usize]);
        assert_eq!(
            bins.order(),
            expected,
            "{name}: the grouping is not a stable sort"
        );
        assert_eq!(bins.buckets(), buckets, "{name}");
        let mut counts = vec![0; buckets];
        for &key in keys.iter() {
            counts[key] += 1;
        }
        for (bucket, &count) in counts.iter().enumerate() {
            let items = bins.bucket(bucket);
            assert_eq!(items.len(), count, "{name}: bucket {bucket}");
            assert!(items.iter().all(|&at| keys[at as usize] == bucket));
        }
        let occupied: Vec<usize> = bins.occupied().map(|(bucket, _)| bucket).collect();
        let nonempty: Vec<usize> = (0..buckets).filter(|&b| counts[b] > 0).collect();
        assert_eq!(occupied, nonempty, "{name}: occupied buckets, ascending");
        assert_eq!(
            bins.largest(),
            counts.iter().copied().max().unwrap_or(0),
            "{name}"
        );
        assert_eq!(bins.allocated(), allocated, "{name}: a grouping allocated");
    }
    assert_eq!(bins.largest(), 10);
    assert!(
        bins.bucket(5).is_empty() && bins.range(5).is_empty(),
        "out of range"
    );
    Ok(())
}

#[test]
fn bins_refuse_what_they_did_not_declare_and_are_left_empty() -> Result<(), CoreError> {
    let mut bins = Bins::with_capacity("test bins", 4, 10, &BufferLimits::host())?;
    let allocated = bins.allocated();
    bins.group(4, 3, |at| at)?;
    for (result, needle) in [
        (bins.group(5, 3, |at| at), "buckets beyond the declaration"),
        (
            bins.group(4, 11, |at| at % 4),
            "items beyond the declaration",
        ),
    ] {
        match result {
            Err(CoreError::BudgetExhausted { budget, .. }) => assert_eq!(budget, "test bins"),
            other => panic!("{needle}: expected BudgetExhausted, got {other:?}"),
        }
        assert!(
            bins.order().is_empty() && bins.buckets() == 0,
            "{needle}: not emptied"
        );
    }
    match bins.group(4, 3, |at| at * 2) {
        Err(CoreError::LayoutMismatch { buffer, detail }) => {
            assert_eq!(buffer, "test bins");
            assert!(detail.contains("bucket 4 of 4"), "{detail}");
        }
        other => panic!("expected LayoutMismatch, got {other:?}"),
    }
    assert!(bins.order().is_empty() && bins.largest() == 0);
    assert_eq!(bins.allocated(), allocated, "a refusal allocated");

    // Declaring more than the limits admit, or than u32 indexes, is refused.
    match Bins::with_capacity("small", 4, 5, &BufferLimits::new(16)) {
        Err(CoreError::BudgetExhausted {
            required: 20,
            available: 16,
            ..
        }) => {}
        other => panic!("expected a 20-byte refusal, got {other:?}"),
    }
    assert!(matches!(
        Bins::with_capacity("wide", 4, u64::from(u32::MAX) + 1, &BufferLimits::host()),
        Err(CoreError::BudgetExhausted { .. })
    ));
    Ok(())
}

#[test]
fn uptake_is_the_sorted_reduction_in_any_demand_order_and_any_clustering() {
    let grid = grid(32, 24);
    let snapshot = field(grid, 11);
    let demands = clustered(&grid, 6_000, 3);
    let mut grants = Grants::with_capacity(&grid, 6_000, &BufferLimits::host()).expect("declare");
    let allocated = grants.allocated();

    let summary = resolve_uptake(&snapshot, &demands, &mut grants).expect("resolve");
    let got: Vec<(usize, OrganismId, f64)> = grants
        .packets()
        .iter()
        .map(|packet| (packet.cell, packet.organism, packet.requested))
        .collect();
    assert_eq!(
        got,
        oracle_packets(&grid, &demands),
        "not the (cell, organism) order"
    );
    assert!(
        grants
            .packets()
            .iter()
            .all(|p| demands[p.demand].organism == p.organism),
        "a packet names the wrong demand"
    );
    assert!(summary.contended_cells > 0, "the hotspot must be contended");
    assert_eq!(summary.largest_cell_packets, grants.bins().largest());
    // The hotspot cell holds at least every positive demand placed at its
    // center (its footprint is that one cell).
    let hot = demands
        .iter()
        .filter(|demand| demand.position == [7.5, 5.5] && demand.amount > 0.0)
        .count();
    assert!(hot > 1_000, "{hot}");
    assert!(
        summary.largest_cell_packets >= hot,
        "{}",
        summary.largest_cell_packets
    );
    assert_eq!(
        grants.allocated(),
        allocated,
        "a declared resolution allocated"
    );

    // Any order of the same demands grants the same, bit for bit.
    let (packets, debits) = (grants.packets().to_vec(), grants.debits().to_vec());
    let totals = grants.totals().to_vec();
    for seed in [1, 2, 3] {
        let order = shuffled(demands.len(), seed);
        let permuted: Vec<Demand> = order.iter().map(|&at| demands[at]).collect();
        let again = resolve_uptake(&snapshot, &permuted, &mut grants).expect("resolve");
        assert_eq!(again, summary, "seed {seed}");
        assert_eq!(grants.debits(), debits, "seed {seed}");
        for (got, want) in grants.packets().iter().zip(&packets) {
            assert_eq!(
                (
                    got.cell,
                    got.organism,
                    got.requested.to_bits(),
                    got.granted.to_bits()
                ),
                (
                    want.cell,
                    want.organism,
                    want.requested.to_bits(),
                    want.granted.to_bits()
                ),
                "seed {seed}"
            );
            assert_eq!(order[got.demand], want.demand, "seed {seed}: demand index");
        }
        for (at, &original) in order.iter().enumerate() {
            assert_eq!(grants.totals()[at].to_bits(), totals[original].to_bits());
        }
        assert_eq!(grants.allocated(), allocated, "seed {seed}: allocated");
    }
}

#[test]
fn a_hotspot_of_every_organism_drains_its_cell_exactly_and_ties_go_to_lower_identities() {
    let grid = grid(16, 16);
    let mut snapshot = FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate");
    let energy = 3.0_f32;
    snapshot
        .set_cell(
            5,
            9,
            &Cell {
                energy,
                moments: [0.0; CHANNELS],
            },
        )
        .expect("cell");
    // 20,000 equal demands at the center of cell (5, 9), whose footprint is
    // that one cell, given in descending identity order.
    let n = 20_000_u64;
    let demands: Vec<Demand> = (0..n)
        .rev()
        .map(|at| Demand {
            organism: OrganismId::new(1_000 + at),
            position: [5.5, 9.5],
            amount: 0.25,
        })
        .collect();
    let mut grants = Grants::with_capacity(&grid, n, &BufferLimits::host()).expect("declare");
    let allocated = grants.allocated();
    let summary = resolve_uptake(&snapshot, &demands, &mut grants).expect("resolve");
    assert_eq!(
        (
            summary.cells,
            summary.contended_cells,
            summary.drained_cells
        ),
        (1, 1, 1)
    );
    assert_eq!(summary.largest_cell_packets, n as usize);
    assert_eq!(grants.bins().largest(), n as usize);
    assert_eq!(grants.allocated(), allocated, "the hotspot allocated");
    let [debit] = grants.debits() else {
        panic!("one debit expected, got {}", grants.debits().len());
    };
    assert_eq!(
        (debit.energy_after, debit.granted),
        (0.0, f64::from(energy))
    );

    // Every share is `3/20000` cell: whole quanta, and the leftover quanta
    // go one each to the lowest identities.
    let quantum = f64::from(energy) / f64::from(0xc0_0000_u32);
    let shares: Vec<u64> = grants
        .packets()
        .iter()
        .map(|packet| (packet.granted / quantum) as u64)
        .collect();
    assert!(
        grants
            .packets()
            .windows(2)
            .all(|w| w[0].organism < w[1].organism)
    );
    let (low, high) = (shares.iter().min().copied(), shares.iter().max().copied());
    assert_eq!(
        high.zip(low).map(|(h, l)| h - l),
        Some(1),
        "shares differ by one quantum"
    );
    let first_low = shares.iter().position(|&s| Some(s) == low).unwrap_or(0);
    assert!(
        shares[first_low..].iter().all(|&s| Some(s) == low),
        "a tie went up"
    );
    assert_eq!(shares.iter().sum::<u64>(), 0xc0_0000, "the cell's quanta");

    let mut after = snapshot.clone();
    assert_eq!(apply_grants(&mut after, &mut grants).expect("apply"), 3.0);
    assert_eq!(after.energy().iter().sum::<f32>(), 0.0);
}

#[test]
fn uptake_beyond_its_declaration_is_refused_before_anything_is_staged() {
    let grid = grid(8, 8);
    let snapshot = field(grid, 5);
    let demands = clustered(&grid, 40, 9);
    let mut grants = Grants::with_capacity(&grid, 39, &BufferLimits::host()).expect("declare");
    let allocated = grants.allocated();
    match resolve_uptake(&snapshot, &demands, &mut grants) {
        Err(CoreError::BudgetExhausted {
            budget,
            required: 40,
            available: 39,
        }) => {
            assert_eq!(budget, "uptake packets");
        }
        other => panic!("expected the declaration's refusal, got {other:?}"),
    }
    assert!(grants.packets().is_empty() && grants.debits().is_empty());
    assert_eq!(grants.allocated(), allocated, "a refusal allocated");
    resolve_uptake(&snapshot, &demands[..39], &mut grants).expect("39 fit");
    assert_eq!(grants.allocated(), allocated);

    // Of several invalid demands, the first given is reported, even when a
    // later one has the lower identity (and is staged first).
    let mut invalid = demands[..39].to_vec();
    let (first, later) = (0..39)
        .flat_map(|i| (i + 1..39).map(move |j| (i, j)))
        .find(|&(i, j)| invalid[j].organism < invalid[i].organism)
        .expect("a later demand with a lower identity");
    invalid[later].amount = f64::NAN;
    invalid[first].amount = -1.0;
    match resolve_uptake(&snapshot, &invalid, &mut grants) {
        Err(CoreError::NumericalInvalid { detail, .. }) => {
            let organism = invalid[first].organism.get();
            assert!(
                detail.contains(&format!("organism {organism} requests -1")),
                "{detail}"
            );
        }
        other => panic!("expected the first invalid demand, got {other:?}"),
    }
}

#[test]
fn deposits_reduce_the_same_in_any_staging_order_and_any_clustering() {
    let grid = grid(32, 24);
    let base = field(grid, 21);
    let demands = clustered(&grid, 5_000, 17);
    let mut next = lcg(4);
    let deposits: Vec<Staged> = demands
        .iter()
        .map(|demand| {
            let paid = next() * 0.02;
            let mut direction = [0.0; CHANNELS];
            direction[(next() * CHANNELS as f64) as usize % CHANNELS] = next() - 0.5;
            (
                demand.organism,
                demand.position,
                paid,
                paid * 0.75,
                direction,
            )
        })
        .collect();
    let limits = BufferLimits::host();
    let mut reduced = Vec::new();
    for seed in [0, 1, 2] {
        let order = if seed == 0 {
            (0..deposits.len()).collect()
        } else {
            shuffled(deposits.len(), seed)
        };
        let mut staged = Deposits::with_capacity(&grid, 5_000, &limits).expect("declare");
        let allocated = staged.allocated();
        for &at in &order {
            let (organism, position, paid, offered, direction) = deposits[at];
            staged
                .push(&grid, organism, position, paid, offered, &direction)
                .expect("stage");
        }
        let mut state = base.clone();
        let summary = apply_deposits(&mut state, &mut staged).expect("apply");
        assert!(
            staged.bins().largest() >= 1_250,
            "{}",
            staged.bins().largest()
        );
        assert_eq!(summary.cells, staged.bins().occupied().count());
        assert_eq!(
            staged.allocated(),
            allocated,
            "seed {seed}: staging allocated"
        );
        reduced.push((summary, planes(&state)));
    }
    for (seed, other) in reduced.iter().enumerate().skip(1) {
        assert_eq!(other.0, reduced[0].0, "seed {seed}: summary");
        assert!(other.1 == reduced[0].1, "seed {seed}: the field differs");
    }
    assert!(reduced[0].1 != planes(&base), "nothing was deposited");

    // Staging beyond the declaration is refused, and stages nothing.
    let mut staged = Deposits::with_capacity(&grid, 2, &limits).expect("declare");
    for (organism, position, paid, offered, direction) in deposits.iter().take(2) {
        staged
            .push(&grid, *organism, *position, *paid, *offered, direction)
            .expect("stage");
    }
    let (organism, position, paid, offered, direction) = deposits[2];
    match staged.push(&grid, organism, position, paid, offered, &direction) {
        Err(CoreError::BudgetExhausted {
            budget,
            required: 3,
            available: 2,
        }) => {
            assert_eq!(budget, "deposit packets");
        }
        other => panic!("expected the declaration's refusal, got {other:?}"),
    }
    assert_eq!(staged.len(), 2);
}

/// Plan §8.1's phase 7 stages emissions on several threads (C3): each
/// thread stages its run of organisms into deposits of its own
/// ([`Deposits::fit_staging`]), and the tick appends them in run order
/// ([`Deposits::append`]). The tick's deposits must then reduce exactly as
/// if every deposit had been pushed into them in that order: the same
/// field to the bit, the same summary and the same bins, with no
/// allocation on either side. An append that is refused moves nothing.
#[test]
fn appended_deposits_reduce_as_if_pushed_in_order() {
    let grid = grid(32, 24);
    let base = field(grid, 22);
    let demands = clustered(&grid, 3_000, 19);
    let mut next = lcg(5);
    let deposits: Vec<Staged> = demands
        .iter()
        .map(|demand| {
            let paid = next() * 0.02;
            let mut direction = [0.0; CHANNELS];
            direction[(next() * CHANNELS as f64) as usize % CHANNELS] = next() - 0.5;
            direction[(next() * CHANNELS as f64) as usize % CHANNELS] += next() * 0.4;
            (
                demand.organism,
                demand.position,
                paid,
                paid * 0.75,
                direction,
            )
        })
        .collect();
    let limits = BufferLimits::host();
    let push = |staged: &mut Deposits, deposit: &Staged| {
        let (organism, position, paid, offered, direction) = *deposit;
        staged.push(&grid, organism, position, paid, offered, &direction)
    };

    let mut pushed = Deposits::with_capacity(&grid, 3_000, &limits).expect("declare");
    for deposit in &deposits {
        push(&mut pushed, deposit).expect("stage");
    }
    let mut direct = base.clone();
    let expected = apply_deposits(&mut direct, &mut pushed).expect("apply");
    let expected_bins: Vec<_> = pushed.bins().occupied().collect();
    assert!(planes(&direct) != planes(&base), "nothing was deposited");

    for runs in [1, 2, 7, 16, 3_000] {
        let length = deposits.len().div_ceil(runs);
        let mut whole = Deposits::with_capacity(&grid, 3_000, &limits).expect("declare");
        let allocated = whole.allocated();
        let mut lane = Deposits::new();
        lane.fit_staging(length).expect("room for a run");
        let room = lane.allocated();
        for run in deposits.chunks(length) {
            for deposit in run {
                push(&mut lane, deposit).expect("stage");
            }
            assert_eq!(
                lane.allocated(),
                room,
                "{runs} runs: a run outgrew its room"
            );
            whole.append(&mut lane).expect("append");
            assert!(lane.is_empty(), "appending empties the lane");
            assert_eq!(lane.allocated(), room, "the lane keeps its room");
        }
        assert_eq!(
            whole.allocated(),
            allocated,
            "{runs} runs: appending allocated"
        );
        assert_eq!(whole.len(), deposits.len());
        let mut state = base.clone();
        let summary = apply_deposits(&mut state, &mut whole).expect("apply");
        assert_eq!(summary, expected, "{runs} runs: summary");
        assert_eq!(
            (
                summary.paid.to_bits(),
                summary.offered.to_bits(),
                summary.applied.to_bits()
            ),
            (
                expected.paid.to_bits(),
                expected.offered.to_bits(),
                expected.applied.to_bits()
            ),
            "{runs} runs: summary bits"
        );
        assert!(
            planes(&state) == planes(&direct),
            "{runs} runs: the field differs"
        );
        assert_eq!(
            whole.bins().occupied().collect::<Vec<_>>(),
            expected_bins,
            "{runs} runs: bins"
        );
    }

    // Refusals move nothing.
    let mut whole = Deposits::with_capacity(&grid, 2, &limits).expect("declare");
    let mut lane = Deposits::new();
    for deposit in deposits.iter().take(3) {
        push(&mut lane, deposit).expect("stage");
    }
    match whole.append(&mut lane) {
        Err(CoreError::BudgetExhausted {
            budget,
            required: 3,
            available: 2,
        }) => assert_eq!(budget, "deposit packets"),
        other => panic!("expected the declaration's refusal, got {other:?}"),
    }
    assert_eq!(
        (whole.len(), lane.len()),
        (0, 3),
        "a refused append moved deposits"
    );

    let other = self::grid(16, 12);
    let mut elsewhere = Deposits::new();
    let (organism, _, paid, offered, direction) = deposits[0];
    elsewhere
        .push(&other, organism, [1.5, 1.5], paid, offered, &direction)
        .expect("stage on the other grid");
    let mut whole = Deposits::with_capacity(&grid, 3_000, &limits).expect("declare");
    push(&mut whole, &deposits[1]).expect("stage");
    assert!(matches!(
        whole.append(&mut elsewhere),
        Err(CoreError::StateConflict { .. })
    ));
    assert_eq!((whole.len(), elsewhere.len()), (1, 1));

    let mut applied = base.clone();
    apply_deposits(&mut applied, &mut whole).expect("apply");
    let mut lane = Deposits::new();
    push(&mut lane, &deposits[2]).expect("stage");
    assert!(matches!(
        whole.append(&mut lane),
        Err(CoreError::StateConflict { .. })
    ));
    assert_eq!(lane.len(), 1, "appending to applied deposits moved one");
}

/// A summary's every field, its sums as bits.
fn summary_bits(summary: &DepositSummary) -> (usize, usize, u64, u64, u64) {
    (
        summary.organisms,
        summary.cells,
        summary.paid.to_bits(),
        summary.offered.to_bits(),
        summary.applied.to_bits(),
    )
}

/// Phase 9 reduces deposits on the field's workers (C4): the cells are
/// split into contiguous runs, and each run's thread reduces and writes its
/// own cells. Every worker count must give one thread's field and summary
/// to the bit, with no allocation of the deposits' scratch. The clustered
/// population puts a quarter of the organisms in one hotspot cell, so some
/// runs' bounds fall inside it and those runs are empty; the counts include
/// zero (taken as one) and more workers than there are runs.
#[test]
fn deposits_reduce_the_same_on_any_number_of_workers() {
    let grid = grid(32, 24);
    let base = field(grid, 23);
    let demands = clustered(&grid, 6_000, 29);
    let mut next = lcg(6);
    let deposits: Vec<Staged> = demands
        .iter()
        .map(|demand| {
            let paid = next() * 0.02;
            let mut direction = [0.0; CHANNELS];
            direction[(next() * CHANNELS as f64) as usize % CHANNELS] = next() - 0.5;
            direction[(next() * CHANNELS as f64) as usize % CHANNELS] += next() * 0.4;
            (
                demand.organism,
                demand.position,
                paid,
                paid * 0.75,
                direction,
            )
        })
        .collect();
    let limits = BufferLimits::host();
    let staged = || {
        let mut staged = Deposits::with_capacity(&grid, 6_000, &limits).expect("declare");
        for &(organism, position, paid, offered, direction) in &deposits {
            staged
                .push(&grid, organism, position, paid, offered, &direction)
                .expect("stage");
        }
        staged
    };

    let mut one = base.clone();
    let mut reference = staged();
    let expected = summary_bits(&apply_deposits(&mut one, &mut reference).expect("apply"));
    let expected_planes = planes(&one);
    assert!(expected_planes != planes(&base), "nothing was deposited");
    assert_eq!(expected.1, reference.bins().occupied().count());
    let packets = reference.bins().order().len();
    assert!(
        packets >= 16 * 1_024,
        "{packets} packets: too few for 16 runs"
    );
    assert!(
        reference.bins().largest() > packets / 16,
        "the hotspot holds {} of {packets}: no run falls inside it",
        reference.bins().largest()
    );

    for workers in [0, 1, 2, 3, 5, 8, 13, 16, 17, 64] {
        let mut state = base.clone();
        let mut deposits = staged();
        let allocated = deposits.allocated();
        let summary = apply_deposits_on(&mut state, &mut deposits, workers).expect("apply");
        assert_eq!(
            summary_bits(&summary),
            expected,
            "{workers} workers: summary"
        );
        assert!(
            planes(&state) == expected_planes,
            "{workers} workers: the field differs"
        );
        assert_eq!(
            deposits.allocated(),
            allocated,
            "{workers} workers: the reduction allocated"
        );
        assert!(deposits.is_applied());
    }
}

/// The grants' debit on any number of workers writes every plane as one
/// thread does, drained cells included, and refuses what one thread
/// refuses without writing anything.
#[test]
fn grants_apply_the_same_on_any_number_of_workers() {
    let grid = grid(32, 24);
    let mut base = field(grid, 31);
    // A moment in every channel of every cell, so that every plane has
    // debited cells to scale.
    let mut next = lcg(8);
    for y in 0..grid.cells_y() {
        for x in 0..grid.cells_x() {
            let mut cell = base.cell(x, y).expect("a cell inside the grid");
            for moment in &mut cell.moments {
                *moment = cell.energy * (next() as f32 - 0.5) * 0.05;
            }
            base.set_cell(x, y, &cell).expect("a cell inside the grid");
        }
    }
    let demands = clustered(&grid, 6_000, 37);
    let resolved = || {
        let mut grants =
            Grants::with_capacity(&grid, 6_000, &BufferLimits::host()).expect("declare");
        resolve_uptake(&base, &demands, &mut grants).expect("resolve");
        grants
    };

    let mut one = base.clone();
    let mut grants = resolved();
    let granted = apply_grants(&mut one, &mut grants).expect("apply");
    let expected = planes(&one);
    let debited = |drained: bool| {
        grants
            .debits()
            .iter()
            .filter(|debit| debit.granted > 0.0 && (debit.energy_after == 0.0) == drained)
            .count()
    };
    assert!(debited(true) >= 1, "no drained cell");
    assert!(debited(false) > 300, "{} scaled cells", debited(false));
    for (plane, (before, after)) in planes(&base).iter().zip(&expected).enumerate() {
        assert!(before != after, "plane {plane} was not debited");
    }

    for workers in [0, 1, 2, 3, 5, 16, 31, 32, 33, 64] {
        let mut state = base.clone();
        let mut grants = resolved();
        let got = apply_grants_on(&mut state, &mut grants, workers).expect("apply");
        assert_eq!(got.to_bits(), granted.to_bits(), "{workers} workers");
        assert!(
            planes(&state) == expected,
            "{workers} workers: the field differs"
        );
        // Applied again: refused as on one thread, with nothing written.
        let refused = apply_grants_on(&mut state, &mut grants, workers).expect_err("twice");
        let mut again = one.clone();
        let mut applied = resolved();
        apply_grants(&mut base.clone(), &mut applied).expect("apply");
        assert_eq!(
            refused,
            apply_grants(&mut again, &mut applied).expect_err("twice"),
            "{workers} workers"
        );
        assert!(planes(&state) == expected, "{workers} workers: written");

        // A debited cell changed since the grants were resolved: refused,
        // with nothing written.
        let mut stale = base.clone();
        let cell = grants.debits()[grants.debits().len() / 2].cell;
        stale.energy_mut()[cell] += 1.0;
        let before = planes(&stale);
        let mut fresh = resolved();
        let refused = apply_grants_on(&mut stale, &mut fresh, workers).expect_err("stale");
        let mut fresh_one = resolved();
        assert_eq!(
            refused,
            apply_grants(&mut stale.clone(), &mut fresh_one).expect_err("stale"),
            "{workers} workers"
        );
        assert!(planes(&stale) == before, "{workers} workers: written");
    }
}

#[test]
fn uptake_resolves_the_same_on_any_number_of_workers() {
    let grid = grid(32, 32);
    let snapshot = field(grid, 99);
    // 4,000 demands including a large hotspot (> 32 packets in one cell) to
    // exercise both stack and heap paths and parallel scatter + cell solve.
    let demands = clustered(&grid, 4_000, 123);
    let mut serial =
        Grants::with_capacity(&grid, demands.len() as u64, &BufferLimits::host()).expect("declare");
    let expected = resolve_uptake(&snapshot, &demands, &mut serial).expect("serial");
    let want_packets = serial.packets().to_vec();
    let want_debits = serial.debits().to_vec();
    let want_totals = serial.totals().to_vec();

    for workers in [0, 1, 2, 4, 8, 16] {
        let mut grants = Grants::with_capacity(&grid, demands.len() as u64, &BufferLimits::host())
            .expect("declare");
        let allocated = grants.allocated();
        let got = resolve_uptake_on_workers(&snapshot, &demands, &mut grants, workers)
            .expect("parallel resolve");
        assert_eq!(got, expected, "{workers} workers");
        assert_eq!(grants.debits(), want_debits, "{workers} workers debits");
        assert_eq!(grants.packets(), want_packets, "{workers} workers packets");
        assert_eq!(grants.totals(), want_totals, "{workers} workers totals");
        assert_eq!(grants.allocated(), allocated, "{workers} workers allocated");
    }
}

#[test]
fn pyramid_build_on_and_sample_batch_match_serial_bit_for_bit() {
    // 128×128 >= 16,384 cells so `build_on` exercises the parallel 33-plane path.
    let grid = grid(128, 128);
    let snapshot = field(grid, 77);
    let mut serial = SensorPyramid::new(grid, 4, &BufferLimits::host()).expect("pyramid");
    let serial_view = serial.build(&snapshot).expect("serial build");

    let mut next = lcg(555);
    let level0_queries: Vec<SensorQuery> = (0..100)
        .map(|_| SensorQuery {
            position: [next() * 128.0, next() * 128.0],
            scale: 1.0,
        })
        .collect();
    let mixed_queries: Vec<SensorQuery> = (0..100)
        .map(|_| SensorQuery {
            position: [next() * 128.0, next() * 128.0],
            scale: 1.0 + next() * 15.0,
        })
        .collect();

    let zero = RawSample {
        values: [0.0; PLANES],
        dx: [0.0; PLANES],
        dy: [0.0; PLANES],
    };
    let compare = |got: &[RawSample], want: &[RawSample], label: &str| {
        assert_eq!(got.len(), want.len(), "{label}");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert_eq!(
                g.values.map(f64::to_bits),
                w.values.map(f64::to_bits),
                "{label} values[{i}]"
            );
            assert_eq!(
                g.dx.map(f64::to_bits),
                w.dx.map(f64::to_bits),
                "{label} dx[{i}]"
            );
            assert_eq!(
                g.dy.map(f64::to_bits),
                w.dy.map(f64::to_bits),
                "{label} dy[{i}]"
            );
        }
    };

    let want_l0: Vec<RawSample> = level0_queries
        .iter()
        .map(|q| serial_view.sample(q.position, q.scale).expect("l0"))
        .collect();
    let want_mixed: Vec<RawSample> = mixed_queries
        .iter()
        .map(|q| serial_view.sample(q.position, q.scale).expect("mixed"))
        .collect();

    for workers in [1, 2, 4, 16] {
        let mut par = SensorPyramid::new(grid, 4, &BufferLimits::host()).expect("pyramid");
        let view = par.build_on(&snapshot, workers).expect("parallel build");
        let mut got_l0 = vec![zero; level0_queries.len()];
        view.sample_batch(&level0_queries, &mut got_l0)
            .expect("batch l0");
        compare(&got_l0, &want_l0, "level0");

        let mut got_mixed = vec![zero; mixed_queries.len()];
        view.sample_batch(&mixed_queries, &mut got_mixed)
            .expect("batch mixed");
        compare(&got_mixed, &want_mixed, "mixed");
    }

    // Also exercise power-of-two (non-unit) and non-power-of-two cell sizes,
    // including wrapped negative/overshoot coordinates and exact cell centers.
    for (label, cell_w, cell_h) in [("pow2", 2.0_f64, 0.5_f64), ("non_pow2", 1.3_f64, 0.7_f64)] {
        let custom = FieldGrid::new(32, 32, 32.0 * cell_w, 32.0 * cell_h).expect("grid");
        let snap = field(custom, 88);
        let mut pyr = SensorPyramid::new(custom, 3, &BufferLimits::host()).expect("pyramid");
        let view = pyr.build_on(&snap, 4).expect("build");
        let finest = cell_w.max(cell_h);
        let mut rng = lcg(999);
        let l0: Vec<SensorQuery> = (0..64)
            .map(|i| SensorQuery {
                position: if i % 8 == 0 {
                    [-0.25 * cell_w, 32.25 * cell_h]
                } else {
                    [rng() * 32.0 * cell_w, rng() * 32.0 * cell_h]
                },
                scale: finest,
            })
            .collect();
        let mixed: Vec<SensorQuery> = (0..64)
            .map(|_| SensorQuery {
                position: [(rng() - 0.1) * 35.0 * cell_w, (rng() - 0.1) * 35.0 * cell_h],
                scale: finest * (1.0 + rng() * 6.9),
            })
            .collect();
        let want0: Vec<RawSample> = l0
            .iter()
            .map(|q| view.sample(q.position, q.scale).expect("sample l0"))
            .collect();
        let mut got0 = vec![zero; l0.len()];
        view.sample_batch(&l0, &mut got0).expect("batch l0");
        compare(&got0, &want0, label);

        let wantm: Vec<RawSample> = mixed
            .iter()
            .map(|q| view.sample(q.position, q.scale).expect("sample mixed"))
            .collect();
        let mut gotm = vec![zero; mixed.len()];
        view.sample_batch(&mixed, &mut gotm).expect("batch mixed");
        compare(&gotm, &wantm, label);
    }
}

/// Regression: `Bins::group` and `Bins::range` must not overflow on
/// `bucket == usize::MAX` (which internal callers use as their out-of-range
/// sentinel).
#[test]
fn bins_handles_usize_max_bucket_without_overflow_panic() {
    let mut bins = Bins::new("test-bins");
    let refused = bins.group(4, 1, |_| usize::MAX);
    assert!(
        matches!(refused, Err(CoreError::LayoutMismatch { .. })),
        "expected LayoutMismatch for usize::MAX bucket, got {refused:?}"
    );
    assert_eq!(bins.buckets(), 0);
    assert_eq!(bins.range(usize::MAX), 0..0);
    assert!(bins.bucket(usize::MAX).is_empty());

    bins.group(4, 2, |item| item).expect("valid grouping");
    assert_eq!(bins.range(usize::MAX), 0..0);
    assert!(bins.bucket(usize::MAX).is_empty());
}

/// Regression: deposits whose total overflows F64 (`CompensatedSum` -> NaN /
/// Inf) must be refused cleanly by `apply_deposits` without writing `+Inf` or
/// `NaN` into the field (INV-01).
#[test]
fn apply_deposits_refuses_f64_overflow_without_mutating_field() {
    let g = grid(4, 4);
    let mut state = FieldState::zeroed(g, &BufferLimits::host()).expect("field");
    let before: Vec<u32> = state.as_slice().iter().map(|v| v.to_bits()).collect();
    let mut deposits = Deposits::new();
    let mut dir = [0.0; CHANNELS];
    dir[0] = 1.0;
    deposits
        .push(&g, OrganismId::new(1), [0.5, 0.5], 1.0e308, 1.0e308, &dir)
        .expect("stages first");
    deposits
        .push(&g, OrganismId::new(2), [0.5, 0.5], 1.0e308, 1.0e308, &dir)
        .expect("stages second");

    let err =
        apply_deposits(&mut state, &mut deposits).expect_err("overflowing total must be refused");
    assert!(
        matches!(err, CoreError::NumericalInvalid { .. }),
        "expected NumericalInvalid, got {err:?}"
    );
    let after: Vec<u32> = state.as_slice().iter().map(|v| v.to_bits()).collect();
    assert_eq!(
        after, before,
        "refused apply_deposits must leave field state bit-for-bit unchanged"
    );
}

/// Regression: mixed chunks containing 31 level-0 queries + 1 mutated
/// blended/coarse query (and arbitrary blends across levels 0..=3) must match
/// scalar `PyramidView::sample` AND `PyramidView::sample_dual` bit-for-bit
/// on unit, power-of-two, and non-power-of-two grids.
#[test]
fn pyramid_mixed_scale_chunks_match_scalar_bit_for_bit_single_and_dual() {
    use flife_field::pyramid::{DualRawSample, MAX_PLANES};

    let zero_single = RawSample {
        values: [0.0; PLANES],
        dx: [0.0; PLANES],
        dy: [0.0; PLANES],
    };
    let zero_dual = DualRawSample {
        values: [0.0; MAX_PLANES],
        dx: [0.0; MAX_PLANES],
        dy: [0.0; MAX_PLANES],
    };

    for (label, cell_w, cell_h) in [
        ("unit", 1.0_f64, 1.0_f64),
        ("pow2", 2.0_f64, 0.5_f64),
        ("non_pow2", 1.3_f64, 0.7_f64),
    ] {
        let g = FieldGrid::new(32, 32, 32.0 * cell_w, 32.0 * cell_h).expect("grid");
        let finest = cell_w.max(cell_h);
        let mut rng = lcg(2026);
        // 96 queries: block 0 has 31 level-0 + 1 level-0/1 blend; block 1 has
        // 30 level-0 + 1 level-0/1 + 1 coarse level-2/3; block 2 has all mixed.
        let mut queries = Vec::with_capacity(96);
        for i in 0..96 {
            let position = [rng() * 32.0 * cell_w, rng() * 32.0 * cell_h];
            let scale = match i {
                17 => finest * 1.37,
                40 => finest * 1.75,
                55 => finest * 4.9,
                64.. => finest * (1.0 + rng() * 7.0),
                _ => finest,
            };
            queries.push(SensorQuery { position, scale });
        }

        // Single-carrier 33-plane pyramid.
        let snap = field(g, 303);
        let mut pyr = SensorPyramid::new(g, 3, &BufferLimits::host()).expect("single pyramid");
        let view = pyr.build_on(&snap, 4).expect("build single");
        let want: Vec<RawSample> = queries
            .iter()
            .map(|q| view.sample(q.position, q.scale).expect("scalar single"))
            .collect();
        let mut got = vec![zero_single; queries.len()];
        view.sample_batch(&queries, &mut got).expect("batch single");
        for (i, (g_s, w_s)) in got.iter().zip(&want).enumerate() {
            assert_eq!(
                g_s.values.map(f64::to_bits),
                w_s.values.map(f64::to_bits),
                "{label} single v[{i}]"
            );
            assert_eq!(
                g_s.dx.map(f64::to_bits),
                w_s.dx.map(f64::to_bits),
                "{label} single dx[{i}]"
            );
            assert_eq!(
                g_s.dy.map(f64::to_bits),
                w_s.dy.map(f64::to_bits),
                "{label} single dy[{i}]"
            );
        }

        // Dual-carrier 34-plane pyramid.
        let mut dual_state =
            FieldState::zeroed_dual_carrier(g, &BufferLimits::host()).expect("dual field");
        let mut fill_rng = lcg(404);
        for slot in dual_state.as_mut_slice() {
            *slot = (fill_rng() * 3.0) as f32;
        }
        let mut dual_pyr = SensorPyramid::new_with_planes(
            g,
            3,
            dual_state.planes() as usize,
            &BufferLimits::host(),
        )
        .expect("dual pyramid");
        let dual_view = dual_pyr.build_on(&dual_state, 4).expect("build dual");
        let want_dual: Vec<DualRawSample> = queries
            .iter()
            .map(|q| {
                dual_view
                    .sample_dual(q.position, q.scale)
                    .expect("scalar dual")
            })
            .collect();
        let mut got_dual = vec![zero_dual; queries.len()];
        dual_view
            .sample_dual_batch(&queries, &mut got_dual)
            .expect("batch dual");
        for (i, (g_s, w_s)) in got_dual.iter().zip(&want_dual).enumerate() {
            assert_eq!(
                g_s.values.map(f64::to_bits),
                w_s.values.map(f64::to_bits),
                "{label} dual v[{i}]"
            );
            assert_eq!(
                g_s.dx.map(f64::to_bits),
                w_s.dx.map(f64::to_bits),
                "{label} dual dx[{i}]"
            );
            assert_eq!(
                g_s.dy.map(f64::to_bits),
                w_s.dy.map(f64::to_bits),
                "{label} dual dy[{i}]"
            );
        }
    }
}

/// Round-2 regression: 256-query 8x8 tile-sorted spatial batching in
/// `PyramidView::sample_batch` / `sample_dual_batch`, fused multi-stream
/// `apply_deposit_streams_to_carrier_on` (including contiguous identity-sequence
/// skip), and hoisted `exp_neg_4` `FixedWorldGaussianV1::footprint_into` must
/// all match their scalar oracles bit-for-bit.
#[test]
fn round2_spatial_pyramid_256_batch_fused_streams_and_exp_neg_4_footprint_are_bit_exact() {
    use flife_field::{DualRawSample, MAX_PLANES};

    // 1. 320-query pyramid batch (full 256-query tile-sorted block + 64 tail).
    let g = FieldGrid::new(32, 32, 32.0, 32.0).expect("grid");
    let snap = field(g, 909);
    let mut pyr = SensorPyramid::new(g, 3, &BufferLimits::host()).expect("pyr");
    let view = pyr.build_on(&snap, 4).expect("build");
    let mut rng = lcg(202_611);
    let mut queries = Vec::with_capacity(320);
    for i in 0..320 {
        let position = [rng() * 32.0, rng() * 32.0];
        let scale = match i % 5 {
            0 | 1 => 1.0,                // level 0 (> 32 queries -> triggers tile sort)
            2 | 3 => 1.0 + rng() * 0.99, // level 0/1 blend (> 32 queries -> triggers tile sort)
            _ => 2.1 + rng() * 5.0,      // coarse levels (> 8 queries -> triggers tile sort)
        };
        queries.push(SensorQuery { position, scale });
    }
    let zero_single = RawSample {
        values: [0.0; PLANES],
        dx: [0.0; PLANES],
        dy: [0.0; PLANES],
    };
    let mut got = vec![zero_single; queries.len()];
    view.sample_batch(&queries, &mut got).expect("batch 320");
    for (i, (q, g_s)) in queries.iter().zip(&got).enumerate() {
        let w_s = view.sample(q.position, q.scale).expect("scalar");
        assert_eq!(
            g_s.values.map(f64::to_bits),
            w_s.values.map(f64::to_bits),
            "v[{i}]"
        );
        assert_eq!(
            g_s.dx.map(f64::to_bits),
            w_s.dx.map(f64::to_bits),
            "dx[{i}]"
        );
        assert_eq!(
            g_s.dy.map(f64::to_bits),
            w_s.dy.map(f64::to_bits),
            "dy[{i}]"
        );
    }

    let mut dual_state =
        FieldState::zeroed_dual_carrier(g, &BufferLimits::host()).expect("dual field");
    let mut fill_rng = lcg(777);
    for slot in dual_state.as_mut_slice() {
        *slot = (fill_rng() * 2.5) as f32;
    }
    let mut dual_pyr =
        SensorPyramid::new_with_planes(g, 3, dual_state.planes() as usize, &BufferLimits::host())
            .expect("dual pyr");
    let dual_view = dual_pyr.build_on(&dual_state, 4).expect("dual build");
    let zero_dual = DualRawSample {
        values: [0.0; MAX_PLANES],
        dx: [0.0; MAX_PLANES],
        dy: [0.0; MAX_PLANES],
    };
    let mut got_dual = vec![zero_dual; queries.len()];
    dual_view
        .sample_dual_batch(&queries, &mut got_dual)
        .expect("dual batch 320");
    for (i, (q, g_s)) in queries.iter().zip(&got_dual).enumerate() {
        let w_s = dual_view
            .sample_dual(q.position, q.scale)
            .expect("dual scalar");
        assert_eq!(
            g_s.values.map(f64::to_bits),
            w_s.values.map(f64::to_bits),
            "dv[{i}]"
        );
        assert_eq!(
            g_s.dx.map(f64::to_bits),
            w_s.dx.map(f64::to_bits),
            "ddx[{i}]"
        );
        assert_eq!(
            g_s.dy.map(f64::to_bits),
            w_s.dy.map(f64::to_bits),
            "ddy[{i}]"
        );
    }

    // 2. Fused 3-stream `apply_deposit_streams_to_carrier_on` vs sequential
    //    `apply_deposits_to_carrier_on` across workers 1 & 4.
    let build_streams = |empty_third: bool| -> [Deposits; 3] {
        let mut s0 = Deposits::with_capacity(&g, 600, &BufferLimits::host()).expect("s0");
        let mut s1 = Deposits::with_capacity(&g, 600, &BufferLimits::host()).expect("s1");
        let mut s2 = Deposits::with_capacity(&g, 600, &BufferLimits::host()).expect("s2");
        let mut r = lcg(12345);
        // s0: strictly ascending IDs (exercises identity_sequence fast path).
        for id in 0..500_u64 {
            let mut dir = [0.0_f64; CHANNELS];
            dir[(id as usize) % CHANNELS] = 0.75;
            s0.push(
                &g,
                OrganismId::new(id + 1),
                [r() * 32.0, r() * 32.0],
                0.25,
                0.2,
                &dir,
            )
            .expect("push s0");
        }
        // s1: descending IDs (exercises non-identity sequence path).
        for id in (0..250_u64).rev() {
            let mut dir = [0.0_f64; CHANNELS];
            dir[(id as usize + 5) % CHANNELS] = -0.6;
            s1.push(
                &g,
                OrganismId::new(id + 10_000),
                [r() * 32.0, r() * 32.0],
                0.15,
                0.12,
                &dir,
            )
            .expect("push s1");
        }
        if !empty_third {
            for id in 0..400_u64 {
                let mut dir = [0.0_f64; CHANNELS];
                dir[(id as usize + 11) % CHANNELS] = 0.5;
                s2.push(
                    &g,
                    OrganismId::new(id + 20_000),
                    [r() * 32.0, r() * 32.0],
                    0.1,
                    0.1,
                    &dir,
                )
                .expect("push s2");
            }
        }
        [s0, s1, s2]
    };

    for empty_third in [false, true] {
        let mut seq_field = snap.clone();
        let [mut a0, mut a1, mut a2] = build_streams(empty_third);
        let c0 = seq_field.carrier0();
        let w0 = apply_deposits_to_carrier_on(&mut seq_field, &mut a0, c0, 1).expect("a0");
        let w1 = apply_deposits_to_carrier_on(&mut seq_field, &mut a1, c0, 1).expect("a1");
        let w2 = apply_deposits_to_carrier_on(&mut seq_field, &mut a2, c0, 1).expect("a2");
        let want_summaries = [w0, w1, w2];

        for workers in [1, 4] {
            let mut fused_field = snap.clone();
            let [mut f0, mut f1, mut f2] = build_streams(empty_third);
            let mut out = [None; 3];
            apply_deposit_streams_to_carrier_on(
                &mut fused_field,
                &mut [&mut f0, &mut f1, &mut f2],
                c0,
                workers,
                &mut out,
            )
            .expect("fused");
            for (idx, (got_s, want_s)) in out.iter().zip(&want_summaries).enumerate() {
                let got_s = got_s.expect("summary");
                assert_eq!(
                    got_s.organisms, want_s.organisms,
                    "stream {idx} w={workers}"
                );
                assert_eq!(got_s.cells, want_s.cells, "stream {idx} w={workers}");
                assert_eq!(
                    got_s.paid.to_bits(),
                    want_s.paid.to_bits(),
                    "paid stream {idx} w={workers}"
                );
                assert_eq!(
                    got_s.offered.to_bits(),
                    want_s.offered.to_bits(),
                    "offered stream {idx} w={workers}"
                );
                assert_eq!(
                    got_s.applied.to_bits(),
                    want_s.applied.to_bits(),
                    "applied stream {idx} w={workers}"
                );
            }
            let got_bits: Vec<u32> = fused_field.as_slice().iter().map(|v| v.to_bits()).collect();
            let want_bits: Vec<u32> = seq_field.as_slice().iter().map(|v| v.to_bits()).collect();
            assert_eq!(
                got_bits, want_bits,
                "field bits w={workers} empty={empty_third}"
            );
        }
    }

    // 3. `FixedWorldGaussianV1::footprint_into` (interior fast path + hoisted
    //    dx_sq/dy_sq + exp_neg_4) vs independent scalar minimum-image oracle.
    let g_gauss = FieldGrid::new(32, 24, 16.0, 12.0).expect("gauss grid");
    for radius in [0.75, 1.5, 3.25] {
        let kernel = ExchangeKernel::fixed_world_gaussian(radius).expect("kernel");
        let sigma = kernel.sigma().expect("sigma");
        let mut scratch = ExchangeScratch::for_kernel(kernel, &g_gauss).expect("scratch");
        let before_bytes = scratch.allocated_bytes();
        let test_positions = [
            [8.0, 6.0],     // interior
            [0.05, 0.05],   // corner seam wrap
            [15.99, 11.99], // upper corner seam wrap
            [0.1, 6.13],    // left edge wrap
            [7.37, 11.95],  // top edge wrap
            [3.25, 4.75],   // exact cell corner
        ];
        for pos in test_positions {
            kernel
                .footprint_into(&g_gauss, pos, &mut scratch)
                .expect("footprint");
            let center = g_gauss.wrap_position(pos).expect("wrap");
            let r2 = radius * radius;
            let two_s2 = 2.0 * sigma * sigma;
            let mut oracle: Vec<(usize, f64)> = Vec::new();
            let mut sum = 0.0;
            for row in 0..g_gauss.cells_y() {
                let cy = (f64::from(row) + 0.5) * g_gauss.cell_height();
                for col in 0..g_gauss.cells_x() {
                    let cx = (f64::from(col) + 0.5) * g_gauss.cell_width();
                    let [dx, dy] = g_gauss.displacement(center, [cx, cy]).expect("disp");
                    let d2 = dx * dx + dy * dy;
                    if d2 > r2 {
                        continue;
                    }
                    let raw = flife_field::numerics::exp_neg(d2 / two_s2).expect("exp");
                    if raw > 0.0 {
                        let cell = g_gauss.cell_index(col, row).expect("cell");
                        oracle.push((cell, raw));
                        sum += raw;
                    }
                }
            }
            for (_, w) in &mut oracle {
                *w /= sum;
            }
            oracle.retain(|&(_, w)| w > 0.0);
            assert_eq!(
                scratch.cells().len(),
                oracle.len(),
                "radius={radius} pos={pos:?}"
            );
            for (got_c, want_c) in scratch.cells().iter().zip(&oracle) {
                assert_eq!(got_c.0, want_c.0, "cell idx radius={radius} pos={pos:?}");
                assert_eq!(
                    got_c.1.to_bits(),
                    want_c.1.to_bits(),
                    "weight bits radius={radius} pos={pos:?}"
                );
            }
        }
        assert_eq!(scratch.allocated_bytes(), before_bytes, "scratch grew");
    }
}

#[test]
fn deposits_append_interleaved_empty_and_nonempty_lanes_bit_exact() {
    let grid = grid(16, 16);
    let base = field(grid, 99);
    let demands = clustered(&grid, 64, 37);
    let mut next = lcg(73);
    let deposits: Vec<Staged> = demands
        .iter()
        .map(|demand| {
            let paid = 0.005 + next() * 0.02;
            let mut direction = [0.0; CHANNELS];
            direction[(next() * CHANNELS as f64) as usize % CHANNELS] = next() - 0.5;
            (
                demand.organism,
                demand.position,
                paid,
                paid * 0.8,
                direction,
            )
        })
        .collect();
    let limits = BufferLimits::host();

    let mut direct =
        Deposits::with_capacity(&grid, deposits.len() as u64, &limits).expect("direct");
    for &(org, pos, paid, offered, dir) in &deposits {
        direct
            .push(&grid, org, pos, paid, offered, &dir)
            .expect("push");
    }
    let mut direct_field = base.clone();
    let expected = apply_deposits(&mut direct_field, &mut direct).expect("apply direct");

    // Interleave empty lanes before, between, and after non-empty chunks so
    // both the empty-dest fast path (entries == 0 / packets == 0) and the
    // non-empty in-place offset shift path are exercised.
    let mut merged =
        Deposits::with_capacity(&grid, deposits.len() as u64, &limits).expect("merged");
    let allocated = merged.allocated();
    let mut lane = Deposits::new();
    lane.fit_staging(deposits.len()).expect("lane");
    // Leading empty lane into empty destination.
    merged.append(&mut lane).expect("empty into empty");
    assert!(merged.is_empty());
    for chunk in deposits.chunks(13) {
        for &(org, pos, paid, offered, dir) in chunk {
            lane.push(&grid, org, pos, paid, offered, &dir)
                .expect("lane push");
        }
        merged.append(&mut lane).expect("non-empty append");
        assert!(lane.is_empty());
        // Empty lane after non-empty destination.
        merged.append(&mut lane).expect("empty after non-empty");
    }
    assert_eq!(merged.allocated(), allocated, "append allocated");
    assert_eq!(merged.len(), deposits.len());
    for i in 0..deposits.len() {
        let direct_entry = direct.entry(i);
        let merged_entry = merged.entry(i);
        assert_eq!(merged_entry, direct_entry, "entry {i}");
        let d_pkts: Vec<(usize, u64)> = direct
            .entry_packets(i)
            .map(|(c, e)| (c, e.to_bits()))
            .collect();
        let m_pkts: Vec<(usize, u64)> = merged
            .entry_packets(i)
            .map(|(c, e)| (c, e.to_bits()))
            .collect();
        assert_eq!(m_pkts, d_pkts, "entry_packets {i}");
    }
    let mut merged_field = base;
    let got = apply_deposits(&mut merged_field, &mut merged).expect("apply merged");
    assert_eq!(summary_bits(&got), summary_bits(&expected));
    assert!(planes(&merged_field) == planes(&direct_field));
}

#[test]
fn sample_batch_detects_nonfinite_across_all_packed_buckets() {
    let g = FieldGrid::new(32, 32, 32.0, 32.0).expect("grid");
    for bad_val in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut snap = field(g, 4242);
        // Inject one nonfinite cell into moment plane 17.
        snap.moment_mut(16).expect("m16")[123] = bad_val;
        let mut pyr = SensorPyramid::new(g, 3, &BufferLimits::host()).expect("pyr");
        let view = pyr.build_on(&snap, 2).expect("build");

        // Query directly over cell 123 = (27, 3) across all 3 buckets:
        // level-0 only (`scale=1.0`), level-0/1 blend (`scale=1.35`), and coarse (`scale=2.7`).
        for &(scale, label) in &[
            (1.0, "l0"),
            (1.35, "l01_blend"),
            (2.7, "coarse_blend"),
            (4.0, "coarse_exact"),
        ] {
            let mut queries = vec![
                SensorQuery {
                    position: [1.25, 1.25],
                    scale,
                };
                40 // > SUB_BATCH=32 so packed tile-sorted path runs
            ];
            queries[37] = SensorQuery {
                position: [27.5, 3.5],
                scale,
            };
            let zero = RawSample {
                values: [0.0; PLANES],
                dx: [0.0; PLANES],
                dy: [0.0; PLANES],
            };
            let mut out = vec![zero; queries.len()];
            let err = view
                .sample_batch(&queries, &mut out)
                .expect_err(&format!("{label} with {bad_val:?} must refuse"));
            assert!(
                matches!(
                    err,
                    CoreError::NumericalInvalid {
                        quantity: "sensor sample",
                        ..
                    }
                ),
                "{label} ({bad_val:?}): unexpected error {err:?}"
            );
        }
    }
}
