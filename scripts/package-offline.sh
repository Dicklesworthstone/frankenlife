#!/usr/bin/env bash
# Package an offline FrankenLife native macOS app bundle + CLI + validated presets +
# license/stack notices + live capability report, and verify fresh-install launch and
# explicit capability refusals in a sanitized offline environment (plan §20.5, FL-051).
#
# Usage:
#   scripts/package-offline.sh --out DIR [OPTIONS]
#   scripts/package-offline.sh --verify-only DIR
#
# Options:
#   --out DIR           Destination directory outside the repository (must not exist or must be empty)
#   --verify-only DIR   Verify an already-staged offline package directory without modifying it
#   --flife PATH        Path to compiled release `flife` CLI binary
#   --studio PATH       Path to compiled release `flife-studio` binary (macOS native app)
#   --allow-cli-only    Permit packaging without `flife-studio` (e.g. headless Linux bundle)
#   --skip-smoke        Stage the bundle without running fresh-install verification (recorded honestly)
#   --tarball           Also write `<out>.tar.gz` beside `<out>`
#   -h, --help          Show this usage message
#
# Honesty contract (AGENTS.md, plan §20.5, FL-051):
# - Never ships a stub .app bundle or placeholder binary. On macOS, missing `flife-studio`
#   is refused unless `--allow-cli-only` is passed explicitly.
# - Fresh-install verification runs in `env -i PATH=/usr/bin:/bin HOME=<isolated>` outside
#   the repository checkout so no Cargo/Rustup/repo path leaks into the test.
# - Explicit capability refusals tested directly:
#     1. `flife render --video` without ffmpeg on PATH fails with `frankenlife.error.v1`
#        before writing any frame PNG.
#     2. Unknown V2 feature override (`--disable nonexistent-v2-feature`) fails with typed
#        `frankenlife.error.v1` JSON on stderr.
#     3. V2-only visual mapping (`--color cell-state`) on a V1 preset fails with typed
#        `frankenlife.error.v1` JSON on stderr before writing any frame PNG.
#     4. On non-macOS hosts, `--controllers metal` fails with explicit missing-capability refusal.
#     5. On macOS with FrankenLife Studio.app, `launch-studio` verifies offline `--offscreen`
#        rendering on its bundled preset and explicit `frankenlife.error.v1` refusal on `--color cell-state`.
# - Never deletes user files or modifies repository tracked sources.

set -euo pipefail

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
REPO_ROOT="$(cd "$(dirname "${SCRIPT_PATH}")/.." && pwd)"

OUT_DIR=""
VERIFY_ONLY=""
FLIFE_BIN=""
STUDIO_BIN=""
ALLOW_CLI_ONLY=0
SKIP_SMOKE=0
MAKE_TARBALL=0

usage() {
  sed -n '2,22p' "${SCRIPT_PATH}" | sed 's/^# \{0,1\}//'
}

die() {
  printf 'package-offline: %s\n' "$*" >&2
  exit 2
}

log() {
  printf '[package-offline] %s\n' "$*"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      OUT_DIR="${2:?missing value for --out}"
      shift 2
      ;;
    --verify-only)
      VERIFY_ONLY="${2:?missing value for --verify-only}"
      shift 2
      ;;
    --flife)
      FLIFE_BIN="${2:?missing value for --flife}"
      shift 2
      ;;
    --studio)
      STUDIO_BIN="${2:?missing value for --studio}"
      shift 2
      ;;
    --allow-cli-only)
      ALLOW_CLI_ONLY=1
      shift
      ;;
    --skip-smoke)
      SKIP_SMOKE=1
      shift
      ;;
    --tarball)
      MAKE_TARBALL=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option '$1' (run with --help)"
      ;;
  esac
done

if [[ -n "${VERIFY_ONLY}" && -n "${OUT_DIR}" ]]; then
  die "pass either --out DIR or --verify-only DIR, not both"
fi
if [[ -z "${VERIFY_ONLY}" && -z "${OUT_DIR}" ]]; then
  die "either --out DIR or --verify-only DIR is required (run with --help)"
fi

# Portable SHA-256 helper (macOS shasum or Linux sha256sum).
sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$1"
  fi
}

