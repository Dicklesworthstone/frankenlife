#!/usr/bin/env python3
"""Independent known-answer analytical oracle tests for V2-17 Stage-2 diagnostics.

Every expected value below is derived analytically by hand (plan §21.1) — zero
production helpers are reused to compute expected outputs and zero mocks are
used. Run via:

    python3 -m unittest discover -s experiments/v2-stage2 -p 'test_*.py' -v
"""

from __future__ import annotations

import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import diagnostics as diag  # noqa: E402


def _org(
    oid: int,
    lineage: int,
    pos: tuple[float, float],
    vel: tuple[float, float] = (0.0, 0.0),
    h0: float | None = None,
) -> dict[str, object]:
    held = None
    if h0 is not None:
        held = {
            "evaluated_at": 64,
            "outputs": [0.0] * 40,
            "recurrent_state": [h0, 0.0],
        }
    return {
        "record": "organism",
        "id": oid,
        "genome": oid,
        "lineage": lineage,
        "generation": 1,
        "parent": None,
        "birth_tick": 0,
        "position": [pos[0], pos[1]],
        "velocity": [vel[0], vel[1]],
        "energy": 1.5,
        "topology": 0,
        "parameters": 1000,
        "affinity": [0.0] * 32,
        "motion_response": [0.0] * 32,
        "sensing_scale": 0.25,
        "held": held,
    }


