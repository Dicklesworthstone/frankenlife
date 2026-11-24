#!/usr/bin/env bash
# V2-17 Stage-2 Verification & Ablation Harness
# (Bundle A: presets/morphogenesis-tissue-v2.toml;
#  Bundle B: presets/supercolony-v2.toml)
#
# Invokes the real `flife` CLI (`validate`, `run`, `inspect --field`, optional
# `render`) and evaluates Stage-2 diagnostics via `diagnostics.py` (Python
# stdlib only, zero mocks, zero fabricated results). Every run — complete,
# extinct, paused, or failed — is retained and recorded in
# `$OUT_DIR/stage2_diagnostics.ndjson` (outside git).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
DIAGNOSTICS_PY="${SCRIPT_DIR}/diagnostics.py"

BUNDLE_A_PRESET="${REPO_ROOT}/presets/morphogenesis-tissue-v2.toml"
BUNDLE_B_PRESET="${REPO_ROOT}/presets/supercolony-v2.toml"

MODE="full"
BUNDLE="both"
VALIDATE_ONLY=0
DO_RENDER=0
FLIFE_BIN="${FLIFE_BIN:-}"
OUT_DIR=""
CONTROLLERS="auto"
CPU_WORKERS="auto"
TICKS_OVERRIDE=""
SEEDS_OVERRIDE=""

usage() {
  cat <<'EOF'
Usage: experiments/v2-stage2/run_stage2.sh [OPTIONS]

Options:
  --validate-only       Validate Bundle A/B presets, fixed-controller configs,
                        all CLI ablation toggle chains, and the notch-delta ->
                        epigenetic-recurrence dependency refusal, then exit.
  --smoke               Fast scaled 64x64 smoke run (256 ticks, seed 42, core
                        ablations; clearly tagged mode="smoke").
  --full                Full Stage-2 campaign on unmodified 320x320 Bundle A/B
                        presets (default: 4096 ticks, seeds 42,43,101,102).
  --bundle a|b|both     Which showcase bundle(s) to run (default: both).
  --ticks N             Override simulation horizon ticks (positive integer).
  --seeds CSV           Override comma-separated seeds (e.g. 42,43,101,102).
  --render              Render raw + sphere + all four V2-16 overlays
                        (cell-state, polarity, trail-carrier,
                        vascular-conductance) for treatment checkpoints.
  --flife PATH          Path to compiled `flife` binary (default: auto-detect
                        target/{release,debug}/flife or PATH).
  --out DIR             Artifact directory outside git
                        (default: artifacts/v2-stage2/<mode>).
  --controllers ROUTE   Controller backend: auto|cpu|torch|metal (default: auto).
  --cpu-workers N|auto  Worker threads for flife (default: auto).
  -h, --help            Show this help message.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --validate-only)
      VALIDATE_ONLY=1
      shift
      ;;
    --smoke)
      MODE="smoke"
      shift
      ;;
    --full)
      MODE="full"
      shift
      ;;
    --bundle)
      BUNDLE="${2:?missing value for --bundle}"
      shift 2
      ;;
    --ticks)
      TICKS_OVERRIDE="${2:?missing value for --ticks}"
      shift 2
      ;;
    --seeds)
      SEEDS_OVERRIDE="${2:?missing value for --seeds}"
      shift 2
      ;;
    --render)
      DO_RENDER=1
      shift
      ;;
    --flife)
      FLIFE_BIN="${2:?missing value for --flife}"
      shift 2
      ;;
    --out)
      OUT_DIR="${2:?missing value for --out}"
      shift 2
      ;;
    --controllers)
      CONTROLLERS="${2:?missing value for --controllers}"
      shift 2
      ;;
    --cpu-workers)
      CPU_WORKERS="${2:?missing value for --cpu-workers}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option '$1'" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "${BUNDLE}" in
  a|b|both) ;;
  *)
    echo "error: --bundle must be a, b, or both (got '${BUNDLE}')" >&2
    exit 2
    ;;
