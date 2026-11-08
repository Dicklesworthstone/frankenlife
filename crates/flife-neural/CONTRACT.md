# flife-neural — contract

**Purpose.** FL-007 (`flife-bzl`) covers the sensor/action ABI (plan §6.1, §6.3, §7.4, A.1, Appendix C.10, C.11):
- `energy-moment-gradient-104-v1` sensing with `fixed-local-scales-v1` normalization. A raw sample from the
  `positive-box-v1` density pyramid, blended `log-linear-v1` at the organism's sensing scale, becomes the
  controller's 104 inputs in their fixed order, plus the unsaturated physical gradients that motion follows.
- Decoding of the 36 raw outputs of a linear final layer into a reproduction attempt probability, a child energy
  fraction and an emission proposal.

The pyramid itself, its storage and its sampling are `flife_field::pyramid`; this crate encodes and decodes. The
scalar reference controller and the FrankenTorch population adapter are FL-008. Applying decoded actions
(emission, reproduction) is FL-011.

**Inputs.**
- A `flife_field::RawSample`: the blended raw densities `R`, `M_0..M_31` and their physical derivatives along x
  and y, per world-length, taken from a `PyramidView` built from this tick's observation snapshot.
- An organism's `Intrinsic` state: internal energy, age in simulated seconds, velocity, and the committed energy
  change rate of the last tick.
- `SensorScales` (`[sensors]`): `R_scale`, `L_gradient`, `R_floor`, `E_scale`, the age, velocity and
  energy-change scales, and the admitted sensing-scale range.
- 36 raw controller outputs, the `TimeStep`, and `ActionDecoding` (`[reproduction]`, `[emission]`).

**Outputs.**
- `observe` returns an `Observation`:
  - `inputs[0..33]`: `q = R/(R + R_scale)`, then `Z_k = M_k/(R + R_floor)`;
  - `inputs[33..66]` and `inputs[66..99]`: `tanh(L_gradient ∂q)` and `tanh(L_gradient ∂Z_k)` along x, then along
    y, in the same order;
  - `inputs[99..104]`: `E/(E + E_scale)`, `age/(age + age_scale)`, `tanh(v_x/v_scale)`, `tanh(v_y/v_scale)`
    and `tanh(ΔE/change_scale)`;
  - `gradients`: the unsaturated `[∂/∂x, ∂/∂y]` of `q` and each `Z_k`, with `∂q = R_scale ∂R/(R + R_scale)²`
    and `∂Z = (∂M − Z ∂R)/(R + R_floor)`. These are what motion uses (§5.4).
- `decode` returns `DecodedActions`:
  - `attempt_rate = λ_max σ(o_0)` and `attempt_probability = 1 − e^(−λΔt)`;
  - `child_fraction = β_min + (β_max − β_min) σ(o_1)`, clamped to `[β_min, β_max]` as computed;
  - `emission_gate = σ(o_2)`;
  - `emission`: a unit direction from `o_3..o_34` and `spend_fraction = 1 − e^(−r_max σ(o_35) Δt)`, present only
    when the gate is open and the direction is valid.
- Index constants for every ABI slot (`VALUES`, `X_DERIVATIVES`, …, `EMISSION_SPEND`), with `SENSOR_WIDTH = 104`
  and `ACTION_WIDTH = 36`.

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

**Determinism.** Pure functions of their inputs, in fixed channel order. The only elementary functions are
`flife_core::numerics` (`tanh`, `sigmoid`, `expm1_neg`, `log2`) and the correctly rounded `sqrt`, so every
IEEE-754 host computes the same bits. The pyramid's coarse levels are F64 and each is a fixed-order 2×2 average.

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

**Validity domain.** `energy-moment-gradient-104-v1` with `fixed-local-scales-v1`, `positive-box-v1`,
`log-linear-v1` and `no-emission-no-charge`, for the 32-channel field of `field-ecology-v1`.

**Known gaps.**
- Mutation of the heritable sensing scale (A.3 item 4) is FL-010's, so that registry entry stays `Specified`
  under FL-010. This crate samples at any admitted scale.
- The controller that produces the 36 outputs from the 104 inputs is FL-008. Charging for emission, reproduction
  attempts and their budget order are FL-011.
- The reference pyramid is F64 and rebuilt in full each tick. A Metal pyramid (FL-032) and an F32 pyramid are
  separate numerical profiles with their own tolerances.
- Bilinear interpolation is `C^0`. A gradient-following organism sees a jump in its derivative at each knot. That
  is the specified interpolant, but it is a modelling choice motion tests should keep in mind.
