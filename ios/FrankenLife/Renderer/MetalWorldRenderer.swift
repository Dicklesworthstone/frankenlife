import Foundation
import Metal
import MetalKit
import QuartzCore
import FlifeIOSBridge

/// Visual settings synchronized from SwiftUI to the 120Hz Metal renderer (pure presentation — INV-09).
struct RenderStyleState: Equatable, Sendable {
    var colorMapping: UInt32 = 0       // 0..8
    var lookMode: UInt32 = 0           // 0 = Glow, 1 = Sphere, 2 = Raw
    var fieldMode: UInt32 = 0          // 0 = Energy, 1 = Composition, 2 = Channel, 3 = Trail, 4 = Vascular
    var fieldChannel: UInt8 = 0        // 0..31 when fieldMode == 2
    var showField: Bool = true
    var bloomStrength: Float = 0.78
    var pointRadiusPt: Float = 3.2
    var interpolate120Hz: Bool = true
}

/// Interactive camera & GPU vector overlay state.
struct CameraState: Sendable {
    var centerX: Double = 32.0
    var centerY: Double = 32.0
    var zoom: Double = 1.0             // 1.0 = full world width fits screen shorter axis
    var worldWidth: Double = 64.0
    var worldHeight: Double = 64.0

    var selectedOrganismId: UInt64?
    var selectedPos: SIMD2<Float>?
    var selectedPrevPos: SIMD2<Float>?
    var followedPos: SIMD2<Float>?
    var followedPrevPos: SIMD2<Float>?

    var measureCenter: SIMD2<Float>?
    var measureRadius: Float = 0.0

    var brushCenter: SIMD2<Float>?
    var brushRadius: Float = 0.0

    func worldUnitsPerPixel(viewport: CGSize) -> Double {
        let minSide = max(Double(min(viewport.width, viewport.height)), 1.0)
        let baseExtent = max(worldWidth, worldHeight)
        return baseExtent / (minSide * max(zoom, 0.25))
    }

    func screenPointToWorld(_ pointPx: CGPoint, viewportPx: CGSize) -> SIMD2<Double> {
        let upp = worldUnitsPerPixel(viewport: viewportPx)
        let dx = (Double(pointPx.x) - Double(viewportPx.width) * 0.5) * upp
        let dy = (Double(pointPx.y) - Double(viewportPx.height) * 0.5) * upp
        let wx = (centerX + dx).truncatingRemainder(dividingBy: worldWidth)
        let wy = (centerY + dy).truncatingRemainder(dividingBy: worldHeight)
        return SIMD2<Double>(wx < 0 ? wx + worldWidth : wx, wy < 0 ? wy + worldHeight : wy)
    }
}

/// Must match `FrameUniforms` in `FieldAndOrganisms.metal` byte-for-byte (160 bytes).
struct MetalFrameUniforms {
    var viewportSize: SIMD2<Float> = .zero
    var worldExtent: SIMD2<Float> = SIMD2(64, 64)
    var cameraCenter: SIMD2<Float> = SIMD2(32, 32)
    var worldUnitsPerPixel: Float = 0.1
    var interpAlpha: Float = 1.0

    var maxSpeed: Float = 2.0
    var attemptEnergy: Float = 2.0
    var fieldScale: Float = 1.0
    var pointRadiusPx: Float = 3.2

    var colorMapping: UInt32 = 0
    var lookMode: UInt32 = 0
    var fieldMode: UInt32 = 0
    var showField: UInt32 = 1

    var bloomStrength: Float = 0.78
    var edrHeadroom: Float = 1.0
    var timeSeconds: Float = 0.0
    var pixelScale: Float = 3.0

    var selectedPos: SIMD2<Float> = .zero
    var selectedPrevPos: SIMD2<Float> = .zero
    var followedPos: SIMD2<Float> = .zero
    var followedPrevPos: SIMD2<Float> = .zero
    var measureCenter: SIMD2<Float> = .zero
    var brushCenter: SIMD2<Float> = .zero

    var measureRadius: Float = 0.0
    var brushRadius: Float = 0.0
    var hasSelected: UInt32 = 0
    var hasFollowed: UInt32 = 0

    var organismCount: UInt32 = 0
    var tilesX: UInt32 = 1
    var tilesY: UInt32 = 1
    var _pad0: UInt32 = 0
}

/// Must match `OrganismGPU` in `FieldAndOrganisms.metal` byte-for-byte (64 bytes).
struct MetalOrganismInstance {
    var currPos: SIMD2<Float> = .zero
    var prevPos: SIMD2<Float> = .zero
    var velocity: SIMD2<Float> = .zero
    var energy: Float = 0.0
    var birthAlpha: Float = 1.0
    var lineageLo: UInt32 = 0
    var lineageHi: UInt32 = 0
    var generation: UInt32 = 0
    var packedMeta: UInt32 = 0
    var aff0: Float = 0.0
    var aff1: Float = 0.0
    var aff2: Float = 0.0
    var flags: UInt32 = 0
}

