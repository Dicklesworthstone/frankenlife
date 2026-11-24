//! V2-13 (`cadherin-4d`) genome-layer evidence: unit 4-D cadherin tag and
//! cohesiveness, exact left-fold unit-ball safety (`shrink_to_left_fold_norm_limit`),
//! zero-norm refusal, semantic Philox founder init and mutation determinism,
//! `Traits::reseal` preservation, dynamic `genome_header_bytes`, and checkpoint
//! v3 serialization round-trip with 0 extra checkpoint bytes and bit-exact
//! baseline parity when disabled.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure here is a test failure"
)]

use flife_core::checkpoint::{
    ByteReader, ByteWriter, CheckpointReader, CheckpointWriter, ChunkFamily,
};
use flife_core::ids::{GenomeId, OrganismId};
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream, box_muller, half_open_unit};
use flife_core::{CoreError, FeatureOverride, Sha256Stream, Tick, ValidatedConfig};
use flife_genome::{
    AFFINITY_TERMS, BASELINE_GENOME_HEADER_BYTES, BirthAttempt, CADHERIN_CHECKPOINT_BYTES,
    CADHERIN_COHESIVENESS_LANE, CADHERIN_TAG_LANE_BASE, CADHERIN_TERMS, CadherinTraits,
    DetachedProposal, Genome, GenomeQuota, MAXIMUM_CANDIDATES, MOTION_RESPONSE_TERMS,
    MutationParameters, Mutator, POLARITY_CHECKPOINT_BYTES, Proposal, RejectionReason,
    TopologyCatalog, TraitLimits, Traits, WeightDtype, WeightsMut, cadherin_checkpoint_bytes,
    founder, founder_cadherin_traits, founder_traits, founder_traits_v2, founder_v2,
    genome_header_bytes, genome_header_bytes_v2, initial_scale, mutate_cadherin_traits,
    reflect_unit_interval,
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

fn left_fold_sq(values: &[f64]) -> f64 {
    values.iter().fold(0.0_f64, |acc, &v| acc + v * v)
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

/// V2-13 disabled parity: `field-ecology-v1` and `field-ecology-v2 --disable
/// cadherin-4d` match baseline `founder` / `founder_traits` / `Mutator` bit for
/// bit with 0 extra checkpoint bytes and `None` cadherin accessors.
#[test]
fn cadherin_disabled_matches_v1_bit_exact_with_zero_extra_checkpoint_bytes() {
    let v1 = v1_config();
    let v2_disabled = v2_contact_config(
        "field-ecology-v2",
        "",
        &[
            FeatureOverride::Disable("cadherin-4d".into()),
            FeatureOverride::Disable("pcp-polarity".into()),
        ],
    );
    let v2_cad_only_disabled = v2_contact_config(
        "field-ecology-v2",
        "",
        &[FeatureOverride::Disable("cadherin-4d".into())],
    );

    let limits_v1 = TraitLimits::from_config(&v1).expect("v1 limits");
    let limits_v2_off = TraitLimits::from_config(&v2_disabled).expect("v2 disabled limits");
    let limits_cad_off =
        TraitLimits::from_config(&v2_cad_only_disabled).expect("v2 cad-only off limits");
    assert!(!limits_v1.cadherin_enabled());
    assert!(!limits_v2_off.cadherin_enabled());
    assert!(!limits_cad_off.cadherin_enabled());
    assert!(limits_cad_off.polarity_enabled());
    assert_eq!(limits_v1.cadherin_mutation_std(), 0.0);
    assert_eq!(limits_v2_off.cadherin_mutation_std(), 0.0);
    assert_eq!(genome_header_bytes(false), BASELINE_GENOME_HEADER_BYTES);
    assert_eq!(genome_header_bytes(false), 553);
    assert_eq!(cadherin_checkpoint_bytes(false), 0);
    assert_eq!(limits_v1.genome_header_bytes(), 553);
    assert_eq!(limits_v2_off.genome_header_bytes(), 553);
    assert_eq!(
        limits_cad_off.genome_header_bytes(),
        BASELINE_GENOME_HEADER_BYTES + POLARITY_CHECKPOINT_BYTES
    );

    let rng = SemanticRng::new(77);
    let scale = initial_scale(&v1);
    let catalog = TopologyCatalog::from_config(&v1).expect("catalog");
    let mut q_base = quota();
    let mut q_v2_off = quota();

    for id in 1..=16_u64 {
        let base_traits = founder_traits(&rng, OrganismId::new(id), &limits_v1, scale).expect("t");
        let off_traits =
            founder_traits_v2(&rng, OrganismId::new(id), &limits_v2_off, scale).expect("t2");
        assert_eq!(base_traits, off_traits);
        assert!(!off_traits.has_cadherin());
        assert_eq!(off_traits.cadherin(), None);
        assert_eq!(off_traits.cohesiveness(), None);
        assert_eq!(off_traits.cadherin_traits(), None);
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
            &mut q_v2_off,
            &limits_v2_off,
            scale,
        )
        .expect("founder_v2");
        assert_eq!(g_base.traits(), g_off.traits());
        assert!(g_base.parameters().same_bits(&g_off.parameters()));

        // Checkpoint field encoding writes exact baseline 528 bytes (+0 extra).
        let mut raw_base = ByteWriter::new();
        g_base
            .traits()
            .write_bytes(&mut raw_base, false)
            .expect("write base");
        let mut raw_off = ByteWriter::new();
        g_off
            .traits()
            .write_bytes(&mut raw_off, false)
            .expect("write off");
        assert_eq!(raw_base.as_slice().len(), 528);
        assert_eq!(raw_base.as_slice(), raw_off.as_slice());

        // Mutator produces bit-identical children when cadherin-4d is disabled.
        let rates = MutationParameters::from_config(&v1).expect("rates");
        let m_base = Mutator::new(rng, rates, &catalog, limits_v1).expect("m1");
        let m_off = Mutator::new(rng, rates, &catalog, limits_v2_off).expect("m2");
        let p_base = m_base
            .propose(
                &g_base,
                attempt(id, 5),
                GenomeId::new(100 + id),
                &mut q_base,
            )
            .expect("propose base");
        let p_off = m_off
            .propose(
                &g_off,
                attempt(id, 5),
                GenomeId::new(100 + id),
                &mut q_v2_off,
            )
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
                assert!(!b.traits().has_cadherin());
            }
            (other_a, other_b) => panic!("unexpected mismatch: {other_a:?} vs {other_b:?}"),
        }
    }
}