class TestClonalDifferentiation(unittest.TestCase):
    def test_identical_clones_zero_within_variance(self) -> None:
        # Lineage 1 all have h_0 = -1.0, lineage 2 all have h_0 = +1.0.
        # Analytical: grand mean = 0.0, SS_within = 0, SS_between = 4,
        # between_var = 1.0, total_var = 1.0, within/total = 0.0.
        orgs = [
            _org(1, 1, (1.0, 1.0), h0=-1.0),
            _org(2, 1, (2.0, 1.0), h0=-1.0),
            _org(3, 2, (5.0, 5.0), h0=1.0),
            _org(4, 2, (6.0, 5.0), h0=1.0),
        ]
        res = diag.compute_clonal_differentiation(orgs, 10.0, 10.0, crowding_radius=0.375)
        self.assertTrue(res["available"])
        self.assertEqual(res["evaluated_count"], 4)
        self.assertEqual(res["multi_member_lineages"], 2)
        self.assertAlmostEqual(res["within_variance_weighted"], 0.0, places=12)
        self.assertAlmostEqual(res["within_variance_mean"], 0.0, places=12)
        self.assertAlmostEqual(res["within_variance_bessel_weighted"], 0.0, places=12)
        self.assertAlmostEqual(res["between_lineage_variance"], 1.0, places=12)
        self.assertAlmostEqual(res["total_h0_variance"], 1.0, places=12)
        self.assertAlmostEqual(res["within_to_total_ratio"], 0.0, places=12)
        self.assertAlmostEqual(res["max_lineage_h0_std"], 0.0, places=12)

    def test_differentiated_clones_exact_unit_variance(self) -> None:
        # Two clones, each with {-1.0, +1.0}:
        # Within-clone population variance = ((1)^2 + (1)^2) / 2 = 1.0.
        # Bessel-corrected pooled variance = 4.0 / (4 - 2) = 2.0.
        # Between-lineage variance = 0.0; within/total = 1.0.
        orgs = [
            _org(1, 10, (1.0, 1.0), h0=-1.0),
            _org(2, 10, (1.1, 1.0), h0=1.0),
            _org(3, 20, (5.0, 5.0), h0=-1.0),
            _org(4, 20, (5.1, 5.0), h0=1.0),
        ]
        res = diag.compute_clonal_differentiation(orgs, 10.0, 10.0, crowding_radius=0.375)
        self.assertTrue(res["available"])
        self.assertAlmostEqual(res["within_variance_weighted"], 1.0, places=12)
        self.assertAlmostEqual(res["within_variance_mean"], 1.0, places=12)
        self.assertAlmostEqual(res["within_variance_bessel_weighted"], 2.0, places=12)
        self.assertAlmostEqual(res["between_lineage_variance"], 0.0, places=12)
        self.assertAlmostEqual(res["total_h0_variance"], 1.0, places=12)
        self.assertAlmostEqual(res["within_to_total_ratio"], 1.0, places=12)
        self.assertAlmostEqual(res["max_lineage_h0_std"], 1.0, places=12)

    def test_crowding_correlation_across_periodic_torus_seam(self) -> None:
        # 10x10 periodic torus, r_c = 0.375:
        # - Org 1 alone at (5.0, 5.0) => c_den = 0
        # - Orgs 2,3 pair across x-seam at (0.05, 2.0) and (9.95, 2.0), dist = 0.10 => c_den = 1 each
        # - Orgs 4,5,6 tight triangle at (3.0, 3.0), (3.1, 3.0), (3.0, 3.1), pairwise dist <= 0.1414 => c_den = 2 each
        # Exact linear h_0 = -0.8 + 0.6 * c_den => exact Pearson r = +1.0 (both global & within-clone!).
        coords_and_cden = [
            ((5.0, 5.0), 0),
            ((0.05, 2.0), 1),
            ((9.95, 2.0), 1),
            ((3.0, 3.0), 2),
            ((3.1, 3.0), 2),
            ((3.0, 3.1), 2),
        ]
        orgs_pos = [
            _org(i + 1, 1, pos, h0=-0.8 + 0.6 * cd)
            for i, (pos, cd) in enumerate(coords_and_cden)
        ]
        res_pos = diag.compute_clonal_differentiation(
            orgs_pos, 10.0, 10.0, crowding_radius=0.375
        )
        self.assertTrue(res_pos["available"])
        self.assertAlmostEqual(res_pos["mean_cden"], (0 + 1 + 1 + 2 + 2 + 2) / 6.0, places=12)
        self.assertAlmostEqual(res_pos["corr_h0_cden"], 1.0, places=12)
        self.assertAlmostEqual(res_pos["within_lineage_corr_h0_cden"], 1.0, places=12)

        orgs_neg = [
            _org(i + 1, 1, pos, h0=0.8 - 0.6 * cd)
            for i, (pos, cd) in enumerate(coords_and_cden)
        ]
        res_neg = diag.compute_clonal_differentiation(
            orgs_neg, 10.0, 10.0, crowding_radius=0.375
        )
        self.assertAlmostEqual(res_neg["corr_h0_cden"], -1.0, places=12)
        self.assertAlmostEqual(res_neg["within_lineage_corr_h0_cden"], -1.0, places=12)

    def test_missing_recurrent_state_reports_unavailable(self) -> None:
        orgs = [_org(1, 1, (1.0, 1.0), h0=None), _org(2, 1, (1.1, 1.0), h0=None)]
        res = diag.compute_clonal_differentiation(orgs, 10.0, 10.0)
        self.assertFalse(res["available"])
        self.assertEqual(res["reason"], "no_recurrent_state")
        self.assertIsNone(res["within_variance_weighted"])
        self.assertIsNone(res["corr_h0_cden"])


