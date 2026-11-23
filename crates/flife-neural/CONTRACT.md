# flife-neural — contract

**Purpose.** FL-007 (`flife-bzl`) covers the sensor/action ABI (plan §6.1, §6.3, §7.4, A.1, Appendix C.10, C.11):
- `energy-moment-gradient-104-v1` sensing with `fixed-local-scales-v1` normalization. A raw sample from the
  `positive-box-v1` density pyramid, blended `log-linear-v1` at the organism's sensing scale, becomes the
  controller's 104 inputs in their fixed order, plus the unsaturated physical gradients that motion follows.
- Decoding of the 36 raw outputs of a linear final layer into a reproduction attempt probability, a child energy
  fraction and an emission proposal.

FL-008 (`flife-6fh`) covers the scalar reference controller between the two (plan §6.2, §6.5, §10.1, §10.3, C.11,
C.16): `independent-dense-mlp-v1` with `tanh` hidden layers, a `linear` output layer and `f32` accumulation in a
declared order. It is the known-answer oracle that accelerated routes are compared against.

FL-054 (`flife-xk0`) adds the gated recurrent controller family `independent-recurrent-mlp-v1` (plan §17.5, §25.2):
a bounded per-organism internal state `h ∈ [-1, 1]^S` (`1 ≤ S ≤ MAXIMUM_RECURRENT_STATE_WIDTH = 16`) appended to the
104 sensor inputs, with effective topology `(104 + S) → hidden… → (36 + 2S)`. Output channels `0..36` stay the linear
action logits; `36..36+S` feed candidate `c_k = tanh_f32(z_c,k)` and `36+S..36+2S` feed update gate
`g_k = sigmoid(f64::from(z_g,k)) as f32` via host-independent `flife_core::numerics`, producing the bounded convex
state transition `h'_k = ((1 − g_k) h_k + g_k c_k).clamp(-1, 1)` in F32. Every recurrent input, candidate, and gate
parameter is part of `DenseTopology::parameter_count()`, so maintenance and genome quotas charge state transitions
automatically. `ReferenceController::evaluate` refuses recurrent topologies (`LayoutMismatch`) and
`evaluate_recurrent` / `trace_recurrent` require matching bounded state slices.

The pyramid itself, its storage and its sampling are `flife_field::pyramid`; this crate encodes, evaluates and
decodes. The FrankenTorch population adapter is FL-025 and the Metal kernels are FL-030. Genomes, their storage and
initialization are FL-009. Applying decoded actions (emission, reproduction) is FL-011.

**Inputs.**
- A `flife_field::RawSample` (33-plane single-carrier) or `flife_field::DualRawSample` (`V2-15` 34-plane
  dual-carrier): the blended raw densities `R` (`R_0`, `R_1`), moments (`M_0..M_31` or `M^(0)_0..23` + `M^(1)_0..7`)
  and their physical derivatives along x and y, per world-length, taken from a `PyramidView` built from this tick's
  observation snapshot.
- An organism's `Intrinsic` state: internal energy, age in simulated seconds, velocity, and the committed energy
  change rate of the last tick.
- `SensorScales` (`[sensors]`): `R_scale`, `L_gradient`, `R_floor`, `E_scale`, the age, velocity and
  energy-change scales, and the admitted sensing-scale range.
- 36 raw controller outputs, the `TimeStep`, and `ActionDecoding` (`[reproduction]`, `[emission]`).
- For the controller: a `DenseTopology` (`[controller]`, or hidden widths directly), one organism's own
  parameters in the declared layout as `Weights` (F32, or binary16 under `f16`, FL-030), and
  `maximum_absolute_parameter`. A binary16 parameter is widened exactly to F32 where the arithmetic reads it, so
  the evaluation below is the same for both dtypes.

**Outputs.**
- `observe` returns an `Observation`:
  - `inputs[0..33]`: `q = R/(R + R_scale)`, then `Z_k = M_k/(R + R_floor)`;
  - `inputs[33..66]` and `inputs[66..99]`: `tanh(L_gradient ∂q)` and `tanh(L_gradient ∂Z_k)` along x, then along
    y, in the same order;
  - `inputs[99..104]`: `E/(E + E_scale)`, `age/(age + age_scale)`, `tanh(v_x/v_scale)`, `tanh(v_y/v_scale)`
    and `tanh(ΔE/change_scale)`;
  - `gradients`: the unsaturated `[∂/∂x, ∂/∂y]` of `q` and each `Z_k`, with `∂q = R_scale ∂R/(R + R_scale)²`
    and `∂Z = (∂M − Z ∂R)/(R + R_floor)`. These are what motion uses (§5.4).
