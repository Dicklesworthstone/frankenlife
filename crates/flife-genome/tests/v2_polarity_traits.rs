//! V2-08 (`pcp-polarity`) genome-layer evidence: heritable `polarity_strength`
//! (`α_pol ∈ [0, 1]`) and `wedge_curvature` (`β ∈ [-β_max, β_max]`, `β_max <=
//! 0.4`), zero extra checkpoint bytes and bit-exact baseline parity when
//! disabled, semantic Philox founder initialization and mutation reflection,
//! `Traits::reseal` preservation and bound re-enforcement, dynamic
//! `genome_header_bytes_v2`, and checkpoint v3 round-trip both standalone and
//! composed with `V2-13` `cadherin-4d`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure here is a test failure"
)]

use flife_core::checkpoint::{
    ByteReader, ByteWriter, CheckpointReader, CheckpointWriter, ChunkFamily,
};
use flife_core::ids::{GenomeId, OrganismId};
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream, half_open_unit};
use flife_core::{CoreError, FeatureOverride, Sha256Stream, Tick, ValidatedConfig};
use flife_genome::{
    BASELINE_GENOME_HEADER_BYTES, BirthAttempt, CADHERIN_CHECKPOINT_BYTES, DetachedProposal,
    Genome, GenomeQuota, MAXIMUM_CANDIDATES, MAXIMUM_WEDGE_CURVATURE_CAP, MutationParameters,
    Mutator, POLARITY_CHECKPOINT_BYTES, POLARITY_STRENGTH_LANE, POLARITY_WEDGE_LANE,
    PolarityTraits, Proposal, RejectionReason, TopologyCatalog, TraitLimits, Traits, WeightDtype,
    WeightsMut, founder, founder_polarity_traits, founder_traits, founder_traits_v2, founder_v2,
    genome_header_bytes_v2, initial_scale, mutate_polarity_traits, polarity_checkpoint_bytes,
    reflect_interval,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn appendix_a_toml() -> String {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    appendix[start..start + length].to_owned()
}

fn v1_config() -> ValidatedConfig {
    ValidatedConfig::from_toml(&appendix_a_toml()).expect("Appendix A is valid")
}

fn v2_contact_config(
    model: &str,
    extra_tail: &str,
    overrides: &[FeatureOverride],
) -> ValidatedConfig {
    let mut toml = appendix_a_toml().replace(
        "model = \"field-ecology-v1\"",
        &format!("model = \"{model}\""),
    );
    toml.push_str(
        "\n[contact]\ndiameter = 0.75\nstiffness = 0.5\nadhesion = 1.0\nadhesion_range = 2.0\n",
    );
    toml.push_str(extra_tail);
    ValidatedConfig::from_toml_with_overrides(&toml, overrides).expect("v2 contact config valid")
}

fn quota() -> GenomeQuota {
    GenomeQuota::new(1 << 30, 1 << 30).expect("quotas")
}

fn attempt(parent: u64, tick: u64) -> BirthAttempt {
    BirthAttempt {
        parent: OrganismId::new(parent),
        tick: Tick::new(tick),
    }
}

fn quiet_rates() -> MutationParameters {
    MutationParameters {
        topology_change_probability: 0.0,
        weight_probability: 0.0,
        weight_standard_deviation: 0.0,
        replacement_probability: 0.0,
        chemistry_standard_deviation: 0.0,
        sensing_log_scale_standard_deviation: 0.0,
        reject_effective_noop: false,
        maximum_candidates: MAXIMUM_CANDIDATES,
    }
}

/// V2-08 disabled parity: `field-ecology-v1` and `field-ecology-v2 --disable
/// pcp-polarity --disable cadherin-4d` match baseline `founder` /
/// `founder_traits` / `Mutator` bit for bit with 0 extra checkpoint bytes.
#[test]
fn polarity_disabled_matches_v1_bit_exact_with_zero_extra_checkpoint_bytes() {
    let v1 = v1_config();
    let v2_both_off = v2_contact_config(
        "field-ecology-v2",
        "",
        &[
            FeatureOverride::Disable("pcp-polarity".into()),
            FeatureOverride::Disable("cadherin-4d".into()),
        ],
    );
    let v2_pol_only_off = v2_contact_config(
        "field-ecology-v2",
        "",
        &[FeatureOverride::Disable("pcp-polarity".into())],
    );

    let limits_v1 = TraitLimits::from_config(&v1).expect("v1 limits");
    let limits_both_off = TraitLimits::from_config(&v2_both_off).expect("both off");
    let limits_pol_off = TraitLimits::from_config(&v2_pol_only_off).expect("pol off");

    assert!(!limits_v1.polarity_enabled());
    assert!(!limits_both_off.polarity_enabled());
    assert!(!limits_pol_off.polarity_enabled());
    assert!(limits_pol_off.cadherin_enabled());
    assert_eq!(limits_v1.polarity_mutation_std(), 0.0);
    assert_eq!(limits_v1.initial_polarity_strength_max(), 0.0);
    assert_eq!(limits_v1.maximum_wedge_curvature(), 0.0);
    assert_eq!(polarity_checkpoint_bytes(false), 0);
    assert_eq!(polarity_checkpoint_bytes(true), POLARITY_CHECKPOINT_BYTES);
    assert_eq!(POLARITY_CHECKPOINT_BYTES, 16);
    assert_eq!(genome_header_bytes_v2(false, false), 553);
    assert_eq!(genome_header_bytes_v2(true, false), 569);
    assert_eq!(genome_header_bytes_v2(false, true), 593);
    assert_eq!(genome_header_bytes_v2(true, true), 609);
    assert_eq!(limits_v1.genome_header_bytes(), 553);
    assert_eq!(limits_both_off.genome_header_bytes(), 553);
    assert_eq!(limits_pol_off.genome_header_bytes(), 593);

    let rng = SemanticRng::new(88);
    let scale = initial_scale(&v1);
    let catalog = TopologyCatalog::from_config(&v1).expect("catalog");
    let mut q_base = quota();
    let mut q_off = quota();

    for id in 1..=16_u64 {
        let base_traits = founder_traits(&rng, OrganismId::new(id), &limits_v1, scale).expect("t");
        let off_traits =
            founder_traits_v2(&rng, OrganismId::new(id), &limits_both_off, scale).expect("t2");
        assert_eq!(base_traits, off_traits);
        assert!(!off_traits.has_polarity());
        assert_eq!(off_traits.polarity_strength(), None);
        assert_eq!(off_traits.wedge_curvature(), None);
        assert_eq!(off_traits.polarity_traits(), None);
        assert_eq!(off_traits.checkpoint_bytes(), 528);

        let g_base = founder(
            &rng,
            OrganismId::new(id),
            GenomeId::new(id),
            &catalog,
            &mut q_base,
            &limits_v1,
            scale,
        )
        .expect("founder");
        let g_off = founder_v2(
            &rng,
            OrganismId::new(id),
            GenomeId::new(id),
            &catalog,
            &mut q_off,
            &limits_both_off,
            scale,
        )
        .expect("founder_v2");
        assert_eq!(g_base.traits(), g_off.traits());
        assert!(g_base.parameters().same_bits(&g_off.parameters()));

        let mut raw_base = ByteWriter::new();
        g_base
            .traits()
            .write_bytes_v2(&mut raw_base, false, false)
            .expect("write base");
        let mut raw_off = ByteWriter::new();
        g_off
            .traits()
            .write_bytes_v2(&mut raw_off, false, false)
            .expect("write off");
        assert_eq!(raw_base.as_slice().len(), 528);
        assert_eq!(raw_base.as_slice(), raw_off.as_slice());

        let rates = MutationParameters::from_config(&v1).expect("rates");
        let m_base = Mutator::new(rng, rates, &catalog, limits_v1).expect("m1");
        let m_off = Mutator::new(rng, rates, &catalog, limits_both_off).expect("m2");
        let p_base = m_base
            .propose(
                &g_base,
                attempt(id, 5),
                GenomeId::new(100 + id),
                &mut q_base,
            )
            .expect("propose base");
        let p_off = m_off
            .propose(&g_off, attempt(id, 5), GenomeId::new(100 + id), &mut q_off)
            .expect("propose off");
        match (p_base, p_off) {
            (
                Proposal::Child {
                    genome: a,
                    record: ra,
                },
                Proposal::Child {
                    genome: b,
                    record: rb,
                },
            ) => {
                assert_eq!(ra, rb);
                assert_eq!(a.traits(), b.traits());
                assert!(a.parameters().same_bits(&b.parameters()));
                assert!(!b.traits().has_polarity());
            }
            (other_a, other_b) => panic!("unexpected mismatch: {other_a:?} vs {other_b:?}"),
        }
    }
}

/// Bounds, hard `0.4` rad pinwheel cap, degenerate `β_max = 0`, and non-finite
/// refusals on `PolarityTraits` and `TraitLimits::with_polarity`.
#[test]
fn polarity_traits_bounds_and_refusals() {
    for (strength, wedge, beta_max) in [
        (0.0, 0.0, 0.0),
        (1.0, 0.0, 0.0),
        (0.0, -0.35, 0.35),
        (0.5, 0.35, 0.35),
        (1.0, 0.4, MAXIMUM_WEDGE_CURVATURE_CAP),
        (1.0, -0.4, MAXIMUM_WEDGE_CURVATURE_CAP),
    ] {
        let pol = PolarityTraits::new(strength, wedge, beta_max).expect("valid polarity");
        assert_eq!(pol.polarity_strength(), strength);
        assert_eq!(pol.wedge_curvature(), wedge);
    }

    // Invalid polarity_strength, wedge_curvature, or beta_max > 0.4 must refuse.
    for (strength, wedge, beta_max) in [
        (-1.0, 0.0, 0.35),
        (-f64::MIN_POSITIVE, 0.0, 0.35),
        (1.0 + 1e-15, 0.0, 0.35),
        (f64::NAN, 0.0, 0.35),
        (f64::INFINITY, 0.0, 0.35),
        (0.5, 0.350_000_1, 0.35),
        (0.5, -0.350_000_1, 0.35),
        (0.5, 0.01, 0.0),
        (0.5, f64::NAN, 0.35),
        (0.5, f64::NEG_INFINITY, 0.35),
        (0.5, 0.0, -0.01),
        (0.5, 0.0, 0.41), // above hard 0.4 rad cap
        (0.5, 0.0, f64::NAN),
    ] {
        let err = PolarityTraits::new(strength, wedge, beta_max).expect_err("must reject");
        assert!(
            matches!(err, CoreError::NumericalInvalid { .. }),
            "({strength}, {wedge}, {beta_max}) gave {err:?}"
        );
    }

    // TraitLimits::with_polarity validates all three parameters.
    let base = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 4.0).expect("base");
    assert!(base.with_polarity(-0.1, 0.35, 0.05).is_err());
    assert!(base.with_polarity(1.1, 0.35, 0.05).is_err());
    assert!(base.with_polarity(0.3, -0.01, 0.05).is_err());
    assert!(base.with_polarity(0.3, 0.41, 0.05).is_err());
    assert!(base.with_polarity(0.3, 0.35, -0.01).is_err());
    assert!(base.with_polarity(0.3, 0.35, f64::NAN).is_err());
}

