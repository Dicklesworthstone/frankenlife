#!/usr/bin/env python3
"""V2-17 Stage-2 diagnostic evaluator (Python stdlib only; zero mocks).

Consumes real `flife inspect --field` (`frankenlife.inspect.v1`) NDJSON dumps
and optional `flife run --metrics-every` (`frankenlife.run.v1` metrics records)
and emits one structured `frankenlife.v2-stage2-diagnostics.v1` JSON object.

Computed Stage-2 diagnostics:
  1. Clonal differentiation (V2-12): within-lineage h_0 variance, between-
     lineage h_0 variance, and Pearson corr(h_0, c_den) on the periodic torus.
  2. Murray vascular bimodality (V2-14): high-decile (>= P90) vs mid-band
     ([P25, P75]) conductance ratio, P90/P50, P95/P50, and finite-sample
     Sarle bimodality coefficient from `field.vascular_conductance`.
  3. Trail-carrier SNR vs Carrier-0 (V2-15): Carrier-1 (`field.trail_energy`)
     ridge SNR ((P95 - median) / std, P95 / median, top-decile share) vs
     Carrier-0 (`field.energy`) SNR plus spatial decoupling `1 - |corr|`.
  4. Trophallaxis flux (V2-13): total credited `trophallaxis` energy transfer,
     `trophallaxis-loss` dissipation, per-tick rate, and per-organism rate from
     `flife run --metrics-every` records.
  5. Flock polarization (V2-13): global Vicsek order parameter P, local
     neighborhood Vicsek polarization, local pairwise cosine alignment, and
     kin-restricted local alignment on the periodic torus.

Honesty contract (AGENTS.md):
  Every metric reports `"available": false` and `null` values whenever its
  underlying field plane, recurrent state, or metrics stream is absent. Nothing
  is fabricated, imputed, or substituted with a target value.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

DIAGNOSTICS_SCHEMA = "frankenlife.v2-stage2-diagnostics.v1"
DEFAULT_CROWDING_RADIUS = 0.375
DEFAULT_ALIGN_RADIUS = 0.375
DEFAULT_SPEED_EPS = 1.0e-9


def min_image(delta: float, span: float) -> float:
    """Periodic minimum-image displacement on [0, span)."""
    half = 0.5 * span
    if delta > half:
        return delta - span
    if delta < -half:
        return delta + span
    return delta


def linear_quantile(sorted_vals: list[float], q: float) -> float:
    """Linearly interpolated quantile (NumPy/Hyndman-Fan type 7) on sorted data."""
    n = len(sorted_vals)
    if n == 0:
        raise ValueError("linear_quantile requires at least one value")
    if q <= 0.0:
        return sorted_vals[0]
    if q >= 1.0:
        return sorted_vals[-1]
    pos = q * (n - 1)
    lo = int(math.floor(pos))
    hi = min(lo + 1, n - 1)
    frac = pos - lo
    return sorted_vals[lo] * (1.0 - frac) + sorted_vals[hi] * frac


def pearson_corr(xs: list[float], ys: list[float]) -> float | None:
    """Exact two-pass Pearson correlation, returning None when undefined."""
    n = len(xs)
    if n < 2 or n != len(ys):
        return None
    mx = sum(xs) / n
    my = sum(ys) / n
    sxx = 0.0
    syy = 0.0
    sxy = 0.0
    for x, y in zip(xs, ys):
        dx = x - mx
        dy = y - my
        sxx += dx * dx
        syy += dy * dy
        sxy += dx * dy
    denom = math.sqrt(sxx * syy)
    if not math.isfinite(denom) or denom <= 0.0:
        return None
    r = sxy / denom
    if r > 1.0:
        r = 1.0
    elif r < -1.0:
        r = -1.0
    return r


def _build_periodic_grid(
    positions: list[tuple[float, float]],
    width: float,
    height: float,
    radius: float,
) -> tuple[int, int, float, float, list[list[int]]]:
    """O(N) periodic spatial hash grid with cell width/height >= radius."""
    cell_min = max(radius, 1.0e-6)
    gx = max(1, int(math.floor(width / cell_min)))
    gy = max(1, int(math.floor(height / cell_min)))
    cw = width / gx
    ch = height / gy
    grid: list[list[int]] = [[] for _ in range(gx * gy)]
    for idx, (px, py) in enumerate(positions):
        bx = int(math.floor(px / cw)) % gx
        by = int(math.floor(py / ch)) % gy
        grid[by * gx + bx].append(idx)
    return gx, gy, cw, ch, grid


def _neighbor_offsets(n_cells: int) -> tuple[int, ...]:
    """Distinct cell offsets covering [-1, 0, +1] without duplicate buckets."""
    if n_cells == 1:
        return (0,)
    if n_cells == 2:
        return (0, 1)
    return (-1, 0, 1)


def _extract_h0(org: dict[str, Any]) -> float | None:
    """Extract recurrent cell-state h_0 = held.recurrent_state[0], if present."""
    held = org.get("held")
    if not isinstance(held, dict):
        return None
    state = held.get("recurrent_state")
    if not isinstance(state, list) or not state:
        return None
    val = state[0]
    if not isinstance(val, (int, float)):
        return None
    fval = float(val)
    if not math.isfinite(fval):
        return None
    return fval


def compute_clonal_differentiation(
    organisms: list[dict[str, Any]],
    width: float,
    height: float,
    crowding_radius: float = DEFAULT_CROWDING_RADIUS,
) -> dict[str, Any]:
    """Compute V2-12 clonal h_0 variance and corr(h_0, c_den) on the torus."""
    n_total = len(organisms)
    if width <= 0.0 or height <= 0.0:
        raise ValueError("world width and height must be strictly positive")
    if crowding_radius < 0.0:
        raise ValueError("crowding_radius must be nonnegative")

    h0_values: list[float | None] = [_extract_h0(org) for org in organisms]
    evaluated_indices = [i for i, val in enumerate(h0_values) if val is not None]
    if not evaluated_indices:
        return {
            "available": False,
            "reason": "no_recurrent_state",
            "crowding_radius": crowding_radius,
            "total_count": n_total,
            "evaluated_count": 0,
            "multi_member_lineages": 0,
            "multi_member_organisms": 0,
            "within_variance_weighted": None,
            "within_variance_mean": None,
            "within_variance_bessel_weighted": None,
            "between_lineage_variance": None,
            "total_h0_variance": None,
            "within_to_total_ratio": None,
            "max_lineage_h0_std": None,
            "corr_h0_cden": None,
            "within_lineage_corr_h0_cden": None,
            "mean_cden": None,
        }

    positions = [
        (float(org["position"][0]), float(org["position"][1])) for org in organisms
    ]
    gx, gy, cw, ch, grid = _build_periodic_grid(
        positions, width, height, crowding_radius
    )
    ox_list = _neighbor_offsets(gx)
    oy_list = _neighbor_offsets(gy)
    r_sq = crowding_radius * crowding_radius

    cden: list[int] = [0] * n_total
    for i in evaluated_indices:
        px, py = positions[i]
        bx = int(math.floor(px / cw)) % gx
        by = int(math.floor(py / ch)) % gy
        count = 0
        for oy in oy_list:
            row = ((by + oy) % gy) * gx
            for ox in ox_list:
                for j in grid[row + ((bx + ox) % gx)]:
                    if j == i:
                        continue
                    dx = min_image(positions[j][0] - px, width)
                    dy = min_image(positions[j][1] - py, height)
                    if dx * dx + dy * dy <= r_sq:
                        count += 1
        cden[i] = count

    by_lineage: dict[int, list[tuple[float, float]]] = defaultdict(list)
    all_h0: list[float] = []
    all_cden: list[float] = []
    for i in evaluated_indices:
        h0 = h0_values[i]
        assert h0 is not None
        lin = int(organisms[i]["lineage"])
        cd = float(cden[i])
        by_lineage[lin].append((h0, cd))
        all_h0.append(h0)
        all_cden.append(cd)

    n_eval = len(all_h0)
    grand_mean = sum(all_h0) / n_eval
    ss_total = sum((v - grand_mean) ** 2 for v in all_h0)
    total_var = ss_total / n_eval

    ss_between = 0.0
    ss_within_multi = 0.0
    n_multi_org = 0
    k_multi_lin = 0
    lineage_vars: list[float] = []
    centered_h0: list[float] = []
    centered_cden: list[float] = []

    for members in by_lineage.values():
        k = len(members)
        mean_h = sum(m[0] for m in members) / k
        ss_between += k * (mean_h - grand_mean) ** 2
        if k >= 2:
            k_multi_lin += 1
            n_multi_org += k
            mean_c = sum(m[1] for m in members) / k
            ss_l = 0.0
            for h_val, c_val in members:
                dh = h_val - mean_h
                ss_l += dh * dh
                centered_h0.append(dh)
                centered_cden.append(c_val - mean_c)
            ss_within_multi += ss_l
            lineage_vars.append(ss_l / k)

    between_var = ss_between / n_eval
    if n_multi_org > 0:
        within_weighted: float | None = ss_within_multi / n_multi_org
        within_mean: float | None = sum(lineage_vars) / k_multi_lin
        within_bessel: float | None = ss_within_multi / (n_multi_org - k_multi_lin)
        max_lin_std: float | None = math.sqrt(max(lineage_vars))
    else:
        within_weighted = None
        within_mean = None
        within_bessel = None
        max_lin_std = None

    within_to_total: float | None = (
        (ss_within_multi / ss_total)
        if (n_multi_org > 0 and ss_total > 0.0)
        else None
    )

    return {
        "available": True,
        "reason": None,
        "crowding_radius": crowding_radius,
        "total_count": n_total,
        "evaluated_count": n_eval,
        "multi_member_lineages": k_multi_lin,
        "multi_member_organisms": n_multi_org,
        "within_variance_weighted": within_weighted,
        "within_variance_mean": within_mean,
        "within_variance_bessel_weighted": within_bessel,
        "between_lineage_variance": between_var,
        "total_h0_variance": total_var,
        "within_to_total_ratio": within_to_total,
        "max_lineage_h0_std": max_lin_std,
        "corr_h0_cden": pearson_corr(all_h0, all_cden),
        "within_lineage_corr_h0_cden": pearson_corr(centered_h0, centered_cden),
        "mean_cden": sum(all_cden) / n_eval,
    }


def compute_vascular_bimodality(
    vascular_plane: list[float] | None,
) -> dict[str, Any]:
    """Compute V2-14 Murray vascular high/mid conductance ratio and bimodality."""
    if not vascular_plane:
        return {
            "available": False,
            "reason": "missing_vascular_conductance_plane",
            "cell_count": 0,
            "non_finite_count": 0,
            "min": None,
            "max": None,
            "mean": None,
            "std": None,
            "p25": None,
            "p50": None,
            "p75": None,
            "p90": None,
            "p95": None,
            "p99": None,
            "high_band_mean": None,
            "mid_band_mean": None,
            "high_to_mid_ratio": None,
            "p90_to_p50_ratio": None,
            "p95_to_p50_ratio": None,
            "high_fraction_above_midpoint": None,
            "sarle_bimodality_coefficient": None,
        }

    vals: list[float] = []
    non_finite = 0
    for raw in vascular_plane:
        v = float(raw)
        if math.isfinite(v):
            vals.append(v)
        else:
            non_finite += 1

    if not vals:
        return {
            "available": False,
            "reason": "non_finite_vascular_conductance_plane",
            "cell_count": 0,
            "non_finite_count": non_finite,
            "min": None,
            "max": None,
            "mean": None,
            "std": None,
            "p25": None,
            "p50": None,
            "p75": None,
            "p90": None,
            "p95": None,
            "p99": None,
            "high_band_mean": None,
            "mid_band_mean": None,
            "high_to_mid_ratio": None,
            "p90_to_p50_ratio": None,
            "p95_to_p50_ratio": None,
            "high_fraction_above_midpoint": None,
            "sarle_bimodality_coefficient": None,
        }

    n = len(vals)
    vals.sort()
    vmin = vals[0]
    vmax = vals[-1]
    mean = sum(vals) / n
    m2 = sum((v - mean) ** 2 for v in vals) / n
    std = math.sqrt(m2)

    p25 = linear_quantile(vals, 0.25)
    p50 = linear_quantile(vals, 0.50)
    p75 = linear_quantile(vals, 0.75)
    p90 = linear_quantile(vals, 0.90)
    p95 = linear_quantile(vals, 0.95)
    p99 = linear_quantile(vals, 0.99)

    high_band = [v for v in vals if v >= p90]
    mid_band = [v for v in vals if p25 <= v <= p75]
    high_mean = sum(high_band) / len(high_band) if high_band else vmax
    mid_mean = sum(mid_band) / len(mid_band) if mid_band else p50

    high_to_mid = (high_mean / mid_mean) if mid_mean > 0.0 else None
    p90_to_p50 = (p90 / p50) if p50 > 0.0 else None
    p95_to_p50 = (p95 / p50) if p50 > 0.0 else None

    if vmax > vmin:
        midpoint = 0.5 * (vmin + vmax)
        high_frac = sum(1 for v in vals if v >= midpoint) / n
    else:
        high_frac = 0.0

    # Finite-sample Sarle's bimodality coefficient b = (g1^2 + 1) / (g2 + 3*c)
    # where uniform distribution yields b = 5/9 (~0.5556) and sharply bimodal
    # distributions exceed 5/9.
    sarle_b: float | None = None
    if n >= 4 and std > 0.0:
        m3 = sum((v - mean) ** 3 for v in vals) / n
        m4 = sum((v - mean) ** 4 for v in vals) / n
        g1 = m3 / (std**3)
        g2_excess = (m4 / (m2 * m2)) - 3.0
        correction = 3.0 * ((n - 1) ** 2) / ((n - 2) * (n - 3))
        denom = g2_excess + correction
        if denom > 0.0 and math.isfinite(denom):
            sarle_b = (g1 * g1 + 1.0) / denom

    return {
        "available": True,
        "reason": None,
        "cell_count": n,
        "non_finite_count": non_finite,
        "min": vmin,
        "max": vmax,
        "mean": mean,
        "std": std,
        "p25": p25,
        "p50": p50,
        "p75": p75,
        "p90": p90,
        "p95": p95,
        "p99": p99,
        "high_band_mean": high_mean,
        "mid_band_mean": mid_mean,
        "high_to_mid_ratio": high_to_mid,
        "p90_to_p50_ratio": p90_to_p50,
        "p95_to_p50_ratio": p95_to_p50,
        "high_fraction_above_midpoint": high_frac,
        "sarle_bimodality_coefficient": sarle_b,
    }


def _summarize_scalar_plane(plane: list[float]) -> dict[str, Any]:
    """Compute ridge SNR and concentration statistics for one nonnegative plane."""
    vals = [float(v) for v in plane if math.isfinite(float(v))]
    n = len(vals)
    if n == 0:
        return {
            "cell_count": 0,
            "mean": None,
            "std": None,
            "median": None,
            "iqr": None,
            "p95": None,
            "p99": None,
            "max": None,
            "peak_z_snr": None,
            "max_z_snr": None,
            "p95_to_median_ratio": None,
            "top_decile_share": None,
        }
    sorted_vals = sorted(vals)
    total = sum(sorted_vals)
    mean = total / n
    var = sum((v - mean) ** 2 for v in sorted_vals) / n
    std = math.sqrt(var)
    p25 = linear_quantile(sorted_vals, 0.25)
    median = linear_quantile(sorted_vals, 0.50)
    p75 = linear_quantile(sorted_vals, 0.75)
    p95 = linear_quantile(sorted_vals, 0.95)
    p99 = linear_quantile(sorted_vals, 0.99)
    vmax = sorted_vals[-1]

    peak_z = ((p95 - median) / std) if std > 0.0 else 0.0
    max_z = ((vmax - median) / std) if std > 0.0 else 0.0
    p95_ratio = (p95 / median) if median > 0.0 else None

    top_k = max(1, int(math.ceil(0.10 * n)))
    top_sum = sum(sorted_vals[-top_k:])
    top_decile_share = (top_sum / total) if total > 0.0 else None

    return {
        "cell_count": n,
        "mean": mean,
        "std": std,
        "median": median,
        "iqr": p75 - p25,
        "p95": p95,
        "p99": p99,
        "max": vmax,
        "peak_z_snr": peak_z,
        "max_z_snr": max_z,
        "p95_to_median_ratio": p95_ratio,
        "top_decile_share": top_decile_share,
    }


def compute_trail_snr(
    trail_plane: list[float] | None,
    energy_plane: list[float] | None,
) -> dict[str, Any]:
    """Compute V2-15 Carrier-1 trail SNR vs Carrier-0 energy SNR and decoupling."""
    carrier0_stats = (
        _summarize_scalar_plane(energy_plane) if energy_plane else None
    )
    if not trail_plane:
        return {
            "available": False,
            "reason": "missing_trail_energy_plane",
            "trail": None,
            "carrier0": carrier0_stats,
            "snr_ratio_vs_carrier0": None,
            "max_snr_ratio_vs_carrier0": None,
            "corr_trail_carrier0": None,
            "spatial_decoupling": None,
        }

    trail_stats = _summarize_scalar_plane(trail_plane)
    if trail_stats["cell_count"] == 0:
        return {
            "available": False,
            "reason": "non_finite_trail_energy_plane",
            "trail": None,
            "carrier0": carrier0_stats,
            "snr_ratio_vs_carrier0": None,
            "max_snr_ratio_vs_carrier0": None,
            "corr_trail_carrier0": None,
            "spatial_decoupling": None,
        }

    snr_ratio: float | None = None
    max_snr_ratio: float | None = None
    corr: float | None = None
    decoupling: float | None = None

    if carrier0_stats is not None and carrier0_stats["cell_count"] > 0:
        c0_peak = carrier0_stats["peak_z_snr"]
        c0_max = carrier0_stats["max_z_snr"]
        if c0_peak is not None and c0_peak > 0.0 and trail_stats["peak_z_snr"] is not None:
            snr_ratio = trail_stats["peak_z_snr"] / c0_peak
        if c0_max is not None and c0_max > 0.0 and trail_stats["max_z_snr"] is not None:
            max_snr_ratio = trail_stats["max_z_snr"] / c0_max

    if energy_plane and len(energy_plane) == len(trail_plane):
        paired_t: list[float] = []
        paired_e: list[float] = []
        for t_raw, e_raw in zip(trail_plane, energy_plane):
            tv = float(t_raw)
            ev = float(e_raw)
            if math.isfinite(tv) and math.isfinite(ev):
                paired_t.append(tv)
                paired_e.append(ev)
        corr = pearson_corr(paired_t, paired_e)
        if corr is not None:
            decoupling = 1.0 - abs(corr)

    return {
        "available": True,
        "reason": None,
        "trail": trail_stats,
        "carrier0": carrier0_stats,
        "snr_ratio_vs_carrier0": snr_ratio,
        "max_snr_ratio_vs_carrier0": max_snr_ratio,
        "corr_trail_carrier0": corr,
        "spatial_decoupling": decoupling,
    }


def compute_trophallaxis_flux(
    metrics_records: list[dict[str, Any]] | None,
) -> dict[str, Any]:
    """Extract V2-13 trophallaxis transfer flux from `flife run --metrics-every`."""
    if not metrics_records:
        return {
            "available": False,
            "reason": "no_metrics_records",
            "window_count": 0,
            "key_present": False,
            "nonzero_windows": 0,
            "total_ticks": 0,
            "total_credited": None,
            "total_dissipated": None,
            "total_debited": None,
            "mean_flux_per_tick": None,
            "mean_live": None,
            "flux_per_organism_tick": None,
        }

    window_count = 0
    key_present = False
    nonzero_windows = 0
    total_ticks = 0
    total_credited = 0.0
    total_dissipated = 0.0
    weighted_live = 0.0

    for rec in metrics_records:
        window = rec.get("window")
        if not isinstance(window, dict):
            continue
        transfers = window.get("transfers")
        flows = window.get("flows")
        if not isinstance(transfers, dict):
            continue
        window_count += 1

        tick_from = int(window.get("from", 0))
        tick_to = int(window.get("to", tick_from))
        dt = max(0, tick_to - tick_from)
        total_ticks += dt

        credited = 0.0
        if "trophallaxis" in transfers:
            key_present = True
            credited = float(transfers["trophallaxis"])
        else:
            for k, v in transfers.items():
                if "trophallaxis" in str(k):
                    key_present = True
                    credited += float(v)
        total_credited += credited
        if credited > 0.0:
            nonzero_windows += 1

        if isinstance(flows, dict):
            if "trophallaxis-loss" in flows:
                total_dissipated += float(flows["trophallaxis-loss"])
            else:
                for k, v in flows.items():
                    if "trophallaxis" in str(k):
                        total_dissipated += float(v)

        pop = rec.get("population")
        if isinstance(pop, dict) and "live" in pop:
            weighted_live += float(pop["live"]) * dt

    if window_count == 0:
        return {
            "available": False,
            "reason": "no_transfer_windows",
            "window_count": 0,
            "key_present": False,
            "nonzero_windows": 0,
            "total_ticks": 0,
            "total_credited": None,
            "total_dissipated": None,
            "total_debited": None,
            "mean_flux_per_tick": None,
            "mean_live": None,
            "flux_per_organism_tick": None,
        }

    total_debited = total_credited + total_dissipated
    mean_flux_per_tick = (
        (total_credited / total_ticks) if total_ticks > 0 else None
    )
    mean_live = (weighted_live / total_ticks) if total_ticks > 0 else None
    flux_per_org_tick = (
        (total_credited / weighted_live) if weighted_live > 0.0 else None
    )

    return {
        "available": True,
        "reason": None,
        "window_count": window_count,
        "key_present": key_present,
        "nonzero_windows": nonzero_windows,
        "total_ticks": total_ticks,
        "total_credited": total_credited,
        "total_dissipated": total_dissipated,
        "total_debited": total_debited,
        "mean_flux_per_tick": mean_flux_per_tick,
        "mean_live": mean_live,
        "flux_per_organism_tick": flux_per_org_tick,
    }


def compute_flock_polarization(
    organisms: list[dict[str, Any]],
    width: float,
    height: float,
    align_radius: float = DEFAULT_ALIGN_RADIUS,
    speed_eps: float = DEFAULT_SPEED_EPS,
) -> dict[str, Any]:
    """Compute V2-13 global and local (all-neighbor & kin) flock polarization."""
    if width <= 0.0 or height <= 0.0:
        raise ValueError("world width and height must be strictly positive")
    if align_radius < 0.0:
        raise ValueError("align_radius must be nonnegative")

    n_total = len(organisms)
    moving_pos: list[tuple[float, float]] = []
    moving_unit: list[tuple[float, float]] = []
    moving_lineage: list[int] = []

    sum_ux = 0.0
    sum_uy = 0.0
    for org in organisms:
        vx = float(org["velocity"][0])
        vy = float(org["velocity"][1])
        speed = math.hypot(vx, vy)
        if math.isfinite(speed) and speed > speed_eps:
            ux = vx / speed
            uy = vy / speed
            moving_pos.append((float(org["position"][0]), float(org["position"][1])))
            moving_unit.append((ux, uy))
            moving_lineage.append(int(org["lineage"]))
            sum_ux += ux
            sum_uy += uy

    n_moving = len(moving_unit)
    if n_moving == 0:
        return {
            "available": True,
            "reason": "no_moving_organisms",
            "align_radius": align_radius,
            "speed_eps": speed_eps,
            "total_count": n_total,
            "moving_count": 0,
            "moving_fraction": 0.0 if n_total > 0 else None,
            "global_polarization": None,
            "local_vicsek_polarization": None,
            "local_pair_alignment": None,
            "local_kin_vicsek_polarization": None,
            "local_kin_pair_alignment": None,
            "neighbor_pair_count": 0,
            "kin_neighbor_pair_count": 0,
        }

    global_p = min(1.0, math.hypot(sum_ux, sum_uy) / n_moving)

    gx, gy, cw, ch, grid = _build_periodic_grid(
        moving_pos, width, height, align_radius
    )
    ox_list = _neighbor_offsets(gx)
    oy_list = _neighbor_offsets(gy)
    r_sq = align_radius * align_radius

    local_vicsek_sum = 0.0
    local_vicsek_n = 0
    kin_vicsek_sum = 0.0
    kin_vicsek_n = 0
    pair_dot_sum = 0.0
    pair_count = 0
    kin_pair_dot_sum = 0.0
    kin_pair_count = 0

    for i in range(n_moving):
        px, py = moving_pos[i]
        ux_i, uy_i = moving_unit[i]
        lin_i = moving_lineage[i]
        bx = int(math.floor(px / cw)) % gx
        by = int(math.floor(py / ch)) % gy

        nb_ux = ux_i
        nb_uy = uy_i
        nb_count = 0
        kin_ux = ux_i
        kin_uy = uy_i
        kin_count = 0

        for oy in oy_list:
            row = ((by + oy) % gy) * gx
            for ox in ox_list:
                for j in grid[row + ((bx + ox) % gx)]:
                    if j == i:
                        continue
                    dx = min_image(moving_pos[j][0] - px, width)
                    dy = min_image(moving_pos[j][1] - py, height)
                    if dx * dx + dy * dy <= r_sq:
                        ux_j, uy_j = moving_unit[j]
                        nb_ux += ux_j
                        nb_uy += uy_j
                        nb_count += 1
                        same_kin = moving_lineage[j] == lin_i
                        if same_kin:
                            kin_ux += ux_j
                            kin_uy += uy_j
                            kin_count += 1
                        if j > i:
                            dot = ux_i * ux_j + uy_i * uy_j
                            pair_dot_sum += dot
                            pair_count += 1
                            if same_kin:
                                kin_pair_dot_sum += dot
                                kin_pair_count += 1

        if nb_count > 0:
            local_vicsek_sum += min(1.0, math.hypot(nb_ux, nb_uy) / (nb_count + 1))
            local_vicsek_n += 1
        if kin_count > 0:
            kin_vicsek_sum += min(1.0, math.hypot(kin_ux, kin_uy) / (kin_count + 1))
            kin_vicsek_n += 1

    return {
        "available": True,
        "reason": None,
        "align_radius": align_radius,
        "speed_eps": speed_eps,
        "total_count": n_total,
        "moving_count": n_moving,
        "moving_fraction": n_moving / n_total,
        "global_polarization": global_p,
        "local_vicsek_polarization": (
            local_vicsek_sum / local_vicsek_n if local_vicsek_n > 0 else None
        ),
        "local_pair_alignment": (
            pair_dot_sum / pair_count if pair_count > 0 else None
        ),
        "local_kin_vicsek_polarization": (
            kin_vicsek_sum / kin_vicsek_n if kin_vicsek_n > 0 else None
        ),
        "local_kin_pair_alignment": (
            kin_pair_dot_sum / kin_pair_count if kin_pair_count > 0 else None
        ),
        "neighbor_pair_count": pair_count,
        "kin_neighbor_pair_count": kin_pair_count,
    }


def load_inspection(
    path: Path,
) -> tuple[dict[str, Any], list[dict[str, Any]], dict[str, Any] | None]:
    """Load one `flife inspect [--field]` NDJSON file."""
    header: dict[str, Any] | None = None
    organisms: list[dict[str, Any]] = []
    field: dict[str, Any] | None = None
    with path.open("r", encoding="utf-8") as fh:
        for line_no, raw_line in enumerate(fh, start=1):
            line = raw_line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"{path}:{line_no}: invalid JSON: {exc}") from exc
            kind = rec.get("record")
            if kind == "inspect":
                header = rec
            elif kind == "organism":
                organisms.append(rec)
            elif kind == "field":
                field = rec
    if header is None:
        raise ValueError(f"{path}: missing 'inspect' header record")
    return header, organisms, field


def load_metrics(path: Path) -> list[dict[str, Any]]:
    """Load `record == 'metrics'` entries from `flife run --metrics-every` NDJSON."""
    records: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as fh:
        for line_no, raw_line in enumerate(fh, start=1):
            line = raw_line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"{path}:{line_no}: invalid JSON: {exc}") from exc
            if rec.get("record") == "metrics" or (
                "window" in rec and "population" in rec
            ):
                records.append(rec)
    return records


def evaluate_stage2(
    inspect_path: Path,
    metrics_path: Path | None = None,
    *,
    crowding_radius: float = DEFAULT_CROWDING_RADIUS,
    align_radius: float = DEFAULT_ALIGN_RADIUS,
    bundle: str | None = None,
    condition: str | None = None,
    mode: str | None = None,
    seed: int | None = None,
) -> dict[str, Any]:
    """Run all five V2-17 Stage-2 diagnostics on real CLI outputs."""
    header, organisms, field = load_inspection(inspect_path)
    world = header["world"]
    width = float(world["width"])
    height = float(world["height"])

    metrics_records = load_metrics(metrics_path) if metrics_path is not None else None
    energy_plane = field.get("energy") if isinstance(field, dict) else None
    trail_plane = field.get("trail_energy") if isinstance(field, dict) else None
    vascular_plane = (
        field.get("vascular_conductance") if isinstance(field, dict) else None
    )

    checkpoint = header.get("checkpoint", {})
    return {
        "schema": DIAGNOSTICS_SCHEMA,
        "status": "ok",
        "bundle": bundle,
        "condition": condition,
        "mode": mode,
        "seed": seed,
        "inspect_path": str(inspect_path),
        "metrics_path": str(metrics_path) if metrics_path is not None else None,
        "run_dir": header.get("run_dir"),
        "model": header.get("model"),
        "v2_features": header.get("v2_features", []),
        "inv08_pure_field": header.get("inv08_pure_field"),
        "tick": checkpoint.get("tick"),
        "digest": header.get("digest"),
        "live": header.get("live", len(organisms)),
        "field_energy": header.get("field_energy"),
        "organism_energy": header.get("organism_energy"),
        "world": world,
        "diagnostics": {
            "clonal_differentiation": compute_clonal_differentiation(
                organisms, width, height, crowding_radius=crowding_radius
            ),
            "vascular_bimodality": compute_vascular_bimodality(vascular_plane),
            "trail_snr": compute_trail_snr(trail_plane, energy_plane),
            "trophallaxis_flux": compute_trophallaxis_flux(metrics_records),
            "flock_polarization": compute_flock_polarization(
                organisms, width, height, align_radius=align_radius
            ),
        },
    }


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Evaluate V2-17 Stage-2 diagnostics from real `flife inspect --field` "
            "and `flife run --metrics-every` NDJSON outputs."
        )
    )
    parser.add_argument(
        "--inspect",
        required=True,
        type=Path,
        help="Path to `flife inspect --field` NDJSON output.",
    )
    parser.add_argument(
        "--metrics",
        type=Path,
        default=None,
        help="Optional path to `flife run --metrics-every` NDJSON output.",
    )
    parser.add_argument(
        "--crowding-radius",
        type=float,
        default=DEFAULT_CROWDING_RADIUS,
        help="Periodic crowding radius r_c in world units (default: 0.375).",
    )
    parser.add_argument(
        "--align-radius",
        type=float,
        default=DEFAULT_ALIGN_RADIUS,
        help="Periodic velocity-alignment radius in world units (default: 0.375).",
    )
    parser.add_argument("--bundle", default=None, help="Optional bundle label (a|b).")
    parser.add_argument("--condition", default=None, help="Optional condition label.")
    parser.add_argument("--mode", default=None, help="Optional run mode (smoke|full).")
    parser.add_argument("--seed", type=int, default=None, help="Optional world seed.")
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Optional file to write pretty JSON output (otherwise prints compact JSON to stdout).",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(argv)
    result = evaluate_stage2(
        args.inspect,
        args.metrics,
        crowding_radius=args.crowding_radius,
        align_radius=args.align_radius,
        bundle=args.bundle,
        condition=args.condition,
        mode=args.mode,
        seed=args.seed,
    )
    if args.out is not None:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    else:
        print(json.dumps(result, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