# Verify MANIFEST.json SHA-256 digests and capabilities_summary consistency.
verify_manifest() {
  local bundle_dir="$1"
  local expected_smoke="${2:-any}"
  python3 - "${bundle_dir}" "${expected_smoke}" <<'PY'
import hashlib, json, os, sys
root, expected_smoke = sys.argv[1], sys.argv[2]
manifest = json.load(open(os.path.join(root, "MANIFEST.json"), "r", encoding="utf-8"))
if manifest.get("schema") != "frankenlife.offline-package.v1":
    sys.exit(f"unexpected manifest schema: {manifest.get('schema')}")
if expected_smoke == "true" and manifest.get("smoke_verified") is not True:
    sys.exit(f"expected smoke_verified=true, got {manifest.get('smoke_verified')!r}")
if expected_smoke == "false" and manifest.get("smoke_verified") is not False:
    sys.exit(f"expected smoke_verified=false, got {manifest.get('smoke_verified')!r}")

caps = json.load(open(os.path.join(root, "capabilities.json"), "r", encoding="utf-8"))
summary = manifest.get("capabilities_summary", {})
pinned = caps.get("world", {}).get("pinned_digest")
cpu_route = next((r for r in caps.get("controllers", []) if r.get("route") == "cpu"), {})
verified_routes = [
    r.get("route") for r in caps.get("controllers", []) if r.get("status") == "verified"
]
if summary.get("schema") != "frankenlife.capabilities.v1":
    sys.exit(f"bad capabilities_summary.schema: {summary.get('schema')!r}")
if not isinstance(summary.get("preset_digest"), str) or summary.get("preset_digest") != pinned:
    sys.exit(f"preset_digest mismatch: manifest={summary.get('preset_digest')!r}, caps={pinned!r}")
if not summary["preset_digest"].startswith("82e0720d"):
    sys.exit(f"unexpected preset_digest: {summary['preset_digest']!r}")
if summary.get("verified_cpu_digest") != cpu_route.get("digest"):
    sys.exit(
        f"verified_cpu_digest mismatch: manifest={summary.get('verified_cpu_digest')!r}, "
        f"caps={cpu_route.get('digest')!r}"
    )
if summary.get("verified_routes") != verified_routes or "cpu" not in verified_routes:
    sys.exit(f"verified_routes mismatch: manifest={summary.get('verified_routes')!r}, caps={verified_routes!r}")
expected_auto = caps.get("auto", {}).get("takes")
if not isinstance(summary.get("auto_route"), str) or summary.get("auto_route") != expected_auto:
    sys.exit(f"auto_route mismatch: manifest={summary.get('auto_route')!r}, caps={expected_auto!r}")
if summary.get("metal_device") != caps.get("auto", {}).get("metal_device"):
    sys.exit(
        f"metal_device mismatch: manifest={summary.get('metal_device')!r}, "
        f"caps={caps.get('auto', {}).get('metal_device')!r}"
    )
expected_video = caps.get("video", {}).get("status")
if summary.get("video_encoder") not in ("available", "unavailable") or summary.get("video_encoder") != expected_video:
    sys.exit(f"video_encoder mismatch: manifest={summary.get('video_encoder')!r}, caps={expected_video!r}")

files = manifest.get("files", [])
if not files:
    sys.exit("MANIFEST.json lists no files")
for entry in files:
    rel = entry["path"]
    full = os.path.join(root, rel)
    if not os.path.isfile(full):
        sys.exit(f"missing packaged file: {rel}")
    data = open(full, "rb").read()
    digest = hashlib.sha256(data).hexdigest()
    if len(data) != entry["bytes"] or digest != entry["sha256"]:
        sys.exit(f"digest/size mismatch for {rel}: expected {entry['sha256']} ({entry['bytes']} B), got {digest} ({len(data)} B)")
print(
    f"MANIFEST verified: {len(files)} files match SHA-256; "
    f"auto={summary['auto_route']}, verified={','.join(verified_routes)}, "
    f"smoke_verified={manifest.get('smoke_verified')}"
)
PY
}

