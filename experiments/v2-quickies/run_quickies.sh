#!/usr/bin/env bash
# Preregistered V2-10 Quickie Verification Campaign Runner
# (Arm 1: Neural Agency H2; Arm 2: Tissue Morphogenesis + H4)
#
# Invokes the real `flife` CLI (`validate`, `experiment`, `run`, `branch`,
# `resume`, `inspect`, optional `render`) with zero mocks and keeps every
# run (complete, extinct, paused, or failed). All run directories, checkpoints,
# inspections, and rendered PNGs live outside git (`--out`).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

MODE="full"
ARM="both"
VALIDATE_ONLY=0
DO_RENDER=0
FLIFE_BIN="${FLIFE_BIN:-}"
OUT_DIR=""
CONTROLLERS="auto"
CPU_WORKERS="auto"

usage() {
  cat <<'EOF'
Usage: experiments/v2-quickies/run_quickies.sh [OPTIONS]

Options:
  --smoke               Fast 64x64 CI mode (256 ticks, 1 discovery + 1 held-out seed)
  --full                Full preregistered 128x128 campaign (default: 4 discovery + 2 held-out)
  --arm 1|2|both        Which quickie arm(s) to run (default: both)
  --validate-only       Validate all V2-10/V2-17 TOMLs and CLI toggle matrices, then exit
  --render              Also render raw + sphere + v2 visual mapping PNGs into $OUT/renders/
  --flife PATH          Path to compiled `flife` binary (default: auto-detect target/{release,debug}/flife)
  --out DIR             Output artifact directory outside git (default: artifacts/v2-quickies/<mode>)
  --controllers ROUTE   Controller backend: auto|cpu|torch|metal (default: auto)
  --cpu-workers N|auto  Worker threads for flife (default: auto)
  -h, --help            Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --smoke)
      MODE="smoke"
      shift
      ;;
    --full)
      MODE="full"
      shift
      ;;
    --arm)
      ARM="${2:?missing value for --arm}"
      shift 2
      ;;
    --validate-only)
      VALIDATE_ONLY=1
      shift
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

case "${ARM}" in
  1|2|both) ;;
  *)
    echo "error: --arm must be 1, 2, or both (got '${ARM}')" >&2
    exit 2
    ;;
esac

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
  OUT_DIR="${REPO_ROOT}/artifacts/v2-quickies/${MODE}"
fi

mkdir -p "${OUT_DIR}"

