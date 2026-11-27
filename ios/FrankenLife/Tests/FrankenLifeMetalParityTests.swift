import XCTest
import Metal
import FlifeIOSBridge
@testable import FrankenLife

/// Gate G4 Native iOS Metal Parity Test Suite:
/// Verifies that `MetalControllerEngine` (compiling FrankenLife's exact
/// `softfloat64.metal` + `controller.metal` on Apple iOS Metal with `.safe`
/// IEEE math) reproduces the CPU `ReferenceController` **bit-for-bit (0 ULP)**
/// across F32 V1, AoSoA F16 V1, and V2 dual-carrier/vascular configurations!
final class FrankenLifeMetalParityTests: XCTestCase {

    private func loadPresetTOML(_ stem: String) throws -> String {
        let bundles = [Bundle.main, Bundle(for: Self.self)]
        for b in bundles {
            if let url = b.url(forResource: stem, withExtension: "toml", subdirectory: "Presets")
                ?? b.url(forResource: stem, withExtension: "toml") {
                return try String(contentsOf: url, encoding: .utf8)
            }
        }
        throw XCTSkip("Preset \(stem).toml not found in test bundle")
    }

    private func verifyParity(presetStem: String, ticks: UInt32) throws {
        let engine = try MetalControllerEngine(bundle: Bundle.main)
        let toml = try loadPresetTOML(presetStem)

        var vtable = engine.makeVtable()
        var result = FlifeParityResult()
        toml.withCString { cToml in
            flife_verify_metal_parity(cToml, &vtable, ticks, 4, &result)
        }

        let errStr = cArrayToString(result.error)
        XCTAssertTrue(result.success, "Parity harness failed for \(presetStem): \(errStr)")
        XCTAssertTrue(
            result.digest_matched,
            "SHA-256 World::digest() MISMATCH on \(presetStem) at tick \(result.mismatch_tick)! Metal=\(cArrayToString(result.metal_digest_hex)) CPU=\(cArrayToString(result.cpu_digest_hex))"
        )
        XCTAssertEqual(result.ticks_verified, UInt64(ticks), "Expected all \(ticks) ticks verified")
        XCTAssertGreaterThan(result.metal_batched, 0, "Expected Metal GPU to batch-evaluate organisms!")

        let gpuMs = Double(result.metal_device_ns) * 1e-6
        let metalWallMs = Double(result.metal_wall_ns) * 1e-6
        let cpuWallMs = Double(result.cpu_wall_ns) * 1e-6
        print("[G4 iOS Metal Parity PASS] \(presetStem): \(ticks) ticks, live=\(result.live_final), batched=\(result.metal_batched), deferred=\(result.metal_deferred), GPU=\(String(format: "%.2f", gpuMs))ms, wall Metal=\(String(format: "%.1f", metalWallMs))ms vs CPU=\(String(format: "%.1f", cpuWallMs))ms, digest=\(cArrayToString(result.metal_digest_hex).prefix(16))")
    }

    func testMetalParityReferenceSmallWorldF32() throws {
        // Exercises `controller_forward` (F32 canonical weights, 4,096 founders)
        try verifyParity(presetStem: "reference-small-world", ticks: 12)
    }

    func testMetalParityTissueWorldF16AoSoA() throws {
        // Exercises `controller_forward_half` (AoSoA-packed binary16 weights, 10,954 founders)
        try verifyParity(presetStem: "tissue-world", ticks: 6)
    }

    func testMetalParitySupercolonyV2F16() throws {
        // Exercises V2 108-input / 40-output ABI + AoSoA binary16 weights + vascular + dual-carrier
        try verifyParity(presetStem: "supercolony-v2", ticks: 4)
    }

    func testLiveSessionLifecycleAndInspection() throws {
        let engine = try MetalControllerEngine(bundle: Bundle.main)
        let toml = try loadPresetTOML("reference-small-world")
        var vtable = engine.makeVtable()
        var errBuf = [CChar](repeating: 0, count: 512)

        guard let session = toml.withCString({ cToml in
            flife_session_create(cToml, &vtable, 4, &errBuf, errBuf.count)
        }) else {
            let errBytes = errBuf.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }
            XCTFail("flife_session_create failed: \(String(decoding: errBytes, as: UTF8.self))")
            return
        }
        defer { flife_session_destroy(session) }

        // Pause and advance deterministic ticks via step_once
        flife_session_set_playing(session, false)
        flife_session_step_once(session)
        flife_session_step_once(session)

