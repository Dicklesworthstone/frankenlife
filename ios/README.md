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

## Memory on a phone

A world is admitted as `flife run` admits it (FL-043, plan §19.4): its configuration's `maximum_total_bytes` must cover every term but the controller backend's, and the Metal backend holds at most what those leave. Where the device reports a per-app limit (`os_proc_available_memory`; the simulator reports none), the bridge also keeps free what one step needs (the admission's births staging and its reserve):

- It refuses to found a world when the device has less than that free, and says how much each side has.
- It makes no Metal parameter shard that would leave less than that free or pass the backend's budget. That shard's organisms are evaluated on the reference, which gives the same bits; only speed changes.
- It takes no step the device cannot hold. The world pauses at that unchanged boundary and the banner gives the figures; play or step to check again.

The bundled presets are desktop presets. Their declared budgets are 2–64 GiB, mostly an allocated genome quota (up to 18 GiB) that a world fills only if its genomes grow that far. What four of them held after founding and two ticks, measured with the desktop `flife` on Metal controllers and 4 workers on an M5 Ultra Mac (process peak footprint, `/usr/bin/time -l`):

| Preset | Founders | Peak footprint | Of it, Metal shards |
|---|---:|---:|---:|
| reference-small-world | 4,096 | 0.62 GiB | 0.16 GiB |
| tissue-world | 10,954 | 0.83 GiB | 0.26 GiB |
| interactive-world | 25,000 | 2.61 GiB | 1.02 GiB |
| desktop-world | 100,000 | 5.09 GiB | 1.98 GiB |

The app's renderer adds its own buffers, and a world's genomes grow as it evolves.

## What has been checked

Observed on 2026-11-27 with the iOS sources of the commit that adds this section and the engine at 87baba7, on an M5 Ultra Mac in the iOS 27.1 simulator "FrankenLife iPhone 18 Pro Max" (Xcode 27.1). The bridge is built for `aarch64-apple-ios-sim` there, so its Rust ran on the Mac's CPU and its Metal on the Mac's GPU. All 10 XCTests passed, about 5 s each time, in the Debug configuration (`xcodebuild test` as above) and in Release (add `-configuration Release ENABLE_TESTABILITY=YES`; Release does not otherwise allow `@testable import`):

- Metal and CPU controllers give the same world at every tick of reference-small-world (12 ticks), tissue-world (6) and supercolony-v2 (4). Those worlds are the desktop's: a release `flife run` with CPU controllers on 4 workers ends at the same digests (`35f0a219…e7742820`, `7bc1e183…1d1ebcbc`, `9578835d…dd25d469`).
- reference-small-world reaches `flife capabilities`' pinned digest after 8 ticks (`82e0720d…144df71e`, "the world every qualified host reaches") on both routes. It reaches it again when no Metal shard may be made and all 32,768 evaluations run on the reference.
- A device with 1 MiB free refuses to found that world. A session whose device runs short pauses with its world unchanged, and steps again once memory returns.

Not checked yet: any physical iPhone (its GPU, its memory limit and the app on screen), runs longer than 12 ticks, and the larger presets past founding. To qualify a phone, run the same tests on it with your signing team:

```bash
cd ios && xcodebuild test -project FrankenLife.xcodeproj -scheme FrankenLife \
  -destination 'platform=iOS,name=<your iPhone>' -allowProvisioningUpdates DEVELOPMENT_TEAM=<team id> \
  -only-testing:FrankenLifeTests/FrankenLifeMetalParityTests
```