/// Snapshot of live metrics pushed to SwiftUI at ~15Hz (never causing 120Hz SwiftUI view churn).
struct LiveTelemetrySnapshot: Sendable {
    var epochId: UInt64 = 0
    var tick: UInt64 = 0
    var dtSeconds: Double = 1.0 / 32.0
    var liveCount: UInt64 = 0
    var maxGeneration: UInt32 = 0
    var totalParameters: UInt64 = 0
    var organismEnergy: Double = 0.0
    var fieldEnergy: Double = 0.0
    var ledgerResidual: Double = 0.0
    var ledgerNormalized: Double? = nil
    var ledgerInjected: Double = 0.0
    var ledgerDissipated: Double = 0.0
    var faulted: Bool = false
    var births: UInt64 = 0
    var deaths: UInt64 = 0
    var evaluations: UInt64 = 0
    var refusedBirths: UInt64 = 0
    var simTicksPerSecond: Double = 0.0
    var renderFPS: Double = 0.0
    var lastTickWallMs: Double = 0.0
    var lastGpuMs: Double = 0.0
    var metalBatched: UInt64 = 0
    var metalDeferred: UInt64 = 0
    var metalResidentPeakBytes: UInt64 = 0
    var metalActive: Bool = false
    var isPlaying: Bool = true
    var isCapacityPaused: Bool = false
    /// The last step was refused: the device had `memoryAvailableBytes` free and the step needs `memoryNeededBytes`.
    var isMemoryShort: Bool = false
    var memoryAvailableBytes: UInt64 = 0
    var memoryNeededBytes: UInt64 = 0
    var hasCellStates: Bool = false
    var hasPolarities: Bool = false
    var hasTrailEnergy: Bool = false
    var hasVascular: Bool = false
    var followedOriginId: UInt64? = nil
    var followedOrganismId: UInt64? = nil
    var followedHandoffs: UInt64 = 0
    var followedAlive: Bool = false
    var edrHeadroom: Double = 1.0
}

/// Native 120Hz ProMotion HDR Metal renderer reading zero-copy `FlifeEpochView`s
/// from the Rust simulation feed. Zero CPU rasterization; pure GPU instancing + Dual-Kawase bloom.
final class MetalWorldRenderer: NSObject, MTKViewDelegate {
    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue

    // Render pipelines
    private let fieldPipeline: MTLRenderPipelineState
    private let organismPipeline: MTLRenderPipelineState
    private let bloomPrefilterPipeline: MTLRenderPipelineState
    private let bloomDownPipeline: MTLRenderPipelineState
    private let bloomUpPipeline: MTLRenderPipelineState
    private let compositePipeline: MTLRenderPipelineState

    // Triple-buffered GPU instance buffers + field textures (`storageModeShared` unified memory).
    // With `inFlightSemaphore(value: 3)` waited before any upload, at most 2 prior GPU frames
    // can be in-flight, reading at most `{ (active - 1) % 3, active }`. Therefore advancing each
    // resource's own write slot to `(active + 1) % 3` on upload guarantees 0 CPU/GPU hazards
    // even when sim TPS and 120Hz display FPS differ arbitrarily.
    private let inFlightSemaphore = DispatchSemaphore(value: 3)
    private var organismBuffers: [MTLBuffer?] = [nil, nil, nil]
    private var activeOrganismSlot: Int = 0
    private var currentOrganismCount: Int = 0

    // Previous & current epoch IDs/positions for O(N) toroidal interpolation & accurate picking
    private var prevIds: [UInt64] = []
    private var prevPositions: [SIMD2<Float>] = []
    private var currIds: [UInt64] = []
    private var currPositions: [SIMD2<Float>] = []
    private var currMatchedPrevPositions: [SIMD2<Float>] = []
    private var lastInterpAlpha: Float = 1.0

    // Triple-buffered field textures (zero CPU replace vs GPU fragment read race!)
    private var energyTextures: [MTLTexture?] = [nil, nil, nil]
    private var compTextures: [MTLTexture?] = [nil, nil, nil]
    private var v2OverlayTextures: [MTLTexture?] = [nil, nil, nil]
    private var activeEnergySlot: Int = 0
    private var activeCompSlot: Int = 0
    private var activeV2Slot: Int = 0
    private var fieldTexSize: (Int, Int) = (0, 0)
    private var compStaging: [Float16] = []
    private var v2Staging: [Float] = []
    private var energyDisplayScale: Float = 0.0
    private var trailDisplayScale: Float = 0.0
    private var vascularDisplayScale: Float = 0.0
    private var lastFieldChannelUploaded: UInt8 = 255

    // Cached raw field components for instant channel switching while paused
    private var cachedTexelsX: Int = 0
    private var cachedTexelsY: Int = 0
    private var cachedComposition: [Float] = []
    private var cachedComponents: [Int8] = []

    // Offscreen HDR scene & Dual-Kawase bloom pyramid textures
    // Separate downsample (`bloomMips`: 1/2, 1/4, 1/8) and upsample (`bloomUpMips`: 1/2, 1/4)
    // targets guarantee zero read/write render-target aliasing hazards on Apple TBDR GPUs.
    private var sceneTexture: MTLTexture?
    private var bloomMips: [MTLTexture] = []
    private var bloomUpMips: [MTLTexture] = []
    private var offscreenSize: (Int, Int) = (0, 0)

    // Session & epoch synchronization
    private let stateLock = NSLock()
    private var sessionPtr: OpaquePointer?
    private var lastEpochId: UInt64 = 0
    private var lastEpochArrivalTime: CFTimeInterval = CACurrentMediaTime()
    private var epochIntervalEMA: CFTimeInterval = 1.0 / 30.0
    private var epochMaxSpeed: Float = 2.0
    private var epochAttemptEnergy: Float = 2.0
    private var epochWorldExtent: SIMD2<Float> = SIMD2(64, 64)
    private var startTime: CFTimeInterval = CACurrentMediaTime()

    // Display FPS measurement
    private var frameCountWindow: Int = 0
    private var fpsWindowStart: CFTimeInterval = CACurrentMediaTime()
    private var measuredFPS: Double = 60.0
    private var lastTelemetryPush: CFTimeInterval = 0.0
    private var latestTelemetry = LiveTelemetrySnapshot()

    private var style = RenderStyleState()
    private var camera = CameraState()
    var onTelemetryUpdate: ((LiveTelemetrySnapshot) -> Void)?

