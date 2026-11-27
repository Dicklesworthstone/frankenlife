import Foundation
import Metal
import FlifeIOSBridge

/// Native Apple iOS Metal compute engine implementing FrankenLife's exact
/// `ControllerBackend` (`softfloat64.metal` + `controller.metal`) with
/// IEEE-safe math (`mathMode = .safe`), persistent `storageModeShared` unified
/// memory parameter shards, and concurrent multi-shard command encoding for
/// Apple Silicon Pro GPUs (iPhone 18 Pro Max and Apple7+ iPhones).
final class MetalControllerEngine: @unchecked Sendable {
    struct DeviceCapabilities: Sendable {
        let deviceName: String
        let gpuFamilyLabel: String
        let recommendedWorkingSetBytes: UInt64
        let maxBufferLength: UInt64
        let hasUnifiedMemory: Bool
        let supportsDynamicCaching: Bool
    }

    let device: MTLDevice
    let capabilities: DeviceCapabilities

    private let commandQueue: MTLCommandQueue
    private let forwardPipeline: MTLComputePipelineState
    private let forwardHalfPipeline: MTLComputePipelineState

    private let lock = NSLock()
    private let dispatchLock = NSLock()
    private var nextHandle: UInt64 = 1
    private var shards: [UInt64: MTLBuffer] = [:]

    // Reusable staging buffers grown by powers of two (zero per-tick allocations).
    private var inputsBuffer: MTLBuffer?
    private var outputsBuffer: MTLBuffer?
    private var faultsBuffer: MTLBuffer?
    private var dimsBuffer: MTLBuffer?
    private var cachedDims: (UInt32, UInt32) = (0, 0)
    private var jobsBuffers: [MTLBuffer] = []

    // Stable C string for `FlifeMetalVtable.device_name`.
    private let deviceNameCString: UnsafeMutablePointer<CChar>

    enum EngineError: Error, LocalizedError {
        case metalUnavailable
        case commandQueueFailed
        case shaderResourceMissing(String)
        case shaderCompileFailed(String)
        case pipelineMissing(String)

        var errorDescription: String? {
            switch self {
            case .metalUnavailable: return "Metal GPU device unavailable on this platform."
            case .commandQueueFailed: return "Failed to create Metal compute command queue."
            case .shaderResourceMissing(let name): return "Missing bundled Metal shader source: \(name)"
            case .shaderCompileFailed(let msg): return "Metal shader compilation failed: \(msg)"
            case .pipelineMissing(let fn): return "Missing Metal compute kernel function: \(fn)"
            }
        }
    }

    init(device existingDevice: MTLDevice? = nil, bundle: Bundle? = nil) throws {
        guard let dev = existingDevice ?? MTLCreateSystemDefaultDevice() else {
            throw EngineError.metalUnavailable
        }
        guard let queue = dev.makeCommandQueue() else {
            throw EngineError.commandQueueFailed
        }

        let mslSource = try Self.loadControllerMSL(preferredBundle: bundle)
        let options = MTLCompileOptions()
        // CRITICAL (INV-13 / Gate G4): IEEE-safe math reproduces ReferenceController bit-for-bit (0 ULP).
        options.mathMode = .safe
        options.mathFloatingPointFunctions = .precise
        options.preserveInvariance = true

        let library: MTLLibrary
        do {
            library = try dev.makeLibrary(source: mslSource, options: options)
        } catch {
            throw EngineError.shaderCompileFailed(error.localizedDescription)
        }

        guard let fwdFn = library.makeFunction(name: "controller_forward") else {
            throw EngineError.pipelineMissing("controller_forward")
        }
        guard let fwdHalfFn = library.makeFunction(name: "controller_forward_half") else {
            throw EngineError.pipelineMissing("controller_forward_half")
        }

        let fwdPipe = try dev.makeComputePipelineState(function: fwdFn)
        let fwdHalfPipe = try dev.makeComputePipelineState(function: fwdHalfFn)

        self.device = dev
        self.commandQueue = queue
        self.commandQueue.label = "com.frankenlife.ios.compute"
        self.capabilities = Self.probeCapabilities(device: dev)
        self.forwardPipeline = fwdPipe
        self.forwardHalfPipeline = fwdHalfPipe
        // Allocate C string LAST after every throwing call so a throwing init never leaks.
        self.deviceNameCString = strdup(dev.name)
    }