/// Founder Philox initialization on exact lanes `0x0800` and `0x0801`:
/// independent oracle bit parity, forward/reverse order invariance, configured
/// bounds, and uniform moments over 512 founders.
#[test]
fn founder_polarity_philox_determinism_order_invariance_and_bounds() {
    let v2 = v2_contact_config(
        "field-ecology-v2",
        "",
        &[FeatureOverride::Disable("cadherin-4d".into())],
    );
    let limits = TraitLimits::from_config(&v2).expect("v2 limits");
    assert!(limits.polarity_enabled());
    assert!(!limits.cadherin_enabled());
    assert!((limits.initial_polarity_strength_max() - 0.3).abs() <= 1e-15);
    assert!((limits.maximum_wedge_curvature() - 0.35).abs() <= 1e-15);
    assert!((limits.polarity_mutation_std() - 0.05).abs() <= 1e-15);
    assert_eq!(
        limits.genome_header_bytes(),
        BASELINE_GENOME_HEADER_BYTES + POLARITY_CHECKPOINT_BYTES
    );
    assert_eq!(limits.genome_header_bytes(), 569);

    let rng = SemanticRng::new(4242);
    let scale = initial_scale(&v2);

    // Independent oracle on documented Philox lanes for organism 19.
    let org = OrganismId::new(19);
    let s_block = rng.block(Address {
        stream: Stream::Founders,
        entity: org.get(),
        ordinal: 0,
        lane: Lane::new(0, LaneKind::Direction, POLARITY_STRENGTH_LANE).expect("lane"),
        draw: 0,
    });
    let w_block = rng.block(Address {
        stream: Stream::Founders,
        entity: org.get(),
        ordinal: 0,
        lane: Lane::new(0, LaneKind::Direction, POLARITY_WEDGE_LANE).expect("lane"),
        draw: 0,
    });
    let expected_strength = (half_open_unit(s_block[0]) * 0.3).clamp(0.0, 0.3);
    let expected_wedge = ((2.0 * half_open_unit(w_block[0]) - 1.0) * 0.35).clamp(-0.35, 0.35);
    let expected = PolarityTraits::new(expected_strength, expected_wedge, 0.35).expect("expected");

    let actual_traits = founder_traits_v2(&rng, org, &limits, scale).expect("founder_traits_v2");
    assert_eq!(actual_traits.polarity_traits(), Some(expected));
    assert_eq!(actual_traits.cadherin_traits(), None);
    let baseline = founder_traits(&rng, org, &limits, scale).expect("baseline");
    assert_eq!(actual_traits.affinity(), baseline.affinity());
    assert_eq!(actual_traits.motion_response(), baseline.motion_response());
    assert_eq!(
        actual_traits.sensing_scale().to_bits(),
        baseline.sensing_scale().to_bits()
    );

    // Degenerate zero ceilings yield exact 0.0.
    let zero_founder = founder_polarity_traits(&rng, org, 0.0, 0.0).expect("zero");
    assert_eq!(zero_founder.polarity_strength(), 0.0);
    assert_eq!(zero_founder.wedge_curvature(), 0.0);

    // Forward vs reverse order invariance and uniform statistics over 512 founders.
    let forward: Vec<PolarityTraits> = (1..=512)
        .map(|id| founder_polarity_traits(&rng, OrganismId::new(id), 0.3, 0.35).expect("f"))
        .collect();
    let mut reverse: Vec<PolarityTraits> = (1..=512)
        .rev()
        .map(|id| founder_polarity_traits(&rng, OrganismId::new(id), 0.3, 0.35).expect("r"))
        .collect();
    reverse.reverse();
    assert_eq!(forward, reverse);

    let mut sum_s = 0.0;
    let mut sum_w = 0.0;
    for pol in &forward {
        assert!((0.0..=0.3).contains(&pol.polarity_strength()));
        assert!((-0.35..=0.35).contains(&pol.wedge_curvature()));
        sum_s += pol.polarity_strength();
        sum_w += pol.wedge_curvature();
    }
    let n = forward.len() as f64;
    // Uniform[0, 0.3] has mean 0.15; Uniform[-0.35, 0.35] has mean 0.0.
    assert!(
        ((sum_s / n) - 0.15).abs() < 0.02,
        "strength mean biased: {}",
        sum_s / n
    );
    assert!((sum_w / n).abs() < 0.04, "wedge mean biased: {}", sum_w / n);
}