- `observe_dual` / `observe_dual_for_controller` (`V2-15` `dual-carrier-trails`) return a `DualObservation`:
  - `base`: an `Observation` whose `inputs[0]` / `gradients[0]` come from Carrier 0 `R_0`, whose `Z_{0..23}` and
    `gradients[1..25]` normalize Carrier-0 moments by `R_0 + R_floor`, and whose `Z_{24..31}` and `gradients[25..33]`
    normalize Carrier-1 trail moments independently by `R_1 + R_floor` (decoupled from `R_0`);
  - `trail_inputs[4]`: `[q_1, tanh(L_gradient ∂_x q_1), tanh(L_gradient ∂_y q_1), ||Z^(1)||_2.min(1.0)]` where
    `q_1 = R_1 / (R_1 + R_scale_1)` and `∂q_1 = R_scale_1 ∂R_1 / (R_1 + R_scale_1)²`, appended to the 4
    `SensorLayout::trail_slots()` slots (`V2-15` / Bundle B 108 inputs).
- `decode` / `decode_slice` return `DecodedActions` for the baseline 36 outputs (`motor`, `budding`, `emt_gate`,
  `wake_pump` all `None`):
  - `attempt_rate = λ_max σ(o_0)` and `attempt_probability = 1 − e^(−λΔt)`;
  - `child_fraction = β_min + (β_max − β_min) σ(o_1)`, clamped to `[β_min, β_max]` as computed;
  - `emission_gate = σ(o_2)`;
  - `emission`: a unit direction from `o_3..o_34` and `spend_fraction = 1 − e^(−r_max σ(o_35) Δt)`, present only
    when the gate is open and the direction is valid.
- `decode_with_layout` additionally decodes every active `field-ecology-v2` action slot declared by `ActionLayout`:
  - `motor` (`ActionLayout::motor_slots()`, `V2-05` `so2-motor`): `MotorSteering` decoded via host-independent
    `flife_core::numerics::tanh` (`m_e, m_c, m_perp, m_prop ∈ [-1, 1]`);
  - `budding` (`ActionLayout::budding_slots()`, `V2-06` `so2-budding`): `BuddingSteering` decoded from
    `[tanh(o_rad), tanh(o_tan)]` and radially clamped to exact left-fold unit-disk norm `<= 1.0` via
    `flife_core::numerics::shrink_to_left_fold_norm_limit`;
  - `emt_gate` (`ActionLayout::emt_slot()`, `V2-08`): `σ(o_adh) ∈ [0, 1]`;
  - `wake_pump` (`ActionLayout::pump_slot()`, `V2-04`): `tanh(o_pump) ∈ [-1, 1]`.
- Index constants for every ABI slot (`VALUES`, `X_DERIVATIVES`, …, `EMISSION_SPEND`), with `SENSOR_WIDTH = 104`,
  `ACTION_WIDTH = 36`, and V2-01 variable-width ceilings `MAX_SENSOR_WIDTH = 128`, `MAX_ACTION_WIDTH = 48`, and
  `MAX_CONTROLLER_OUTPUT_WIDTH = 80`.
- V2-01 pure scientific layouts `SensorLayout` and `ActionLayout` (`from_config`, `from_v2_abi`, `new`, `v1`,
  `v1_recurrent`): identical across Reference CPU, FrankenTorch CPU, and Metal, with zero silent Metal-only dead lanes.
- `DenseTopology`: generalized `(input_width <= 128, raw_output_width <= 80)` via `with_io`, `with_io_recurrent`, and
  `from_layouts` alongside `new` and `new_recurrent`; hidden widths, `parameter_count` (`P`, weights plus biases, the
  figure maintenance charges), and each layer's `LayerShape`.
- `RawOutputs` (inline `Copy` up to 80 F32s), shared `update_recurrent_state(state, raw_tail, gate_bias, next_state)`
  (`gate_k = sigmoid(f64::from(raw_tail[S + k]) - gate_bias) as f32` with `gate_bias = 0.0` passing `z_g` unmodified
  for v1 bit-exactness and `b_epi = 2.5` closing the gate to `σ(-2.5) ≈ 0.076`), and pure `rotate_epigenetic_affinity`
  (`a_eff(h_0) = cos((π/2) h_0) a + sin((π/2) h_0) J a` via `normalize_turns` + `sin_cos_turns` + `symplectic_rotate_channels`
  + `shrink_to_left_fold_norm_limit`) so Reference, FrankenTorch CPU, and Metal F16/F32 share one bit-exact recurrent
  transition and clonal caste rotation.