/// Unit-norm, left-fold shrink, cohesiveness bounds, and zero-norm guards.
#[test]
fn cadherin_4d_unit_norm_cohesiveness_and_zero_norm_guards() {
    // Valid exact unit vectors and boundary cohesiveness values.
    for tag in [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, -1.0, 0.0, 0.0],
        [0.5, 0.5, 0.5, 0.5],
        [0.6, -0.8, 0.0, 0.0],
    ] {
        for coh in [0.0, 0.5, 1.0] {
            let cad = CadherinTraits::new(tag, coh).expect("valid cadherin");
            assert_eq!(cad.tag(), &tag);
            assert_eq!(cad.cohesiveness(), coh);
            assert!(left_fold_sq(cad.tag()) > 0.0);
            assert!(left_fold_sq(cad.tag()) <= 1.0);
        }
    }

    // Adversarial vectors whose naive `v / norm` can round above `1.0` in
    // left-fold squared norm: `normalized` must guarantee `left_fold_sq <= 1.0`.
    let adversarial = [
        [1.0, 1.0, 1.0, 1.0],
        [1.0, 2.0, 3.0, 4.0],
        [1.0e150, -1.0e150, 1.0e150, -1.0e150],
        [3.0, 5.0, 7.0, 11.0],
        [0.3, 0.7, 0.11, 0.13],
        [1.0 + f64::EPSILON, 1.0, 1.0, 1.0],
        [1.0e-100, 2.0e-100, 3.0e-100, 4.0e-100],
    ];
    for raw in adversarial {
        let cad = CadherinTraits::normalized(raw, 0.75).expect("normalizes");
        let sq = left_fold_sq(cad.tag());
        assert!(sq > 0.0 && sq <= 1.0, "raw={raw:?} gave sq={sq:?}");
        assert!(
            (sq.sqrt() - 1.0).abs() <= 4.0 * f64::EPSILON,
            "norm drifted from 1: {}",
            sq.sqrt()
        );
    }

    // Zero norm, subnormal-underflow zero norm, out-of-ball, and non-finite
    // tags must all be strictly refused.
    for bad_tag in [
        [0.0; CADHERIN_TERMS],
        [-0.0; CADHERIN_TERMS],
        [1.0e-200, 0.0, 0.0, 0.0], // squared norm underflows to 0.0
        [1.0, 1.0, 0.0, 0.0],      // squared norm 2.0 > 1.0
        [1.0 + 1e-12, 0.0, 0.0, 0.0],
        [f64::NAN, 0.0, 0.0, 0.0],
        [f64::INFINITY, 0.0, 0.0, 0.0],
        [f64::NEG_INFINITY, 0.0, 0.0, 0.0],
    ] {
        let err = CadherinTraits::new(bad_tag, 0.5).expect_err("must reject bad tag");
        assert!(
            matches!(err, CoreError::NumericalInvalid { .. }),
            "bad_tag={bad_tag:?} got {err:?}"
        );
    }
    for bad_norm_input in [
        [0.0; CADHERIN_TERMS],
        [1.0e-200, 0.0, 0.0, 0.0],
        [f64::NAN, 1.0, 0.0, 0.0],
        [f64::INFINITY, 1.0, 0.0, 0.0],
    ] {
        let err = CadherinTraits::normalized(bad_norm_input, 0.5)
            .expect_err("normalized must reject zero/non-finite");
        assert!(
            matches!(err, CoreError::NumericalInvalid { .. }),
            "bad_norm_input={bad_norm_input:?} got {err:?}"
        );
    }

    // Out-of-range or non-finite cohesiveness must be strictly refused.
    for bad_coh in [
        -1.0,
        -f64::MIN_POSITIVE,
        1.0 + 1e-15,
        f64::NAN,
        f64::INFINITY,
    ] {
        let err = CadherinTraits::new([1.0, 0.0, 0.0, 0.0], bad_coh)
            .expect_err("must reject bad cohesiveness");
        assert!(
            matches!(err, CoreError::NumericalInvalid { .. }),
            "bad_coh={bad_coh:?} got {err:?}"
        );
    }
}

