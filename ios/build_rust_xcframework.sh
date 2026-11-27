#!/usr/bin/env bash
set -euo pipefail

# Builds the standalone Rust static library `libflife_ios_bridge.a` for BOTH
# iOS Device (aarch64-apple-ios) and iOS Simulator (aarch64-apple-ios-sim),
# packages them into ios/Frameworks/FlifeIOSBridge.xcframework, and syncs
# FrankenLife's canonical Metal compute shaders & TOML presets into the iOS app bundle.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BRIDGE_DIR="${SCRIPT_DIR}/FlifeIOSBridge"
OUT_FW="${SCRIPT_DIR}/Frameworks/FlifeIOSBridge.xcframework"
TARGET_DIR="${CARGO_TARGET_DIR:-/Volumes/SANDISK_SSD/build/frankenlife-target}/ios-bridge"

export RCH_SHIM_LOCAL_IDE=1
export CARGO_TARGET_DIR="${TARGET_DIR}"

# Target iOS 18.0+ minimum (compatible with iOS 18..27+, Apple A15–A20 Pro chips)
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-18.0}"

echo "==> Building flife-ios-bridge for aarch64-apple-ios (iPhone ARM64 Device)..."
cargo build --manifest-path "${BRIDGE_DIR}/Cargo.toml" \
    --release \
    --target aarch64-apple-ios

echo "==> Building flife-ios-bridge for aarch64-apple-ios-sim (iOS Simulator ARM64)..."
cargo build --manifest-path "${BRIDGE_DIR}/Cargo.toml" \
    --release \
    --target aarch64-apple-ios-sim

DEVICE_LIB="${TARGET_DIR}/aarch64-apple-ios/release/libflife_ios_bridge.a"
SIM_LIB="${TARGET_DIR}/aarch64-apple-ios-sim/release/libflife_ios_bridge.a"

if [[ ! -f "${DEVICE_LIB}" || ! -f "${SIM_LIB}" ]]; then
    echo "ERROR: Expected static libraries missing!" >&2
    exit 1
fi

echo "==> Assembling FlifeIOSBridge.xcframework..."
mkdir -p "${SCRIPT_DIR}/Frameworks"
STAGE_FW="${TARGET_DIR}/FlifeIOSBridge.xcframework"
if [[ -d "${STAGE_FW}" ]]; then
    # Use a unique staging path so we never invoke rm inside the repo without permission.
    STAGE_FW="${TARGET_DIR}/FlifeIOSBridge-$(date +%s).xcframework"
fi

xcodebuild -create-xcframework \
    -library "${DEVICE_LIB}" -headers "${BRIDGE_DIR}/include" \
    -library "${SIM_LIB}" -headers "${BRIDGE_DIR}/include" \
    -output "${STAGE_FW}"

mkdir -p "${OUT_FW}"
rsync -a --delete "${STAGE_FW}/" "${OUT_FW}/"

echo "==> Syncing canonical FrankenLife Metal compute shaders & TOML presets..."
RES_SHADERS="${SCRIPT_DIR}/FrankenLife/Resources/Shaders"
RES_PRESETS="${SCRIPT_DIR}/FrankenLife/Resources/Presets"
mkdir -p "${RES_SHADERS}" "${RES_PRESETS}"

# Copy exact upstream softfloat64.metal + controller.metal (read at runtime by MetalControllerEngine
# with MTLCompileOptions.mathMode = .safe for exact 0-ULP bit-for-bit parity).
cp "${REPO_ROOT}/crates/flife-backend-metal/src/softfloat64.metal" "${RES_SHADERS}/softfloat64.metal.txt"
cp "${REPO_ROOT}/crates/flife-backend-metal/src/controller.metal" "${RES_SHADERS}/controller.metal.txt"

# Copy curated presets from repo presets/ into the app bundle
for preset in \
    reference-small-world.toml \
    interactive-world.toml \
    colony-world.toml \
    tissue-world.toml \
    showcase-world.toml \
    confluent-world.toml \
    supercolony-v2.toml \
    supercolony-v2-abundant.toml \
    morphogenesis-tissue-v2.toml \
    morphogenesis-tissue-v2-abundant.toml \
    desktop-world.toml; do
    if [[ -f "${REPO_ROOT}/presets/${preset}" ]]; then
        cp "${REPO_ROOT}/presets/${preset}" "${RES_PRESETS}/${preset}"
    fi
done

echo "==> Done! XCFramework ready at: ${OUT_FW}"
ls -lh "${DEVICE_LIB}" "${SIM_LIB}"