- `ShardGeometry` (Trap A defense-in-depth): `new_for_dtype` and `fitting_for_dtype` pad F16 `slot_stride_elements`
  to `parameters.next_multiple_of(4)`, guaranteeing 0 F16 Metal alignment deferrals even for odd-output topologies.
- `ReferenceController::evaluate` (baseline 104/36), `evaluate_raw` (dynamic up to 128/80), `evaluate_recurrent`,
  `evaluate_recurrent_slice`, `trace`, `trace_raw`, and `trace_recurrent`. `ControllerScratch` is reusable activation
  storage.
- `ReferenceController::trace` (FL-021, plan §14.2's sampled activations) is `evaluate` keeping each hidden layer's
  activations: a `Trace { hidden, outputs }` whose `outputs` are `evaluate`'s bit for bit and whose `hidden` are the
  F32 values the next layer read, one vector per hidden layer in evaluation order. It refuses what `evaluate`
  refuses, and `AllocationFailed` when the copies cannot be stored. `flife-engine`'s `tests/watch.rs` checks its
  activations against the declared arithmetic written out over real genomes.
- FL-043: `ControllerBackend::resident_bytes`, required, the host and device bytes a backend holds now;
  `BackendBudget { bytes, organisms }` with `UNBOUNDED`, what a backend may hold and for how many organisms; and
  `ControllerScratch::allocated_bytes`.

**Invariants.**
- Normalization comes after blending. The pyramid stores raw densities only. A coarse composition is a ratio of
  averaged densities, never an average of per-cell ratios.
- Every normalizer is a fixed local constant. No population statistic, batch statistic or running estimate
  reaches a controller (A.1).
- A uniform density field gives the same observation at every admitted scale and position, with every gradient
  exactly zero (C.10). The pyramid's lerp form makes that exact rather than approximately true.
- Gradient inputs are saturated; motion's gradients are not. Raw derivatives are per world-length and use each
  level's own physical spacing, so anisotropic cells are handled.
- Sigmoid is applied exactly once, to the linear outputs, with the sign-stable branch. A `tanh` applied first
  would confine every gate to about `[0.269, 0.731]` (C.11).
- Every rate-to-fraction conversion uses `expm1_neg`, so a vanishing rate gives `λΔt` to full relative precision
  and a zero rate gives exactly zero. Two half ticks compose to one whole tick.
- `no-emission-no-charge`: a gate at or below `θ_emit`, or a direction whose norm is at or below the zero
  threshold, emits nothing. The decoder returns fractions, never amounts. A held output is decoded again each tick
  against that tick's budget.
- Decoding happens before any state changes (§6.3).
- Controller arithmetic, the reference F32 profile:
  - each F64 input is rounded once to F32;
  - neuron `o` accumulates `((b_o + w_o,0 x_0) + w_o,1 x_1) + …` in F32: the bias first, then the inputs in
    ascending index, every product and sum rounded, no fused multiply-add;
  - a hidden activation is `flife_core::numerics::tanh` of the F32 pre-activation, in F64, rounded once to F32,
    computed as `flife_core::numerics::tanh_f32`, which gives those bits for every F32 argument;
  - the output layer is linear, so its `z` is the raw output.
- Parameter layout: layers in evaluation order, each `weight[out][in]` row-major and then `bias[out]` (torch's
  `parameters_to_vector` order). A neuron is named by layer and index, not by packed offset (A.3 item 5).
- Every parameter is finite and within `±maximum_absolute_parameter`, checked when the controller is built. ABI
  inputs are saturated or regularized ratios of magnitude about one, so every affine calculation stays far from
  F32 overflow; an input large enough to overflow anyway is refused.
- Independent weights (§6.5, INV-13). A controller is a view over one organism's own parameters; organisms of a
  topology share only its shape. Scratch carries nothing from one evaluation to the next, whatever topology used
  it before.

**Refusal cases.**
- A negative or nonfinite sensed density, a negative or nonfinite internal energy or age, or any nonfinite input
  after encoding (`NumericalInvalid`).
- Nonpositive or nonfinite sensor normalizers, or a sensing-scale range out of order.
- A sensing scale outside the pyramid's `[s_0, s_0 2^L]`, a nonfinite position, a level past the pyramid, a
  pyramid built for another grid (`LayoutMismatch`), and storage above the buffer limit (`BudgetExhausted`).
- A nonfinite controller output, or a raw emission direction whose norm overflows.
- Decoding constants outside `λ_max ≥ 0`, `0 < β_min ≤ β_max < 1`, `θ_emit ∈ [0, 1)`, `r_max ≥ 0`, or a negative
  zero threshold.
- A configuration naming another sensor schema, normalization, pyramid filter, scale blend or zero-direction
  policy (`UnknownAlgorithm`).
- Controller: no hidden layer or more than two, a zero width, a parameter count past `usize` (`Overflow`), a
  parameter slice of the wrong length (`LayoutMismatch`), a nonfinite or out-of-bound parameter, a nonpositive or
  nonfinite bound, an input that is not finite in F32, and a pre-activation that overflows. Overflow is refused,
  never saturated. Another controller family, activation, weight dtype or accumulation dtype is `UnknownAlgorithm`.

**Determinism.** Pure functions of their inputs, in fixed channel order. The only elementary functions are
`flife_core::numerics` (`tanh`, `sigmoid`, `expm1_neg`, `log2`) and the correctly rounded `sqrt`, so every
IEEE-754 host computes the same bits. The pyramid's coarse levels are F64 and each is a fixed-order 2×2 average.
The controller uses F32 `+` and `×` in the declared order; Rust never contracts them into a fused multiply-add.

**Independent tests.** `tests/sensing.rs`:
- C.10: a uniform field on a 32² grid with four levels gives one identical observation at eight scales (blended
  and not) and four positions, including the torus edge, with all gradients zero. Averaging integrated energy
  instead of density would give `0.5` where the density is `2`.
- The pyramid against a slow direct oracle on an anisotropic 32 × 16 grid over a 48 × 40 world: block averages
  computed from the base cells at every level, and a tent-function interpolant with right-hand derivatives at
  knots, cell edges and random points. Scale blending is checked against `std`'s `log₂`.
- Knots take the interval to the right. A geometric cell edge is interior. A scale that is an exact power of two
  uses its level alone, and `√2 · 2 s_0` blends with `t = ½`.
- The ABI order and every formula on an anisotropic ramp field (`h_x = 2`, `h_y = 0.5`), the quotient rule in a
  second algebraic form, and the raw derivatives against central differences.
- A coarse composition of `0.5/(5 + R_floor)` where the average of per-cell ratios is about `0.278`.
- Translation by a multiple of the coarsest block preserves observations bit for bit. A one-cell shift preserves
  level 0 but resamples coarse levels.
- Refusals: grids, level counts, buffer limits, layouts, scales, positions, densities and intrinsic state. An
  empty field is neutral. Appendix A needs four levels for scales 1 to 16.
- C.11: `σ(8) ≈ 0.99966465` against `σ(tanh 8) ≈ 0.731`, child shares across the whole `[0.1, 0.9]`, and rates that
  are `σ` of the raw output exactly once.
- C.11: a hazard below `1e-17` per second decodes to `λΔt` within `1e-15` relative, where F32 `1 − exp` is zero.
  Exact zero limits, saturation, and half-tick composition for attempt and spend.
- Emission: the gate threshold (`σ(0) = ½` is not open at `θ = ½`), a `(0.6, 0.8)` unit direction, zero and
  sub-threshold directions emitting nothing with the gate open, and overflow refused.
- Decoding constants checked one by one.

`tests/controller.rs`. The expected values do not come from the evaluator. The hand-set networks have dyadic
weights, so every pre-activation is exact. Their `tanh` values were computed to 60 digits and rounded directly to
F32, and each lies at least 0.10 F32 ulp from a rounding midpoint.
- C.11: zero weights with nonzero hidden biases, on a real observation, pass the output biases through bit for bit.
  Within Appendix A's `|p| ≤ 4` that is `σ(4) = 0.98201379`, against `tanh(4) ≈ 0.99932930` for a `tanh` head. The
  literal logit 8 is outside that bound, so it runs under a declared fixture bound of 8 and decodes to
  `0.99966465`. A logit of 8 is still reachable within the bound: a hidden unit saturated at exactly 1, through
  weight 4, plus bias 4.
- A hand-set `104 → 2 → 36` network: all 36 outputs pinned bit for bit.
- C.16: `tanh(1) = 0x3f42f7d6` (`≈ 0.76159416`) through one hidden layer, and `tanh(tanh(1)) = 0x3f245b18`
  (`≈ 0.64201499`) through an identity bridge.
- The accumulation order: with `b = 2^-22` and products `±4`, the declared order gives `0` for `(4, −4)` and
  `2^-22` for `(−4, 4)`. Products first, descending order and F64 accumulation each give a different answer.
  Inputs are rounded to F32 once.
- The layout of `104 → 3 → 2 → 36` by hand (431 parameters), with edits at named indices reaching exactly the
  outputs they name, including through the middle layer.
- An input-major F64 oracle with `std`'s `tanh` over four topologies and 96 evaluations, under a rigorous running
  error bound (`γ_n Σ|w x|` per dot product, plus propagated and rounding error). The largest observed error is
  5.3e-7, or 0.5% of its bound.
- FL-021: `trace` over `104 → 8 → 36`, the default and the widest network, each from a scratch the widest net used
  last: its outputs are `evaluate`'s bit for bit, and each hidden layer, at its width, is the declared arithmetic
  written out over the layer shapes, bit for bit.
- INV-13: two genomes of one topology answer differently, and a shared scratch alternating between narrow, wide
  and default topologies gives the same bits as fresh scratch.
- §6.4's parameter counts (1,164; 4,548; 9,988; 18,628; 29,316) and Appendix A's default layer offsets.
- Refusals, including the first F32 value above the bound and a finite input that overflows its pre-activation.
- A regression pin: the digest of 64 evaluations of a seeded default network. The same constant is checked on
  macOS arm64 and on Linux x86_64.

**Validity domain.** `energy-moment-gradient-104-v1` with `fixed-local-scales-v1`, `positive-box-v1`,
`log-linear-v1` and `no-emission-no-charge`, for the 32-channel field of `field-ecology-v1`. The controller covers
`independent-dense-mlp-v1` with one or two hidden layers, `tanh`, `linear` and `f32` accumulation over F32 or
binary16 weights: the reference F32 and compact F16 profiles of §10.3.

**Known gaps.**
- Mutation of the heritable sensing scale (A.3 item 4) is FL-010's, so that registry entry stays `Specified`
  under FL-010. This crate samples at any admitted scale.
- Charging for emission, reproduction attempts and their budget order are FL-011.
- The reference controller is scalar and single-threaded: 9,856 multiply-adds per default evaluation, written for
  a declared order rather than for speed. Batched population inference goes through `backend::ControllerBackend`
  (FL-030), whose implementations must return this evaluator's outputs bit for bit and defer the rest to it. The
  Metal one (`flife-backend-metal`) does: in the gateway's IEEE-safe mode, with contraction off and a software
  binary64 behind its fast `tanh`, it matches this evaluator exactly. FL-025's FrankenTorch CPU route is still to
  come.
- Measured and not adopted (2026-11-09): the hidden `tanh` is the largest single item in a steady-state CPU profile
  (`expm1_neg` under `tanh` under `evaluate`), about 9.5 ns a call. Replacing its series divisions by `k` with
  multiplications by `1/k` and guarding the F32 rounding is exact after rounding for every F32 (checked
  exhaustively over the 1.1e9 positive inputs below 20), but measured 9.6 ns a call and no faster per tick
  (interleaved A/B, 1 and 16 threads, same digest): on this core F64 division pipelines as well as the
  multiplications do. A faster activation needs fewer operations, not cheaper ones.
- Genome storage, the `f32` weight-dtype entry (canonical rounding and checkpoint identity), the
  `fan-in-scaled-symmetric-v1` initializer and the topology catalog are FL-009. The tests draw A.3 item 2's
  distribution from a local generator, not from FL-010's Philox addressing.
- Held raw outputs and the evaluation cadence (§6.6) belong to the ordered tick, FL-013.
- The reference pyramid is F64 and rebuilt in full each tick. A Metal pyramid (FL-032) and an F32 pyramid are
  separate numerical profiles with their own tolerances.
- Bilinear interpolation is `C^0`. A gradient-following organism sees a jump in its derivative at each knot. That
  is the specified interpolant, but it is a modelling choice motion tests should keep in mind.
