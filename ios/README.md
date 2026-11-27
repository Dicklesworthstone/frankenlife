# FrankenLife for iOS (Native SwiftUI + Apple Silicon Metal)

Native iOS port of FrankenLife targeting modern Apple Silicon iPhones (A15–A20 Pro, including iPhone 18 Pro Max with ProMotion 120Hz EDR OLED), running the genuine Rust simulation engine (`flife-core`, `flife-field`, `flife-genome`, `flife-neural`, `flife-engine`, `flife-render`) paired with native Apple iOS Metal compute and rendering.

## Architecture

- **`FlifeIOSBridge/`** (standalone Rust static library): Exposes a C-ABI (`flife_ios.h`) bridging `flife_engine::World` and `flife_render::{Publisher, Feed, Viewer, Lease}` to Swift without disturbing root `Cargo.lock`, `franken-stack.lock`, or `#![forbid(unsafe_code)]` on workspace crates.
- **`MetalControllerEngine.swift`** (Native iOS Metal Compute): Plugs into `World::set_controller_backend` via `FlifeMetalVtable`, compiling FrankenLife's exact `softfloat64.metal` + `controller.metal` (`controller_forward` F32 and `controller_forward_half` AoSoA binary16) with `MTLCompileOptions.mathMode = .safe` and concurrent multi-shard encoding (`MTLDispatchType.concurrent`) over persistent `storageModeShared` unified-memory parameter shards. Overlaps GPU inference with CPU phases (`prefers_cpu_overlap() == true`) while reproducing `ReferenceController` **bit-for-bit (0 ULP)**.
- **`FieldAndOrganisms.metal` + `MetalWorldRenderer.swift`** (120Hz ProMotion HDR): Zero CPU rasterization. Single-draw-call GPU instanced SDF organism rendering with seamless periodic toroidal tiling, GPU-side toroidal minimum-image epoch interpolation, all 9 canonical FrankenLife color mappings (including V2 `cell-state-v1` and `polarity-v1` apical ticks), 3 looks (`Glow HDR`, `3D Sphere`, honest `Raw G5`), Dual-Kawase HDR bloom, and `rgba16Float` Extended Dynamic Range (EDR) output.
- **`FrankenLife/UI/`** (SwiftUI Observatory): Full-bleed edge-to-edge Metal canvas, floating Liquid Glass HUD with honest separation of simulation clocks (`ticks/s`, `GPU ms`, `INV-03`) and display pacing (`FPS`, `EDR`, display-only thermal adaptation), multi-touch gestures, interactive scientific interventions (`InjectEnergy`, `InjectMaterial`, `IntroduceOrganisms`, `RemoveOrganisms`), non-invasive region measurement, organism inspector with 32-channel affinity spectrum and lineage follow, and an in-app Gate G4 Metal vs CPU parity verifier.

## Build & Run on iPhone

Prerequisites: Rust toolchain pinned by root `rust-toolchain.toml` with `aarch64-apple-ios` and `aarch64-apple-ios-sim` targets, Xcode 16+ (tested on Xcode 27.1 / iOS 27 SDK), and `xcodegen`.

```bash
# 1. Build Rust static libraries (arm64 device + arm64 simulator) and assemble FlifeIOSBridge.xcframework
./ios/build_rust_xcframework.sh

# 2. Generate FrankenLife.xcodeproj
cd ios && xcodegen generate

# 3. Open in Xcode, select your iPhone device + Signing Team, and press Cmd+R
open FrankenLife.xcodeproj
```

## Verify Gate G4 Metal Parity (XCTest)

```bash
cd ios && xcodebuild test \
  -project FrankenLife.xcodeproj \
  -scheme FrankenLife \
  -destination 'platform=iOS Simulator,name=FrankenLife iPhone 18 Pro Max' \
  -only-testing:FrankenLifeTests/FrankenLifeMetalParityTests
```