        // Wait briefly for worker to commit ticks
        var view = FlifeEpochView()
        var handle: OpaquePointer? = nil
        for _ in 0..<40 {
            if let h = flife_session_acquire_latest_epoch(session, 0, &view) {
                if view.tick >= 2 {
                    handle = h
                    break
                }
                flife_epoch_release(h)
            }
            Thread.sleep(forTimeInterval: 0.025)
        }
        guard let leased = handle else {
            XCTFail("Session never published tick >= 2")
            return
        }
        defer { flife_epoch_release(leased) }

        XCTAssertGreaterThan(view.organism_count, 0)
        XCTAssertGreaterThan(view.field_texels_x, 0)
        XCTAssertTrue(view.metal_active)
        XCTAssertFalse(view.faulted)

        // Inspect organism #1 (pure INV-09 read)
        let firstId = view.ids![0]
        var info = FlifeOrganismInfo()
        XCTAssertTrue(flife_session_inspect_organism(session, firstId, &info))
        XCTAssertTrue(info.found)
        XCTAssertEqual(info.id, firstId)
        XCTAssertGreaterThan(info.energy, 0.0)

        // Measure region non-invasively (pure INV-09 read)
        var measure = FlifeRegionMeasure()
        XCTAssertTrue(flife_session_measure_region(session, view.world_width * 0.5, view.world_height * 0.5, 8.0, &measure))
        XCTAssertTrue(measure.valid)
        XCTAssertGreaterThan(measure.cells, 0)

        // Verify background pause & quiesce parks the worker cleanly and resumes deterministically
        XCTAssertTrue(flife_session_pause_and_quiesce(session, 1000))
        flife_session_step_once(session)
        var advanced = false
        for _ in 0..<40 {
            if let h = flife_session_acquire_latest_epoch(session, view.epoch_id, &view) {
                let tickNow = view.tick
                flife_epoch_release(h)
                if tickNow >= 3 {
                    advanced = true
                    break
                }
            }
            Thread.sleep(forTimeInterval: 0.025)
        }
        XCTAssertTrue(advanced, "Session should advance cleanly after pause_and_quiesce")
    }

    func testMetalFrameUniformsLayoutMatchesShader() {
        // Exact 160-byte alignment contract with `FrameUniforms` in FieldAndOrganisms.metal
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.stride, 160)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.selectedPos), 80)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.selectedPrevPos), 88)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.followedPos), 96)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.followedPrevPos), 104)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.measureCenter), 112)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.brushCenter), 120)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.measureRadius), 128)
        XCTAssertEqual(MemoryLayout<MetalFrameUniforms>.offset(of: \.organismCount), 144)
        XCTAssertEqual(MemoryLayout<MetalOrganismInstance>.stride, 64)
    }

    func testConcurrentMetalControllerDispatchSafety() throws {
        // Stress-tests `MetalControllerEngine.dispatchLock` under two simultaneous parity runs
        // sharing ONE `MetalControllerEngine` instance (verifying zero buffer corruption or race).
        final class OutcomesBox: @unchecked Sendable {
            private let lock = NSLock()
            private var items: [FlifeParityResult] = []
            func append(_ r: FlifeParityResult) {
                lock.lock()
                items.append(r)
                lock.unlock()
            }
            func snapshot() -> [FlifeParityResult] {
                lock.lock()
                defer { lock.unlock() }
                return items
            }
        }

        let sharedEngine = try MetalControllerEngine(bundle: Bundle.main)
        let toml = try loadPresetTOML("reference-small-world")
        let group = DispatchGroup()
        let box = OutcomesBox()

        for _ in 0..<2 {
            group.enter()
            DispatchQueue.global(qos: .userInitiated).async {
                var vtable = sharedEngine.makeVtable()
                var res = FlifeParityResult()
                toml.withCString { cToml in
                    flife_verify_metal_parity(cToml, &vtable, 6, 2, &res)
                }
                box.append(res)
                group.leave()
            }
        }
        XCTAssertEqual(group.wait(timeout: .now() + .seconds(30)), .success)
        let outcomes = box.snapshot()
        XCTAssertEqual(outcomes.count, 2)
        for res in outcomes {
            XCTAssertTrue(res.success, "Concurrent parity failed: \(cArrayToString(res.error))")
            XCTAssertTrue(res.digest_matched, "Concurrent Metal dispatch caused digest mismatch!")
            XCTAssertGreaterThan(res.metal_batched, 0)
        }
    }
}