    init(device: MTLDevice) throws {
        self.device = device
        guard let queue = device.makeCommandQueue() else {
            throw NSError(domain: "FrankenLife.Renderer", code: 1, userInfo: [NSLocalizedDescriptionKey: "Failed to make render MTLCommandQueue"])
        }
        self.commandQueue = queue
        self.commandQueue.label = "com.frankenlife.ios.render"

        guard let library = device.makeDefaultLibrary() else {
            throw NSError(domain: "FrankenLife.Renderer", code: 2, userInfo: [NSLocalizedDescriptionKey: "Missing default Metal shader library"])
        }

        let hdrFormat: MTLPixelFormat = .rgba16Float

        func makePipe(
            vert: String,
            frag: String,
            format: MTLPixelFormat,
            blending: Bool
        ) throws -> MTLRenderPipelineState {
            let desc = MTLRenderPipelineDescriptor()
            desc.vertexFunction = library.makeFunction(name: vert)
            desc.fragmentFunction = library.makeFunction(name: frag)
            desc.colorAttachments[0].pixelFormat = format
            if blending {
                // `organism_fragment` emits premultiplied alpha across Glow, Sphere, and Raw looks
                // so dense clusters preserve rich chromatic saturation without white washout.
                let att = desc.colorAttachments[0]!
                att.isBlendingEnabled = true
                att.rgbBlendOperation = .add
                att.alphaBlendOperation = .add
                att.sourceRGBBlendFactor = .one
                att.destinationRGBBlendFactor = .oneMinusSourceAlpha
                att.sourceAlphaBlendFactor = .one
                att.destinationAlphaBlendFactor = .oneMinusSourceAlpha
            }
            return try device.makeRenderPipelineState(descriptor: desc)
        }

        self.fieldPipeline = try makePipe(vert: "fullscreen_vertex", frag: "field_fragment", format: hdrFormat, blending: false)
        self.organismPipeline = try makePipe(vert: "organism_vertex", frag: "organism_fragment", format: hdrFormat, blending: true)
        self.bloomPrefilterPipeline = try makePipe(vert: "fullscreen_vertex", frag: "bloom_prefilter_fragment", format: hdrFormat, blending: false)
        self.bloomDownPipeline = try makePipe(vert: "fullscreen_vertex", frag: "bloom_downsample_fragment", format: hdrFormat, blending: false)
        self.bloomUpPipeline = try makePipe(vert: "fullscreen_vertex", frag: "bloom_upsample_fragment", format: hdrFormat, blending: false)
        self.compositePipeline = try makePipe(vert: "fullscreen_vertex", frag: "composite_fragment", format: hdrFormat, blending: false)

        super.init()
    }

    func readCamera() -> CameraState {
        stateLock.lock()
        defer { stateLock.unlock() }
        return camera
    }

    func mutateCamera(_ body: (inout CameraState) -> Void) {
        stateLock.lock()
        body(&camera)
        stateLock.unlock()
    }

    func updateStyle(_ newStyle: RenderStyleState) {
        stateLock.lock()
        style = newStyle
        stateLock.unlock()
    }

    func readStyle() -> RenderStyleState {
        stateLock.lock()
        defer { stateLock.unlock() }
        return style
    }

    /// Look up an organism's `(currPos, prevPos)` pair in O(log N) via binary search on
    /// strictly-increasing `currIds` so selecting while paused lights up the reticle immediately.
    func positionPair(forOrganismId id: UInt64) -> (curr: SIMD2<Float>, prev: SIMD2<Float>)? {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard !currIds.isEmpty,
              currIds.count == currPositions.count,
              currIds.count == currMatchedPrevPositions.count else {
            return nil
        }
        var lo = 0
        var hi = currIds.count - 1
        while lo <= hi {
            let mid = lo + (hi - lo) / 2
            let midId = currIds[mid]
            if midId == id {
                return (currPositions[mid], currMatchedPrevPositions[mid])
            } else if midId < id {
                lo = mid + 1
            } else {
                if mid == 0 { break }
                hi = mid - 1
            }
        }
        return nil
    }

    func attachSession(_ ptr: OpaquePointer?, worldWidth: Double, worldHeight: Double) {
        stateLock.lock()
        sessionPtr = ptr
        lastEpochId = 0
        currentOrganismCount = 0
        prevIds.removeAll(keepingCapacity: true)
        prevPositions.removeAll(keepingCapacity: true)
        currIds.removeAll(keepingCapacity: true)
        currPositions.removeAll(keepingCapacity: true)
        currMatchedPrevPositions.removeAll(keepingCapacity: true)
        lastInterpAlpha = 1.0
        energyDisplayScale = 0.0
        trailDisplayScale = 0.0
        vascularDisplayScale = 0.0
        lastFieldChannelUploaded = 255
        fieldTexSize = (0, 0)
        camera.worldWidth = worldWidth
        camera.worldHeight = worldHeight
        camera.centerX = worldWidth * 0.5
        camera.centerY = worldHeight * 0.5
        camera.zoom = 1.0
        camera.selectedOrganismId = nil
        camera.selectedPos = nil
        camera.selectedPrevPos = nil
        camera.followedPos = nil
        camera.followedPrevPos = nil
        camera.measureCenter = nil
        camera.measureRadius = 0
        camera.brushRadius = 0
        epochWorldExtent = SIMD2(Float(worldWidth), Float(worldHeight))
        stateLock.unlock()
    }

