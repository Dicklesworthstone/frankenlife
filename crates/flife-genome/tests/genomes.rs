//! FL-009 evidence (plan §23.1): parameter counts, fixed logical quota
//! accounting, checked allocation refusal, and width/depth transition
//! fixtures (§6.4, §6.5, §7.1, §7.5, §9.2, A.2, A.3 items 2, 5 and 6,
//! Appendix C.16).
//!
//! Every directed catalog transition is checked against an expectation the
//! map does not compute. Widening must reproduce the parent's outputs
//! exactly. Shrinking must reproduce the parent with its trailing neurons'
//! outgoing weights zeroed. A depth change must match an F64 evaluation of
//! the declared map written from the parent's weights. The initializer's
//! edge values were computed independently in Python.

use flife_core::ids::GenomeId;
use flife_core::{CoreError, ValidatedConfig};
use flife_genome::{
    DENSE_104_36_MEMBERS, Genome, GenomeQuota, TopologyCatalog, TopologyId, TraitLimits, Traits,
    Transition, WeightAddress, allocate_parameters, fan_in_weight, initialize, logical_bytes,
    map_parameters,
};
use flife_neural::{
    ACTION_WIDTH, ControllerScratch, DenseTopology, ReferenceController, SENSOR_WIDTH,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

const BOUND: f64 = 4.0;

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a() -> ValidatedConfig {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    ValidatedConfig::from_toml(&appendix[start..start + length]).expect("Appendix A is valid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn catalog() -> TopologyCatalog {
    TopologyCatalog::from_config(&appendix_a()).expect("Appendix A catalog")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn member(catalog: &TopologyCatalog, id: TopologyId) -> &DenseTopology {
    catalog.topology(id).expect("member")
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A raw word that depends only on the seed and the address, never on the
/// order in which words are requested.
fn address_word(seed: u64, address: WeightAddress) -> u64 {
    splitmix(
        splitmix(splitmix(seed ^ address.layer as u64) ^ address.output as u64)
            ^ address.input as u64,
    )
}

/// A founder-style parent with nonzero biases, so that biases matter.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn parent(topology: &DenseTopology, seed: u64) -> Vec<f32> {
    let mut parameters = vec![0.0_f32; topology.parameter_count()];
    initialize(topology, &mut parameters, |address| {
        address_word(seed, address)
    })
    .expect("init");
    let mut state = seed;
    for shape in topology.layers() {
        for slot in &mut parameters[shape.biases..shape.end()] {
            state = splitmix(state);
            *slot = ((state >> 11) as f64 / 9_007_199_254_740_992.0 - 0.5) as f32 * 0.5;
        }
    }
    parameters
}

fn inputs(seed: u64) -> [f64; SENSOR_WIDTH] {
    let mut state = seed;
    let mut values = [0.0; SENSOR_WIDTH];
    for slot in &mut values {
        state = splitmix(state);
        *slot = 2.0 * ((state >> 11) as f64 / 9_007_199_254_740_992.0) - 1.0;
    }
    values
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn evaluate(
    topology: &DenseTopology,
    parameters: &[f32],
    x: &[f64; SENSOR_WIDTH],
) -> [f32; ACTION_WIDTH] {
    let controller = ReferenceController::new(topology, parameters, BOUND).expect("controller");
    let mut scratch = ControllerScratch::new(topology).expect("scratch");
    controller.evaluate(x, &mut scratch).expect("evaluate")
}

/// `parameters[layer].weight(output, input)`.
fn weight(
    topology: &DenseTopology,
    parameters: &[f32],
    layer: usize,
    output: usize,
    input: usize,
) -> f32 {
    let shape = topology.layers()[layer];
    parameters[shape.weight(output, input).unwrap_or(usize::MAX)]
}

fn bias(topology: &DenseTopology, parameters: &[f32], layer: usize, output: usize) -> f32 {
    let shape = topology.layers()[layer];
    parameters[shape.bias(output).unwrap_or(usize::MAX)]
}

/// Map `source → target`, recording every address that drew a word.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn mapped(
    source: &DenseTopology,
    parent: &[f32],
    target: &DenseTopology,
    seed: u64,
) -> (Vec<f32>, Transition, Vec<WeightAddress>) {
    // A child buffer full of junk: the map must write every value.
    let mut child = vec![f32::NAN; target.parameter_count()];
    let mut drawn = Vec::new();
    let transition = map_parameters(source, parent, target, &mut child, |address| {
        drawn.push(address);
        address_word(seed, address)
    })
    .expect("map");
    (child, transition, drawn)
}

#[test]
fn catalog_is_section_6_4_in_order_under_the_parameter_cap() {
    let catalog = catalog();
    assert_eq!(catalog.len(), 5);
    let counts: Vec<usize> = catalog
        .ids()
        .map(|id| member(&catalog, id).parameter_count())
        .collect();
    assert_eq!(counts, [1_164, 4_548, 9_988, 18_628, 29_316]);
    for (id, hidden) in catalog.ids().zip(DENSE_104_36_MEMBERS) {
        assert_eq!(member(&catalog, id).hidden_widths(), hidden);
        assert_eq!(catalog.find(hidden), Some(id));
        assert!(catalog.admits(id));
        let targets: Vec<TopologyId> = catalog.transition_targets(id).collect();
        assert_eq!(targets.len(), 4);
        assert!(!targets.contains(&id));
    }
    assert_eq!(
        member(&catalog, catalog.default_topology()).hidden_widths(),
        &[64, 32]
    );
    assert_eq!(catalog.maximum_parameters(), 30_000);
    assert_eq!(catalog.find(&[64, 64]), None);

    // A tighter cap keeps the largest member listed but not admitted.
    let capped = TopologyCatalog::dense_104_36_v1(20_000, &[64, 32]).expect("capped");
    let largest = capped.find(&[128, 96]).expect("still listed");
    assert!(!capped.admits(largest));
    assert!(
        capped
            .ids()
            .all(|id| !capped.transition_targets(id).any(|t| t == largest))
    );
    // The default must be a member within the cap.
    assert!(TopologyCatalog::dense_104_36_v1(9_000, &[64, 32]).is_err());
    assert!(TopologyCatalog::dense_104_36_v1(30_000, &[64, 64]).is_err());
}

#[test]
fn logical_quota_charges_every_genome_at_full_size() {
    let catalog = catalog();
    let bytes: Vec<u64> = catalog
        .ids()
        .map(|id| logical_bytes(member(&catalog, id)).expect("bytes"))
        .collect();
    assert_eq!(bytes, [4_656, 18_192, 39_952, 74_512, 117_264]);

    // A.2: 16,384 organisms of the 29,316-parameter F32 topology need
    // 1,832.25 MiB, above the 1 GiB logical quota; 9,156 fit. The default
    // topology fits 26,875, more than the population ceiling.
    let config = appendix_a();
    let quota = GenomeQuota::from_config(&config).expect("Appendix A quota");
    let largest = member(&catalog, catalog.find(&[128, 96]).expect("largest"));
    assert_eq!(
        16_384 * logical_bytes(largest).expect("bytes"),
        1_921_253_376
    );
    assert_eq!(1_921_253_376_u64, 1_832 * 1_048_576 + 262_144);
    assert_eq!(quota.capacity_for(largest).expect("capacity"), 9_156);
    let default = member(&catalog, catalog.default_topology());
    assert_eq!(quota.capacity_for(default).expect("capacity"), 26_875);
    assert!(!config.report().worst_case_fits_logical_quota);

    // A quota with room for three large genomes and one byte short of a
    // fourth. Every genome is charged its full size, refusals change
    // nothing, and retiring one makes room again.
    let large = catalog.find(&[128, 96]).expect("large");
    let small = catalog.find(&[8]).expect("small");
    let mut quota = GenomeQuota::new(4 * 117_264 - 1, 1 << 30).expect("quota");
    let mut buffers = Vec::new();
    for _ in 0..3 {
        buffers.push(quota.allocate(&catalog, large).expect("fits"));
    }
    assert_eq!(quota.logical_bytes(), 3 * 117_264);
    assert_eq!(quota.live(), 3);
    let before = quota.clone();
    assert_eq!(
        quota.allocate(&catalog, large),
        Err(CoreError::BudgetExhausted {
            budget: "logical genome bytes",
            required: 117_264,
            available: 117_263,
        })
    );
    assert_eq!(quota, before);
    let tiny = quota
        .allocate(&catalog, small)
        .expect("a small genome still fits");
    assert_eq!(quota.logical_bytes(), 3 * 117_264 + 4_656);
    quota.abandon(tiny).expect("abandon");
    quota
        .abandon(buffers.pop().expect("buffer"))
        .expect("abandon");
    assert_eq!(quota.logical_bytes(), 2 * 117_264);
    assert!(quota.allocate(&catalog, large).is_ok());
}

#[test]
fn allocation_refusals_are_checked_before_anything_is_charged() {
    let catalog = catalog();
    let default = catalog.default_topology();
    // The allocated quota binds on its own.
    let mut quota = GenomeQuota::new(39_952, 39_952).expect("quota");
    let buffer = quota.allocate(&catalog, default).expect("one fits");
    assert_eq!(quota.allocated_bytes(), 39_952);
    assert!(matches!(
        quota.allocate(&catalog, default),
        Err(CoreError::BudgetExhausted { .. })
    ));
    quota.abandon(buffer).expect("abandon");
    assert_eq!(
        (quota.logical_bytes(), quota.allocated_bytes(), quota.live()),
        (0, 0, 0)
    );
    assert!(GenomeQuota::new(2, 1).is_err());

    // A member above the parameter cap is refused before allocation.
    let capped = TopologyCatalog::dense_104_36_v1(20_000, &[64, 32]).expect("capped");
    let mut quota = GenomeQuota::new(1 << 30, 1 << 30).expect("quota");
    assert_eq!(
        quota.allocate(&capped, capped.find(&[128, 96]).expect("listed")),
        Err(CoreError::BudgetExhausted {
            budget: "architecture_mutation.maximum_parameters",
            required: 29_316,
            available: 20_000,
        })
    );
    assert_eq!(quota.live(), 0);

    // A size the host cannot represent is an error, not an abort, and
    // allocates nothing.
    assert!(matches!(
        allocate_parameters(usize::MAX / 2),
        Err(CoreError::AllocationFailed {
            buffer: "genome parameters",
            ..
        })
    ));

    // Releasing a charge this quota does not hold is refused.
    let mut other = GenomeQuota::new(1 << 30, 1 << 30).expect("quota");
    let stray = quota.allocate(&catalog, default).expect("buffer");
    assert!(matches!(
        other.abandon(stray),
        Err(CoreError::StateConflict { .. })
    ));
}

#[test]
fn founders_draw_each_weight_from_its_own_address() {
    // Edge words for fan-in 104, against independently computed F32 bits:
    // k = 0 gives −√(3/104); k = 2^51 gives exactly 0; k = 3·2^50 gives
    // ½√(3/104); the largest word rounds to +√(3/104) in F32.
    assert_eq!(fan_in_weight(0, 104).expect("w").to_bits(), 0xbe2d_eaf2);
    assert_eq!(fan_in_weight(1 << 63, 104).expect("w"), 0.0);
    assert_eq!(
        fan_in_weight(3 << 62, 104).expect("w").to_bits(),
        0x3dad_eaf2
    );
    assert_eq!(
        fan_in_weight(u64::MAX, 104).expect("w").to_bits(),
        0x3e2d_eaf2
    );
    assert_eq!(fan_in_weight(0, 8).expect("w").to_bits(), 0xbf1c_c471);
    assert_eq!(fan_in_weight(0, 64).expect("w").to_bits(), 0xbe5d_b3d7);
    // The trailing 12 bits are not used.
    assert_eq!(fan_in_weight(0xfff, 104), fan_in_weight(0, 104));
    assert!(fan_in_weight(0, 0).is_err());

    let default = DenseTopology::new(&[64, 32]).expect("default");
    let mut parameters = vec![f32::NAN; 9_988];
    let mut addresses = Vec::new();
    initialize(&default, &mut parameters, |address| {
        addresses.push(address);
        address_word(7, address)
    })
    .expect("init");
    // One word per weight (9,988 − 132 biases), in ascending address order,
    // each address once.
    assert_eq!(addresses.len(), 9_856);
    assert!(addresses.windows(2).all(|pair| pair[0] < pair[1]));
    let mut second_moment = [0.0_f64; 3];
    for (layer, shape) in default.layers().iter().enumerate() {
        let limit = (3.0 / shape.inputs as f64).sqrt() as f32;
        let weights = &parameters[shape.weights..shape.biases];
        assert!(weights.iter().all(|w| (-limit..=limit).contains(w)));
        assert!(
            parameters[shape.biases..shape.end()]
                .iter()
                .all(|&b| b == 0.0)
        );
        second_moment[layer] = weights.iter().map(|&w| f64::from(w).powi(2)).sum::<f64>()
            / weights.len() as f64
            * shape.inputs as f64;
    }
    // Variance 1/I: the scaled second moment is 1 within sampling error
    // (over 1,152 to 6,656 weights, a few percent).
    for moment in second_moment {
        assert!((moment - 1.0).abs() < 0.1, "{moment}");
    }
    // Words depend on addresses, not on request order: the same seed gives
    // the same founder.
    let mut again = vec![0.0_f32; 9_988];
    initialize(&default, &mut again, |address| address_word(7, address)).expect("init");
    assert_eq!(
        again.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        parameters.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert!(initialize(&default, &mut again[..9_987], |_| 0).is_err());
}

/// All 20 directed transitions of the catalog, classified.
fn transitions(catalog: &TopologyCatalog) -> Vec<(TopologyId, TopologyId)> {
    catalog
        .ids()
        .flat_map(|from| catalog.transition_targets(from).map(move |to| (from, to)))
        .collect()
}

fn widens(source: &DenseTopology, target: &DenseTopology) -> bool {
    source.hidden_widths().len() == target.hidden_widths().len()
        && source
            .hidden_widths()
            .iter()
            .zip(target.hidden_widths())
            .all(|(from, to)| to >= from)
}

#[test]
fn every_directed_catalog_transition_is_classified() {
    let catalog = catalog();
    let all = transitions(&catalog);
    assert_eq!(all.len(), 20);
    let mut counts = [0; 4];
    for &(from, to) in &all {
        let (source, target) = (member(&catalog, from), member(&catalog, to));
        let kind = Transition::between(source, target).expect("kind");
        let slot = match kind {
            Transition::EqualDepth if widens(source, target) => 0,
            Transition::EqualDepth => 1,
            Transition::Deepen => 2,
            Transition::Flatten => 3,
        };
        counts[slot] += 1;
        let (_, mapped_kind, _) = mapped(source, &parent(source, 1), target, 2);
        assert_eq!(mapped_kind, kind);
    }
    // 4 widenings, 4 shrinkings, 6 deepenings, 6 flattenings.
    assert_eq!(counts, [4, 4, 6, 6]);
}

#[test]
fn equal_depth_widening_preserves_the_parent_function() {
    let catalog = catalog();
    let mut checked = 0;
    for (from, to) in transitions(&catalog) {
        let (source, target) = (member(&catalog, from), member(&catalog, to));
        if Transition::between(source, target).ok() != Some(Transition::EqualDepth)
            || !widens(source, target)
        {
            continue;
        }
        let parent = parent(source, 11);
        let (child, _, drawn) = mapped(source, &parent, target, 12);
        assert!(child.iter().all(|v| v.is_finite()));
        // Draws only for added neurons' incoming weights from inherited
        // inputs: for each hidden layer, (added outputs) × (inherited inputs).
        let mut expected_draws = 0;
        for (layer, (s, t)) in source.layers().iter().zip(target.layers()).enumerate() {
            if layer + 1 < target.layers().len() {
                expected_draws += (t.outputs - s.outputs) * s.inputs;
            }
        }
        assert_eq!(drawn.len(), expected_draws, "{:?}", target.hidden_widths());
        for seed in 0..16 {
            let x = inputs(seed);
            assert_eq!(
                evaluate(target, &child, &x),
                evaluate(source, &parent, &x),
                "{:?} → {:?}",
                source.hidden_widths(),
                target.hidden_widths()
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 4);
}

#[test]
fn shrinking_keeps_the_leading_neurons() {
    let catalog = catalog();
    let mut checked = 0;
    for (from, to) in transitions(&catalog) {
        let (source, target) = (member(&catalog, from), member(&catalog, to));
        if Transition::between(source, target).ok() != Some(Transition::EqualDepth)
            || widens(source, target)
        {
            continue;
        }
        let parent = parent(source, 21);
        let (child, _, drawn) = mapped(source, &parent, target, 22);
        assert!(drawn.is_empty());
        // Every child value is the parent's value for the same neurons.
        for (layer, t) in target.layers().iter().enumerate() {
            for output in 0..t.outputs {
                for input in 0..t.inputs {
                    assert_eq!(
                        weight(target, &child, layer, output, input),
                        weight(source, &parent, layer, output, input)
                    );
                }
                assert_eq!(
                    bias(target, &child, layer, output),
                    bias(source, &parent, layer, output)
                );
            }
        }
        // Functionally: the parent with its trailing neurons' outgoing
        // weights zeroed. Not the parent itself.
        let mut pruned = parent.clone();
        for (layer, (s, t)) in source
            .layers()
            .iter()
            .zip(target.layers())
            .enumerate()
            .skip(1)
        {
            for output in 0..s.outputs {
                for input in t.inputs..s.inputs {
                    let offset = source.layers()[layer]
                        .weight(output, input)
                        .unwrap_or(usize::MAX);
                    pruned[offset] = 0.0;
                }
            }
        }
        let mut differs = false;
        for seed in 0..16 {
            let x = inputs(seed);
            let ours = evaluate(target, &child, &x);
            assert_eq!(ours, evaluate(source, &pruned, &x));
            differs |= ours != evaluate(source, &parent, &x);
        }
        assert!(differs, "shrinking is not function-preserving");
        checked += 1;
    }
    assert_eq!(checked, 4);
}

/// `tanh` of each first-hidden-layer pre-activation, in F64.
fn first_layer(topology: &DenseTopology, parameters: &[f32], x: &[f64; SENSOR_WIDTH]) -> Vec<f64> {
    let shape = topology.layers()[0];
    (0..shape.outputs)
        .map(|output| {
            let mut z = f64::from(bias(topology, parameters, 0, output));
            for (input, &value) in x.iter().enumerate() {
                z += f64::from(weight(topology, parameters, 0, output, input))
                    * f64::from(value as f32);
            }
            z.tanh()
        })
        .collect()
}

#[test]
fn deepening_inserts_an_identity_bridge() {
    let catalog = catalog();
    let mut checked = 0;
    for (from, to) in transitions(&catalog) {
        let (source, target) = (member(&catalog, from), member(&catalog, to));
        if Transition::between(source, target).ok() != Some(Transition::Deepen) {
            continue;
        }
        let parent = parent(source, 31);
        let (child, _, drawn) = mapped(source, &parent, target, 32);
        let h = source.hidden_widths()[0] as usize;
        let (a, b) = (
            target.hidden_widths()[0] as usize,
            target.hidden_widths()[1] as usize,
        );
        assert_eq!(drawn.len(), (a - h) * SENSOR_WIDTH);
        for output in 0..a {
            for input in 0..SENSOR_WIDTH {
                let value = weight(target, &child, 0, output, input);
                if output < h {
                    assert_eq!(value, weight(source, &parent, 0, output, input));
                } else {
                    let address = WeightAddress {
                        layer: 0,
                        output,
                        input,
                    };
                    assert_eq!(
                        value,
                        fan_in_weight(address_word(32, address), SENSOR_WIDTH).expect("w")
                    );
                }
            }
            let expected = if output < h {
                bias(source, &parent, 0, output)
            } else {
                0.0
            };
            assert_eq!(bias(target, &child, 0, output), expected);
        }
        for output in 0..b {
            for input in 0..a {
                let identity = if output == input && input < h {
                    1.0
                } else {
                    0.0
                };
                assert_eq!(weight(target, &child, 1, output, input), identity);
            }
            assert_eq!(bias(target, &child, 1, output), 0.0);
        }
        for output in 0..ACTION_WIDTH {
            for input in 0..b {
                let expected = if input < h {
                    weight(source, &parent, 1, output, input)
                } else {
                    0.0
                };
                assert_eq!(weight(target, &child, 2, output, input), expected);
            }
            assert_eq!(
                bias(target, &child, 2, output),
                bias(source, &parent, 1, output)
            );
        }
        // Functionally, the declared map in F64: each inherited activation
        // passes through tanh a second time (C.16).
        for seed in 0..8 {
            let x = inputs(seed);
            let hidden = first_layer(source, &parent, &x);
            let ours = evaluate(target, &child, &x);
            for (output, &value) in ours.iter().enumerate() {
                let mut expected = f64::from(bias(source, &parent, 1, output));
                for (j, &activation) in hidden.iter().enumerate().take(b.min(h)) {
                    expected +=
                        f64::from(weight(source, &parent, 1, output, j)) * activation.tanh();
                }
                assert!((f64::from(value) - expected).abs() < 1.0e-5);
            }
            assert_ne!(ours, evaluate(source, &parent, &x));
        }
        checked += 1;
    }
    assert_eq!(checked, 6);

    // C.16 by itself: 104 → 1 → 36 to 104 → 1 → 1 → 36 turns tanh(1) into
    // tanh(tanh(1)).
    let shallow = DenseTopology::new(&[1]).expect("shallow");
    let deep = DenseTopology::new(&[1, 1]).expect("deep");
    let mut one = vec![0.0_f32; shallow.parameter_count()];
    one[shallow.layers()[0].weight(0, 0).unwrap_or(usize::MAX)] = 1.0;
    one[shallow.layers()[1].weight(0, 0).unwrap_or(usize::MAX)] = 1.0;
    let (bridged, kind, _) = mapped(&shallow, &one, &deep, 0);
    assert_eq!(kind, Transition::Deepen);
    let mut x = [0.0; SENSOR_WIDTH];
    x[0] = 1.0;
    assert_eq!(evaluate(&shallow, &one, &x)[0].to_bits(), 0x3f42_f7d6);
    assert_eq!(evaluate(&deep, &bridged, &x)[0].to_bits(), 0x3f24_5b18);
}

#[test]
fn flattening_discards_the_middle_layer() {
    let catalog = catalog();
    let mut checked = 0;
    for (from, to) in transitions(&catalog) {
        let (source, target) = (member(&catalog, from), member(&catalog, to));
        if Transition::between(source, target).ok() != Some(Transition::Flatten) {
            continue;
        }
        let parent = parent(source, 41);
        let (child, _, drawn) = mapped(source, &parent, target, 42);
        let (a, b) = (
            source.hidden_widths()[0] as usize,
            source.hidden_widths()[1] as usize,
        );
        let h = target.hidden_widths()[0] as usize;
        // Every catalog flattening narrows the first layer (h ≤ 32 < a).
        assert!(h < a);
        assert!(drawn.is_empty());
        for output in 0..h {
            for input in 0..SENSOR_WIDTH {
                assert_eq!(
                    weight(target, &child, 0, output, input),
                    weight(source, &parent, 0, output, input)
                );
            }
            assert_eq!(
                bias(target, &child, 0, output),
                bias(source, &parent, 0, output)
            );
        }
        for output in 0..ACTION_WIDTH {
            for input in 0..h {
                let expected = if input < b.min(a) {
                    weight(source, &parent, 2, output, input)
                } else {
                    0.0
                };
                assert_eq!(weight(target, &child, 1, output, input), expected);
            }
            assert_eq!(
                bias(target, &child, 1, output),
                bias(source, &parent, 2, output)
            );
        }
        // Functionally: the old output weights now read the first layer.
        for seed in 0..8 {
            let x = inputs(seed);
            let hidden = first_layer(source, &parent, &x);
            let ours = evaluate(target, &child, &x);
            for (output, &value) in ours.iter().enumerate() {
                let mut expected = f64::from(bias(source, &parent, 2, output));
                for (j, &activation) in hidden.iter().enumerate().take(b.min(h)) {
                    expected += f64::from(weight(source, &parent, 2, output, j)) * activation;
                }
                assert!((f64::from(value) - expected).abs() < 1.0e-5);
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 6);

    // Maps refuse buffers of the wrong length and unsupported depths.
    let one = DenseTopology::new(&[8]).expect("one");
    let two = DenseTopology::new(&[64, 32]).expect("two");
    let mut child = vec![0.0_f32; two.parameter_count()];
    assert!(map_parameters(&one, &[0.0; 3], &two, &mut child, |_| 0).is_err());
    assert!(map_parameters(&one, &parent(&one, 1), &two, &mut child[..10], |_| 0).is_err());
}

fn unit_affinity() -> [f64; 32] {
    let mut affinity = [0.0; 32];
    affinity[3] = 0.6;
    affinity[7] = -0.8;
    affinity
}

#[test]
fn genomes_are_sealed_once_and_shared_read_only() {
    let config = appendix_a();
    let catalog = catalog();
    let limits = TraitLimits::from_config(&config).expect("limits");
    let default = catalog.default_topology();
    let topology = member(&catalog, default);
    let mut quota = GenomeQuota::from_config(&config).expect("quota");
    let traits = Traits::new(unit_affinity(), [0.1; 33], 1.0, &limits).expect("traits");

    // A parameter outside ±4 is refused, and the charge comes back.
    let mut buffer = quota.allocate(&catalog, default).expect("buffer");
    buffer.f32_mut().expect("an F32 buffer")[17] = 4.5;
    let unsealed = Genome::seal(buffer, GenomeId::new(1), &catalog, traits, None, &limits)
        .expect_err("out of bound");
    assert!(matches!(unsealed.error, CoreError::NumericalInvalid { .. }));
    quota.abandon(unsealed.buffer).expect("abandon");
    assert_eq!((quota.logical_bytes(), quota.live()), (0, 0));

    // Two founders with identical content are two genomes: two buffers,
    // two full charges (exact deduplication never lowers the charge).
    let mut founders = Vec::new();
    for raw in [2, 3] {
        let mut buffer = quota.allocate(&catalog, default).expect("buffer");
        initialize(
            topology,
            buffer.f32_mut().expect("an F32 buffer"),
            |address| address_word(5, address),
        )
        .expect("init");
        founders.push(
            Genome::seal(buffer, GenomeId::new(raw), &catalog, traits, None, &limits)
                .map_err(|unsealed| unsealed.error)
                .expect("seal"),
        );
    }
    assert_eq!(quota.logical_bytes(), 2 * 39_952);
    assert_eq!(founders[0].parameters(), founders[1].parameters());
    assert!(
        !founders[0]
            .shared_parameters()
            .same_buffer(&founders[1].shared_parameters())
    );

    // A child maps its parent and records it.
    let wider = catalog.find(&[96, 64]).expect("wider");
    let mut buffer = quota.allocate(&catalog, wider).expect("buffer");
    map_parameters(
        topology,
        founders[0].parameters().as_f32().expect("an F32 genome"),
        member(&catalog, wider),
        buffer.f32_mut().expect("an F32 buffer"),
        |address| address_word(9, address),
    )
    .expect("map");
    let child = Genome::seal(
        buffer,
        GenomeId::new(4),
        &catalog,
        traits,
        Some(founders[0].id()),
        &limits,
    )
    .map_err(|unsealed| unsealed.error)
    .expect("seal");
    assert_eq!(child.parent(), Some(GenomeId::new(2)));
    assert_eq!(child.topology(), wider);
    assert_eq!(child.logical_bytes(), 74_512);
    let x = inputs(3);
    let mut scratch = ControllerScratch::default();
    let parent_outputs = founders[0]
        .controller(&catalog, &limits)
        .expect("controller")
        .evaluate(&x, &mut scratch)
        .expect("evaluate");
    let child_outputs = child
        .controller(&catalog, &limits)
        .expect("controller")
        .evaluate(&x, &mut scratch)
        .expect("evaluate");
    assert_eq!(parent_outputs, child_outputs);

    // Readers keep a retired genome's parameters; the quota does not.
    let reader = child.shared_parameters();
    quota.retire(child).expect("retire");
    assert_eq!(reader.weights().len(), 18_628);
    assert_eq!(quota.logical_bytes(), 2 * 39_952);
    for founder in founders {
        quota.retire(founder).expect("retire");
    }
    assert_eq!(
        (quota.logical_bytes(), quota.allocated_bytes(), quota.live()),
        (0, 0, 0)
    );
}

#[test]
fn traits_are_bounded() {
    let limits = TraitLimits::from_config(&appendix_a()).expect("limits");
    let affinity = unit_affinity();
    let response = [1.0 / 33.0_f64.sqrt() * 0.999_999; 33];
    let traits = Traits::new(affinity, response, 16.0, &limits).expect("at the bounds");
    assert_eq!(traits.affinity(), &affinity);
    assert_eq!(traits.motion_response(), &response);
    assert_eq!(traits.sensing_scale(), 16.0);

    let mut long = affinity;
    long[0] = 0.01;
    let mut nan = affinity;
    nan[5] = f64::NAN;
    assert!(Traits::new(long, response, 1.0, &limits).is_err());
    assert!(Traits::new(nan, response, 1.0, &limits).is_err());
    assert!(Traits::new(affinity, [0.2; 33], 1.0, &limits).is_err());
    for scale in [0.99, 16.01, f64::NAN] {
        assert!(
            Traits::new(affinity, response, scale, &limits).is_err(),
            "{scale}"
        );
    }
    assert!(TraitLimits::new(0.0, 1.0, [1.0, 16.0], 4.0).is_err());
    assert!(TraitLimits::new(1.0, 1.0, [16.0, 1.0], 4.0).is_err());
    assert!(TraitLimits::new(1.0, 1.0, [1.0, 16.0], f64::INFINITY).is_err());
}

#[test]
fn a_sealed_genome_is_rescanned_only_under_a_tighter_bound() {
    let config = appendix_a();
    let catalog = catalog();
    let limits = TraitLimits::from_config(&config).expect("limits");
    let default = catalog.default_topology();
    let topology = member(&catalog, default);
    let mut quota = GenomeQuota::from_config(&config).expect("quota");
    let traits = Traits::new(unit_affinity(), [0.1; 33], 1.0, &limits).expect("traits");
    let mut buffer = quota.allocate(&catalog, default).expect("buffer");
    initialize(
        topology,
        buffer.f32_mut().expect("an F32 buffer"),
        |address| address_word(7, address),
    )
    .expect("init");
    // Inside Appendix A's ±4, outside ±3.
    buffer.f32_mut().expect("an F32 buffer")[17] = 3.5;
    let genome = Genome::seal(buffer, GenomeId::new(1), &catalog, traits, None, &limits)
        .map_err(|unsealed| unsealed.error)
        .expect("seal");

    // Under the sealing bound (or a looser one) the controller is the one a
    // full check builds, and evaluates identically.
    let x = inputs(5);
    let mut scratch = ControllerScratch::default();
    let checked = ReferenceController::new(topology, genome.parameters(), 4.0)
        .expect("checked")
        .evaluate(&x, &mut scratch)
        .expect("evaluate");
    let looser = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 8.0).expect("looser");
    for bound in [&limits, &looser] {
        let sealed = genome
            .controller(&catalog, bound)
            .expect("sealed")
            .evaluate(&x, &mut scratch)
            .expect("evaluate");
        assert_eq!(
            sealed.map(f32::to_bits),
            checked.map(f32::to_bits),
            "the sealed controller is the checked one"
        );
    }

    // A tighter bound rescans, and refuses the parameter it no longer admits.
    let tighter = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 3.0).expect("tighter");
    assert!(matches!(
        genome.controller(&catalog, &tighter),
        Err(CoreError::NumericalInvalid { .. })
    ));
}