class TestVascularBimodality(unittest.TestCase):
    def test_uniform_conductance_ratio_is_one(self) -> None:
        plane = [0.25] * 64
        res = diag.compute_vascular_bimodality(plane)
        self.assertTrue(res["available"])
        self.assertEqual(res["cell_count"], 64)
        self.assertAlmostEqual(res["high_to_mid_ratio"], 1.0, places=12)
        self.assertAlmostEqual(res["p90_to_p50_ratio"], 1.0, places=12)
        self.assertAlmostEqual(res["p95_to_p50_ratio"], 1.0, places=12)
        self.assertAlmostEqual(res["high_fraction_above_midpoint"], 0.0, places=12)
        self.assertIsNone(res["sarle_bimodality_coefficient"])

    def test_sharp_murray_arteries_exact_ratio_40(self) -> None:
        # 90 background parenchyma cells at g_min = 0.025 and 10 artery cells at 1.0.
        # Exact sorted quantiles:
        #   p25 = p50 = p75 = 0.025 -> mid_band_mean = 0.025
        #   p90 at pos 89.1 = 0.025 + 0.1 * (1.0 - 0.025) = 0.1225
        #   high_band (>= p90) is exactly the 10 cells at 1.0 -> high_band_mean = 1.0
        #   high_to_mid_ratio = 1.0 / 0.025 = 40.0
        #   p95 = 1.0 -> p95_to_p50_ratio = 40.0
        #   two-point Bernoulli(p=0.1) has skewness g1 = (1-2p)/sqrt(p(1-p)) = 0.8/0.3 = 8/3
        #   and excess kurtosis g2 = (1 - 6p(1-p))/(p(1-p)) = 0.46/0.09 = 46/9.
        plane = [0.025] * 90 + [1.0] * 10
        res = diag.compute_vascular_bimodality(plane)
        self.assertTrue(res["available"])
        self.assertEqual(res["cell_count"], 100)
        self.assertAlmostEqual(res["high_band_mean"], 1.0, places=12)
        self.assertAlmostEqual(res["mid_band_mean"], 0.025, places=12)
        self.assertAlmostEqual(res["high_to_mid_ratio"], 40.0, places=12)
        self.assertAlmostEqual(res["p95_to_p50_ratio"], 40.0, places=12)
        self.assertAlmostEqual(res["high_fraction_above_midpoint"], 0.10, places=12)

        g1 = 8.0 / 3.0
        g2 = 46.0 / 9.0
        expected_b = (g1 * g1 + 1.0) / (g2 + 3.0 * (99**2) / (98 * 97))
        self.assertAlmostEqual(res["sarle_bimodality_coefficient"], expected_b, places=12)
        self.assertGreater(res["sarle_bimodality_coefficient"], 5.0 / 9.0)

    def test_missing_plane_reports_unavailable(self) -> None:
        res = diag.compute_vascular_bimodality(None)
        self.assertFalse(res["available"])
        self.assertEqual(res["reason"], "missing_vascular_conductance_plane")
        self.assertIsNone(res["high_to_mid_ratio"])


class TestTrailSnr(unittest.TestCase):
    def test_sharp_trail_ridge_vs_uniform_carrier0(self) -> None:
        # 100 cells: 90 background at 0.01, 10 trail highway cells at 1.01.
        # Hand-computed:
        #   mean = 0.11, std = sqrt(0.9 * 0.1^2 + 0.1 * 0.9^2) = 0.30
        #   median = 0.01, p95 = 1.01, max = 1.01
        #   peak_z_snr = (1.01 - 0.01) / 0.30 = 10/3
        #   p95_to_median_ratio = 101.0
        #   top_decile_share = (10 * 1.01) / 11.0 = 10.1 / 11.0
        trail = [0.01] * 90 + [1.01] * 10
        carrier0 = [2.0] * 100
        res = diag.compute_trail_snr(trail, carrier0)
        self.assertTrue(res["available"])
        self.assertAlmostEqual(res["trail"]["mean"], 0.11, places=12)
        self.assertAlmostEqual(res["trail"]["std"], 0.30, places=12)
        self.assertAlmostEqual(res["trail"]["peak_z_snr"], 10.0 / 3.0, places=12)
        self.assertAlmostEqual(res["trail"]["p95_to_median_ratio"], 101.0, places=12)
        self.assertAlmostEqual(res["trail"]["top_decile_share"], 10.1 / 11.0, places=12)
        self.assertAlmostEqual(res["carrier0"]["peak_z_snr"], 0.0, places=12)
        self.assertIsNone(res["snr_ratio_vs_carrier0"])
        self.assertIsNone(res["corr_trail_carrier0"])

    def test_orthogonal_and_collinear_spatial_decoupling(self) -> None:
        # 4x4 grid: trail varies strictly with row y, carrier0 strictly with col x -> orthogonal (corr = 0, decoupling = 1).
        trail = [float(y) for y in range(4) for _x in range(4)]
        carrier0 = [float(x) for _y in range(4) for x in range(4)]
        res_orth = diag.compute_trail_snr(trail, carrier0)
        self.assertTrue(res_orth["available"])
        self.assertAlmostEqual(res_orth["corr_trail_carrier0"], 0.0, places=12)
        self.assertAlmostEqual(res_orth["spatial_decoupling"], 1.0, places=12)
        self.assertAlmostEqual(res_orth["snr_ratio_vs_carrier0"], 1.0, places=12)

        # Collinear scaled plane -> corr = 1, decoupling = 0.
        res_same = diag.compute_trail_snr([3.0 * v + 1.0 for v in trail], trail)
        self.assertAlmostEqual(res_same["corr_trail_carrier0"], 1.0, places=12)
        self.assertAlmostEqual(res_same["spatial_decoupling"], 0.0, places=12)

    def test_missing_trail_plane_reports_unavailable(self) -> None:
        res = diag.compute_trail_snr(None, [1.0, 2.0, 3.0, 4.0])
        self.assertFalse(res["available"])
        self.assertEqual(res["reason"], "missing_trail_energy_plane")
        self.assertIsNone(res["trail"])
        self.assertIsNotNone(res["carrier0"])