    /// Pick the nearest organism to a tap in world coordinates within `maxWorldRadius`,
    /// using the same 120Hz toroidal interpolated positions drawn on screen.
    func pickOrganism(near worldPos: SIMD2<Double>, maxWorldRadius: Double) -> UInt64? {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard !currIds.isEmpty,
              currIds.count == currPositions.count,
              currIds.count == currMatchedPrevPositions.count else { return nil }
        let lw = Double(epochWorldExtent.x)
        let lh = Double(epochWorldExtent.y)
        let alpha = Double(lastInterpAlpha)
        var bestDistSq = maxWorldRadius * maxWorldRadius
        var bestId: UInt64? = nil
        for i in 0..<currIds.count {
            let cur = currPositions[i]
            let prev = currMatchedPrevPositions[i]
            var stepX = Double(cur.x - prev.x)
            var stepY = Double(cur.y - prev.y)
            stepX -= lw * (stepX / lw).rounded()
            stepY -= lh * (stepY / lh).rounded()
            let px = Double(prev.x) + stepX * alpha
            let py = Double(prev.y) + stepY * alpha
            var dx = px - worldPos.x
            var dy = py - worldPos.y
            dx -= lw * (dx / lw).rounded()
            dy -= lh * (dy / lh).rounded()
            let d2 = dx * dx + dy * dy
            if d2 <= bestDistSq {
                bestDistSq = d2
                bestId = currIds[i]
            }
        }
        return bestId
    }

    // MARK: - MTKViewDelegate

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {
        ensureOffscreenTargets(width: Int(size.width), height: Int(size.height))
    }

    private func ensureOffscreenTargets(width: Int, height: Int) {
        let w = max(width, 2)
        let h = max(height, 2)
        if offscreenSize.0 == w && offscreenSize.1 == h && sceneTexture != nil {
            return
        }
        offscreenSize = (w, h)

        let sceneDesc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba16Float,
            width: w,
            height: h,
            mipmapped: false
        )
        sceneDesc.usage = [.renderTarget, .shaderRead]
        sceneDesc.storageMode = .private
        sceneTexture = device.makeTexture(descriptor: sceneDesc)

