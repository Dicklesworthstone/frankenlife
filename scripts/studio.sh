#!/usr/bin/env bash
# Build and open FrankenLife Studio on this Mac, from any directory.
#
#   scripts/studio.sh presets/confluent-world.toml --color lineage-energy --look sphere --no-field
#   scripts/studio.sh presets/reference-small-world.toml --width 960 --height 540
#
# Arguments go to flife-studio unchanged (see `scripts/studio.sh --help`).
#
# Why not plain `cargo run --release -p flife-studio`: the Studio draws through
# Metal into an AppKit window, so it has to be built and run on this Apple
# Silicon host. On a machine with the rch cargo shim, `cargo run` is sent to a
# Linux build worker, which can neither build the Metal route nor open the
# window. Run from outside the repository, rch refuses with RCH-E415 ("manifest
# does not exist: <cwd>/Cargo.toml") instead of cargo's "could not find
# Cargo.toml". This wrapper:
#   - finds the workspace from its own location, so the current directory does
#     not matter, and resolves a relative *.toml argument against the
#     repository when it does not exist from the current directory;
#   - selects the toolchain pinned in rust-toolchain.toml, which rustup would
#     otherwise choose from the current directory;
#   - keeps this one build local with the shim's per-build escape hatch
#     (unsets RCH_REQUIRE_REMOTE and sets RCH_SHIM_LOCAL_IDE=1; the shim caps
#     local jobs at RCH_LOCAL_MAX_JOBS, default 8);
#   - builds into ${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/flife-studio (AGENTS.md),
#     normalizing any trailing slash on TMPDIR/RCH_TARGET_BASE, unless
#     CARGO_TARGET_DIR is set.
set -euo pipefail

if [ "$(uname -s)" != Darwin ]; then
  echo "studio.sh: flife-studio needs macOS (Metal and an AppKit window); this host is $(uname -s)." >&2
  exit 1
fi

script=$(readlink -f "${BASH_SOURCE[0]}")
repo=$(cd "$(dirname "$script")/.." && pwd)

toolchain=$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$repo/rust-toolchain.toml")
if [ -z "$toolchain" ]; then
  echo "studio.sh: no channel in $repo/rust-toolchain.toml" >&2
  exit 1
fi

args=()
for arg in "$@"; do
  case "$arg" in
    -*|/*) ;;
    *.toml)
      if [ ! -e "$arg" ] && [ -e "$repo/$arg" ]; then
        arg="$repo/$arg"
      fi
      ;;
  esac
  args+=("$arg")
done

target_base="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}"
while [[ "$target_base" == */ ]]; do
  target_base="${target_base%/}"
done

export RUSTUP_TOOLCHAIN="$toolchain"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${target_base}/flife-studio}"
unset RCH_REQUIRE_REMOTE
export RCH_SHIM_LOCAL_IDE=1

exec cargo run --release --manifest-path "$repo/Cargo.toml" -p flife-studio -- ${args[@]+"${args[@]}"}