class TestTrophallaxisFlux(unittest.TestCase):
    def test_multi_window_exact_sum_and_rates(self) -> None:
        # Window 1: ticks 0..128 (dt=128), live=100, credited=12.8, loss=0.64
        # Window 2: ticks 128..256 (dt=128), live=300, credited=38.4, loss=1.92
        # Analytical totals:
        #   total_credited = 51.2, total_dissipated = 2.56, total_debited = 53.76
        #   total_ticks = 256, mean_flux_per_tick = 51.2 / 256 = 0.2
        #   mean_live = (100*128 + 300*128) / 256 = 200.0
        #   flux_per_organism_tick = 51.2 / (200.0 * 256) = 0.001
        records = [
            {
                "record": "metrics",
                "tick": 128,
                "window": {
                    "from": 0,
                    "to": 128,
                    "transfers": {"uptake": 100.0, "trophallaxis": 12.8},
                    "flows": {"maintenance": 10.0, "trophallaxis-loss": 0.64},
                },
                "population": {"live": 100},
            },
            {
                "record": "metrics",
                "tick": 256,
                "window": {
                    "from": 128,
                    "to": 256,
                    "transfers": {"uptake": 250.0, "trophallaxis": 38.4},
                    "flows": {"maintenance": 30.0, "trophallaxis-loss": 1.92},
                },
                "population": {"live": 300},
            },
        ]
        res = diag.compute_trophallaxis_flux(records)
        self.assertTrue(res["available"])
        self.assertTrue(res["key_present"])
        self.assertEqual(res["window_count"], 2)
        self.assertEqual(res["nonzero_windows"], 2)
        self.assertEqual(res["total_ticks"], 256)
        self.assertAlmostEqual(res["total_credited"], 51.2, places=12)
        self.assertAlmostEqual(res["total_dissipated"], 2.56, places=12)
        self.assertAlmostEqual(res["total_debited"], 53.76, places=12)
        self.assertAlmostEqual(res["mean_flux_per_tick"], 0.2, places=12)
        self.assertAlmostEqual(res["mean_live"], 200.0, places=12)
        self.assertAlmostEqual(res["flux_per_organism_tick"], 0.001, places=12)

    def test_disabled_trophallaxis_zero_and_missing_metrics_unavailable(self) -> None:
        disabled = [
            {
                "record": "metrics",
                "tick": 128,
                "window": {
                    "from": 0,
                    "to": 128,
                    "transfers": {"uptake": 42.0},
                    "flows": {"maintenance": 5.0},
                },
                "population": {"live": 50},
            }
        ]
        res_off = diag.compute_trophallaxis_flux(disabled)
        self.assertTrue(res_off["available"])
        self.assertFalse(res_off["key_present"])
        self.assertEqual(res_off["nonzero_windows"], 0)
        self.assertAlmostEqual(res_off["total_credited"], 0.0, places=12)

        res_missing = diag.compute_trophallaxis_flux(None)
        self.assertFalse(res_missing["available"])
        self.assertIsNone(res_missing["total_credited"])