# Verify a staged offline bundle in an isolated fresh-install environment (env -i).
verify_bundle() {
  local bundle_dir="$1"
  bundle_dir="$(cd "${bundle_dir}" 2>/dev/null && pwd)" || die "bundle directory '${bundle_dir}' does not exist"

  local flife="${bundle_dir}/bin/flife"
  [[ -x "${flife}" ]] || die "missing executable CLI binary at ${flife}"
  [[ -f "${bundle_dir}/MANIFEST.json" ]] || die "missing ${bundle_dir}/MANIFEST.json"
  [[ -f "${bundle_dir}/capabilities.json" ]] || die "missing ${bundle_dir}/capabilities.json"
  [[ -f "${bundle_dir}/features.json" ]] || die "missing ${bundle_dir}/features.json"
  [[ -f "${bundle_dir}/notices/LICENSE" ]] || die "missing ${bundle_dir}/notices/LICENSE"
  [[ -f "${bundle_dir}/notices/franken-stack.lock" ]] || die "missing ${bundle_dir}/notices/franken-stack.lock"
  [[ -f "${bundle_dir}/presets/reference-small-world.toml" ]] || die "missing bundled presets/reference-small-world.toml"

  # 1. Cryptographically verify every file listed in MANIFEST.json and its capability summary.
  log "Verifying MANIFEST.json SHA-256 digests and capability summary..."
  verify_manifest "${bundle_dir}" "any"

  # 2. Fresh-install sanitized environment (no Cargo, Rustup, repo cwd, or Homebrew PATH).
  local smoke_root
  smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/flife-offline-smoke.XXXXXX")"
  local smoke_home="${smoke_root}/home"
  local empty_path="${smoke_root}/empty-bin"
  mkdir -p "${smoke_home}" "${empty_path}"

  # Cleanly remove only the ephemeral mktemp smoke directory on exit (never repo or bundle files).
  cleanup_smoke() {
    if [[ -n "${smoke_root:-}" && "${smoke_root}" == *"/flife-offline-smoke."* ]]; then
      rm -rf "${smoke_root}"
    fi
  }
  trap cleanup_smoke EXIT

  log "Running offline fresh-install check #1: flife capabilities (env -i, outside repo)..."
  local caps_out="${smoke_root}/capabilities.json"
  (
    cd "${smoke_home}"
    env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" TMPDIR="${smoke_root}" \
      "${flife}" capabilities > "${caps_out}"
  )
  python3 - "${caps_out}" <<'PY'
import json, sys
rec = json.load(open(sys.argv[1], "r", encoding="utf-8"))
assert rec.get("schema") == "frankenlife.capabilities.v1", f"bad schema: {rec.get('schema')}"
world = rec.get("world", {})
pinned = world.get("pinned_digest", "")
assert pinned.startswith("82e0720d"), f"unexpected preset pinned_digest: {pinned!r}"
controllers = rec.get("controllers", [])
cpu_ok = any(
    r.get("route") == "cpu"
    and r.get("status") == "verified"
    and r.get("digest") == pinned
    for r in controllers
)
assert cpu_ok, f"cpu route not verified against pinned_digest: {controllers}"
verified_routes = [r.get("route") for r in controllers if r.get("status") == "verified"]
build = rec.get("build", {})
print(
    f"capabilities OK: host={rec.get('host', {}).get('os')}/{build.get('arch')}, "
    f"digest={pinned[:12]}..., verified={','.join(verified_routes)}"
)
PY

  log "Running offline fresh-install check #2: flife --list-features..."
  local feat_out="${smoke_root}/features.json"
  (
    cd "${smoke_home}"
    env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" \
      "${flife}" --list-features > "${feat_out}"
  )
  python3 - "${feat_out}" <<'PY'
import json, sys
rec = json.load(open(sys.argv[1], "r", encoding="utf-8"))
assert rec.get("schema") == "frankenlife.features.v1", f"bad schema: {rec.get('schema')}"
feats = rec.get("features", [])
assert len(feats) == 21, f"expected 21 v2 features, got {len(feats)}"
print("features OK: 21 registered field-ecology-v2 slugs")
PY

  log "Running offline fresh-install check #3: validate every bundled preset..."
  local preset
  local preset_count=0
  for preset in "${bundle_dir}"/presets/*.toml; do
    [[ -f "${preset}" ]] || continue
    (
      cd "${smoke_home}"
      env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" \
        "${flife}" validate "${preset}" >/dev/null
    )
    preset_count=$((preset_count + 1))
  done
  [[ "${preset_count}" -gt 0 ]] || die "no presets found in ${bundle_dir}/presets/"
  log "Validated ${preset_count} bundled presets offline."

  log "Running offline fresh-install check #4: flife run hits pinned reference digest at tick 8..."
  local run_out="${smoke_root}/run.ndjson"
  (
    cd "${smoke_home}"
    env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" TMPDIR="${smoke_root}" \
      "${flife}" run "${bundle_dir}/presets/reference-small-world.toml" \
      --ticks 8 --every 8 > "${run_out}"
  )
  python3 - "${run_out}" "${caps_out}" <<'PY'
import json, sys
records = [json.loads(line) for line in open(sys.argv[1], "r", encoding="utf-8") if line.strip()]
caps = json.load(open(sys.argv[2], "r", encoding="utf-8"))
pinned = caps["world"]["pinned_digest"]
end = records[-1]
assert end.get("record") == "end", f"expected end record, got {end}"
assert end.get("stop", {}).get("end") == "reached", f"expected stop.end=='reached', got {end}"
assert end.get("tick") == 8, f"expected tick==8, got {end}"
assert end.get("digest") == pinned, f"run digest {end.get('digest')} != pinned {pinned}"
assert end.get("tally", {}).get("normalized_residual", 1.0) <= 1.0e-5, f"residual out of budget: {end}"
print(f"offline run OK: tick=8, digest={end['digest'][:12]}... matches pinned reference")
PY

  log "Running explicit capability refusal #1: flife render --video with ffmpeg absent..."
  local no_ffmpeg_frames="${smoke_root}/no-ffmpeg-frames"
  local no_ffmpeg_err="${smoke_root}/no-ffmpeg-err.json"
  local rc=0
  (
    cd "${smoke_home}"
    env -i PATH="${empty_path}" HOME="${smoke_home}" TMPDIR="${smoke_root}" \
      "${flife}" render "${bundle_dir}/presets/reference-small-world.toml" \
      --ticks 4 --every 4 --width 128 --height 128 \
      --out "${no_ffmpeg_frames}" --video "${smoke_root}/refused.mp4" \
      >/dev/null 2>"${no_ffmpeg_err}"
  ) || rc=$?
  if [[ "${rc}" -eq 0 ]]; then
    die "expected non-zero refusal when ffmpeg is absent from PATH, got exit 0"
  fi
  if [[ -e "${no_ffmpeg_frames}/frame-000000.png" ]]; then
    die "render --video wrote frame-000000.png before checking missing ffmpeg"
  fi
  python3 - "${no_ffmpeg_err}" <<'PY'
import json, sys
text = open(sys.argv[1], "r", encoding="utf-8").read().strip().splitlines()
assert text, "empty stderr on missing-encoder refusal"
rec = json.loads(text[-1])
assert rec.get("schema") == "frankenlife.error.v1", f"expected frankenlife.error.v1, got {rec}"
print(f"missing-encoder refusal OK: kind={rec.get('kind')} ({rec.get('detail', '')[:80]})")
PY

  log "Running explicit capability refusal #2: unknown V2 toggle emits typed frankenlife.error.v1..."
  local bad_toggle_err="${smoke_root}/bad-toggle-err.json"
  rc=0
  (
    cd "${smoke_home}"
    env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" \
      "${flife}" validate "${bundle_dir}/presets/reference-small-world.toml" \
      --disable nonexistent-v2-feature >/dev/null 2>"${bad_toggle_err}"
  ) || rc=$?
  if [[ "${rc}" -eq 0 ]]; then
    die "expected non-zero refusal for unknown V2 feature slug"
  fi
  python3 - "${bad_toggle_err}" <<'PY'
import json, sys
text = open(sys.argv[1], "r", encoding="utf-8").read().strip().splitlines()
assert text, "empty stderr on unknown-toggle refusal"
rec = json.loads(text[-1])
assert rec.get("schema") == "frankenlife.error.v1", f"expected frankenlife.error.v1, got {rec}"
print(f"typed error refusal OK: kind={rec.get('kind')}")
PY

  log "Running explicit capability refusal #3: V2 --color cell-state on V1 preset..."
  local v2_color_frames="${smoke_root}/v2-color-frames"
  local v2_color_err="${smoke_root}/v2-color-err.json"
  rc=0
  (
    cd "${smoke_home}"
    env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" TMPDIR="${smoke_root}" \
      "${flife}" render "${bundle_dir}/presets/reference-small-world.toml" \
      --color cell-state --ticks 1 --every 1 --width 128 --height 128 \
      --out "${v2_color_frames}" >/dev/null 2>"${v2_color_err}"
  ) || rc=$?
  if [[ "${rc}" -eq 0 ]]; then
    die "expected non-zero refusal for --color cell-state on V1 preset"
  fi
  if [[ -e "${v2_color_frames}/frame-000000.png" ]]; then
    die "render --color cell-state wrote frame-000000.png before rejecting V2-only color mapping"
  fi
  python3 - "${v2_color_err}" <<'PY'
import json, sys
text = open(sys.argv[1], "r", encoding="utf-8").read().strip().splitlines()
assert text, "empty stderr on V2 color refusal"
rec = json.loads(text[-1])
assert rec.get("schema") == "frankenlife.error.v1", f"expected frankenlife.error.v1, got {rec}"
print(f"V2 color mapping refusal OK: kind={rec.get('kind')}")
PY

  if [[ "$(uname -s)" != "Darwin" ]]; then
    log "Running explicit capability refusal #4 (non-macOS): --controllers metal refused..."
    local metal_err="${smoke_root}/metal-err.json"
    rc=0
    (
      cd "${smoke_home}"
      env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" \
        "${flife}" run "${bundle_dir}/presets/reference-small-world.toml" \
        --ticks 1 --controllers metal >/dev/null 2>"${metal_err}"
    ) || rc=$?
    if [[ "${rc}" -eq 0 ]]; then
      die "expected non-zero refusal for --controllers metal on $(uname -s)"
    fi
    python3 - "${metal_err}" <<'PY'
import json, sys
rec = json.loads(open(sys.argv[1], "r", encoding="utf-8").read().strip().splitlines()[-1])
assert rec.get("schema") == "frankenlife.error.v1", f"expected frankenlife.error.v1, got {rec}"
print(f"missing-metal refusal OK: kind={rec.get('kind')}")
PY
  fi

  # 3. macOS .app bundle verification when FrankenLife Studio.app is packaged.
  local app_dir="${bundle_dir}/FrankenLife Studio.app"
  if [[ -d "${app_dir}" ]]; then
    log "Verifying macOS app bundle: ${app_dir}..."
    local plist="${app_dir}/Contents/Info.plist"
    local app_bin="${app_dir}/Contents/MacOS/flife-studio"
    local app_launcher="${app_dir}/Contents/MacOS/launch-studio"
    [[ -f "${plist}" ]] || die "missing ${plist}"
    [[ -x "${app_bin}" ]] || die "missing executable ${app_bin}"
    [[ -x "${app_launcher}" ]] || die "missing executable ${app_launcher}"
    if command -v plutil >/dev/null 2>&1; then
      plutil -lint "${plist}" >/dev/null || die "plutil -lint failed on ${plist}"
    fi
    (
      cd "${smoke_home}"
      env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" "${app_launcher}" --version >/dev/null
      env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" "${app_launcher}" --list-features >/dev/null
    )
    local studio_refusal_err="${smoke_root}/studio-v2-color-err.json"
    rc=0
    (
      cd "${smoke_home}"
      env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" TMPDIR="${smoke_root}" \
        "${app_launcher}" "${app_dir}/Contents/Resources/presets/reference-small-world.toml" \
        --color cell-state >/dev/null 2>"${studio_refusal_err}"
    ) || rc=$?
    if [[ "${rc}" -eq 0 ]]; then
      die "expected non-zero refusal for packaged Studio --color cell-state on V1 preset"
    fi
    python3 - "${studio_refusal_err}" <<'PY'
import json, sys
text = open(sys.argv[1], "r", encoding="utf-8").read().strip().splitlines()
assert text, "empty stderr on packaged Studio V2 color refusal"
rec = json.loads(text[-1])
assert rec.get("schema") == "frankenlife.error.v1", f"expected frankenlife.error.v1, got {rec}"
print(f"packaged Studio V2 refusal OK: kind={rec.get('kind')}")
PY
    local studio_offscreen_out="${smoke_root}/studio-offscreen.ndjson"
    (
      cd "${smoke_home}"
      env -i PATH="/usr/bin:/bin" HOME="${smoke_home}" TMPDIR="${smoke_root}" \
        "${app_launcher}" "${app_dir}/Contents/Resources/presets/reference-small-world.toml" \
        --offscreen 0.25 --width 320 --height 240 --scale 1 > "${studio_offscreen_out}"
    )
    python3 - "${studio_offscreen_out}" <<'PY'
import json, sys
records = [json.loads(line) for line in open(sys.argv[1], "r", encoding="utf-8") if line.strip()]
assert records and records[0].get("record") == "open", f"missing Studio open record: {records[:1]}"
assert records[0].get("schema") == "frankenlife.studio.v1", f"bad Studio schema: {records[0]}"
end = records[-1]
assert end.get("record") == "end", f"missing Studio end record: {end}"
assert end.get("rendered", 0) >= 1, f"expected >=1 offscreen rendered frame, got {end}"
assert end.get("tick", 0) >= 1, f"expected >=1 committed tick, got {end}"
print(f"packaged Studio offscreen launch OK: tick={end['tick']}, rendered={end['rendered']}")
PY
    log "macOS app bundle Info.plist + launch-studio offline + refusal checks PASS."
  fi

  trap - EXIT
  cleanup_smoke
  log "Fresh-install offline verification PASSED for ${bundle_dir}."
}

if [[ -n "${VERIFY_ONLY}" ]]; then
  verify_bundle "${VERIFY_ONLY}"
  exit 0
fi

# Refuse staging inside the repository tree (AGENTS.md: large artifacts stay outside git).
abs_out="$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "${OUT_DIR}")"
case "${abs_out}" in
  "${REPO_ROOT}"|"${REPO_ROOT}"/*)
    die "--out '${OUT_DIR}' lies inside the repository (${REPO_ROOT}); stage offline packages outside git"
    ;;
esac

if [[ -e "${abs_out}" ]]; then
  if [[ ! -d "${abs_out}" ]]; then
    die "--out '${abs_out}' exists and is not a directory"
  fi
  if [[ -n "$(ls -A "${abs_out}" 2>/dev/null)" ]]; then
    die "--out '${abs_out}' is not empty; choose a fresh directory (this script never deletes existing files)"
  fi
fi

# Locate release binaries (explicitly refuse missing binaries rather than shipping placeholders).
if [[ -z "${FLIFE_BIN}" ]]; then
  for candidate in \
    "${CARGO_TARGET_DIR:-}/release/flife" \
    "${REPO_ROOT}/target/release/flife"
  do
    if [[ -n "${candidate}" && -x "${candidate}" ]]; then
      FLIFE_BIN="${candidate}"
      break
    fi
  done
fi
[[ -n "${FLIFE_BIN}" && -x "${FLIFE_BIN}" ]] || die \
  "no compiled release 'flife' binary found; pass --flife PATH (e.g. build with 'cargo build --release -p flife-cli')"

if [[ -z "${STUDIO_BIN}" && "$(uname -s)" == "Darwin" ]]; then
  target_base="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}"
  while [[ "${target_base}" == */ ]]; do
    target_base="${target_base%/}"
  done
  for candidate in \
    "${CARGO_TARGET_DIR:-}/release/flife-studio" \
    "${target_base}/flife-studio/release/flife-studio" \
    "${REPO_ROOT}/target/release/flife-studio"
  do
    if [[ -n "${candidate}" && -x "${candidate}" ]]; then
      STUDIO_BIN="${candidate}"
      break
    fi
  done