esac

case "${CONTROLLERS}" in
  auto|cpu|torch|metal) ;;
  *)
    echo "error: --controllers must be auto, cpu, torch, or metal (got '${CONTROLLERS}')" >&2
    exit 2
    ;;
esac

if [[ -n "${TICKS_OVERRIDE}" ]] && ! [[ "${TICKS_OVERRIDE}" =~ ^[1-9][0-9]*$ ]]; then
  echo "error: --ticks must be a positive integer (got '${TICKS_OVERRIDE}')" >&2
  exit 2
fi

if [[ -n "${SEEDS_OVERRIDE}" ]] && ! [[ "${SEEDS_OVERRIDE}" =~ ^[0-9]+(,[0-9]+)*$ ]]; then
  echo "error: --seeds must be a comma-separated list of non-negative integers (got '${SEEDS_OVERRIDE}')" >&2
  exit 2
fi

if [[ ! -f "${DIAGNOSTICS_PY}" ]]; then
  echo "error: missing diagnostics evaluator at '${DIAGNOSTICS_PY}'" >&2
  exit 2
fi

if [[ -z "${FLIFE_BIN}" ]]; then
  if [[ -x "${REPO_ROOT}/target/release/flife" ]]; then
    FLIFE_BIN="${REPO_ROOT}/target/release/flife"
  elif [[ -x "${REPO_ROOT}/target/debug/flife" ]]; then
    FLIFE_BIN="${REPO_ROOT}/target/debug/flife"
  elif command -v flife >/dev/null 2>&1; then
    FLIFE_BIN="$(command -v flife)"
  else
    echo "error: no compiled 'flife' binary found (checked target/release/flife, target/debug/flife, PATH)." >&2
    echo "hint: build via RCH or pass --flife /path/to/flife" >&2
    exit 2
  fi
fi

if [[ ! -x "${FLIFE_BIN}" ]]; then
  echo "error: flife binary '${FLIFE_BIN}' is not executable" >&2
  exit 2
fi

if [[ -z "${OUT_DIR}" ]]; then
  OUT_DIR="${REPO_ROOT}/artifacts/v2-stage2/${MODE}"
fi
OUT_DIR="$(python3 -c 'import os, sys; print(os.path.abspath(sys.argv[1]))' "${OUT_DIR}")"

# Guard against writing large run directories into tracked source paths.
case "${OUT_DIR}" in
  "${REPO_ROOT}/crates"| "${REPO_ROOT}/crates/"*|\
  "${REPO_ROOT}/presets"| "${REPO_ROOT}/presets/"*|\
  "${REPO_ROOT}/experiments"| "${REPO_ROOT}/experiments/"*|\
  "${REPO_ROOT}/web"| "${REPO_ROOT}/web/"*|\
  "${REPO_ROOT}/scripts"| "${REPO_ROOT}/scripts/"*)
    echo "error: --out '${OUT_DIR}' points inside a tracked repository directory; choose an artifact path outside git (e.g. artifacts/v2-stage2/${MODE})." >&2
    exit 2
    ;;
esac

mkdir -p "${OUT_DIR}/configs"