/// Polarity mutation: interval reflection, zero-std identity, Philox
/// determinism across retries and detached commit, polarity-only
/// `field-ecology-v2-fixed-controller`, and strict `EffectiveNoop` both ways.
#[test]
fn polarity_mutation_philox_determinism_zero_rate_and_noop_rules() {
    // Symmetric interval reflection on [-0.35, 0.35] and degenerate [0, 0].
    assert!((reflect_interval(0.30, 0.10, [-0.35, 0.35]).expect("high") - 0.30).abs() <= 1e-14);
    assert!((reflect_interval(-0.30, -0.10, [-0.35, 0.35]).expect("low") + 0.30).abs() <= 1e-14);
    assert_eq!(
        reflect_interval(0.0, 0.25, [0.0, 0.0]).expect("degenerate"),
        0.0
    );
    assert!(reflect_interval(0.40, 0.0, [-0.35, 0.35]).is_err());
    assert!(reflect_interval(0.0, f64::NAN, [-0.35, 0.35]).is_err());
    assert!(reflect_interval(0.0, 100.0, [-0.35, 0.35]).is_err());

    let rng = SemanticRng::new(2080);
    let parent_pol = PolarityTraits::new(0.45, -0.12, 0.35).expect("parent");
    let unchanged =
        mutate_polarity_traits(&rng, parent_pol, 0.0, 0.35, attempt(10, 3), 0).expect("zero std");
    assert_eq!(unchanged, parent_pol);
    assert!(mutate_polarity_traits(&rng, parent_pol, -0.01, 0.35, attempt(10, 3), 0).is_err());
    assert!(mutate_polarity_traits(&rng, parent_pol, 0.05, 0.45, attempt(10, 3), 0).is_err());

    // Zero beta_max keeps wedge curvature at exact 0.0 while mutating strength.
    let flat_parent = PolarityTraits::new(0.45, 0.0, 0.0).expect("flat");
    let flat_child =
        mutate_polarity_traits(&rng, flat_parent, 0.05, 0.0, attempt(10, 3), 0).expect("flat mut");
    assert_eq!(flat_child.wedge_curvature(), 0.0);
    assert_ne!(
        flat_child.polarity_strength(),
        flat_parent.polarity_strength()
    );

    // Determinism across retry, ticks, and candidates.
    let m_a = mutate_polarity_traits(&rng, parent_pol, 0.08, 0.35, attempt(10, 3), 0).expect("a");
    let m_retry =
        mutate_polarity_traits(&rng, parent_pol, 0.08, 0.35, attempt(10, 3), 0).expect("retry");
    let m_tick4 =
        mutate_polarity_traits(&rng, parent_pol, 0.08, 0.35, attempt(10, 4), 0).expect("tick4");
    let m_cand1 =
        mutate_polarity_traits(&rng, parent_pol, 0.08, 0.35, attempt(10, 3), 1).expect("cand1");
    assert_eq!(m_a, m_retry);
    assert_ne!(m_a, parent_pol);
    assert_ne!(m_a, m_tick4);
    assert_ne!(m_a, m_cand1);
    for mutated in [m_a, m_tick4, m_cand1] {
        assert!((0.0..=1.0).contains(&mutated.polarity_strength()));
        assert!((-0.35..=0.35).contains(&mutated.wedge_curvature()));
    }

    // `field-ecology-v2-fixed-controller` with chemistry=0, sensing=0,
    // cadherin disabled, reject_effective_noop=true, and polarity_mutation_std=0.05
    // mutates ONLY polarity while leaving weights/affinity/motion/scale exact.
    let tail = "\n[v2.polarity]\nenabled = true\npolarity_mutation_std = 0.05\n\n\
                [v2.contact.cadherin]\nenabled = false\n";
    let mut raw_toml = appendix_a_toml()
        .replace(
            "model = \"field-ecology-v1\"",
            "model = \"field-ecology-v2-fixed-controller\"",
        )
        .replace(
            "chemistry_standard_deviation = 0.01",
            "chemistry_standard_deviation = 0.0",
        )
        .replace(
            "sensing_log_scale_standard_deviation = 0.02",
            "sensing_log_scale_standard_deviation = 0.0",
        );
    raw_toml.push_str(
        "\n[contact]\ndiameter = 0.75\nstiffness = 0.5\nadhesion = 1.0\nadhesion_range = 2.0\n",
    );
    raw_toml.push_str(tail);
    let cfg = ValidatedConfig::from_toml(&raw_toml).expect("polarity-only fixed-controller valid");
    let limits = TraitLimits::from_config(&cfg).expect("limits");
    let rates = MutationParameters::from_config(&cfg).expect("rates");
    assert_eq!(rates.topology_change_probability, 0.0);
    assert_eq!(rates.weight_probability, 0.0);
    assert_eq!(rates.replacement_probability, 0.0);
    assert_eq!(rates.chemistry_standard_deviation, 0.0);
    assert_eq!(rates.sensing_log_scale_standard_deviation, 0.0);
    assert!(rates.reject_effective_noop);
    assert!(limits.polarity_enabled());
    assert!(!limits.cadherin_enabled());

    let catalog = TopologyCatalog::from_config(&cfg).expect("catalog");
    let mutator =
        Mutator::new(rng, rates, &catalog, limits).expect("mutator accepts polarity-only");

    let mut q_direct = quota();
    let mut q_detached = quota();
    let parent = founder_v2(
        &rng,
        OrganismId::new(1),
        GenomeId::new(1),
        &catalog,
        &mut q_direct,
        &limits,
        initial_scale(&cfg),
    )
    .expect("parent");
    let parent_detached = founder_v2(
        &rng,
        OrganismId::new(1),
        GenomeId::new(1),
        &catalog,
        &mut q_detached,
        &limits,
        initial_scale(&cfg),
    )
    .expect("parent detached");

    let direct = mutator
        .propose(&parent, attempt(1, 15), GenomeId::new(2), &mut q_direct)
        .expect("propose");
    let detached = mutator
        .propose_detached(&parent_detached, attempt(1, 15), WeightDtype::F32)
        .expect("detached");
    assert!(matches!(detached, DetachedProposal::Child { .. }));
    let committed = mutator
        .commit_detached(
            detached,
            &parent_detached,
            attempt(1, 15),
            GenomeId::new(2),
            &mut q_detached,
        )
        .expect("commit");

    let (
        Proposal::Child {
            genome: child_a,
            record: rec_a,
        },
        Proposal::Child {
            genome: child_b,
            record: rec_b,
        },
    ) = (direct, committed)
    else {
        panic!("expected polarity-only child acceptance");
    };
    assert_eq!(rec_a, rec_b);
    assert_eq!(child_a.traits(), child_b.traits());
    assert!(child_a.parameters().same_bits(&parent.parameters()));
    assert_eq!(child_a.traits().affinity(), parent.traits().affinity());
    assert_eq!(
        child_a.traits().motion_response(),
        parent.traits().motion_response()
    );
    assert_eq!(
        child_a.traits().sensing_scale().to_bits(),
        parent.traits().sensing_scale().to_bits()
    );
    assert_ne!(
        child_a.traits().polarity_traits(),
        parent.traits().polarity_traits(),
        "polarity must mutate even when every other operator is zero"
    );

    // EffectiveNoop rejection when polarity_mutation_std == 0.0 and weights
    // round back to 0.5_f32; acceptance when polarity_mutation_std > 0.0.
    let zero_pol_limits = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 4.0)
        .expect("limits")
        .with_polarity(0.3, 0.35, 0.0)
        .expect("zero pol std");
    let mut uniform_buf = q_direct
        .allocate(&catalog, catalog.default_topology())
        .expect("uniform buffer");
    match uniform_buf.values_mut() {
        WeightsMut::F32(values) => values.fill(0.5_f32),
        WeightsMut::F16(_) => panic!("expected F32 quota"),
    }
    let uniform_parent = Genome::seal(
        uniform_buf,
        GenomeId::new(90),
        &catalog,
        *parent.traits(),
        None,
        &zero_pol_limits,
    )
    .expect("uniform polarity parent");

    let mut strict_quiet = quiet_rates();
    strict_quiet.reject_effective_noop = true;
    assert!(Mutator::new(rng, strict_quiet, &catalog, zero_pol_limits).is_err());

    let mut noop_rates = quiet_rates();
    noop_rates.weight_probability = 1.0;
    noop_rates.weight_standard_deviation = 1.0e-12;
    noop_rates.reject_effective_noop = true;
    noop_rates.maximum_candidates = 3;
    let noop_mutator = Mutator::new(rng, noop_rates, &catalog, zero_pol_limits).expect("mutator");
    match noop_mutator
        .propose(
            &uniform_parent,
            attempt(1, 20),
            GenomeId::new(3),
            &mut q_direct,
        )
        .expect("proposal")
    {
        Proposal::Rejected { candidates } => {
            assert_eq!(candidates.len(), 3);
            assert!(
                candidates
                    .iter()
                    .all(|c| c.reason == RejectionReason::EffectiveNoop)
            );
        }
        Proposal::Child { .. } => panic!("expected EffectiveNoop rejection"),
    }

    let active_pol_limits = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 4.0)
        .expect("limits")
        .with_polarity(0.3, 0.35, 0.05)
        .expect("active pol std");
    let active_mutator =
        Mutator::new(rng, noop_rates, &catalog, active_pol_limits).expect("active mutator");
    match active_mutator
        .propose(
            &uniform_parent,
            attempt(1, 20),
            GenomeId::new(4),
            &mut q_direct,
        )
        .expect("active proposal")
    {
        Proposal::Child { genome, .. } => {
            assert!(genome.parameters().same_bits(&uniform_parent.parameters()));
            assert_ne!(
                genome.traits().polarity_traits(),
                uniform_parent.traits().polarity_traits()
            );
        }
        Proposal::Rejected { candidates } => {
            panic!("active polarity must overcome weight no-op: {candidates:?}");
        }
    }
}