        // 3-level Dual-Kawase HDR Bloom downsample pyramid (1/2, 1/4, 1/8) plus
        // 2 dedicated upsample targets (1/2, 1/4) so no pass reads its own render target.
        bloomMips.removeAll()
        bloomUpMips.removeAll()
        var bw = max(w / 2, 2)
        var bh = max(h / 2, 2)
        for level in 0..<3 {
            let desc = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: .rgba16Float,
                width: bw,
                height: bh,
                mipmapped: false
            )
            desc.usage = [.renderTarget, .shaderRead]
            desc.storageMode = .private
            if let tex = device.makeTexture(descriptor: desc) {
                bloomMips.append(tex)
            }
            if level < 2, let upTex = device.makeTexture(descriptor: desc) {
                bloomUpMips.append(upTex)
            }
            bw = max(bw / 2, 2)
            bh = max(bh / 2, 2)
        }
    }

    private func ensureFieldTextures(texelsX: Int, texelsY: Int) {
        guard texelsX > 0, texelsY > 0 else { return }
        if fieldTexSize.0 == texelsX && fieldTexSize.1 == texelsY && energyTextures[0] != nil {
            return
        }
        fieldTexSize = (texelsX, texelsY)

        let eDesc = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .r32Float, width: texelsX, height: texelsY, mipmapped: false)
        eDesc.usage = [.shaderRead]
        eDesc.storageMode = .shared

        let cDesc = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: texelsX, height: texelsY, mipmapped: false)
        cDesc.usage = [.shaderRead]
        cDesc.storageMode = .shared

        let vDesc = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rg32Float, width: texelsX, height: texelsY, mipmapped: false)
        vDesc.usage = [.shaderRead]
        vDesc.storageMode = .shared

        for s in 0..<3 {
            energyTextures[s] = device.makeTexture(descriptor: eDesc)
            compTextures[s] = device.makeTexture(descriptor: cDesc)
            v2OverlayTextures[s] = device.makeTexture(descriptor: vDesc)
        }
        activeEnergySlot = 0
        activeCompSlot = 0
        activeV2Slot = 0
    }

    private func uploadCompTexture(channel: UInt8) {
        let nextCompSlot = (activeCompSlot + 1) % 3
        guard let compTex = compTextures[nextCompSlot], cachedTexelsX > 0, cachedTexelsY > 0 else { return }
        let count = cachedTexelsX * cachedTexelsY
        guard cachedComposition.count >= count * 3 else { return }
        let need = count * 4
        if compStaging.count < need {
            compStaging = [Float16](repeating: 0, count: need)
        }
        let ch = Int(min(channel, 31))
        let hasComponents = cachedComponents.count >= count * 32
        for i in 0..<count {
            let b3 = i * 3
            let b4 = i * 4
            compStaging[b4 + 0] = Float16(cachedComposition[b3 + 0])
            compStaging[b4 + 1] = Float16(cachedComposition[b3 + 1])
            compStaging[b4 + 2] = Float16(cachedComposition[b3 + 2])
            let chVal: Float = hasComponents ? (Float(cachedComponents[i * 32 + ch]) / 127.0) : 0.0
            compStaging[b4 + 3] = Float16(chVal)
        }
        compStaging.withUnsafeBytes { raw in
            if let base = raw.baseAddress {
                compTex.replace(
                    region: MTLRegionMake2D(0, 0, cachedTexelsX, cachedTexelsY),
                    mipmapLevel: 0,
                    withBytes: base,
                    bytesPerRow: cachedTexelsX * 4 * MemoryLayout<Float16>.stride
                )
            }
        }
        activeCompSlot = nextCompSlot
        lastFieldChannelUploaded = channel
    }

    /// Reads whether the session is playing or paused (for population capacity or device memory). A paused world
    /// publishes no new epoch, so a pause reaches the HUD only through this read.
    private func refreshPauseStatus(_ ptr: OpaquePointer) {
        var status = FlifePauseStatus()
        guard flife_session_pause_status(ptr, &status) else { return }
        stateLock.lock()
        latestTelemetry.isPlaying = status.is_playing
        latestTelemetry.isCapacityPaused = status.is_capacity_paused
        latestTelemetry.isMemoryShort = status.is_memory_short
        latestTelemetry.memoryAvailableBytes = status.available_bytes
        latestTelemetry.memoryNeededBytes = status.needed_bytes
        stateLock.unlock()
    }

    private func ingestLatestEpochIfAvailable(now: CFTimeInterval, currentStyle: RenderStyleState) {
        stateLock.lock()
        let ptr = sessionPtr
        let seen = lastEpochId
        stateLock.unlock()

        guard let ptr else { return }
        // Runs last, after any epoch below: a paused world publishes no new epoch.
        defer { refreshPauseStatus(ptr) }
        var view = FlifeEpochView()
        guard let handle = flife_session_acquire_latest_epoch(ptr, seen, &view) else {
            // Even when paused, refresh component texture into the next guaranteed-idle slot!
            if currentStyle.fieldMode == 2 && currentStyle.fieldChannel != lastFieldChannelUploaded {
                uploadCompTexture(channel: currentStyle.fieldChannel)
            }
            return
        }
        defer { flife_epoch_release(handle) }

        // Measure empirical inter-epoch wall interval for silky 120Hz interpolation
        if seen != 0 {
            let dt = now - lastEpochArrivalTime
            if dt > 0.001 && dt < 0.5 {
                epochIntervalEMA = epochIntervalEMA * 0.8 + dt * 0.2
            }
        }
        lastEpochArrivalTime = now

        let count = Int(view.organism_count)
        let nextOrgSlot = (activeOrganismSlot + 1) % 3
        let neededBytes = max(MemoryLayout<MetalOrganismInstance>.stride, count * MemoryLayout<MetalOrganismInstance>.stride)
        if organismBuffers[nextOrgSlot] == nil || organismBuffers[nextOrgSlot]!.length < neededBytes {
            let cap = max(4096, count.nextPowerOfTwo) * MemoryLayout<MetalOrganismInstance>.stride
            organismBuffers[nextOrgSlot] = device.makeBuffer(length: cap, options: .storageModeShared)
        }

        // Build new ID/position arrays into local scratch first, then swap under stateLock
        // so `pickOrganism` and `positionPair` never observe partially populated arrays.
        var nextIds: [UInt64] = []
        var nextPositions: [SIMD2<Float>] = []
        var nextMatchedPrev: [SIMD2<Float>] = []
        nextIds.reserveCapacity(count)
        nextPositions.reserveCapacity(count)
        nextMatchedPrev.reserveCapacity(count)

        stateLock.lock()
        let selectedId = camera.selectedOrganismId
        let followedId: UInt64? = (view.has_followed && view.followed_alive) ? view.followed_organism_id : nil
        // Previous epoch snapshot (already committed on the render thread)
        prevIds = currIds
        prevPositions = currPositions
        stateLock.unlock()

        var newSelectedPos: SIMD2<Float>? = nil
        var newSelectedPrevPos: SIMD2<Float>? = nil
        var newFollowedPos: SIMD2<Float>? = nil
        var newFollowedPrevPos: SIMD2<Float>? = nil

        if count > 0,
           let dstBuf = organismBuffers[nextOrgSlot],
           let ids = view.ids,
           let positions = view.positions,
           let velocities = view.velocities,
           let energies = view.energies,
           let lineages = view.lineages,
           let generations = view.generations,
           let topologies = view.topologies,
           let affinities = view.affinities {

            let dst = dstBuf.contents().assumingMemoryBound(to: MetalOrganismInstance.self)
            let cellStates = view.cell_states
            let polarities = view.polarities

            var pIdx = 0
            let prevCount = prevIds.count

            for i in 0..<count {
                let id = ids[i]
                let px = positions[i * 2]
                let py = positions[i * 2 + 1]
                let curPos = SIMD2<Float>(px, py)
                nextIds.append(id)
                nextPositions.append(curPos)

                while pIdx < prevCount && prevIds[pIdx] < id {
                    pIdx += 1
                }
                let prevPos: SIMD2<Float>
                let birthAlpha: Float
                if pIdx < prevCount && prevIds[pIdx] == id {
                    prevPos = prevPositions[pIdx]
                    birthAlpha = 1.0
                    pIdx += 1
                } else {
                    prevPos = curPos
                    birthAlpha = seen == 0 ? 1.0 : 0.72
                }
                nextMatchedPrev.append(prevPos)

                let lin = lineages[i]
                let topo = UInt32(topologies[i])
                let csByte: UInt32 = cellStates != nil ? UInt32(UInt8(bitPattern: cellStates![i])) : 0
                let polWord: UInt32 = polarities != nil
                    ? UInt32(UInt16(bitPattern: polarities![i]))
                    : UInt32(UInt16(bitPattern: Int16.min))
                let packed = (topo & 0xFF) | ((csByte & 0xFF) << 8) | ((polWord & 0xFFFF) << 16)

                var flags: UInt32 = 0
                if let sel = selectedId, sel == id {
                    flags |= 1
                    newSelectedPos = curPos
                    newSelectedPrevPos = prevPos
                }
                if let fol = followedId, fol == id {
                    flags |= 2
                    newFollowedPos = curPos
                    newFollowedPrevPos = prevPos
                }

                dst[i] = MetalOrganismInstance(
                    currPos: curPos,
                    prevPos: prevPos,
                    velocity: SIMD2<Float>(velocities[i * 2], velocities[i * 2 + 1]),
                    energy: energies[i],
                    birthAlpha: birthAlpha,
                    lineageLo: UInt32(truncatingIfNeeded: lin),
                    lineageHi: UInt32(truncatingIfNeeded: lin >> 32),
                    generation: generations[i],
                    packedMeta: packed,
                    aff0: affinities[i * 3],
                    aff1: affinities[i * 3 + 1],
                    aff2: affinities[i * 3 + 2],
                    flags: flags
                )
            }
            activeOrganismSlot = nextOrgSlot
        }
        currentOrganismCount = count

        // Upload bounded field textures into each texture's own next guaranteed-idle slot
        let tx = Int(view.field_texels_x)
        let ty = Int(view.field_texels_y)
        let texCount = tx * ty
        if tx > 0 && ty > 0 {
            ensureFieldTextures(texelsX: tx, texelsY: ty)
            let nextEnergySlot = (activeEnergySlot + 1) % 3
            if let ePtr = view.field_energy, let eTex = energyTextures[nextEnergySlot] {
                eTex.replace(
                    region: MTLRegionMake2D(0, 0, tx, ty),
                    mipmapLevel: 0,
                    withBytes: ePtr,
                    bytesPerRow: tx * MemoryLayout<Float>.stride
                )
                activeEnergySlot = nextEnergySlot
                var maxE: Float = 0.0
                for i in 0..<texCount {
                    let v = ePtr[i]
                    if v.isFinite && v > maxE { maxE = v }
                }
                energyDisplayScale = max(maxE, energyDisplayScale * 0.98)
            }

            if let cPtr = view.field_composition {
                cachedTexelsX = tx
                cachedTexelsY = ty
                cachedComposition = Array(UnsafeBufferPointer(start: cPtr, count: texCount * 3))
                if let compPtr = view.field_components {
                    cachedComponents = Array(UnsafeBufferPointer(start: compPtr, count: texCount * 32))
                } else {
                    cachedComponents.removeAll(keepingCapacity: true)
                }
                uploadCompTexture(channel: currentStyle.fieldChannel)
            }

            let nextV2Slot = (activeV2Slot + 1) % 3
            if let v2Tex = v2OverlayTextures[nextV2Slot] {
                let need = texCount * 2
                if v2Staging.count < need { v2Staging = [Float](repeating: 0, count: need) }
                var maxTrail: Float = 0.0
                var maxVasc: Float = 0.0
                let tPtr = view.field_trail_energy
                let vPtr = view.field_vascular
                if tPtr != nil || vPtr != nil {
                    for i in 0..<texCount {
                        let tv = tPtr != nil ? tPtr![i] : 0.0
                        let vv = vPtr != nil ? vPtr![i] : 0.0
                        v2Staging[i * 2] = tv
                        v2Staging[i * 2 + 1] = vv
                        if tv.isFinite && tv > maxTrail { maxTrail = tv }
                        if vv.isFinite && vv > maxVasc { maxVasc = vv }
                    }
                    trailDisplayScale = max(maxTrail, trailDisplayScale * 0.98)
                    vascularDisplayScale = max(maxVasc, vascularDisplayScale * 0.98)
                    v2Staging.withUnsafeBytes { raw in
                        if let base = raw.baseAddress {
                            v2Tex.replace(
                                region: MTLRegionMake2D(0, 0, tx, ty),
                                mipmapLevel: 0,
                                withBytes: base,
                                bytesPerRow: tx * 2 * MemoryLayout<Float>.stride
                            )
                        }
                    }
                    activeV2Slot = nextV2Slot
                }
            }
        }

        stateLock.lock()
        currIds = nextIds
        currPositions = nextPositions
        currMatchedPrevPositions = nextMatchedPrev
        lastEpochId = view.epoch_id
        epochMaxSpeed = Float(view.maximum_speed)
        epochAttemptEnergy = Float(view.attempt_energy)
        epochWorldExtent = SIMD2(Float(view.world_width), Float(view.world_height))
        camera.worldWidth = view.world_width
        camera.worldHeight = view.world_height
        camera.selectedPos = newSelectedPos
        camera.selectedPrevPos = newSelectedPrevPos
        camera.followedPos = newFollowedPos
        camera.followedPrevPos = newFollowedPrevPos
        if let fPos = newFollowedPos {
            // Smoothly track followed lineage heir across toroidal seams
            let lw = view.world_width
            let lh = view.world_height
            var dx = Double(fPos.x) - camera.centerX
            var dy = Double(fPos.y) - camera.centerY
            dx -= lw * (dx / lw).rounded()
            dy -= lh * (dy / lh).rounded()
            camera.centerX = (camera.centerX + dx * 0.18).truncatingRemainder(dividingBy: lw)
            if camera.centerX < 0 { camera.centerX += lw }
            camera.centerY = (camera.centerY + dy * 0.18).truncatingRemainder(dividingBy: lh)
            if camera.centerY < 0 { camera.centerY += lh }
        }

        latestTelemetry = LiveTelemetrySnapshot(
            epochId: view.epoch_id,
            tick: view.tick,
            dtSeconds: view.dt_seconds,
            liveCount: view.live_count,
            maxGeneration: view.max_generation,
            totalParameters: view.total_parameters,
            organismEnergy: view.organism_energy_total,
            fieldEnergy: view.field_energy_total,
            ledgerResidual: view.ledger_residual,
            ledgerNormalized: view.has_ledger_normalized ? view.ledger_normalized : nil,
            ledgerInjected: view.ledger_injected,
            ledgerDissipated: view.ledger_dissipated,
            faulted: view.faulted,
            births: view.has_tally ? view.tally_births : 0,
            deaths: view.has_tally ? view.tally_deaths : 0,
            evaluations: view.has_tally ? view.tally_evaluations : 0,
            refusedBirths: view.has_tally ? view.tally_refused_births : 0,
            simTicksPerSecond: view.sim_ticks_per_second,
            renderFPS: measuredFPS,
            lastTickWallMs: view.last_tick_wall_ms,
            lastGpuMs: view.last_gpu_ms,
            metalBatched: view.metal_batched_evals,
            metalDeferred: view.metal_deferred_evals,
            metalResidentPeakBytes: view.metal_resident_peak_bytes,
            metalActive: view.metal_active,
            isPlaying: view.is_playing,
            isCapacityPaused: view.is_capacity_paused,
            hasCellStates: view.cell_states != nil,
            hasPolarities: view.polarities != nil,
            hasTrailEnergy: view.field_trail_energy != nil,
            hasVascular: view.field_vascular != nil,
            followedOriginId: view.has_followed ? view.followed_origin_id : nil,
            followedOrganismId: view.has_followed ? view.followed_organism_id : nil,
            followedHandoffs: view.has_followed ? view.followed_handoffs : 0,
            followedAlive: view.has_followed && view.followed_alive,
            edrHeadroom: latestTelemetry.edrHeadroom
        )
        stateLock.unlock()
    }

    func draw(in view: MTKView) {
        let now = CACurrentMediaTime()

        // Wait for one of the 3 in-flight frame slots BEFORE writing any triple-buffered
        // organism buffer or field texture! If all 3 GPU frames are still busy, drop this
        // display tick cleanly (never inflate the semaphore count!).
        guard inFlightSemaphore.wait(timeout: .now() + .milliseconds(32)) == .success else {
            return
        }

        let drawableSize = view.drawableSize
        guard drawableSize.width >= 2, drawableSize.height >= 2 else {
            inFlightSemaphore.signal()
            return
        }
        ensureOffscreenTargets(width: Int(drawableSize.width), height: Int(drawableSize.height))

        guard let drawable = view.currentDrawable,
              let renderPassDesc = view.currentRenderPassDescriptor,
              let sceneTex = sceneTexture else {
            inFlightSemaphore.signal()
            return
        }

        // Track display FPS honestly (counts presented frames only)
        frameCountWindow += 1
        let fpsElapsed = now - fpsWindowStart
        if fpsElapsed >= 0.4 {
            measuredFPS = Double(frameCountWindow) / fpsElapsed
            frameCountWindow = 0
            fpsWindowStart = now
        }

        let currentStyle = readStyle()
        ingestLatestEpochIfAvailable(now: now, currentStyle: currentStyle)

        guard let cmdBuf = commandQueue.makeCommandBuffer() else {
            inFlightSemaphore.signal()
            return
        }
        cmdBuf.label = "flife.render.frame"
        let sem = inFlightSemaphore
        cmdBuf.addCompletedHandler { _ in
            sem.signal()
        }

        // Detect iPhone Pro Max HDR / EDR headroom
        let edrHeadroom: Float = {
            if let screen = view.window?.screen {
                return Float(max(1.0, screen.currentEDRHeadroom))
            }
            return 1.0
        }()

        stateLock.lock()
        let cam = camera
        let maxSpeed = epochMaxSpeed
        let attemptE = epochAttemptEnergy
        let extent = epochWorldExtent
        let playing = latestTelemetry.isPlaying
        let alpha: Float = (currentStyle.interpolate120Hz && playing)
            ? Float(min(max((now - lastEpochArrivalTime) / max(epochIntervalEMA, 0.004), 0.0), 1.0))
            : 1.0
        lastInterpAlpha = alpha
        latestTelemetry.renderFPS = measuredFPS
        latestTelemetry.edrHeadroom = Double(edrHeadroom)
        let shouldPushTel = (now - lastTelemetryPush) >= 0.08
        var telSnapshot: LiveTelemetrySnapshot? = nil
        if shouldPushTel {
            lastTelemetryPush = now
            telSnapshot = latestTelemetry
        }
        stateLock.unlock()

        if let snap = telSnapshot, let cb = onTelemetryUpdate {
            DispatchQueue.main.async { cb(snap) }
        }

        let activeScale: Float = {
            switch currentStyle.fieldMode {
            case 3: return max(trailDisplayScale, 1e-4)
            case 4: return max(vascularDisplayScale, 1e-4)
            default: return max(energyDisplayScale, 1e-4)
            }
        }()

        let upp = Float(cam.worldUnitsPerPixel(viewport: drawableSize))
        let visibleWorldW = Float(drawableSize.width) * upp
        let visibleWorldH = Float(drawableSize.height) * upp
        // Choose odd periodic tile counts (1, 3, or 5) so tall portrait / wide landscape
        // displays fill 100% edge-to-edge seamlessly across toroidal seams with zero cutoff.
        func oddTiles(_ vis: Float, _ ext: Float) -> UInt32 {
            let ratio = vis / max(ext, 1e-3)
            if ratio <= 0.96 { return 1 }
            if ratio <= 2.90 { return 3 }
            return 5
        }
        let tilesX = oddTiles(visibleWorldW, extent.x)
        let tilesY = oddTiles(visibleWorldH, extent.y)
        let totalTileCopies = Int(tilesX * tilesY)

        var uniforms = MetalFrameUniforms(
            viewportSize: SIMD2(Float(drawableSize.width), Float(drawableSize.height)),
            worldExtent: extent,
            cameraCenter: SIMD2(Float(cam.centerX), Float(cam.centerY)),
            worldUnitsPerPixel: upp,
            interpAlpha: alpha,
            maxSpeed: maxSpeed,
            attemptEnergy: attemptE,
            fieldScale: activeScale,
            pointRadiusPx: currentStyle.pointRadiusPt,
            colorMapping: currentStyle.colorMapping,
            lookMode: currentStyle.lookMode,
            fieldMode: currentStyle.fieldMode,
            showField: currentStyle.showField ? 1 : 0,
            bloomStrength: currentStyle.bloomStrength,
            edrHeadroom: edrHeadroom,
            timeSeconds: Float(now - startTime),
            pixelScale: Float(view.contentScaleFactor),
            selectedPos: cam.selectedPos ?? .zero,
            selectedPrevPos: cam.selectedPrevPos ?? cam.selectedPos ?? .zero,
            followedPos: cam.followedPos ?? .zero,
            followedPrevPos: cam.followedPrevPos ?? cam.followedPos ?? .zero,
            measureCenter: cam.measureCenter ?? .zero,
            brushCenter: cam.brushCenter ?? .zero,
            measureRadius: cam.measureRadius,
            brushRadius: cam.brushRadius,
            hasSelected: cam.selectedPos != nil ? 1 : 0,
            hasFollowed: cam.followedPos != nil ? 1 : 0,
            organismCount: UInt32(max(currentOrganismCount, 1)),
            tilesX: tilesX,
            tilesY: tilesY,
            _pad0: 0
        )

        // PASS 1 & 2: Scene pass (Field + Instanced Organisms) into HDR offscreen texture
        let scenePass = MTLRenderPassDescriptor()
        scenePass.colorAttachments[0].texture = sceneTex
        scenePass.colorAttachments[0].loadAction = .clear
        scenePass.colorAttachments[0].storeAction = .store
        scenePass.colorAttachments[0].clearColor = MTLClearColor(red: 0.006, green: 0.008, blue: 0.018, alpha: 1.0)

        if let enc = cmdBuf.makeRenderCommandEncoder(descriptor: scenePass) {
            enc.label = "ScenePass"
            // 1. Toroidal Field (sampling triple-buffered textures owned by completed slots)
            if let eTex = energyTextures[activeEnergySlot],
               let cTex = compTextures[activeCompSlot],
               let v2Tex = v2OverlayTextures[activeV2Slot] {
                enc.setRenderPipelineState(fieldPipeline)
                enc.setFragmentBytes(&uniforms, length: MemoryLayout<MetalFrameUniforms>.stride, index: 0)
                enc.setFragmentTexture(eTex, index: 0)
                enc.setFragmentTexture(cTex, index: 1)
                enc.setFragmentTexture(v2Tex, index: 2)
                enc.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            }

            // 2. Single-draw-call Instanced SDF Organisms (tiled seamlessly across torus!)
            if currentOrganismCount > 0, let orgBuf = organismBuffers[activeOrganismSlot] {
                enc.setRenderPipelineState(organismPipeline)
                enc.setVertexBytes(&uniforms, length: MemoryLayout<MetalFrameUniforms>.stride, index: 0)
                enc.setVertexBuffer(orgBuf, offset: 0, index: 1)
                enc.setFragmentBytes(&uniforms, length: MemoryLayout<MetalFrameUniforms>.stride, index: 0)
                enc.drawPrimitives(
                    type: .triangle,
                    vertexStart: 0,
                    vertexCount: 6,
                    instanceCount: currentOrganismCount * totalTileCopies
                )
            }
            enc.endEncoding()
        }

        // PASS 3: Dual-Kawase HDR Bloom (Glow look only! Dedicated upsample targets avoid read/write aliasing.)
        var finalBloomTex: MTLTexture = sceneTex
        if currentStyle.lookMode == 0 && currentStyle.bloomStrength > 0.005 && bloomMips.count == 3 && bloomUpMips.count == 2 {
            // Prefilter -> mip 0 (1/2)
            encodeFullscreenPass(cmdBuf: cmdBuf, label: "BloomPrefilter", target: bloomMips[0], pipeline: bloomPrefilterPipeline, tex0: sceneTex, tex1: nil)
            // Downsample 0 -> 1 -> 2 (1/4, 1/8)
            encodeFullscreenPass(cmdBuf: cmdBuf, label: "BloomDown1", target: bloomMips[1], pipeline: bloomDownPipeline, tex0: bloomMips[0], tex1: nil)
            encodeFullscreenPass(cmdBuf: cmdBuf, label: "BloomDown2", target: bloomMips[2], pipeline: bloomDownPipeline, tex0: bloomMips[1], tex1: nil)
            // Upsample 2 + 1 -> up 1 (1/4), then up 1 + 0 -> up 0 (1/2)
            encodeFullscreenPass(cmdBuf: cmdBuf, label: "BloomUp1", target: bloomUpMips[1], pipeline: bloomUpPipeline, tex0: bloomMips[2], tex1: bloomMips[1])
            encodeFullscreenPass(cmdBuf: cmdBuf, label: "BloomUp0", target: bloomUpMips[0], pipeline: bloomUpPipeline, tex0: bloomUpMips[1], tex1: bloomMips[0])
            finalBloomTex = bloomUpMips[0]
        }

        // PASS 4: Composite + EDR Tone Mapping + Precision Vector Overlays -> Display Drawable
        renderPassDesc.colorAttachments[0].loadAction = .dontCare
        if let enc = cmdBuf.makeRenderCommandEncoder(descriptor: renderPassDesc) {
            enc.label = "CompositePass"
            enc.setRenderPipelineState(compositePipeline)
            enc.setFragmentBytes(&uniforms, length: MemoryLayout<MetalFrameUniforms>.stride, index: 0)
            enc.setFragmentTexture(sceneTex, index: 0)
            enc.setFragmentTexture(finalBloomTex, index: 1)
            enc.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            enc.endEncoding()
        }

        cmdBuf.present(drawable)
        cmdBuf.commit()
    }

    private func encodeFullscreenPass(
        cmdBuf: MTLCommandBuffer,
        label: String,
        target: MTLTexture,
        pipeline: MTLRenderPipelineState,
        tex0: MTLTexture,
        tex1: MTLTexture?
    ) {
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .dontCare
        pass.colorAttachments[0].storeAction = .store
        guard let enc = cmdBuf.makeRenderCommandEncoder(descriptor: pass) else { return }
        enc.label = label
        enc.setRenderPipelineState(pipeline)
        enc.setFragmentTexture(tex0, index: 0)
        if let t1 = tex1 {
            enc.setFragmentTexture(t1, index: 1)
        }
        enc.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        enc.endEncoding()
    }
}

private extension Int {
    var nextPowerOfTwo: Int {
        guard self > 1 else { return 1 }
        var p = 1
        while p < self && p < (1 << 29) { p <<= 1 }
        return p
    }
}