/// Decoupled cadherin lets orthogonal-diet symbionts (`a_1 ⊥ a_2`) adhere
/// symmetrically when `c_1 == c_2`, while orthogonal/anti-parallel cadherin
/// tags yield exact `0.0`.
#[test]
fn cadherin_4d_orthogonal_diet_symmetric_adhesion_weight() {
    let limits = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 4.0)
        .expect("limits")
        .with_cadherin(0.05)
        .expect("cadherin");
    let mut a1 = [0.0; AFFINITY_TERMS];
    a1[0] = 1.0;
    let mut a2 = [0.0; AFFINITY_TERMS];
    a2[1] = 1.0;
    let shared_tag = CadherinTraits::normalized([1.0, 2.0, -1.0, 0.5], 0.8).expect("c1");
    let shared_tag_lower = CadherinTraits::normalized([1.0, 2.0, -1.0, 0.5], 0.35).expect("c2");
    let orthog_tag = CadherinTraits::new([0.0, 0.0, 0.0, 1.0], 1.0).expect("orthog");
    let x_tag = CadherinTraits::new([1.0, 0.0, 0.0, 0.0], 1.0).expect("x");
    let neg_x_tag = CadherinTraits::new([-1.0, 0.0, 0.0, 0.0], 1.0).expect("-x");

    let t1 = Traits::with_cadherin(
        a1,
        [0.0; MOTION_RESPONSE_TERMS],
        2.0,
        Some(shared_tag),
        &limits,
    )
    .expect("t1");
    let t2 = Traits::with_cadherin(
        a2,
        [0.0; MOTION_RESPONSE_TERMS],
        2.0,
        Some(shared_tag_lower),
        &limits,
    )
    .expect("t2");

    // Diets are strictly orthogonal, yet cadherin adhesion weight is min(0.8, 0.35) * 1^2 = 0.35.
    let diet_dot: f64 = t1
        .affinity()
        .iter()
        .zip(t2.affinity())
        .map(|(x, y)| x * y)
        .sum();
    assert_eq!(diet_dot, 0.0);
    let w12 = t1
        .cadherin_traits()
        .expect("c1")
        .pair_adhesion_weight(&t2.cadherin_traits().expect("c2"));
    let w21 = t2
        .cadherin_traits()
        .expect("c2")
        .pair_adhesion_weight(&t1.cadherin_traits().expect("c1"));
    assert_eq!(w12.to_bits(), w21.to_bits(), "bit-exact symmetry");
    assert!((w12 - 0.35).abs() <= 1e-14, "expected 0.35, got {w12}");

    // Orthogonal & anti-parallel tags produce exact 0.0.
    assert_eq!(x_tag.pair_adhesion_weight(&orthog_tag), 0.0);
    assert_eq!(x_tag.pair_adhesion_weight(&neg_x_tag), 0.0);

    // 45-degree angle: cos = 1/sqrt(2), cos^2 = 0.5.
    let diag = CadherinTraits::normalized([1.0, 1.0, 0.0, 0.0], 0.6).expect("diag");
    let w_diag = x_tag.pair_adhesion_weight(&diag);
    assert!((w_diag - 0.3).abs() <= 1e-14, "expected 0.3, got {w_diag}");

    // Bit-exact symmetry across 128 Philox founders.
    let rng = SemanticRng::new(9001);
    let founders: Vec<CadherinTraits> = (1..=128)
        .map(|id| founder_cadherin_traits(&rng, OrganismId::new(id)).expect("founder"))
        .collect();
    for i in 0..founders.len() {
        let j = (i * 37 + 13) % founders.len();
        let ab = founders[i].pair_adhesion_weight(&founders[j]);
        let ba = founders[j].pair_adhesion_weight(&founders[i]);
        assert_eq!(ab.to_bits(), ba.to_bits());
        assert!((0.0..=1.0).contains(&ab));
    }
}