log() {
  printf '[v2-stage2 %s] %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

# Generate a deterministic variant TOML (seed override, optional fixed-controller
# model swap, and optional 64x64 smoke scaling preserving h = D = 0.25 and all
# [v2] sections verbatim).
generate_variant_toml() {
  local base_toml="$1"
  local dest_toml="$2"
  local seed_val="$3"
  local model_override="$4"
  local smoke_scale="$5"
  python3 - "$base_toml" "$dest_toml" "$seed_val" "$model_override" "$smoke_scale" <<'PY'
import re
import sys
from pathlib import Path

base_path, dest_path, seed_s, model_override, smoke_s = sys.argv[1:6]
seed = int(seed_s)
smoke = smoke_s == "1"
text = Path(base_path).read_text(encoding="utf-8")

# Parse original area, founder count, and source count so smoke scaling preserves
# founder density, power density, Gaussian overlap ratio (M * pi * sigma^2 / area),
# and dimensionless source-trail smearing (path_speed / (sigma * decay)).
orig_w, orig_h, orig_count, orig_founders = 80.0, 80.0, 16, 10954
scan_sec = ""
for raw in text.splitlines():
    s = raw.strip()
    if s.startswith("[") and s.endswith("]"):
        scan_sec = s[1:-1].strip()
    elif scan_sec == "world":
        m = re.match(r"^width\s*=\s*([0-9.eE+-]+)", s)
        if m:
            orig_w = float(m.group(1))
        m = re.match(r"^height\s*=\s*([0-9.eE+-]+)", s)
        if m:
            orig_h = float(m.group(1))
    elif scan_sec == "population":
        m = re.match(r"^initial_count\s*=\s*([0-9]+)", s)
        if m:
            orig_founders = int(m.group(1))
    elif scan_sec == "sources":
        m = re.match(r"^count\s*=\s*([0-9]+)", s)
        if m:
            orig_count = int(m.group(1))

smoke_side = 16.0
smoke_sources = 4
area_ratio = (smoke_side * smoke_side) / max(orig_w * orig_h, 1.0e-9)
sigma_scale = (area_ratio * (orig_count / smoke_sources)) ** 0.5
smoke_founders = max(256, int(round(orig_founders * area_ratio)))

# Replace top-level seed (and optionally model) before the first [section].
lines = text.splitlines(keepends=True)
out = []
section = ""
for line in lines:
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        section = stripped[1:-1].strip()
    if section == "":
        if re.match(r"^seed\s*=", stripped):
            out.append(f"seed = {seed}\n")
            continue
        if model_override and re.match(r"^model\s*=", stripped):
            out.append(f'model = "{model_override}"\n')
            continue
    elif smoke:
        if section == "world":
            if re.match(r"^width\s*=", stripped):
                out.append("width = 16.0\n")
                continue
            if re.match(r"^height\s*=", stripped):
                out.append("height = 16.0\n")
                continue
            if re.match(r"^grid_width\s*=", stripped):
                out.append("grid_width = 64\n")
                continue
            if re.match(r"^grid_height\s*=", stripped):
                out.append("grid_height = 64\n")
                continue
        elif section == "population":
            if re.match(r"^initial_count\s*=", stripped):
                out.append(f"initial_count = {smoke_founders}\n")
                continue
            if re.match(r"^maximum_count\s*=", stripped):
                out.append("maximum_count = 8192\n")
                continue
        elif section == "sources":
            if re.match(r"^count\s*=", stripped):
                out.append(f"count = {smoke_sources}\n")
                continue
            if re.match(r"^power_per_source\s*=", stripped):
                # Preserve exact mean source power per unit area across grid scaling.
                m = re.search(r"=\s*([0-9.eE+-]+)", stripped)
                if m:
                    scaled = (float(m.group(1)) * orig_count * area_ratio) / smoke_sources
                    out.append(f"power_per_source = {scaled:.4f}\n")
                    continue
            if re.match(r"^sigma_world_units\s*=", stripped):
                # Preserve exact Gaussian overlap fraction M * pi * sigma^2 / area.
                m = re.search(r"=\s*([0-9.eE+-]+)", stripped)
                if m:
                    scaled_s = max(1.0, float(m.group(1)) * sigma_scale)
                    out.append(f"sigma_world_units = {scaled_s:.4f}\n")
                    continue
            if re.match(r"^path_speed\s*=", stripped):
                # Preserve dimensionless smearing ratio path_speed / (sigma * decay).
                m = re.search(r"=\s*([0-9.eE+-]+)", stripped)
                if m:
                    scaled_v = float(m.group(1)) * sigma_scale
                    out.append(f"path_speed = {scaled_v:.4f}\n")
                    continue
        elif section == "v2.oases":
            if re.match(r"^sigma_min_world_units\s*=", stripped):
                m = re.search(r"=\s*([0-9.eE+-]+)", stripped)
                if m:
                    scaled_s = max(0.75, float(m.group(1)) * sigma_scale)
                    out.append(f"sigma_min_world_units = {scaled_s:.4f}\n")
                    continue
            if re.match(r"^sigma_max_world_units\s*=", stripped):
                m = re.search(r"=\s*([0-9.eE+-]+)", stripped)
                if m:
                    scaled_s = max(1.0, float(m.group(1)) * sigma_scale)
                    out.append(f"sigma_max_world_units = {scaled_s:.4f}\n")
                    continue
    out.append(line)

Path(dest_path).write_text("".join(out), encoding="utf-8")
PY
}

validate_stage2_matrix() {
  log "Validating V2-17 Stage-2 presets and CLI toggle matrices with ${FLIFE_BIN}"

  # Canonical Bundle A & Bundle B showcase presets.
  "${FLIFE_BIN}" validate "${BUNDLE_A_PRESET}" > "${OUT_DIR}/validate-bundle-a.json"
  log "  OK validate presets/morphogenesis-tissue-v2.toml"
  "${FLIFE_BIN}" validate "${BUNDLE_B_PRESET}" > "${OUT_DIR}/validate-bundle-b.json"
  log "  OK validate presets/supercolony-v2.toml"

  # Verify fixed-controller ablation configs validate cleanly.
  local fc_a="${OUT_DIR}/configs/validate-bundle-a-fixed-controller.toml"
  local fc_b="${OUT_DIR}/configs/validate-bundle-b-fixed-controller.toml"
  generate_variant_toml "${BUNDLE_A_PRESET}" "${fc_a}" 42 "field-ecology-v2-fixed-controller" 0
  generate_variant_toml "${BUNDLE_B_PRESET}" "${fc_b}" 42 "field-ecology-v2-fixed-controller" 0
  "${FLIFE_BIN}" validate "${fc_a}" > "${OUT_DIR}/validate-bundle-a-fixed-controller.json"
  "${FLIFE_BIN}" validate "${fc_b}" > "${OUT_DIR}/validate-bundle-b-fixed-controller.json"
  log "  OK validate fixed-controller variants (Bundle A & B)"

  # Verify smoke-scaled variants validate cleanly too.
  local sm_a="${OUT_DIR}/configs/validate-bundle-a-smoke64.toml"
  local sm_b="${OUT_DIR}/configs/validate-bundle-b-smoke64.toml"
  generate_variant_toml "${BUNDLE_A_PRESET}" "${sm_a}" 42 "" 1
  generate_variant_toml "${BUNDLE_B_PRESET}" "${sm_b}" 42 "" 1
  "${FLIFE_BIN}" validate "${sm_a}" > "${OUT_DIR}/validate-bundle-a-smoke64.json"
  "${FLIFE_BIN}" validate "${sm_b}" > "${OUT_DIR}/validate-bundle-b-smoke64.json"
  log "  OK validate 64x64 smoke variants (Bundle A & B)"

  # Required V2-17 dependency guard: disabling epigenetic-recurrence alone while
  # notch-delta is enabled MUST be refused.
  local dep_rc=0
  "${FLIFE_BIN}" validate "${BUNDLE_A_PRESET}" --disable epigenetic-recurrence \
    >"${OUT_DIR}/validate-notch-dep-stdout.log" \
    2>"${OUT_DIR}/validate-notch-dep-stderr.log" || dep_rc=$?
  if [[ "${dep_rc}" -eq 0 ]]; then
    echo "error: expected --disable epigenetic-recurrence alone on Bundle A to be refused (notch-delta dependency), but it succeeded" >&2
    exit 1
  fi
  log "  OK refusal verified: --disable epigenetic-recurrence alone fails (exit ${dep_rc}) due to notch-delta dependency"

  # Bundle A single-mechanism CLI ablation matrix.
  local a_toggles=(
    "--disable epigenetic-recurrence,notch-delta"
    "--disable notch-delta"
    "--disable trophallaxis"
    "--disable cadherin-4d"
    "--disable lysis-recycling"
    "--disable symplectic-waste"
    "--disable pcp-polarity"
    "--disable hippo-crowding"
    "--disable tight-junctions"
    "--enable velocity-alignment"
    "--v1-compat"
  )
  local spec
  for spec in "${a_toggles[@]}"; do
    # shellcheck disable=SC2086
    "${FLIFE_BIN}" validate "${BUNDLE_A_PRESET}" ${spec} >/dev/null
  done
  log "  OK Bundle A toggle matrix (${#a_toggles[@]} CLI override chains)"

  # Bundle B single-mechanism CLI ablation matrix (INV-08 pure-field).
  local b_toggles=(
    "--disable vascular-field"
    "--disable dual-carrier-trails"
    "--disable excitable-relay"
    "--disable oases"
    "--disable so2-motor"
    "--disable lysis-recycling"
    "--disable tight-junctions"
    "--enable so2-budding,symplectic-waste"
    "--v1-compat"
  )
  for spec in "${b_toggles[@]}"; do
    # shellcheck disable=SC2086
    "${FLIFE_BIN}" validate "${BUNDLE_B_PRESET}" ${spec} >/dev/null
  done
  log "  OK Bundle B toggle matrix (${#b_toggles[@]} CLI override chains)"
}

render_stage2_views() {
  local bundle_tag="$1"
  local cond_name="$2"
  local seed_val="$3"
  local run_dir="$4"
  local render_root="${OUT_DIR}/renders/bundle-${bundle_tag}/${cond_name}/seed-${seed_val}"
  local render_size=640
  if [[ "${MODE}" == "full" ]]; then
    render_size=1280
  fi

  log "  Rendering Gate G5 + bundle-${bundle_tag} views (${render_size}x${render_size}) for ${cond_name} seed=${seed_val}"

  if [[ "${bundle_tag}" == "a" ]]; then
    # Shared fair mapping (treatment & v1-compat side by side) + Gate G5 raw.
    mkdir -p "${render_root}/sphere-lineage-energy" "${render_root}/raw-lineage-energy"
    "${FLIFE_BIN}" render --run-dir "${run_dir}" \
      --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
      --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
      --look sphere --color lineage-energy --no-field \
      --out "${render_root}/sphere-lineage-energy" \
      > "${render_root}/render-sphere-lineage-energy.ndjson"
    "${FLIFE_BIN}" render --run-dir "${run_dir}" \
      --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
      --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
      --look raw --color lineage-energy --field-mapping energy \
      --out "${render_root}/raw-lineage-energy" \
      > "${render_root}/render-raw-lineage-energy.ndjson"

    if [[ "${cond_name}" == "treatment" ]]; then
      # Bundle A mechanism overlays: V2-12 clonal cell-state + V2-08 polarity/apical ticks (--no-field dark backdrop).
      mkdir -p \
        "${render_root}/glow-cell-state" \
        "${render_root}/raw-cell-state" \
        "${render_root}/sphere-polarity"
      "${FLIFE_BIN}" render --run-dir "${run_dir}" \
        --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
        --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
        --look glow --color cell-state --field-mapping energy \
        --out "${render_root}/glow-cell-state" \
        > "${render_root}/render-glow-cell-state.ndjson"
      "${FLIFE_BIN}" render --run-dir "${run_dir}" \
        --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
        --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
        --look raw --color cell-state --field-mapping energy \
        --out "${render_root}/raw-cell-state" \
        > "${render_root}/render-raw-cell-state.ndjson"
      "${FLIFE_BIN}" render --run-dir "${run_dir}" \
        --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
        --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
        --look sphere --color polarity --no-field \
        --out "${render_root}/sphere-polarity" \
        > "${render_root}/render-sphere-polarity.ndjson"
    fi
  else
    # Bundle B shared fair mapping (treatment & v1-compat) + Gate G5 raw.
    mkdir -p "${render_root}/glow-lineage-energy" "${render_root}/raw-lineage-energy"
    "${FLIFE_BIN}" render --run-dir "${run_dir}" \
      --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
      --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
      --look glow --color lineage-energy --field-mapping energy \
      --out "${render_root}/glow-lineage-energy" \
      > "${render_root}/render-glow-lineage-energy.ndjson"
    "${FLIFE_BIN}" render --run-dir "${run_dir}" \
      --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
      --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
      --look raw --color lineage-energy --field-mapping energy \
      --out "${render_root}/raw-lineage-energy" \
      > "${render_root}/render-raw-lineage-energy.ndjson"

    if [[ "${cond_name}" == "treatment" ]]; then
      # Bundle B mechanism overlays: V2-14 Murray vascular conductance + V2-15 Carrier-1 trails (glow + G5 raw).
      mkdir -p \
        "${render_root}/glow-vascular-conductance" \
        "${render_root}/glow-trail-carrier" \
        "${render_root}/raw-vascular-conductance" \
        "${render_root}/raw-trail-carrier"
      "${FLIFE_BIN}" render --run-dir "${run_dir}" \
        --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
        --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
        --look glow --color lineage-energy --field-mapping vascular-conductance \
        --out "${render_root}/glow-vascular-conductance" \
        > "${render_root}/render-glow-vascular-conductance.ndjson"
      "${FLIFE_BIN}" render --run-dir "${run_dir}" \
        --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
        --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
        --look glow --color lineage-energy --field-mapping trail-carrier \
        --out "${render_root}/glow-trail-carrier" \
        > "${render_root}/render-glow-trail-carrier.ndjson"
      "${FLIFE_BIN}" render --run-dir "${run_dir}" \
        --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
        --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
        --look raw --color lineage-energy --field-mapping vascular-conductance \
        --out "${render_root}/raw-vascular-conductance" \
        > "${render_root}/render-raw-vascular-conductance.ndjson"
      "${FLIFE_BIN}" render --run-dir "${run_dir}" \
        --ticks 0 --every 1 --width "${render_size}" --height "${render_size}" \
        --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
        --look raw --color lineage-energy --field-mapping trail-carrier \
        --out "${render_root}/raw-trail-carrier" \
        > "${render_root}/render-raw-trail-carrier.ndjson"
    fi
  fi
}

run_one_condition() {
  local bundle_tag="$1"
  local base_preset="$2"
  local cond_name="$3"
  local model_override="$4"
  local seed_val="$5"
  local horizon_ticks="$6"
  local metrics_every="$7"
  shift 7
  local extra_flags=("$@")

  local smoke_flag=0
  if [[ "${MODE}" == "smoke" ]]; then
    smoke_flag=1
  fi

  local cond_dir="${OUT_DIR}/bundle-${bundle_tag}/${cond_name}/seed-${seed_val}"
  local run_dir="${cond_dir}/run"
  local cfg_path="${cond_dir}/config.toml"
  local run_log="${cond_dir}/run.ndjson"
  local inspect_log="${cond_dir}/inspect-field.ndjson"
  local diag_json="${cond_dir}/diagnostics.json"

  mkdir -p "${cond_dir}"
  generate_variant_toml "${base_preset}" "${cfg_path}" "${seed_val}" "${model_override}" "${smoke_flag}"

  log "Running [bundle-${bundle_tag} / ${cond_name} / seed=${seed_val} / ticks=${horizon_ticks}]"

  local run_rc=0
  if [[ "${#extra_flags[@]}" -gt 0 ]]; then
    "${FLIFE_BIN}" run "${cfg_path}" \
      --ticks "${horizon_ticks}" \
      --every "${metrics_every}" \
      --metrics-every "${metrics_every}" \
      --controllers "${CONTROLLERS}" \
      --cpu-workers "${CPU_WORKERS}" \
      --run-dir "${run_dir}" \
      --checkpoint-every 0 \
      "${extra_flags[@]}" \
      > "${run_log}" || run_rc=$?
  else
    "${FLIFE_BIN}" run "${cfg_path}" \
      --ticks "${horizon_ticks}" \
      --every "${metrics_every}" \
      --metrics-every "${metrics_every}" \
      --controllers "${CONTROLLERS}" \
      --cpu-workers "${CPU_WORKERS}" \
      --run-dir "${run_dir}" \
      --checkpoint-every 0 \
      > "${run_log}" || run_rc=$?
  fi

  # Keep every run (complete=0, early-stop/capacity-pause/fault=1, failed>=2).
  if [[ "${run_rc}" -ne 0 && "${run_rc}" -ne 1 ]]; then
    log "  RECORDED run failure (exit ${run_rc}) for bundle-${bundle_tag}/${cond_name}/seed-${seed_val}"
    python3 - "${bundle_tag}" "${cond_name}" "${MODE}" "${seed_val}" "${run_rc}" "${run_dir}" <<'PY' \
      >> "${OUT_DIR}/stage2_diagnostics.ndjson"
import json, sys
bundle, cond, mode, seed, rc, run_dir = sys.argv[1:7]
print(json.dumps({
    "schema": "frankenlife.v2-stage2-diagnostics.v1",
    "status": "run_failed",
    "bundle": bundle,
    "condition": cond,
    "mode": mode,
    "seed": int(seed),
    "exit_code": int(rc),
    "run_dir": run_dir,
}, separators=(",", ":")))
PY
    return 0
  fi

  local inspect_rc=0
  "${FLIFE_BIN}" inspect "${run_dir}" --field > "${inspect_log}" || inspect_rc=$?
  if [[ "${inspect_rc}" -ne 0 ]]; then
    log "  RECORDED inspect failure (exit ${inspect_rc}) for bundle-${bundle_tag}/${cond_name}/seed-${seed_val}"
    python3 - "${bundle_tag}" "${cond_name}" "${MODE}" "${seed_val}" "${run_rc}" "${inspect_rc}" "${run_dir}" <<'PY' \
      >> "${OUT_DIR}/stage2_diagnostics.ndjson"
import json, sys
bundle, cond, mode, seed, run_rc, insp_rc, run_dir = sys.argv[1:8]
print(json.dumps({
    "schema": "frankenlife.v2-stage2-diagnostics.v1",
    "status": "inspect_failed",
    "bundle": bundle,
    "condition": cond,
    "mode": mode,
    "seed": int(seed),
    "run_exit_code": int(run_rc),
    "inspect_exit_code": int(insp_rc),
    "run_dir": run_dir,
}, separators=(",", ":")))
PY
    return 0
  fi

  python3 "${DIAGNOSTICS_PY}" \
    --inspect "${inspect_log}" \
    --metrics "${run_log}" \
    --bundle "${bundle_tag}" \
    --condition "${cond_name}" \
    --mode "${MODE}" \
    --seed "${seed_val}" \
    --out "${diag_json}"

  python3 - "${diag_json}" "${run_rc}" <<'PY' >> "${OUT_DIR}/stage2_diagnostics.ndjson"
import json, sys
from pathlib import Path
rec = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
rec["run_exit_code"] = int(sys.argv[2])
print(json.dumps(rec, separators=(",", ":")))
PY

  if [[ "${DO_RENDER}" -eq 1 && ( "${cond_name}" == "treatment" || "${cond_name}" == "v1-compat" ) ]]; then
    render_stage2_views "${bundle_tag}" "${cond_name}" "${seed_val}" "${run_dir}"
  fi
}

validate_stage2_matrix

if [[ "${VALIDATE_ONLY}" -eq 1 ]]; then
  log "Validation complete (--validate-only). Artifacts in ${OUT_DIR}."
  exit 0
fi

if [[ "${MODE}" == "smoke" ]]; then
  HORIZON_TICKS="${TICKS_OVERRIDE:-256}"
  METRICS_EVERY=64
  SEEDS_CSV="${SEEDS_OVERRIDE:-42}"
else
  HORIZON_TICKS="${TICKS_OVERRIDE:-4096}"
  METRICS_EVERY=128
  SEEDS_CSV="${SEEDS_OVERRIDE:-42,43,101,102}"
fi

IFS=',' read -r -a SEEDS <<< "${SEEDS_CSV}"

log "Starting V2-17 Stage-2 campaign (mode=${MODE}, bundle=${BUNDLE}, ticks=${HORIZON_TICKS}, seeds=${SEEDS_CSV}, controllers=${CONTROLLERS}, workers=${CPU_WORKERS}, out=${OUT_DIR})"

if [[ "${BUNDLE}" == "a" || "${BUNDLE}" == "both" ]]; then
  for seed_val in "${SEEDS[@]}"; do
    run_one_condition "a" "${BUNDLE_A_PRESET}" "treatment" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}"
    run_one_condition "a" "${BUNDLE_A_PRESET}" "no-epigenetic-notch" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable epigenetic-recurrence,notch-delta
    run_one_condition "a" "${BUNDLE_A_PRESET}" "no-notch-delta" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable notch-delta
    run_one_condition "a" "${BUNDLE_A_PRESET}" "no-trophallaxis" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable trophallaxis
    run_one_condition "a" "${BUNDLE_A_PRESET}" "v1-compat" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --v1-compat
    run_one_condition "a" "${BUNDLE_A_PRESET}" "fixed-controller" "field-ecology-v2-fixed-controller" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}"
    if [[ "${MODE}" == "full" ]]; then
      run_one_condition "a" "${BUNDLE_A_PRESET}" "no-cadherin-4d" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable cadherin-4d
      run_one_condition "a" "${BUNDLE_A_PRESET}" "no-lysis-recycling" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable lysis-recycling
      run_one_condition "a" "${BUNDLE_A_PRESET}" "no-symplectic-waste" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable symplectic-waste
      run_one_condition "a" "${BUNDLE_A_PRESET}" "no-pcp-polarity" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable pcp-polarity
      run_one_condition "a" "${BUNDLE_A_PRESET}" "with-velocity-alignment" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --enable velocity-alignment
    fi
  done