fi

if [[ "$(uname -s)" == "Darwin" && -z "${STUDIO_BIN}" && "${ALLOW_CLI_ONLY}" -eq 0 ]]; then
  die "no compiled release 'flife-studio' binary found on macOS; pass --studio PATH or pass --allow-cli-only for a CLI-only package"
fi
if [[ -n "${STUDIO_BIN}" && ! -x "${STUDIO_BIN}" ]]; then
  die "--studio '${STUDIO_BIN}' is not an executable file"
fi

# Read workspace version cleanly from Cargo.toml.
version="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "${REPO_ROOT}/Cargo.toml" | head -n 1)"
version="${version:-0.0.1}"
git_rev="$(git -C "${REPO_ROOT}" rev-parse HEAD 2>/dev/null || echo "unknown")"
git_dirty="$(git -C "${REPO_ROOT}" status --porcelain -- crates presets scripts/package-offline.sh Cargo.toml Cargo.lock 2>/dev/null | wc -l | tr -d ' ')"

log "Staging offline package (version ${version}, rev ${git_rev:0:12}) into ${abs_out}..."
mkdir -p "${abs_out}/bin" "${abs_out}/presets" "${abs_out}/fixtures" "${abs_out}/notices"

cp "${FLIFE_BIN}" "${abs_out}/bin/flife"
chmod 755 "${abs_out}/bin/flife"