/// `Traits::reseal` preserves polarity (and enforces tighter host `β_max`),
/// `PolarityTraits::hash_into` distinguishes values, and checkpoint v3
/// round-trips both polarity-only (`+16 B`) and combined polarity + cadherin
/// (`+56 B`) while refusing corrupted bytes.
#[test]
fn traits_reseal_and_checkpoint_v3_round_trip_polarity_and_combined() {
    let v2_both = v2_contact_config("field-ecology-v2", "", &[]);
    let limits_both = TraitLimits::from_config(&v2_both).expect("both limits");
    assert!(limits_both.polarity_enabled() && limits_both.cadherin_enabled());
    assert_eq!(
        limits_both.genome_header_bytes(),
        BASELINE_GENOME_HEADER_BYTES + POLARITY_CHECKPOINT_BYTES + CADHERIN_CHECKPOINT_BYTES
    );

    let rng = SemanticRng::new(808);
    let combined = founder_traits_v2(
        &rng,
        OrganismId::new(42),
        &limits_both,
        initial_scale(&v2_both),
    )
    .expect("founder both");
    assert!(combined.has_polarity());
    assert!(combined.has_cadherin());
    assert_eq!(
        combined.checkpoint_bytes(),
        528 + POLARITY_CHECKPOINT_BYTES + CADHERIN_CHECKPOINT_BYTES
    );

    // Reseal preserves both polarity and cadherin under compatible limits AND
    // under a non-polarity host (global cap 0.4).
    let non_pol_host = TraitLimits::new(2.0, 2.0, [0.5, 32.0], 8.0).expect("host");
    let resealed = combined.reseal(&non_pol_host).expect("reseal host");
    assert_eq!(resealed, combined);

    // Reseal under a polarity host with a tighter beta_max rejects an out-of-range wedge.
    let extreme_pol = PolarityTraits::new(0.8, 0.30, 0.35).expect("extreme");
    let extreme_traits = combined
        .with_polarity_traits(Some(extreme_pol), &limits_both)
        .expect("attach");
    let tight_host = TraitLimits::new(2.0, 2.0, [0.5, 32.0], 8.0)
        .expect("tight")
        .with_polarity(0.3, 0.20, 0.05)
        .expect("tight pol");
    assert!(extreme_traits.reseal(&tight_host).is_err());

    // `hash_into` distinguishes polarity strength and wedge curvature.
    let p1 = combined.polarity_traits().expect("p1");
    let p2 = PolarityTraits::new(
        (p1.polarity_strength() + 0.1) % 1.0,
        p1.wedge_curvature(),
        0.35,
    )
    .expect("p2");
    let p3 = PolarityTraits::new(p1.polarity_strength(), -p1.wedge_curvature(), 0.35).expect("p3");
    let hash_of = |p: PolarityTraits| {
        let mut h = Sha256Stream::new();
        p.hash_into(&mut h);
        h.finish()
    };
    assert_eq!(hash_of(p1), hash_of(p1));
    assert_ne!(hash_of(p1), hash_of(p2));
    if p1.wedge_curvature() != 0.0 {
        assert_ne!(hash_of(p1), hash_of(p3));
    }

    // 1. Polarity-only checkpoint v3 round-trip (+16 B, 0 cadherin bytes).
    let v2_pol_only = v2_contact_config(
        "field-ecology-v2",
        "",
        &[FeatureOverride::Disable("cadherin-4d".into())],
    );
    let limits_pol = TraitLimits::from_config(&v2_pol_only).expect("pol limits");
    let pol_only = founder_traits_v2(
        &rng,
        OrganismId::new(42),
        &limits_pol,
        initial_scale(&v2_pol_only),
    )
    .expect("pol only");
    assert_eq!(pol_only.checkpoint_bytes(), 528 + POLARITY_CHECKPOINT_BYTES);

    let mut writer = CheckpointWriter::new("field-ecology-v2");
    writer
        .chunk_with(ChunkFamily::Genomes, 3, 1, &[], |out| {
            pol_only.write_checkpoint_fields_v2(out, true, false)
        })
        .expect("writes pol only");
    let bytes = writer.finish().expect("finishes");
    let reader = CheckpointReader::parse(&bytes, &[ChunkFamily::Genomes]).expect("parses");
    let chunk = reader.versioned(ChunkFamily::Genomes, 3).expect("chunk");
    assert_eq!(chunk.bytes.len(), 528 + POLARITY_CHECKPOINT_BYTES);
    let mut input = ByteReader::new(chunk.bytes, "polarity chunk");
    let restored = Traits::read_checkpoint_fields_v2(&mut input, &limits_pol, true, false)
        .expect("restores pol only");
    input.finish().expect("no trailing bytes");
    assert_eq!(restored, pol_only);

    // 2. Combined polarity + cadherin checkpoint v3 round-trip (+56 B).
    let mut writer = CheckpointWriter::new("field-ecology-v2");
    writer
        .chunk_with(ChunkFamily::Genomes, 3, 1, &[], |out| {
            combined.write_checkpoint_fields_v2(out, true, true)
        })
        .expect("writes combined");
    let bytes = writer.finish().expect("finishes");
    let reader = CheckpointReader::parse(&bytes, &[ChunkFamily::Genomes]).expect("parses");
    let chunk = reader.versioned(ChunkFamily::Genomes, 3).expect("chunk");
    assert_eq!(
        chunk.bytes.len(),
        528 + POLARITY_CHECKPOINT_BYTES + CADHERIN_CHECKPOINT_BYTES
    );
    let mut input = ByteReader::new(chunk.bytes, "combined chunk");
    let restored_both = Traits::read_checkpoint_fields_v2(&mut input, &limits_both, true, true)
        .expect("restores combined");
    input.finish().expect("no trailing bytes");
    assert_eq!(restored_both, combined);

    // Writing with `include_polarity = true` on traits lacking polarity refuses cleanly.
    let v1_traits = founder_traits(
        &rng,
        OrganismId::new(42),
        &limits_pol,
        initial_scale(&v2_pol_only),
    )
    .expect("v1 traits");
    let mut bad_out = ByteWriter::new();
    let err = v1_traits
        .write_bytes_v2(&mut bad_out, true, false)
        .expect_err("missing polarity must refuse");
    assert!(matches!(err, CoreError::IncompatibleCheckpoint { .. }));

    // Corrupted polarity bytes (out-of-range strength or wedge > beta_max) refuse cleanly.
    let mut raw = ByteWriter::new();
    pol_only
        .write_bytes_v2(&mut raw, true, false)
        .expect("writes");
    let mut bad_strength = raw.into_bytes();
    bad_strength[528..536].copy_from_slice(&1.5_f64.to_le_bytes());
    let mut reader = ByteReader::new(&bad_strength, "corrupt strength");
    assert!(matches!(
        Traits::read_checkpoint_fields_v2(&mut reader, &limits_pol, true, false),
        Err(CoreError::IncompatibleCheckpoint { .. })
    ));

    let mut raw = ByteWriter::new();
    pol_only
        .write_bytes_v2(&mut raw, true, false)
        .expect("writes");
    let mut bad_wedge = raw.into_bytes();
    bad_wedge[536..544].copy_from_slice(&0.39_f64.to_le_bytes()); // > limits_pol 0.35
    let mut reader = ByteReader::new(&bad_wedge, "corrupt wedge");
    assert!(matches!(
        Traits::read_checkpoint_fields_v2(&mut reader, &limits_pol, true, false),
        Err(CoreError::IncompatibleCheckpoint { .. })
    ));
}

/// Regression: `founder_polarity_traits` must validate `maximum_wedge_curvature`
/// before calling `.clamp(-maximum_wedge_curvature, maximum_wedge_curvature)` so
/// negative, NaN, infinite, or over-cap inputs return `NumericalInvalid` instead
/// of panicking inside `f64::clamp`.
#[test]
fn founder_polarity_traits_refuses_invalid_wedge_bounds_without_panicking() {
    let rng = SemanticRng::new(99);
    for bad_wedge in [-0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.5] {
        let err = founder_polarity_traits(&rng, OrganismId::new(1), 0.5, bad_wedge)
            .expect_err("invalid maximum_wedge_curvature must be refused");
        assert!(
            matches!(err, CoreError::NumericalInvalid { .. }),
            "bad_wedge={bad_wedge:?}: expected NumericalInvalid, got {err:?}"
        );
    }
}