fi

if [[ "${BUNDLE}" == "b" || "${BUNDLE}" == "both" ]]; then
  for seed_val in "${SEEDS[@]}"; do
    run_one_condition "b" "${BUNDLE_B_PRESET}" "treatment" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}"
    run_one_condition "b" "${BUNDLE_B_PRESET}" "no-vascular-field" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable vascular-field
    run_one_condition "b" "${BUNDLE_B_PRESET}" "no-dual-carrier-trails" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable dual-carrier-trails
    run_one_condition "b" "${BUNDLE_B_PRESET}" "v1-compat" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --v1-compat
    run_one_condition "b" "${BUNDLE_B_PRESET}" "fixed-controller" "field-ecology-v2-fixed-controller" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}"
    if [[ "${MODE}" == "full" ]]; then
      run_one_condition "b" "${BUNDLE_B_PRESET}" "no-excitable-relay" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable excitable-relay
      run_one_condition "b" "${BUNDLE_B_PRESET}" "no-oases" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable oases
      run_one_condition "b" "${BUNDLE_B_PRESET}" "no-so2-motor" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable so2-motor
      run_one_condition "b" "${BUNDLE_B_PRESET}" "no-lysis-recycling" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable lysis-recycling
      run_one_condition "b" "${BUNDLE_B_PRESET}" "no-tight-junctions" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --disable tight-junctions
      run_one_condition "b" "${BUNDLE_B_PRESET}" "with-so2-budding-waste" "" "${seed_val}" "${HORIZON_TICKS}" "${METRICS_EVERY}" --enable so2-budding,symplectic-waste
    fi
  done
fi

log "V2-17 Stage-2 campaign finished. Diagnostics ledger: ${OUT_DIR}/stage2_diagnostics.ndjson"