class TestFlockPolarization(unittest.TestCase):
    def test_aligned_and_antiparallel_configurations(self) -> None:
        aligned = [
            _org(1, 1, (1.0, 1.0), vel=(2.0, 0.0)),
            _org(2, 1, (1.1, 1.0), vel=(0.5, 0.0)),
            _org(3, 1, (1.2, 1.0), vel=(3.0, 0.0)),
        ]
        res_a = diag.compute_flock_polarization(aligned, 10.0, 10.0, align_radius=0.375)
        self.assertTrue(res_a["available"])
        self.assertEqual(res_a["moving_count"], 3)
        self.assertAlmostEqual(res_a["global_polarization"], 1.0, places=12)
        self.assertAlmostEqual(res_a["local_vicsek_polarization"], 1.0, places=12)
        self.assertAlmostEqual(res_a["local_pair_alignment"], 1.0, places=12)
        self.assertAlmostEqual(res_a["local_kin_vicsek_polarization"], 1.0, places=12)
        self.assertAlmostEqual(res_a["local_kin_pair_alignment"], 1.0, places=12)

        anti = [
            _org(1, 1, (1.0, 1.0), vel=(1.0, 0.0)),
            _org(2, 1, (1.1, 1.0), vel=(-1.0, 0.0)),
        ]
        res_b = diag.compute_flock_polarization(anti, 10.0, 10.0, align_radius=0.375)
        self.assertAlmostEqual(res_b["global_polarization"], 0.0, places=12)
        self.assertAlmostEqual(res_b["local_vicsek_polarization"], 0.0, places=12)
        self.assertAlmostEqual(res_b["local_pair_alignment"], -1.0, places=12)

    def test_two_opposing_flocks_across_periodic_torus_seam(self) -> None:
        # Flock A (lineage 1): 4 organisms straddling x=0 seam moving east (+1, 0)
        # Flock B (lineage 2): 4 organisms at x=5.0 moving west (-1, 0)
        # Analytical: global P = 0.0 (exact vector cancellation),
        # while local Vicsek P = 1.0, local pair alignment = 1.0, and kin local = 1.0.
        orgs = [
            _org(1, 1, (0.05, 1.0), vel=(1.0, 0.0)),
            _org(2, 1, (9.95, 1.0), vel=(2.0, 0.0)),
            _org(3, 1, (0.05, 1.1), vel=(0.5, 0.0)),
            _org(4, 1, (9.95, 1.1), vel=(1.5, 0.0)),
            _org(5, 2, (5.00, 5.0), vel=(-1.0, 0.0)),
            _org(6, 2, (5.10, 5.0), vel=(-2.0, 0.0)),
            _org(7, 2, (5.00, 5.1), vel=(-0.5, 0.0)),
            _org(8, 2, (5.10, 5.1), vel=(-1.5, 0.0)),
        ]
        res = diag.compute_flock_polarization(orgs, 10.0, 10.0, align_radius=0.375)
        self.assertAlmostEqual(res["global_polarization"], 0.0, places=12)
        self.assertAlmostEqual(res["local_vicsek_polarization"], 1.0, places=12)
        self.assertAlmostEqual(res["local_pair_alignment"], 1.0, places=12)
        self.assertAlmostEqual(res["local_kin_vicsek_polarization"], 1.0, places=12)
        self.assertAlmostEqual(res["local_kin_pair_alignment"], 1.0, places=12)
        # Each 4-organism cluster has C(4,2) = 6 pairs -> 12 total pairs, all same-kin.
        self.assertEqual(res["neighbor_pair_count"], 12)
        self.assertEqual(res["kin_neighbor_pair_count"], 12)

    def test_stationary_population_no_nan(self) -> None:
        orgs = [_org(1, 1, (1.0, 1.0), vel=(0.0, 0.0)), _org(2, 1, (1.1, 1.0), vel=(0.0, 0.0))]
        res = diag.compute_flock_polarization(orgs, 10.0, 10.0)
        self.assertTrue(res["available"])
        self.assertEqual(res["moving_count"], 0)
        self.assertAlmostEqual(res["moving_fraction"], 0.0, places=12)
        self.assertIsNone(res["global_polarization"])
        self.assertIsNone(res["local_vicsek_polarization"])


