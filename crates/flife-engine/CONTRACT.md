# flife-engine — contract

**Purpose.** FL-006 (`flife-h6c`) covers the organism phase's reference operators (plan §5.3, §5.4, §5.6, §8.2,
Appendix C.2, C.3, C.9, C.11, C.12):
- `bounded-affinity-v1` chemistry evaluated `per-granted-cell-packet`, with toxicity clipped to what the
  organism holds;
- the mandatory maintenance of one tick;
- field-mediated motion: drag integrated with a stable `expm1`, speed and acceleration limits, affordability
  scaling of the whole velocity, and the wrapped position step.

The ordered tick that calls them is FL-013. This crate computes one organism's settlement; it does not own
organisms or the field.

**Inputs.**
- A `ValidatedConfig` (`[chemistry]`, `[costs]`, the timestep), or the constants directly.
- An organism's `Affinity` and `MotionResponse`, each within its run's norm limit.
- The granted cell packets of one tick: FL-005's `GrantPacket` amounts and each cell's composition, read from the
  observation snapshot with `composition` before the field is debited.
- The organism's `flife_core::EnergyBudget`.
- The organism's normalized sensor gradients (33 two-vectors: energy, then 32 channels). Sensing is FL-007; the
  tests supply fixed gradients.

**Outputs.**
- `Chemistry::efficiency`: `η(z) = η_max tanh(k a·z − b)`.
- `Chemistry::settle` and `settle_efficiencies`: an `UptakeSettlement` with absorbed `U`, useful gain `G`, and a
  toxicity `Payment` holding `T_req` and the paid `T`. `net() = G − T` and `dissipated() = U − G + T`.
  `record` enters an uptake transfer of `U` crediting `G` (its loss charged to `uptake-inefficiency`) and the
  paid toxicity.
- `MaintenanceCosts::mandatory`: `Δt (c_basal + c_sensor s / s_unit) + c_neural P × evaluated`.
- `Motion`: `decay = e^(−γΔt)`, `force_coefficient = (1 − e^(−γΔt))/γ` (or `Δt` at `γ = 0`), `acceleration`,
  `propose`, `settle` (a `MotionSettlement` with the proposal, the applied velocity, the cost `Payment` and
  whether it was scaled) and `displace`.
- `uptake_ratio`: `g_u = U / (Δt U_max)`, clamped to `[0, 1]`, zero at zero capacity.

**Invariants.**
- Chemistry runs on each granted packet, never on the request or on the mixture of packets (C.9).
- The gain is credited before toxicity is paid, so `T = min(T_req, E + G)` (§8.2, C.2). The ledger records the
  paid toxicity, never the requested one.
- An empty cell, or one whose moments cancel, has composition `z = 0`. With `b > 0` that is mildly harmful,
  and it is not special-cased (C.3).
- Maintenance comes from canonical model work only: sensing scale, parameter count and whether the controller
  ran. It never depends on wall-clock time, cache behavior, kernel padding or the backend.
- `||a||₂ ≤ affinity_norm_limit` and `||χ||₂ ≤ motion_response_norm_limit`, both checked in F64.
- `||f||² ≤ a_max²` and `||v*||² ≤ v_max²` hold as computed in F64. A vector that rounding leaves just outside is
  shrunk one ulp at a time.
- Motion is affordable. An unaffordable proposal is scaled as a whole by `sqrt(E_available / cost)`, and all
  available energy is paid, so the organism is exhausted. An exhausted organism still moves by what it paid
  for (C.12). Later stages pay nothing, and credits are refused.
- The force coefficient uses `expm1_neg`. At `γ = 1e-6`, `Δt = 1/128` it is `0.00781249996948`, where an F32
  `1 − exp(−γΔt)` is exactly zero (C.11). `γ = 0` gives exactly `Δt`, and so does a vanishing `γ`.
- An organism's outcome does not depend on the order in which organisms are presented. The integrated fixture's
  organisms and field agree bit for bit under three orders.

**Refusal cases.**
- Chemistry constants outside `η_max ∈ (0, 1]`, `k ≥ 0`, `b > 0`, `κ > 0`, limit `∈ (0, 1]`, or nonfinite.
- An affinity or motion response above its norm limit, or with a nonfinite component.
- A negative or nonfinite grant, an efficiency outside `[−1, 1]`, or a nonfinite composition.
- A gain offered to an exhausted budget (`StateConflict`).
- Negative or nonfinite costs, a nonpositive sensor unit scale, or a nonpositive sensing scale.
- A sensing-scale cost exponent other than `1`: another exponent needs a host-independent power function, which
  the reference profile does not have (`MissingCapability`).
- Negative drag, nonpositive speed, acceleration or response limits, or a negative motion cost.
- An uptake ratio outside `[0, 1]`, a nonfinite gradient, velocity or acceleration.
- A configuration naming another chemistry family or evaluation (`UnknownAlgorithm`).

**Determinism.** Pure functions of their inputs. Sums run in channel or packet order and the packet sums are
compensated. The only elementary functions are `flife_core::numerics` (`exp_neg`, `expm1_neg`, `tanh`) and the
correctly rounded `sqrt`, so every IEEE-754 host computes the same bits. The integrated fixture pins a digest of
its final organisms and field.

**Independent tests.** `tests/organism_phase.rs`:
- C.2 exactly: absorbed 3, `T_req = 5`, `T = 2`, dissipation 5 closing at `ε = 0`; recording `T_req` cannot close.
  Gain bounding toxicity at `E + G`.
- C.9 to 12 digits: `+0.639355310069` per packet and `−0.047643210759` on the mixture, through both the stack
  and the heap packet paths.
- C.3: `z = −0.25a` for `(8, −2a)`, the neutral composition for cancelled and empty cells, and its mild harm.
- C.11: the drag coefficient, the F32 cancellation, the exact zero-rate limit, monotonicity in `γ`, and Appendix
  A's `γ = 2`.
- C.12: the last unit spent on motion, the step taken, one dissipated unit, and every later stage refused.
- Affordability: an opposing force cannot rescue an unaffordable residual velocity; the whole velocity scales.
- Both radial bounds over 20,000 random cases, and the response and affinity norm refusals.
- Maintenance at Appendix A: `0.008556928` per simulated second at unit sensing scale with 9,988 parameters.
- §8.2 ordering: gain before maintenance, unpaid maintenance not dissipated, and no motion after death.
- An integrated simulated second: five organisms on a 16² field with contended cells, real FL-005 grants,
  per-packet chemistry, maintenance and motion. The ledger closes every tick (cumulative residual about 1e-14,
  6.6e-16 of turnover at 48 ticks). The opposed organism dies at tick 29 and never absorbs again.

**Validity domain.** `field-ecology-v1` with `bounded-affinity-v1` and `per-granted-cell-packet`, at sensing-scale
cost exponent 1.

**Known gaps.**
- Maintenance is recorded as one `maintenance` flow. Splitting it into the ledger's `controller-cost` and
  `sensor-cost` flows, and the order in which a partial payment covers them, is FL-012's with
  `dissipate-and-record`.
- Mutation of the affinity and motion-response traits (A.3 item 4) is FL-010's. Until then those two registry
  entries stay `Specified` under FL-010.
- Emission, reproduction and their near-zero rate tests (C.11) are FL-011. Death collection and tiny-reservoir
  clearing are FL-012. The ordered tick and publication are FL-013.
- Sensor gradients come from FL-007.
- A Metal implementation is FL-032; its parity with this reference is a separate numerical profile.