for preset in "${REPO_ROOT}"/presets/*.toml; do
  [[ -f "${preset}" ]] || continue
  cp "${preset}" "${abs_out}/presets/"
done

cp "${REPO_ROOT}/presets/reference-small-world.toml" "${abs_out}/fixtures/reference-small-world.toml"
cp "${REPO_ROOT}/LICENSE" "${abs_out}/notices/LICENSE"
cp "${REPO_ROOT}/franken-stack.lock" "${abs_out}/notices/franken-stack.lock"

cat > "${abs_out}/notices/OFFLINE_LAUNCH.txt" <<EOF
FrankenLife Offline Package (${version}, revision ${git_rev:0:12})
================================================================

This package launches completely offline with zero network access required.

CLI quick start:
  ./bin/flife capabilities
  ./bin/flife --list-features
  ./bin/flife validate ./presets/reference-small-world.toml
  ./bin/flife run ./presets/reference-small-world.toml --ticks 1024 --every 128

Native macOS Studio (when bundled):
  open "./FrankenLife Studio.app"
  # Or with explicit preset / style flags:
  ./bin/flife-studio ./presets/confluent-world.toml --color lineage-energy --look sphere --no-field

Optional video encoding:
  PNG frame export works with no external tools. MP4 encoding (\`flife render --video\`
  and \`flife-studio --export --export-mp4\`) requires \`ffmpeg\` with \`libx264\` on PATH;
  if absent, FrankenLife refuses explicitly with a \`frankenlife.error.v1\` record before
  writing any frame.

Capability honesty (plan §20.5):
  Run \`./bin/flife capabilities\` on the target machine. Missing Metal/GPU capability
  produces a functional CPU route when selected or an explicit refusal for GPU-required
  runs; FrankenLife never silently substitutes a different neural model.
EOF

# Package macOS native FrankenLife Studio.app when flife-studio binary is provided.
studio_packaged=false
if [[ -n "${STUDIO_BIN}" ]]; then
  cp "${STUDIO_BIN}" "${abs_out}/bin/flife-studio"
  chmod 755 "${abs_out}/bin/flife-studio"
  if [[ "$(uname -s)" == "Darwin" ]]; then
    app_dir="${abs_out}/FrankenLife Studio.app"
    mkdir -p "${app_dir}/Contents/MacOS" "${app_dir}/Contents/Resources/presets"
    cp "${STUDIO_BIN}" "${app_dir}/Contents/MacOS/flife-studio"
    chmod 755 "${app_dir}/Contents/MacOS/flife-studio"
    cp "${abs_out}"/presets/*.toml "${app_dir}/Contents/Resources/presets/"

    cat > "${app_dir}/Contents/MacOS/launch-studio" <<'LAUNCHER'
#!/usr/bin/env bash
set -euo pipefail
MACOS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RES_DIR="$(cd "${MACOS_DIR}/../Resources" && pwd)"
if [[ $# -eq 1 && "$1" == -psn_* ]]; then
  shift
fi
if [[ $# -gt 0 ]]; then
  exec "${MACOS_DIR}/flife-studio" "$@"
fi
LOG_DIR="${HOME}/Library/Logs/FrankenLife"
mkdir -p "${LOG_DIR}"
exec "${MACOS_DIR}/flife-studio" \
  "${RES_DIR}/presets/reference-small-world.toml" \
  >> "${LOG_DIR}/studio.ndjson" 2>> "${LOG_DIR}/studio.err.log"
LAUNCHER
    chmod 755 "${app_dir}/Contents/MacOS/launch-studio"

    cat > "${app_dir}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>FrankenLife Studio</string>
  <key>CFBundleExecutable</key>
  <string>launch-studio</string>
  <key>CFBundleIdentifier</key>
  <string>org.frankenlife.studio</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>FrankenLife Studio</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>CFBundleVersion</key>
  <string>${git_rev:0:12}</string>
  <key>LSMinimumSystemVersion</key>
  <string>14.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST
    studio_packaged=true
  fi
fi

# Generate real capability & feature reports using the staged CLI binary.
log "Generating live capabilities.json and features.json from packaged CLI..."
"${abs_out}/bin/flife" capabilities > "${abs_out}/capabilities.json"
"${abs_out}/bin/flife" --list-features > "${abs_out}/features.json"

# Write MANIFEST.json covering every staged file (with honest smoke_verified status).
write_manifest() {
  local smoke_flag="$1"
  python3 - "${abs_out}" "${version}" "${git_rev}" "${git_dirty}" "${studio_packaged}" "${smoke_flag}" <<'PY'
import datetime, hashlib, json, os, platform, sys

root, version, rev, dirty_count, studio_pkg, smoke_flag = sys.argv[1:7]
entries = []
for dirpath, dirnames, filenames in os.walk(root):
    dirnames.sort()
    for name in sorted(filenames):
        if name == "MANIFEST.json" and os.path.abspath(dirpath) == os.path.abspath(root):
            continue
        full = os.path.join(dirpath, name)
        rel = os.path.relpath(full, root).replace(os.sep, "/")
        data = open(full, "rb").read()
        entries.append({
            "path": rel,
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        })

caps = json.load(open(os.path.join(root, "capabilities.json"), "r", encoding="utf-8"))
cpu_route = next((r for r in caps.get("controllers", []) if r.get("route") == "cpu"), {})
verified_routes = [
    r.get("route") for r in caps.get("controllers", []) if r.get("status") == "verified"
]
manifest = {
    "schema": "frankenlife.offline-package.v1",
    "version": version,
    "revision": rev,
    "uncommitted_rust_or_preset_paths": int(dirty_count),
    "built_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "host": {
        "os": platform.system(),
        "arch": platform.machine(),
        "release": platform.release(),
    },
    "studio_app_packaged": studio_pkg == "true",
    "smoke_verified": smoke_flag == "true",
    "capabilities_summary": {
        "schema": caps.get("schema"),
        "preset_digest": caps.get("world", {}).get("pinned_digest"),
        "verified_cpu_digest": cpu_route.get("digest"),
        "verified_routes": verified_routes,
        "auto_route": caps.get("auto", {}).get("takes"),
        "metal_device": caps.get("auto", {}).get("metal_device"),
        "video_encoder": caps.get("video", {}).get("status"),
    },
    "files": entries,
}
with open(os.path.join(root, "MANIFEST.json"), "w", encoding="utf-8") as fh:
    json.dump(manifest, fh, indent=2)
    fh.write("\n")
PY
}

write_manifest "false"

if [[ "${SKIP_SMOKE}" -eq 0 ]]; then
  verify_bundle "${abs_out}"
  write_manifest "true"
  verify_manifest "${abs_out}" "true"
else
  log "WARNING: --skip-smoke set; MANIFEST.json honestly records smoke_verified=false."
fi

if [[ "${MAKE_TARBALL}" -eq 1 ]]; then
  tar_path="${abs_out}.tar.gz"
  tar -C "$(dirname "${abs_out}")" -czf "${tar_path}" "$(basename "${abs_out}")"
  tar_sha="$(sha256_file "${tar_path}")"
  log "Wrote tarball ${tar_path} (sha256: ${tar_sha})"
fi

log "Offline package ready at ${abs_out} (smoke_verified=$([[ "${SKIP_SMOKE}" -eq 0 ]] && echo true || echo false))."