/// Founder Philox initialization: bit-exact oracle parity on lanes `0x1300`,
/// `0x1301`, `0x1310`, forward/reverse order invariance, left-fold unit-ball
/// compliance, and isotropic 4-D moments over 512 founders.
#[test]
fn founder_cadherin_philox_determinism_order_invariance_and_isotropy() {
    let v2 = v2_contact_config("field-ecology-v2", "", &[]);
    let limits = TraitLimits::from_config(&v2).expect("v2 limits");
    assert!(limits.cadherin_enabled());
    assert!(limits.polarity_enabled());
    assert!((limits.cadherin_mutation_std() - 0.05).abs() <= 1e-15);
    assert_eq!(
        limits.genome_header_bytes(),
        BASELINE_GENOME_HEADER_BYTES + POLARITY_CHECKPOINT_BYTES + CADHERIN_CHECKPOINT_BYTES
    );
    assert_eq!(limits.genome_header_bytes(), 609);
    assert_eq!(genome_header_bytes_v2(false, true), 593);
    assert_eq!(genome_header_bytes(true), 593);

    let rng = SemanticRng::new(12345);
    let scale = initial_scale(&v2);

    // Independent oracle on exact documented Philox lanes for organism 7.
    let org = OrganismId::new(7);
    let mut expected_tag = [0.0; CADHERIN_TERMS];
    for pair in 0..2_u64 {
        let block = rng.block(Address {
            stream: Stream::Founders,
            entity: org.get(),
            ordinal: 0,
            lane: Lane::new(0, LaneKind::Direction, CADHERIN_TAG_LANE_BASE + pair).expect("lane"),
            draw: 0,
        });
        let [n0, n1] = box_muller(block[0], block[1]).expect("bm");
        expected_tag[(pair * 2) as usize] = n0;
        expected_tag[(pair * 2 + 1) as usize] = n1;
    }
    let coh_block = rng.block(Address {
        stream: Stream::Founders,
        entity: org.get(),
        ordinal: 0,
        lane: Lane::new(0, LaneKind::Direction, CADHERIN_COHESIVENESS_LANE).expect("lane"),
        draw: 0,
    });
    let expected =
        CadherinTraits::normalized(expected_tag, half_open_unit(coh_block[0])).expect("expected");

    let actual_traits = founder_traits_v2(&rng, org, &limits, scale).expect("founder_traits_v2");
    assert_eq!(actual_traits.cadherin_traits(), Some(expected));
    // Baseline affinity, motion response, and sensing scale are untouched by cadherin init.
    let baseline = founder_traits(&rng, org, &limits, scale).expect("baseline");
    assert_eq!(actual_traits.affinity(), baseline.affinity());
    assert_eq!(actual_traits.motion_response(), baseline.motion_response());
    assert_eq!(
        actual_traits.sensing_scale().to_bits(),
        baseline.sensing_scale().to_bits()
    );

    // Forward vs reverse order invariance and isotropic statistics over 512 founders.
    let forward: Vec<CadherinTraits> = (1..=512)
        .map(|id| founder_cadherin_traits(&rng, OrganismId::new(id)).expect("f"))
        .collect();
    let mut reverse: Vec<CadherinTraits> = (1..=512)
        .rev()
        .map(|id| founder_cadherin_traits(&rng, OrganismId::new(id)).expect("r"))
        .collect();
    reverse.reverse();
    assert_eq!(forward, reverse);

    let mut mean_tag = [0.0; CADHERIN_TERMS];
    let mut mean_sq = [0.0; CADHERIN_TERMS];
    let mut mean_coh = 0.0;
    for cad in &forward {
        let sq = left_fold_sq(cad.tag());
        assert!(sq > 0.0 && sq <= 1.0);
        assert!((sq.sqrt() - 1.0).abs() <= 4.0 * f64::EPSILON);
        assert!((0.0..1.0).contains(&cad.cohesiveness()));
        for k in 0..CADHERIN_TERMS {
            mean_tag[k] += cad.tag()[k];
            mean_sq[k] += cad.tag()[k] * cad.tag()[k];
        }
        mean_coh += cad.cohesiveness();
    }
    let n = forward.len() as f64;
    for k in 0..CADHERIN_TERMS {
        assert!(
            (mean_tag[k] / n).abs() < 0.12,
            "component {k} mean biased: {}",
            mean_tag[k] / n
        );
        // Expected E[c_k^2] = 1/4 = 0.25 on S^3.
        assert!(
            ((mean_sq[k] / n) - 0.25).abs() < 0.06,
            "component {k} variance off: {}",
            mean_sq[k] / n
        );
    }
    assert!(
        ((mean_coh / n) - 0.5).abs() < 0.06,
        "cohesiveness mean off: {}",
        mean_coh / n
    );
}