log() {
  printf '[v2-quickies %s] %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

validate_all_configs() {
  log "Validating V2-10 quickie TOMLs and V2-17 showcase presets with ${FLIFE_BIN}"
  local configs=(
    "${SCRIPT_DIR}/arm1-neural-agency-128.toml"
    "${SCRIPT_DIR}/arm1-neural-agency-smoke64.toml"
    "${SCRIPT_DIR}/arm2-tissue-morphogenesis-128.toml"
    "${SCRIPT_DIR}/arm2-tissue-morphogenesis-smoke64.toml"
    "${REPO_ROOT}/presets/morphogenesis-tissue-v2.toml"
    "${REPO_ROOT}/presets/supercolony-v2.toml"
  )
  local cfg
  for cfg in "${configs[@]}"; do
    "${FLIFE_BIN}" validate "${cfg}" > "${OUT_DIR}/validate-$(basename "${cfg}" .toml).json"
    log "  OK validate $(basename "${cfg}")"
  done

  # Verify exact CLI --disable / --enable / --v1-compat toggle chains.
  "${FLIFE_BIN}" validate "${SCRIPT_DIR}/arm1-neural-agency-128.toml" \
    --disable all-v2 --enable oases,senescence,so2-motor >/dev/null
  "${FLIFE_BIN}" validate "${SCRIPT_DIR}/arm1-neural-agency-128.toml" \
    --disable oases >/dev/null
  "${FLIFE_BIN}" validate "${SCRIPT_DIR}/arm1-neural-agency-128.toml" \
    --v1-compat >/dev/null

  "${FLIFE_BIN}" validate "${SCRIPT_DIR}/arm2-tissue-morphogenesis-128.toml" \
    --disable all-v2 --enable pcp-polarity,hippo-crowding,tight-junctions >/dev/null
  "${FLIFE_BIN}" validate "${SCRIPT_DIR}/arm2-tissue-morphogenesis-128.toml" \
    --v1-compat >/dev/null
  "${FLIFE_BIN}" validate "${SCRIPT_DIR}/arm2-tissue-morphogenesis-128.toml" \
    --disable hippo-crowding >/dev/null
  "${FLIFE_BIN}" validate "${SCRIPT_DIR}/arm2-tissue-morphogenesis-128.toml" \
    --disable tight-junctions >/dev/null

  "${FLIFE_BIN}" validate "${REPO_ROOT}/presets/morphogenesis-tissue-v2.toml" \
    --disable pcp-polarity >/dev/null
  "${FLIFE_BIN}" validate "${REPO_ROOT}/presets/morphogenesis-tissue-v2.toml" \
    --disable epigenetic-recurrence,notch-delta >/dev/null
  "${FLIFE_BIN}" validate "${REPO_ROOT}/presets/morphogenesis-tissue-v2.toml" \
    --v1-compat >/dev/null

  "${FLIFE_BIN}" validate "${REPO_ROOT}/presets/supercolony-v2.toml" \
    --disable vascular-field >/dev/null
  "${FLIFE_BIN}" validate "${REPO_ROOT}/presets/supercolony-v2.toml" \
    --disable dual-carrier-trails >/dev/null
  "${FLIFE_BIN}" validate "${REPO_ROOT}/presets/supercolony-v2.toml" \
    --enable so2-budding,symplectic-waste >/dev/null
  "${FLIFE_BIN}" validate "${REPO_ROOT}/presets/supercolony-v2.toml" \
    --v1-compat >/dev/null

  log "All 6 TOML configurations and 14 CLI toggle matrices validated cleanly."
}

# Compute area-matched boundary / interior / distant removal disks from a real
# `flife inspect --field` NDJSON dump using Python stdlib only (zero mocks,
# zero third-party packages). Emits one JSON line with disk coordinates and
# basic pre-lesion spatial geometry (stacking depth, coordination, hexatic |psi_6|).
plan_h4_lesion_disks() {
  local inspect_ndjson="$1"
  local disk_radius="$2"
  local body_diameter="$3"
  python3 - "$inspect_ndjson" "$disk_radius" "$body_diameter" <<'PY'
import cmath, json, math, sys
from collections import deque

path, radius, diam = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
header = None
organisms = []
field = None
with open(path, "r", encoding="utf-8") as fh:
    for line in fh:
        line = line.strip()
        if not line:
            continue
        rec = json.loads(line)
        kind = rec.get("record")
        if kind == "inspect":
            header = rec
        elif kind == "organism":
            organisms.append(rec)
        elif kind == "field":
            field = rec

if header is None:
    print(json.dumps({"status": "missing_header"}))
    sys.exit(0)

w = float(header["world"]["width"])
h = float(header["world"]["height"])
nx, ny = int(header["world"]["cells"][0]), int(header["world"]["cells"][1])
hx, hy = w / nx, h / ny

def min_img(dx, span):
    if dx > 0.5 * span:
        dx -= span
    elif dx < -0.5 * span:
        dx += span
    return dx

if not organisms:
    print(json.dumps({"status": "extinct", "live": 0}))
    sys.exit(0)

# Bin organisms into field cells and locate largest periodic 4-connected cluster
# above mean occupied density (strictly deterministic tie-breaking).
counts = [0] * (nx * ny)
for org in organisms:
    cx = int(math.floor(org["position"][0] / hx)) % nx
    cy = int(math.floor(org["position"][1] / hy)) % ny
    counts[cy * nx + cx] += 1

mean_c = sum(counts) / float(nx * ny)
thresh = max(1.0, 2.0 * mean_c)
dense = [c >= thresh for c in counts]
if not any(dense):
    dense = [c > 0 for c in counts]

visited = [False] * (nx * ny)
components = []
for start in range(nx * ny):
    if not dense[start] or visited[start]:
        continue
    q = deque([start])
    visited[start] = True
    cells = []
    mass = 0
    while q:
        idx = q.popleft()
        cells.append(idx)
        mass += counts[idx]
        x, y = idx % nx, idx // nx
        for ox, oy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            nb = ((y + oy) % ny) * nx + ((x + ox) % nx)
            if dense[nb] and not visited[nb]:
                visited[nb] = True
                q.append(nb)
    components.append((mass, cells))

components.sort(key=lambda item: (-item[0], min(item[1])))
focal_cells = set(components[0][1])

# Boundary cells: focal cells 4-adjacent to at least one non-focal cell.
boundary_candidates = []
for idx in focal_cells:
    x, y = idx % nx, idx // nx
    if any((((y + oy) % ny) * nx + ((x + ox) % nx)) not in focal_cells for ox, oy in ((1, 0), (-1, 0), (0, 1), (0, -1))):
        boundary_candidates.append(idx)

# Multi-source BFS inward from boundary cells so `interior_idx` lands strictly
# at maximum depth inside the focal colony (never colliding with `boundary_idx`
# when the colony has >= 2 cells).
depth = {}
dq = deque()
for b_idx in sorted(boundary_candidates):
    depth[b_idx] = 1
    dq.append(b_idx)
while dq:
    cur = dq.popleft()
    cx, cy = cur % nx, cur // nx
    next_d = depth[cur] + 1
    for ox, oy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        nb = ((cy + oy) % ny) * nx + ((cx + ox) % nx)
        if nb in focal_cells and nb not in depth:
            depth[nb] = next_d
            dq.append(nb)

interior_idx = min(
    focal_cells,
    key=lambda idx: (-depth.get(idx, 0), -counts[idx], idx // nx, idx % nx),
)
ix, iy = (interior_idx % nx + 0.5) * hx, (interior_idx // nx + 0.5) * hy

def cell_dist_to_interior(idx):
    px = (idx % nx + 0.5) * hx
    py = (idx // nx + 0.5) * hy
    return math.hypot(min_img(px - ix, w), min_img(py - iy, h))

distinct_boundary = [idx for idx in (boundary_candidates or list(focal_cells)) if idx != interior_idx]
if not distinct_boundary:
    boundary_idx = interior_idx
else:
    far_boundary = [idx for idx in distinct_boundary if cell_dist_to_interior(idx) >= 0.5 * radius]
    pool = far_boundary or distinct_boundary
    boundary_idx = min(pool, key=lambda idx: (-counts[idx], -cell_dist_to_interior(idx), idx // nx, idx % nx))

# Distant control disk center: diametrically opposite interior peak on the torus.
bx, by = (boundary_idx % nx + 0.5) * hx, (boundary_idx // nx + 0.5) * hy
dx_c, dy_c = (ix + 0.5 * w) % w, (iy + 0.5 * h) % h

# Geometry diagnostics on up to 2048 deterministic stride-sampled organisms (O(N) spatial hash).
cell_size = max(1.3 * diam, 1e-6)
gx = max(1, int(math.floor(w / cell_size)))
gy = max(1, int(math.floor(h / cell_size)))
cw, ch = w / gx, h / gy
grid = [[] for _ in range(gx * gy)]
positions = [org["position"] for org in organisms]
for i, (px, py) in enumerate(positions):
    bx_g = int(math.floor(px / cw)) % gx
    by_g = int(math.floor(py / ch)) % gy
    grid[by_g * gx + bx_g].append(i)

r_coord = 1.3 * diam
r_coord_sq = r_coord * r_coord
r_overlap_sq = diam * diam
coord_hist = [0] * 9
# Hexatic order uses denominator max(z, 6) so a z=2 1-cell chain/ring cannot
# masquerade as a z=6 crystal via even-harmonic (e^{6i*pi} = +1) addition:
# perfect hex crystal -> mean_abs_psi6 = 1.0, global_abs_mean_psi6 = 1.0;
# 24-cell ring -> P(z=2) = 1.0, mean_abs_psi6 = 0.236 < 0.25, global = 0.0.
psi6_abs_sum = 0.0
psi6_vec_sum = 0j
overlap_sum = 0
d1_over_d = []
for i, (px, py) in enumerate(positions):
    bx_g = int(math.floor(px / cw)) % gx
    by_g = int(math.floor(py / ch)) % gy
    z = 0
    overlaps = 0
    acc = 0j
    min_d2 = float("inf")
    for oy in (-1, 0, 1):
        row = ((by_g + oy) % gy) * gx
        for ox in (-1, 0, 1):
            for j in grid[row + ((bx_g + ox) % gx)]:
                if j == i:
                    continue
                ddx = min_img(positions[j][0] - px, w)
                ddy = min_img(positions[j][1] - py, h)
                d2 = ddx * ddx + ddy * ddy
                if d2 < min_d2:
                    min_d2 = d2
                if d2 <= r_coord_sq:
                    z += 1
                    if d2 > 0.0:
                        acc += cmath.exp(6j * math.atan2(ddy, ddx))
                if d2 < r_overlap_sq:
                    overlaps += 1
    coord_hist[min(z, 8)] += 1
    overlap_sum += overlaps
    if math.isfinite(min_d2):
        d1_over_d.append(math.sqrt(min_d2) / diam)
    if z > 0:
        psi6_i = acc / float(max(z, 6))
        psi6_abs_sum += abs(psi6_i)
        psi6_vec_sum += psi6_i

n_org = len(organisms)
d1_over_d.sort()
median_d1 = d1_over_d[len(d1_over_d) // 2] if d1_over_d else None
print(json.dumps({
    "status": "ok",
    "live": n_org,
    "digest": header["digest"],
    "focal_cells": len(focal_cells),
    "radius": radius,
    "boundary": [round(bx, 4), round(by, 4)],
    "interior": [round(ix, 4), round(iy, 4)],
    "distant": [round(dx_c, 4), round(dy_c, 4)],
    "median_d1_over_D": median_d1,
    "mean_overlaps_within_1D": overlap_sum / float(n_org),
    "mean_abs_psi6": psi6_abs_sum / float(n_org),
    "global_abs_mean_psi6": abs(psi6_vec_sum) / float(n_org),
    "chain_fraction_z1_z2": (coord_hist[1] + coord_hist[2]) / float(n_org),
    "coord_hist_0_to_8": [c / float(n_org) for c in coord_hist],
}))
PY
}

# Summarize post-branch wound-rim vs interior proliferation and field delta
# from real `flife inspect --field` NDJSON dumps (zero mocks, stdlib Python).
summarize_branch_assay() {
  local label="$1"
  local fork_tick="$2"
  local horizon_tick="$3"
  local plan_json="$4"
  local ctrl_ndjson="$5"
  local bnd_ndjson="$6"
  local int_ndjson="$7"
  local dst_ndjson="$8"
  python3 - "$label" "$fork_tick" "$horizon_tick" "$plan_json" \
    "$ctrl_ndjson" "$bnd_ndjson" "$int_ndjson" "$dst_ndjson" <<'PY'
import json, math, sys

label, fork_tick, horizon_tick = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
plan = json.loads(sys.argv[4])
paths = {"control": sys.argv[5], "boundary": sys.argv[6], "interior": sys.argv[7], "distant": sys.argv[8]}

def load(path):
    hdr, orgs, fld = None, [], None
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            rec = json.loads(line)
            k = rec.get("record")
            if k == "inspect":
                hdr = rec
            elif k == "organism":
                orgs.append(rec)
            elif k == "field":
                fld = rec
    return hdr, orgs, fld

loaded = {name: load(p) for name, p in paths.items()}
ctrl_hdr, ctrl_orgs, ctrl_fld = loaded["control"]
w = float(ctrl_hdr["world"]["width"])
h = float(ctrl_hdr["world"]["height"])
radius = float(plan["radius"])

def min_img(dx, span):
    if dx > 0.5 * span:
        dx -= span
    elif dx < -0.5 * span:
        dx += span
    return dx

def dist(a, b):
    return math.hypot(min_img(a[0] - b[0], w), min_img(a[1] - b[1], h))

def field_l1_delta(a_fld, b_fld):
    if a_fld is None or b_fld is None:
        return None
    return sum(abs(x - y) for x, y in zip(a_fld["energy"], b_fld["energy"]))

# Wound-rim births in interior-lesion branch: births occurring within annulus
# [0.8 R, 2.0 R] of wound center vs outside > 2.0 R, normalized by surviving cells.
int_hdr, int_orgs, int_fld = loaded["interior"]
bnd_hdr, bnd_orgs, bnd_fld = loaded["boundary"]
dst_hdr, dst_orgs, dst_fld = loaded["distant"]

rim_births, rim_total = 0, 0
far_births, far_total = 0, 0
ix, iy = plan["interior"]
for org in int_orgs:
    d = dist(org["position"], (ix, iy))
    born_after = int(org["birth_tick"]) > fork_tick
    if 0.8 * radius <= d <= 2.0 * radius:
        rim_total += 1
        if born_after:
            rim_births += 1
    elif d > 2.0 * radius:
        far_total += 1
        if born_after:
            far_births += 1

rim_rate = (rim_births / float(rim_total)) if rim_total > 0 else 0.0
far_rate = (far_births / float(far_total)) if far_total > 0 else 0.0
delta_bnd = field_l1_delta(bnd_fld, ctrl_fld)
delta_int = field_l1_delta(int_fld, ctrl_fld)
delta_dst = field_l1_delta(dst_fld, ctrl_fld)
h4_ratio = (delta_bnd / delta_int) if (delta_bnd is not None and delta_int is not None and delta_int > 1e-12) else None

print(json.dumps({
    "assay": label,
    "fork_tick": fork_tick,
    "horizon_tick": horizon_tick,
    "pre_lesion": plan,
    "live_at_horizon": {k: int(v[0]["live"]) for k, v in loaded.items()},
    "digests": {k: v[0]["digest"] for k, v in loaded.items()},
    "wound_rim_birth_rate": rim_rate,
    "non_rim_birth_rate": far_rate,
    "wound_rim_to_far_ratio": (rim_rate / far_rate) if far_rate > 1e-12 else None,
    "field_l1_delta_boundary_vs_control": delta_bnd,
    "field_l1_delta_interior_vs_control": delta_int,
    "field_l1_delta_distant_vs_control": delta_dst,
    "h4_boundary_to_interior_field_delta_ratio": h4_ratio,
}))
PY
}

run_arm2_branch_assay() {
  local label="$1"
  local cfg="$2"
  local fork_tick="$3"
  local horizon_tick="$4"
  local disk_radius="$5"
  shift 5
  local extra_flags=("$@")

  local assay_dir="${OUT_DIR}/arm2-assays/${label}"
  local parent_dir="${assay_dir}/parent"
  mkdir -p "${assay_dir}"

  log "Arm 2 branch assay [${label}]: parent run to tick ${fork_tick} (flags: ${extra_flags[*]:-none})"
  local run_rc=0
  "${FLIFE_BIN}" run "${cfg}" \
    --ticks "${fork_tick}" \
    --every "${fork_tick}" \
    --metrics-every "${fork_tick}" \
    --controllers "${CONTROLLERS}" \
    --cpu-workers "${CPU_WORKERS}" \
    --run-dir "${parent_dir}" \
    --checkpoint-every "${fork_tick}" \
    "${extra_flags[@]}" \
    > "${assay_dir}/parent-run.ndjson" || run_rc=$?

  if [[ "${run_rc}" -ne 0 && "${run_rc}" -ne 1 ]]; then
    log "  WARNING: parent run [${label}] exited with code ${run_rc} (recorded, not dropped)"
    printf '{"assay":"%s","status":"parent_failed","exit_code":%d}\n' "${label}" "${run_rc}" \
      >> "${OUT_DIR}/assay_summary.ndjson"
    return 0
  fi

  local parent_inspect="${assay_dir}/parent-inspect-t${fork_tick}.ndjson"
  "${FLIFE_BIN}" inspect "${parent_dir}" --at "${fork_tick}" --field > "${parent_inspect}"
  local plan_json
  plan_json="$(plan_h4_lesion_disks "${parent_inspect}" "${disk_radius}" 0.25)"
  local plan_status
  plan_status="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("status","error"))' "${plan_json}")"
  if [[ "${plan_status}" != "ok" ]]; then
    log "  WARNING: [${label}] pre-lesion status=${plan_status} (kept in assay_summary.ndjson)"
    printf '{"assay":"%s","status":"%s","pre_lesion":%s}\n' "${label}" "${plan_status}" "${plan_json}" \
      >> "${OUT_DIR}/assay_summary.ndjson"
    return 0
  fi

  local bxy ixy dxy
  bxy="$(python3 -c 'import json,sys; p=json.loads(sys.argv[1])["boundary"]; print(f"{p[0]},{p[1]}")' "${plan_json}")"
  ixy="$(python3 -c 'import json,sys; p=json.loads(sys.argv[1])["interior"]; print(f"{p[0]},{p[1]}")' "${plan_json}")"
  dxy="$(python3 -c 'import json,sys; p=json.loads(sys.argv[1])["distant"]; print(f"{p[0]},{p[1]}")' "${plan_json}")"

  log "  Branching [${label}] at tick ${fork_tick}: boundary=${bxy}, interior=${ixy}, distant=${dxy}, R=${disk_radius}"
  "${FLIFE_BIN}" branch "${parent_dir}" "${assay_dir}/branch-control" \
    --at "${fork_tick}" --name "${label}-control" > "${assay_dir}/branch-control.json"
  "${FLIFE_BIN}" branch "${parent_dir}" "${assay_dir}/branch-boundary" \
    --at "${fork_tick}" --name "${label}-boundary" \
    --remove "${fork_tick}:${bxy}:${disk_radius}:remove" > "${assay_dir}/branch-boundary.json"
  "${FLIFE_BIN}" branch "${parent_dir}" "${assay_dir}/branch-interior" \
    --at "${fork_tick}" --name "${label}-interior" \
    --remove "${fork_tick}:${ixy}:${disk_radius}:remove" > "${assay_dir}/branch-interior.json"
  "${FLIFE_BIN}" branch "${parent_dir}" "${assay_dir}/branch-distant" \
    --at "${fork_tick}" --name "${label}-distant" \
    --remove "${fork_tick}:${dxy}:${disk_radius}:remove" > "${assay_dir}/branch-distant.json"

  local branch_name
  for branch_name in control boundary interior distant; do
    local bdir="${assay_dir}/branch-${branch_name}"
    local brc=0
    "${FLIFE_BIN}" resume "${bdir}" \
      --until "${horizon_tick}" \
      --every "${horizon_tick}" \
      --metrics-every "${horizon_tick}" \
      --controllers "${CONTROLLERS}" \
      --cpu-workers "${CPU_WORKERS}" \
      --checkpoint-every "${horizon_tick}" \
      > "${assay_dir}/resume-${branch_name}.ndjson" || brc=$?
    if [[ "${brc}" -ne 0 && "${brc}" -ne 1 ]]; then
      log "  WARNING: branch ${label}/${branch_name} exited with ${brc}"
      printf '{"assay":"%s","branch":"%s","status":"resume_failed","exit_code":%d}\n' \
        "${label}" "${branch_name}" "${brc}" >> "${OUT_DIR}/assay_summary.ndjson"
      return 0
    fi
    "${FLIFE_BIN}" inspect "${bdir}" --at "${horizon_tick}" --field \
      > "${assay_dir}/inspect-${branch_name}-t${horizon_tick}.ndjson"
  done

  local summary
  summary="$(summarize_branch_assay \
    "${label}" "${fork_tick}" "${horizon_tick}" "${plan_json}" \
    "${assay_dir}/inspect-control-t${horizon_tick}.ndjson" \
    "${assay_dir}/inspect-boundary-t${horizon_tick}.ndjson" \
    "${assay_dir}/inspect-interior-t${horizon_tick}.ndjson" \
    "${assay_dir}/inspect-distant-t${horizon_tick}.ndjson")"
  printf '%s\n' "${summary}" >> "${OUT_DIR}/assay_summary.ndjson"
  log "  Recorded assay summary for [${label}]: ${summary}"

  if [[ "${DO_RENDER}" -eq 1 ]]; then
    local render_dir="${OUT_DIR}/renders/${label}"
    mkdir -p "${render_dir}/raw" "${render_dir}/sphere"
    "${FLIFE_BIN}" render --run-dir "${assay_dir}/branch-control" \
      --ticks 0 --every 1 --width 640 --height 640 \
      --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
      --look raw --color polarity --out "${render_dir}/raw" \
      > "${render_dir}/render-raw.ndjson"
    "${FLIFE_BIN}" render --run-dir "${assay_dir}/branch-control" \
      --ticks 0 --every 1 --width 640 --height 640 \
      --controllers "${CONTROLLERS}" --cpu-workers "${CPU_WORKERS}" \
      --look sphere --color polarity --out "${render_dir}/sphere" \
      > "${render_dir}/render-sphere.ndjson"
  fi
}

validate_all_configs

if [[ "${VALIDATE_ONLY}" -eq 1 ]]; then
  exit 0
fi

log "Starting V2-10 Quickies (mode=${MODE}, arm=${ARM}, controllers=${CONTROLLERS}, workers=${CPU_WORKERS}, out=${OUT_DIR})"

if [[ "${ARM}" == "1" || "${ARM}" == "both" ]]; then
  if [[ "${MODE}" == "smoke" ]]; then
    ARM1_SPEC="${SCRIPT_DIR}/arm1/spec-smoke.toml"
  else
    ARM1_SPEC="${SCRIPT_DIR}/arm1/spec.toml"
  fi
  log "Running Arm 1 (Neural Agency H2): ${ARM1_SPEC}"
  "${FLIFE_BIN}" experiment "${ARM1_SPEC}" \
    --out "${OUT_DIR}/arm1" \
    --controllers "${CONTROLLERS}" \
    --cpu-workers "${CPU_WORKERS}" \
    > "${OUT_DIR}/arm1-records.ndjson"
  log "Arm 1 experiment complete -> ${OUT_DIR}/arm1/report.json"
fi

if [[ "${ARM}" == "2" || "${ARM}" == "both" ]]; then
  if [[ "${MODE}" == "smoke" ]]; then
    ARM2_SPEC="${SCRIPT_DIR}/arm2/spec-smoke.toml"
    ARM2_CFG="${SCRIPT_DIR}/arm2-tissue-morphogenesis-smoke64.toml"
    FORK_TICK=128
    HORIZON_TICK=192
    DISK_RADIUS=1.5
  else
    ARM2_SPEC="${SCRIPT_DIR}/arm2/spec.toml"
    ARM2_CFG="${SCRIPT_DIR}/arm2-tissue-morphogenesis-128.toml"
    FORK_TICK=2048
    HORIZON_TICK=2304
    DISK_RADIUS=2.5
  fi

  log "Running Arm 2 (Tissue Morphogenesis + H4): ${ARM2_SPEC}"
  "${FLIFE_BIN}" experiment "${ARM2_SPEC}" \
    --out "${OUT_DIR}/arm2" \
    --controllers "${CONTROLLERS}" \
    --cpu-workers "${CPU_WORKERS}" \
    > "${OUT_DIR}/arm2-records.ndjson"
  log "Arm 2 experiment complete -> ${OUT_DIR}/arm2/report.json"

  # Real flife branch --remove wound-punch & H4 boundary-vs-interior lesion assays
  # across treatment, --v1-compat, and --disable tight-junctions.
  run_arm2_branch_assay "treatment" "${ARM2_CFG}" "${FORK_TICK}" "${HORIZON_TICK}" "${DISK_RADIUS}"
  run_arm2_branch_assay "v1-compat" "${ARM2_CFG}" "${FORK_TICK}" "${HORIZON_TICK}" "${DISK_RADIUS}" --v1-compat
  run_arm2_branch_assay "no-tight-junctions" "${ARM2_CFG}" "${FORK_TICK}" "${HORIZON_TICK}" "${DISK_RADIUS}" --disable tight-junctions
fi

log "V2-10 Quickies finished cleanly. Artifacts live in ${OUT_DIR} (outside git)."
