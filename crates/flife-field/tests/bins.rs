//! FL-027 evidence (plan §9.4, §11.2, §11.3, §23.1): bounded cell bins and
//! the contribution reductions that use them. Covers hotspot correctness,
//! scratch limits, and deterministic ordering.
//!
//! The oracles are independent of `Bins`: a stable sort of item indices by
//! bucket, and a sort of each demand's footprint packets by (cell, organism).
//! That sort is what uptake and deposits used before FL-027.

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_field::state::CHANNELS;
use flife_field::{
    Bins, BufferLimits, Cell, Demand, DepositSummary, Deposits, ExchangeStencil, FieldGrid,
    FieldState, Grants, apply_deposits, apply_deposits_on, apply_grants, apply_grants_on,
    resolve_uptake,
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