/// Cadherin mutation: zero-std identity, Philox determinism across retries and
/// detached commit, fixed-controller strict no-op acceptance when only cadherin
/// mutates, and bounded unit-interval reflection.
#[test]
fn cadherin_mutation_philox_determinism_zero_rate_and_noop_rules() {
    // Reflection at boundaries 0.0 and 1.0, plus refusals.
    assert!((reflect_unit_interval(0.95, 0.10).expect("reflect high") - 0.95).abs() <= 1e-14);
    assert!((reflect_unit_interval(0.05, -0.10).expect("reflect low") - 0.05).abs() <= 1e-14);
    assert!((reflect_unit_interval(0.5, 2.0).expect("double reflect") - 0.5).abs() <= 1e-14);
    assert!(reflect_unit_interval(-0.1, 0.0).is_err());
    assert!(reflect_unit_interval(1.1, 0.0).is_err());
    assert!(reflect_unit_interval(0.5, f64::NAN).is_err());
    assert!(reflect_unit_interval(0.5, 100.0).is_err());

    let rng = SemanticRng::new(2026);
    let parent_cad = CadherinTraits::normalized([1.0, 0.5, -0.5, 0.25], 0.6).expect("parent");
    // mutation_std == 0.0 leaves cadherin bit-identical.
    let unchanged =
        mutate_cadherin_traits(&rng, parent_cad, 0.0, attempt(10, 3), 0).expect("zero std");
    assert_eq!(unchanged, parent_cad);
    assert!(mutate_cadherin_traits(&rng, parent_cad, -0.01, attempt(10, 3), 0).is_err());
    assert!(mutate_cadherin_traits(&rng, parent_cad, f64::NAN, attempt(10, 3), 0).is_err());

    // Non-zero mutation_std is deterministic on retry, distinct across ticks
    // and candidates, and stays strictly inside left-fold unit ball & [0, 1].
    let m_a = mutate_cadherin_traits(&rng, parent_cad, 0.08, attempt(10, 3), 0).expect("m_a");
    let m_retry = mutate_cadherin_traits(&rng, parent_cad, 0.08, attempt(10, 3), 0).expect("retry");
    let m_tick4 = mutate_cadherin_traits(&rng, parent_cad, 0.08, attempt(10, 4), 0).expect("tick4");
    let m_cand1 = mutate_cadherin_traits(&rng, parent_cad, 0.08, attempt(10, 3), 1).expect("cand1");
    assert_eq!(m_a, m_retry);
    assert_ne!(m_a, parent_cad);
    assert_ne!(m_a, m_tick4);
    assert_ne!(m_a, m_cand1);
    for mutated in [m_a, m_tick4, m_cand1] {
        let sq = left_fold_sq(mutated.tag());
        assert!(sq > 0.0 && sq <= 1.0);
        assert!((0.0..=1.0).contains(&mutated.cohesiveness()));
    }

    // `field-ecology-v2-fixed-controller` with chemistry=0, sensing=0,
    // reject_effective_noop=true, and cadherin.mutation_std=0.05 validates and
    // mutates ONLY cadherin while keeping weights/affinity/motion/scale exact!
    let tail = "\n[v2.polarity]\nenabled = false\n\n[v2.contact.cadherin]\nenabled = true\nmutation_std = 0.05\n";
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
    let cfg = ValidatedConfig::from_toml(&raw_toml).expect("cadherin-only fixed-controller valid");
    let limits = TraitLimits::from_config(&cfg).expect("limits");
    let rates = MutationParameters::from_config(&cfg).expect("rates");
    assert_eq!(rates.topology_change_probability, 0.0);
    assert_eq!(rates.weight_probability, 0.0);
    assert_eq!(rates.replacement_probability, 0.0);
    assert_eq!(rates.chemistry_standard_deviation, 0.0);
    assert_eq!(rates.sensing_log_scale_standard_deviation, 0.0);
    assert!(rates.reject_effective_noop);
    assert!(limits.cadherin_enabled());
    assert!((limits.cadherin_mutation_std() - 0.05).abs() <= 1e-15);
    let catalog = TopologyCatalog::from_config(&cfg).expect("catalog");
    let mutator =
        Mutator::new(rng, rates, &catalog, limits).expect("mutator accepts cadherin-only");

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
    let parent_clone_for_detached = founder_v2(
        &rng,
        OrganismId::new(1),
        GenomeId::new(1),
        &catalog,
        &mut q_detached,
        &limits,
        initial_scale(&cfg),
    )
    .expect("parent 2");

    let direct = mutator
        .propose(&parent, attempt(1, 12), GenomeId::new(2), &mut q_direct)
        .expect("propose");
    let detached = mutator
        .propose_detached(&parent_clone_for_detached, attempt(1, 12), WeightDtype::F32)
        .expect("detached");
    assert!(matches!(detached, DetachedProposal::Child { .. }));
    let committed = mutator
        .commit_detached(
            detached,
            &parent_clone_for_detached,
            attempt(1, 12),
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
        panic!("expected cadherin-only child acceptance");
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
        child_a.traits().cadherin_traits(),
        parent.traits().cadherin_traits(),
        "cadherin must mutate even when all other operators are zero"
    );

    // Zero cadherin_std with reject_effective_noop=true rejects all candidates as EffectiveNoop.
    // Use a parent whose every parameter is 0.5_f32 (half-ulp 2^-25) so a 1e-12
    // perturbation always rounds bit-identically back to 0.5_f32 (unlike founder
    // 0.0_f32 biases where tiny negative perturbations produce -0.0_f32 or subnormals).
    let zero_cad_limits = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 4.0)
        .expect("limits")
        .with_cadherin(0.0)
        .expect("zero cad std");
    let mut uniform_buf = q_direct
        .allocate(&catalog, catalog.default_topology())
        .expect("uniform buffer");
    match uniform_buf.values_mut() {
        WeightsMut::F32(values) => values.fill(0.5_f32),
        WeightsMut::F16(_) => panic!("expected F32 quota for Appendix A test"),
    }
    let uniform_parent = Genome::seal(
        uniform_buf,
        GenomeId::new(90),
        &catalog,
        *parent.traits(),
        None,
        &zero_cad_limits,
    )
    .expect("uniform cadherin parent");
    let mut strict_quiet = quiet_rates();
    strict_quiet.reject_effective_noop = true;
    assert!(
        Mutator::new(rng, strict_quiet, &catalog, zero_cad_limits).is_err(),
        "strict noop with every operator (including cadherin) at 0 must be refused"
    );
    let mut noop_rates = quiet_rates();
    noop_rates.weight_probability = 1.0;
    noop_rates.weight_standard_deviation = 1.0e-12; // below 0.5_f32 half-ulp (2^-25)
    noop_rates.reject_effective_noop = true;
    noop_rates.maximum_candidates = 3;
    let noop_mutator = Mutator::new(rng, noop_rates, &catalog, zero_cad_limits).expect("mutator");
    let res = noop_mutator
        .propose(
            &uniform_parent,
            attempt(1, 20),
            GenomeId::new(3),
            &mut q_direct,
        )
        .expect("proposal");
    match res {
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
    // With the exact same sub-ULP weight rates, enabling cadherin mutation accepts
    // a child whose weights are bit-identical while cadherin changed.
    let active_cad_limits = TraitLimits::new(1.0, 1.0, [1.0, 16.0], 4.0)
        .expect("limits")
        .with_cadherin(0.05)
        .expect("active cad std");
    let active_mutator =
        Mutator::new(rng, noop_rates, &catalog, active_cad_limits).expect("active mutator");
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
                genome.traits().cadherin_traits(),
                uniform_parent.traits().cadherin_traits()
            );
        }
        Proposal::Rejected { candidates } => {
            panic!("active cadherin must overcome weight no-op: {candidates:?}");
        }
    }
}