    deinit {
        free(deviceNameCString)
    }

    private static func probeCapabilities(device: MTLDevice) -> DeviceCapabilities {
        let family: String
        if #available(iOS 19.0, *), device.supportsFamily(.apple10) {
            family = "Apple10 (A19/A20 Pro Class)"
        } else if device.supportsFamily(.apple9) {
            family = "Apple9 (A17/A18 Pro Dynamic Caching)"
        } else if device.supportsFamily(.apple8) {
            family = "Apple8 (A15/A16 Bionic)"
        } else if device.supportsFamily(.apple7) {
            family = "Apple7 (A14 / M1 Class)"
        } else {
            family = "Apple GPU"
        }
        let dynamicCaching = device.supportsFamily(.apple9)
        return DeviceCapabilities(
            deviceName: device.name,
            gpuFamilyLabel: family,
            recommendedWorkingSetBytes: UInt64(device.recommendedMaxWorkingSetSize),
            maxBufferLength: UInt64(device.maxBufferLength),
            hasUnifiedMemory: device.hasUnifiedMemory,
            supportsDynamicCaching: dynamicCaching
        )
    }

    private static func loadControllerMSL(preferredBundle: Bundle?) throws -> String {
        let candidateBundles: [Bundle] = [
            preferredBundle,
            Bundle.main,
            Bundle(for: MetalControllerEngine.self)
        ].compactMap { $0 }

        func findText(_ name: String) -> String? {
            for b in candidateBundles {
                if let url = b.url(forResource: name, withExtension: "txt", subdirectory: "Shaders")
                    ?? b.url(forResource: name, withExtension: "txt") {
                    if let text = try? String(contentsOf: url, encoding: .utf8) {
                        return text
                    }
                }
            }
            return nil
        }

        guard let softfloat = findText("softfloat64.metal") else {
            throw EngineError.shaderResourceMissing("softfloat64.metal.txt")
        }
        guard let controller = findText("controller.metal") else {
            throw EngineError.shaderResourceMissing("controller.metal.txt")
        }
        return softfloat + "\n" + controller
    }

    /// Build the C-ABI callback vtable passed to `flife_session_create` or `flife_verify_metal_parity`.
    /// Caller MUST keep this `MetalControllerEngine` instance alive while any Rust session holds the vtable.
    func makeVtable() -> FlifeMetalVtable {
        FlifeMetalVtable(
            ctx: Unmanaged.passUnretained(self).toOpaque(),
            max_buffer_bytes: capabilities.maxBufferLength,
            recommended_working_set_bytes: capabilities.recommendedWorkingSetBytes,
            device_name: UnsafePointer(deviceNameCString),
            alloc_shard: { ctx, bytes, outContents in
                guard let ctx else { return 0 }
                let engine = Unmanaged<MetalControllerEngine>.fromOpaque(ctx).takeUnretainedValue()
                return engine.allocShard(bytes: bytes, outContents: outContents)
            },
            free_shard: { ctx, handle in
                guard let ctx else { return }
                let engine = Unmanaged<MetalControllerEngine>.fromOpaque(ctx).takeUnretainedValue()
                engine.freeShard(handle: handle)
            },
            write_shard: { ctx, handle, offset, data, len in
                guard let ctx, let data else { return -1 }
                let engine = Unmanaged<MetalControllerEngine>.fromOpaque(ctx).takeUnretainedValue()
                return engine.writeShard(handle: handle, offset: offset, data: data, len: len)
            },
            copy_shard: { ctx, srcHandle, dstHandle, byteLen in
                guard let ctx else { return -1 }
                let engine = Unmanaged<MetalControllerEngine>.fromOpaque(ctx).takeUnretainedValue()
                return engine.copyShard(src: srcHandle, dst: dstHandle, byteLen: byteLen)
            },
            dispatch_batch: { ctx, dispatches, dispatchCount, totalJobs, inputWidth, outputWidth, inputs, outputs, faults, outGpuNs in
                guard let ctx, let dispatches, let inputs, let outputs, let faults, let outGpuNs else {
                    return -1
                }
                let engine = Unmanaged<MetalControllerEngine>.fromOpaque(ctx).takeUnretainedValue()
                return engine.dispatchBatch(
                    dispatches: dispatches,
                    dispatchCount: Int(dispatchCount),
                    totalJobs: Int(totalJobs),
                    inputWidth: inputWidth,
                    outputWidth: outputWidth,
                    inputs: inputs,
                    outputs: outputs,
                    faults: faults,
                    outGpuNs: outGpuNs
                )
            }
        )
    }

    // MARK: - Shard Buffer Lifecycle (Unified Memory `storageModeShared`)

    private func allocShard(
        bytes: UInt64,
        outContents: UnsafeMutablePointer<UnsafeMutablePointer<UInt8>?>?
    ) -> UInt64 {
        guard bytes <= capabilities.maxBufferLength else {
            outContents?.pointee = nil
            return 0
        }
        let allocLen = max(16, Int(bytes))
        guard let buf = device.makeBuffer(length: allocLen, options: .storageModeShared) else {
            outContents?.pointee = nil
            return 0
        }
        let rawPtr = buf.contents()
        memset(rawPtr, 0, allocLen)
        outContents?.pointee = rawPtr.assumingMemoryBound(to: UInt8.self)
        lock.lock()
        defer { lock.unlock() }
        let h = nextHandle
        nextHandle &+= 1
        if nextHandle == 0 { nextHandle = 1 }
        shards[h] = buf
        return h
    }

    private func freeShard(handle: UInt64) {
        lock.lock()
        shards.removeValue(forKey: handle)
        lock.unlock()
    }

    private func writeShard(handle: UInt64, offset: UInt64, data: UnsafePointer<UInt8>, len: UInt64) -> Int32 {
        lock.lock()
        let buf = shards[handle]
        lock.unlock()
        guard let buf else { return -2 }
        let off = Int(offset)
        let n = Int(len)
        guard off >= 0, n >= 0, off + n <= buf.length else { return -3 }
        memcpy(buf.contents().advanced(by: off), data, n)
        return 0
    }

    private func copyShard(src: UInt64, dst: UInt64, byteLen: UInt64) -> Int32 {
        lock.lock()
        let srcBuf = shards[src]
        let dstBuf = shards[dst]
        lock.unlock()
        guard let srcBuf, let dstBuf else { return -2 }
        let n = Int(byteLen)
        guard n >= 0, n <= srcBuf.length, n <= dstBuf.length else { return -3 }
        memcpy(dstBuf.contents(), srcBuf.contents(), n)
        return 0
    }

    private func ensureBuffer(_ slot: inout MTLBuffer?, minBytes: Int) -> MTLBuffer? {
        if let existing = slot, existing.length >= minBytes {
            return existing
        }
        var cap = max(256, minBytes)
        // Round up to power of two for amortized O(1) growth
        var p = 256
        while p < cap && p < (1 << 30) { p <<= 1 }
        cap = max(p, minBytes)
        guard let fresh = device.makeBuffer(length: cap, options: .storageModeShared) else {
            return nil
        }
        slot = fresh
        return fresh
    }

    // MARK: - Concurrent Multi-Shard Batch Dispatch

    private func dispatchBatch(
        dispatches: UnsafePointer<FlifeMetalShardDispatch>,
        dispatchCount: Int,
        totalJobs: Int,
        inputWidth: UInt32,
        outputWidth: UInt32,
        inputs: UnsafePointer<Float>,
        outputs: UnsafeMutablePointer<Float>,
        faults: UnsafeMutablePointer<UInt32>,
        outGpuNs: UnsafeMutablePointer<UInt64>
    ) -> Int32 {
        dispatchLock.lock()
        defer { dispatchLock.unlock() }

        if dispatchCount == 0 || totalJobs == 0 {
            outGpuNs.pointee = 0
            return 0
        }

        let inBytes = totalJobs * Int(inputWidth) * MemoryLayout<Float>.stride
        let outBytes = totalJobs * Int(outputWidth) * MemoryLayout<Float>.stride
        let faultBytes = totalJobs * MemoryLayout<UInt32>.stride

        guard let inBuf = ensureBuffer(&inputsBuffer, minBytes: inBytes),
              let outBuf = ensureBuffer(&outputsBuffer, minBytes: outBytes),
              let fltBuf = ensureBuffer(&faultsBuffer, minBytes: faultBytes),
              let dBuf = ensureBuffer(&dimsBuffer, minBytes: 8) else {
            return -10
        }

        memcpy(inBuf.contents(), inputs, inBytes)

        if cachedDims.0 != inputWidth || cachedDims.1 != outputWidth {
            let dimsPtr = dBuf.contents().assumingMemoryBound(to: UInt32.self)
            dimsPtr[0] = inputWidth
            dimsPtr[1] = outputWidth
            cachedDims = (inputWidth, outputWidth)
        }

        // Resolve shard buffers and stage per-shard job descriptors.
        var resolvedShards: [(MTLBuffer, MTLBuffer, Int, Int, Bool)] = []
        resolvedShards.reserveCapacity(dispatchCount)

        while jobsBuffers.count < dispatchCount {
            guard let jb = device.makeBuffer(length: 4096, options: .storageModeShared) else {
                return -11
            }
            jobsBuffers.append(jb)
        }

        lock.lock()
        for i in 0..<dispatchCount {
            let spec = dispatches[i]
            guard let shardBuf = shards[spec.shard_handle],
                  let jobWordsPtr = spec.job_words else {
                lock.unlock()
                return -12
            }
            let jobCount = Int(spec.job_count)
            let wordsBytes = jobCount * 5 * MemoryLayout<UInt32>.stride
            if jobsBuffers[i].length < wordsBytes {
                var cap = max(4096, wordsBytes)
                var p = 4096
                while p < cap && p < (1 << 28) { p <<= 1 }
                cap = max(p, wordsBytes)
                guard let grown = device.makeBuffer(length: cap, options: .storageModeShared) else {
                    lock.unlock()
                    return -13
                }
                jobsBuffers[i] = grown
            }
            memcpy(jobsBuffers[i].contents(), jobWordsPtr, wordsBytes)
            resolvedShards.append((
                shardBuf,
                jobsBuffers[i],
                jobCount,
                max(1, Int(spec.threads_per_tg)),
                spec.is_f16 != 0
            ))
        }
        lock.unlock()

        guard let cmdBuf = commandQueue.makeCommandBuffer() else {
            return -14
        }
        cmdBuf.label = "flife.controller.batch"

        // Concurrent compute encoding lets independent shards saturate all Apple Silicon Pro GPU cores.
        let encoder: MTLComputeCommandEncoder? = dispatchCount > 1
            ? cmdBuf.makeComputeCommandEncoder(dispatchType: .concurrent)
            : cmdBuf.makeComputeCommandEncoder()
        guard let enc = encoder else {
            return -15
        }

        for (shardBuf, jobBuf, jobCount, threadsPerTG, isF16) in resolvedShards {
            if jobCount == 0 { continue }
            enc.setComputePipelineState(isF16 ? forwardHalfPipeline : forwardPipeline)
            enc.setBuffer(shardBuf, offset: 0, index: 0)
            enc.setBuffer(inBuf, offset: 0, index: 1)
            enc.setBuffer(jobBuf, offset: 0, index: 2)
            enc.setBuffer(outBuf, offset: 0, index: 3)
            enc.setBuffer(fltBuf, offset: 0, index: 4)
            enc.setBuffer(dBuf, offset: 0, index: 5)

            let tgGrid = MTLSize(width: jobCount, height: 1, depth: 1)
            let tgSize = MTLSize(width: threadsPerTG, height: 1, depth: 1)
            enc.dispatchThreadgroups(tgGrid, threadsPerThreadgroup: tgSize)
        }
        enc.endEncoding()

        cmdBuf.commit()
        cmdBuf.waitUntilCompleted()

        if cmdBuf.status != .completed {
            return -16
        }

        memcpy(outputs, outBuf.contents(), outBytes)
        memcpy(faults, fltBuf.contents(), faultBytes)

        let gpuStart = cmdBuf.gpuStartTime
        let gpuEnd = cmdBuf.gpuEndTime
        if gpuEnd > gpuStart {
            outGpuNs.pointee = UInt64((gpuEnd - gpuStart) * 1_000_000_000.0)
        } else {
            outGpuNs.pointee = 0
        }
        return 0
    }
}