class TestEndToEndCli(unittest.TestCase):
    def test_cli_parses_inspect_and_metrics_ndjson(self) -> None:
        script = Path(__file__).resolve().parent / "diagnostics.py"
        with tempfile.TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            inspect_file = tmp_dir / "inspect.ndjson"
            metrics_file = tmp_dir / "metrics.ndjson"

            header = {
                "record": "inspect",
                "schema": "frankenlife.inspect.v1",
                "run_dir": str(tmp_dir / "run"),
                "checkpoint": {"sequence": 1, "tick": 256, "sha256": "0" * 64},
                "digest": "a" * 64,
                "model": "field-ecology-v2",
                "v2_features": [
                    "epigenetic-recurrence",
                    "notch-delta",
                    "trophallaxis",
                    "vascular-field",
                    "dual-carrier-trails",
                ],
                "inv08_pure_field": False,
                "world": {"width": 10.0, "height": 10.0, "cells": [10, 10]},
                "live": 2,
                "field_energy": 100.0,
                "organism_energy": 3.0,
            }
            orgs = [
                _org(1, 7, (1.0, 1.0), vel=(1.0, 0.0), h0=-1.0),
                _org(2, 7, (1.1, 1.0), vel=(1.0, 0.0), h0=1.0),
            ]
            field = {
                "record": "field",
                "cells": [10, 10],
                "energy": [1.0] * 100,
                "trail_energy": [0.01] * 90 + [1.01] * 10,
                "vascular_conductance": [0.025] * 90 + [1.0] * 10,
            }
            inspect_lines = [header, *orgs, field]
            inspect_file.write_text(
                "\n".join(json.dumps(r) for r in inspect_lines) + "\n",
                encoding="utf-8",
            )

            metrics_lines = [
                {"record": "start", "schema": "frankenlife.run.v1"},
                {
                    "record": "metrics",
                    "schema": "frankenlife.run.v1",
                    "tick": 256,
                    "window": {
                        "from": 0,
                        "to": 256,
                        "transfers": {"trophallaxis": 25.6},
                        "flows": {"trophallaxis-loss": 1.28},
                    },
                    "population": {"live": 2},
                },
            ]
            metrics_file.write_text(
                "\n".join(json.dumps(r) for r in metrics_lines) + "\n",
                encoding="utf-8",
            )

            proc = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    "--inspect",
                    str(inspect_file),
                    "--metrics",
                    str(metrics_file),
                    "--bundle",
                    "a",
                    "--condition",
                    "treatment",
                    "--mode",
                    "smoke",
                    "--seed",
                    "42",
                ],
                capture_output=True,
                text=True,
                check=True,
            )
            payload = json.loads(proc.stdout)
            self.assertEqual(payload["schema"], diag.DIAGNOSTICS_SCHEMA)
            self.assertEqual(payload["bundle"], "a")
            self.assertEqual(payload["condition"], "treatment")
            self.assertEqual(payload["tick"], 256)
            d = payload["diagnostics"]
            self.assertAlmostEqual(
                d["clonal_differentiation"]["within_variance_weighted"], 1.0, places=12
            )
            self.assertAlmostEqual(
                d["vascular_bimodality"]["high_to_mid_ratio"], 40.0, places=12
            )
            self.assertAlmostEqual(
                d["trail_snr"]["trail"]["peak_z_snr"], 10.0 / 3.0, places=12
            )
            self.assertAlmostEqual(
                d["trophallaxis_flux"]["total_credited"], 25.6, places=12
            )
            self.assertAlmostEqual(
                d["flock_polarization"]["global_polarization"], 1.0, places=12
            )
            self.assertTrue(d["field_spatial_contrast"]["available"])

    def test_field_spatial_contrast_oracles(self) -> None:
        uniform = diag.compute_field_spatial_contrast([5.0, 5.0, 5.0, 5.0])
        self.assertTrue(uniform["available"])
        self.assertAlmostEqual(uniform["gini"], 0.0, places=12)
        self.assertAlmostEqual(uniform["dark_fraction_below_10pct_mean"], 0.0, places=12)

        spike = diag.compute_field_spatial_contrast([0.0, 0.0, 0.0, 10.0])
        self.assertAlmostEqual(spike["gini"], 0.75, places=12)
        self.assertAlmostEqual(spike["dark_fraction_below_10pct_mean"], 0.75, places=12)

        half_dark = diag.compute_field_spatial_contrast([0.0, 0.0, 10.0, 10.0])
        self.assertAlmostEqual(half_dark["gini"], 0.5, places=12)
        self.assertAlmostEqual(half_dark["dark_fraction_below_10pct_mean"], 0.5, places=12)

        absent = diag.compute_field_spatial_contrast(None)
        self.assertFalse(absent["available"])
        self.assertIsNone(absent["gini"])


if __name__ == "__main__":
    unittest.main()