/// `Traits::reseal` preserves cadherin tag & cohesiveness cleanly for
/// `crop.rs`/`graft.rs`, `hash_into` tracks cadherin changes, and checkpoint
/// v3 streaming encode/decode round-trips bit-identically while rejecting
/// corrupted cadherin bytes.
#[test]
fn traits_reseal_preserves_cadherin_and_checkpoint_v3_round_trip() {
    let v2 = v2_contact_config(
        "field-ecology-v2",
        "",
        &[FeatureOverride::Disable("pcp-polarity".into())],
    );
    let limits = TraitLimits::from_config(&v2).expect("v2 limits");
    let rng = SemanticRng::new(555);
    let original = founder_traits_v2(&rng, OrganismId::new(42), &limits, initial_scale(&v2))
        .expect("founder v2");
    assert!(original.has_cadherin());
    assert_eq!(original.checkpoint_bytes(), 528 + CADHERIN_CHECKPOINT_BYTES);

    // Reseal under compatible limits (including a host with different scale
    // range or disabled flag) preserves cadherin bit-for-bit.
    let wider = TraitLimits::new(2.0, 2.0, [0.5, 32.0], 8.0)
        .expect("wider")
        .with_cadherin(0.1)
        .expect("cad");
    let resealed = original.reseal(&wider).expect("reseal");
    assert_eq!(resealed, original);
    assert_eq!(resealed.cadherin(), original.cadherin());
    assert_eq!(resealed.cohesiveness(), original.cohesiveness());

    // Reseal still enforces baseline bounds (e.g., tighter sensing scale range).
    let impossible_scale = TraitLimits::new(1.0, 1.0, [10.0, 16.0], 4.0)
        .expect("tight")
        .with_cadherin(0.05)
        .expect("cad");
    assert!(original.reseal(&impossible_scale).is_err());

    // `CadherinTraits::hash_into` distinguishes tags and cohesiveness.
    let c1 = original.cadherin_traits().expect("cad");
    let c2 = CadherinTraits::new(*c1.tag(), (c1.cohesiveness() + 0.1) % 1.0).expect("c2");
    let hash_of = |c: CadherinTraits| {
        let mut h = Sha256Stream::new();
        c.hash_into(&mut h);
        h.finish()
    };
    assert_eq!(hash_of(c1), hash_of(c1));
    assert_ne!(hash_of(c1), hash_of(c2));

    // Full CheckpointWriter / CheckpointReader round-trip with cadherin enabled (+40 B).
    let mut writer = CheckpointWriter::new("field-ecology-v2");
    writer
        .chunk_with(ChunkFamily::Genomes, 3, 1, &[], |out| {
            original.write_checkpoint_fields(out, true)
        })
        .expect("writes v3 traits");
    let bytes = writer.finish().expect("finishes");
    let reader = CheckpointReader::parse(&bytes, &[ChunkFamily::Genomes]).expect("parses");
    let chunk = reader.versioned(ChunkFamily::Genomes, 3).expect("chunk");
    assert_eq!(chunk.bytes.len(), 528 + CADHERIN_CHECKPOINT_BYTES);
    let mut input = ByteReader::new(chunk.bytes, "genomes chunk");
    let restored = Traits::read_checkpoint_fields(&mut input, &limits, true).expect("restores");
    input.finish().expect("no trailing bytes");
    assert_eq!(restored, original);

    // Writing with `include_cadherin = true` on traits lacking cadherin refuses cleanly.
    let v1_traits =
        founder_traits(&rng, OrganismId::new(42), &limits, initial_scale(&v2)).expect("v1 traits");
    let mut bad_out = ByteWriter::new();
    let err = v1_traits
        .write_bytes(&mut bad_out, true)
        .expect_err("missing cadherin must refuse");
    assert!(matches!(err, CoreError::IncompatibleCheckpoint { .. }));

    // Corrupted checkpoint bytes: zero-norm cadherin tag and out-of-range
    // cohesiveness are refused by `read_checkpoint_fields`.
    let mut raw = ByteWriter::new();
    original.write_bytes(&mut raw, true).expect("writes");
    let mut zero_tag = raw.into_bytes();
    let cad_offset = 528;
    zero_tag[cad_offset..cad_offset + 32].fill(0);
    let mut reader = ByteReader::new(&zero_tag, "corrupt zero tag");
    assert!(matches!(
        Traits::read_checkpoint_fields(&mut reader, &limits, true),
        Err(CoreError::IncompatibleCheckpoint { .. })
    ));

    let mut bad_coh = ByteWriter::new();
    original.write_bytes(&mut bad_coh, true).expect("writes");
    let mut bad_coh = bad_coh.into_bytes();
    bad_coh[cad_offset + 32..cad_offset + 40].copy_from_slice(&1.25_f64.to_le_bytes());
    let mut reader = ByteReader::new(&bad_coh, "corrupt cohesiveness");
    assert!(matches!(
        Traits::read_checkpoint_fields(&mut reader, &limits, true),
        Err(CoreError::IncompatibleCheckpoint { .. })
    ));
}
