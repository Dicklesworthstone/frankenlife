import Foundation
import Metal
import Observation
import SwiftUI
import UIKit
import FlifeIOSBridge

/// Bundled FrankenLife preset parsed directly by Rust's `flife_inspect_preset` (zero hardcoded metadata).
struct PresetDescriptor: Identifiable, Hashable, Sendable {
    let id: String
    let fileName: String
    let displayName: String
    let subtitle: String
    let category: PresetCategory
    let tomlContent: String

    let model: String
    let defaultColorMapping: String
    let weightDtype: String
    let worldWidth: Double
    let worldHeight: Double
    let gridWidth: UInt32
    let gridHeight: UInt32
    let initialCount: UInt32
    let maximumCount: UInt32
    let dtNumerator: UInt32
    let dtDenominator: UInt32
    let hasContact: Bool
    let hasV2Visuals: Bool
    let hasTrailCarrier: Bool
    let hasVascularField: Bool
    let hasCellState: Bool
    let hasPolarity: Bool
    let maxTotalBytes: UInt64

    enum PresetCategory: String, CaseIterable, Sendable {
        case featuredV1 = "Featured Ecosystems"
        case v2Morphogenesis = "V2 Morphogenesis & Supercolonies"
        case benchmarks = "Scale & Reference"
    }
}

/// Canonical FrankenLife organism color mappings matching `crates/flife-manim/src/mapping.rs`.
enum OrganismColorMapping: UInt32, CaseIterable, Identifiable, Sendable {
    case lineage = 0
    case energy = 1
    case generation = 2
    case motion = 3
    case topology = 4
    case affinity = 5
    case lineageEnergy = 6
    case cellState = 7
    case polarity = 8

    var id: UInt32 { rawValue }

    var title: String {
        switch self {
        case .lineage: return "Lineage"
        case .energy: return "Energy"
        case .generation: return "Generation"
        case .motion: return "Motion"
        case .topology: return "Topology"
        case .affinity: return "Affinity"
        case .lineageEnergy: return "Lineage × Energy"
        case .cellState: return "Cell State (V2)"
        case .polarity: return "Polarity (V2)"
        }
    }

    var shortDescription: String {
        switch self {
        case .lineage: return "Stable categorical hue per founder"
        case .energy: return "Magma ramp, half-way at 2.0 E"
        case .generation: return "Logarithmic plasma from founder to 256+"
        case .motion: return "Heading hue, greyer when still"
        case .topology: return "Neural architecture catalog index"
        case .affinity: return "OKLab Hadamard chemistry projection"
        case .lineageEnergy: return "Lineage hue lit by division energy"
        case .cellState: return "Recurrent differentiation h₀ in lineage"
        case .polarity: return "Apical-basal angle wheel + 3D ticks"
        }
    }

    var isV2: Bool { self == .cellState || self == .polarity }

    var swatches: [Color] {
        switch self {
        case .lineage:
            return [
                Color(hex: 0x2EC4B6), Color(hex: 0x9B5DE5), Color(hex: 0xF15BB5),
                Color(hex: 0x9BF6FF), Color(hex: 0xFFB627), Color(hex: 0xB5E61D)
            ]
        case .energy:
            return [
                Color(hex: 0x3B0F70), Color(hex: 0x8C2981), Color(hex: 0xDE4968),
                Color(hex: 0xFE9F6D), Color(hex: 0xFCFDBF)
            ]
        case .generation:
            return [
                Color(hex: 0x9C179E), Color(hex: 0xD8576B), Color(hex: 0xFB9F3A),
                Color(hex: 0xF0F921)
            ]
        case .motion:
            return [
                Color(hex: 0xFF6F59), Color(hex: 0xFFB627), Color(hex: 0xB5E61D),
                Color(hex: 0x2EC4B6), Color(hex: 0x4EA8DE), Color(hex: 0x9B5DE5)
            ]
        case .topology:
            return [
                Color(hex: 0x9BF6FF), Color(hex: 0x2EC4B6), Color(hex: 0xB5E61D),
                Color(hex: 0xFFB627), Color(hex: 0xF15BB5)
            ]
        case .affinity:
            return [
                Color(hex: 0x737373), Color(hex: 0xE0607E), Color(hex: 0xD49A3A),
                Color(hex: 0x40B884), Color(hex: 0x4A98E0), Color(hex: 0xA86CE0)
            ]
        case .lineageEnergy:
            return [
                Color(hex: 0x0B2E2B), Color(hex: 0x1D7A72), Color(hex: 0x2EC4B6),
                Color(hex: 0x99EDE5), Color(hex: 0xF2FFFD)
            ]
        case .cellState:
            return [
                Color(hex: 0x18524B), Color(hex: 0x248A80), Color(hex: 0x2EC4B6),
                Color(hex: 0x7BE0D4), Color(hex: 0xD7FAF4)
            ]
        case .polarity:
            return [
                Color(hex: 0x585E6E), Color(hex: 0xE57399), Color(hex: 0xD8A24A),
                Color(hex: 0x57C49B), Color(hex: 0x6AA3F2), Color(hex: 0xB980EE)
            ]
        }
    }

    static func fromRegistry(_ name: String) -> OrganismColorMapping {
        switch name {
        case "energy-lightness-v1": return .energy
        case "generation-log-v1": return .generation
        case "motion-heading-v1": return .motion
        case "topology-catalog-v1": return .topology
        case "affinity-projection-v1": return .affinity
        case "founder-lineage-energy-v1": return .lineageEnergy
        case "cell-state-v1": return .cellState
        case "polarity-v1": return .polarity
        default: return .lineage
        }
    }
}

enum RenderLookMode: UInt32, CaseIterable, Identifiable, Sendable {
    case glow = 0
    case sphere = 1
    case raw = 2

    var id: UInt32 { rawValue }

    var title: String {
        switch self {
        case .glow: return "Glow HDR"
        case .sphere: return "3D Sphere"
        case .raw: return "Raw G5"
        }
    }

    var systemImage: String {
        switch self {
        case .glow: return "sparkle"
        case .sphere: return "circle.inset.filled"
        case .raw: return "dot.scope"
        }
    }
}

enum FieldLayerMode: UInt32, CaseIterable, Identifiable, Sendable {
    case energy = 0
    case composition = 1
    case channel = 2
    case trailCarrier = 3
    case vascularConductance = 4

    var id: UInt32 { rawValue }

    var title: String {
        switch self {
        case .energy: return "Energy"
        case .composition: return "Chemistry"
        case .channel: return "Channel"
        case .trailCarrier: return "Trails (V2)"
        case .vascularConductance: return "Vascular (V2)"
        }
    }
}

struct OrganismDetail: Identifiable, Sendable {
    let id: UInt64
    let tick: UInt64
    let genomeId: UInt64
    let parentId: UInt64?
    let lineageId: UInt64
    let generation: UInt32
    let birthTick: UInt64
    let ageTicks: UInt64
    let posX: Double
    let posY: Double
    let velX: Double
    let velY: Double
    let energy: Double
    let energyChangeRate: Double
    let topologyIndex: UInt32
    let hiddenWidths: [UInt32]
    let parameterCount: UInt64
    let localDensity: Double
    let localRequest: Double
    let localGain: Double
    let localToxicity: Double
    let affinity: [Double]
    let secretoryReadiness: Double?
    let neighborH0: Double?
    let childrenBorn: UInt64?
    let childrenAlive: UInt64?
    let descendantsBorn: UInt64?
    let descendantsAlive: UInt64?

    var speed: Double { hypot(velX, velY) }

    var lineageColor: Color {
        var z = lineageId &+ 0x9E37_79B9_7F4A_7C15
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        z ^= (z >> 31)
        let palette: [UInt32] = [
            0x2EC4B6, 0x9B5DE5, 0xF15BB5, 0x9BF6FF, 0xFFB627, 0xB5E61D,
            0xFF6F59, 0x4EA8DE, 0xFEE440, 0xFF99C8, 0x7AE582, 0xC8B6FF
        ]
        return Color(hex: palette[Int(z % 12)])
    }
}

struct RegionMeasureDetail: Sendable {
    let tick: UInt64
    let centerX: Double
    let centerY: Double
    let radius: Double
    let organisms: UInt64
    let organismEnergy: Double
    let lineages: UInt64
    let cells: UInt64
    let fieldEnergy: Double
}

struct ParityVerificationReport: Sendable {
    let matched: Bool
    let ticksVerified: UInt64
    let liveFinal: UInt64
    let metalBatched: UInt64
    let metalDeferred: UInt64
    let metalDeviceMs: Double
    let metalWallMs: Double
    let cpuWallMs: Double
    let ledgerResidual: Double
    let metalDigestPrefix: String
    let cpuDigestPrefix: String
    let error: String?

    var speedupRatio: Double {
        metalWallMs > 0.01 ? (cpuWallMs / metalWallMs) : 1.0
    }
}

/// Main Observable engine controller coordinating the Rust simulation worker,
/// native Apple Metal compute backend, 120Hz ProMotion HDR Metal renderer,
/// and SwiftUI Observatory chrome.
@MainActor
@Observable
final class FrankenLifeEngine {
    let metalDevice: MTLDevice
    let computeEngine: MetalControllerEngine?
    let renderer: MetalWorldRenderer

    private var sessionPtr: OpaquePointer?

    // Presets & configuration
    private(set) var presets: [PresetDescriptor] = []
    private(set) var activePreset: PresetDescriptor?
    var useMetalCompute: Bool = true
    var cpuWorkers: Int = min(max(ProcessInfo.processInfo.activeProcessorCount - 2, 2), 6)

    // Playback & Display pacing (pure presentation — INV-09)
    var isPlaying: Bool = true
    var preferredFPS: Int = 120
    var thermalState: ProcessInfo.ThermalState = ProcessInfo.processInfo.thermalState
    var cinemaMode: Bool = false

    // Visual style state synced immediately to renderer
    var colorMapping: OrganismColorMapping = .lineage { didSet { syncRenderStyle() } }
    var lookMode: RenderLookMode = .glow { didSet { syncRenderStyle() } }
    var fieldMode: FieldLayerMode = .energy { didSet { syncRenderStyle() } }
    var fieldChannel: Int = 0 { didSet { syncRenderStyle() } }
    var showField: Bool = true { didSet { syncRenderStyle() } }
    var bloomStrength: Double = 0.42 { didSet { syncRenderStyle() } }
    var pointRadiusPt: Double = 2.0 { didSet { syncRenderStyle() } }
    var interpolate120Hz: Bool = true { didSet { syncRenderStyle() } }

    // Interactive tools & scientific intervention parameters
    var activeTool: ObservatoryTool = .explore {
        didSet {
            if activeTool != .measure {
                clearMeasurement()
            }
            renderer.camera.brushRadius = 0
            selectionHaptic.selectionChanged()
        }
    }
    var energyDose: Double = 140.0
    var introduceCount: Int = 64
    var brushRadiusWorld: Double = 4.5
    var removeDisposition: UInt32 = 2 // 0 = Remove, 1 = Dissipate, 2 = Recycle
    var materialBrushDirection: [Double] = {
        // Default coherent unit direction along Hadamard axis 7
        var d = [Double](repeating: 0.0, count: 32)
        let norm = 1.0 / sqrt(32.0)
        for c in 0..<32 {
            d[c] = ((c & 7).nonzeroBitCount % 2 == 0) ? norm : -norm
        }
        return d
    }()
    var materialBrushLabel: String = "Default Axis"

    // Pure inspection & measurement outputs
    private(set) var selectedOrganism: OrganismDetail?
    private(set) var regionMeasurement: RegionMeasureDetail?
    private(set) var isFollowingSelected: Bool = false

    // Live telemetry & rolling sparkline histories
    private(set) var telemetry = LiveTelemetrySnapshot()
    private(set) var populationHistory: [Double] = []
    private(set) var tpsHistory: [Double] = []
    private(set) var gpuMsHistory: [Double] = []
    private(set) var statusBanner: String?
    private(set) var isLoadingWorld: Bool = false

    // On-device Metal vs CPU bit-exactness self-test
    private(set) var isRunningParityTest: Bool = false
    private(set) var parityReport: ParityVerificationReport?

    private let impactLight = UIImpactFeedbackGenerator(style: .light)
    private let impactMedium = UIImpactFeedbackGenerator(style: .medium)
    private let selectionHaptic = UISelectionFeedbackGenerator()
    private var thermalObserver: NSObjectProtocol?
    private var lastInspectorRefreshTime: CFTimeInterval = 0

    var targetDisplayFPS: Int {
        // Honest thermal adaptation: scales DISPLAY refresh only; never changes Δt or simulation rules (INV-09).
        switch thermalState {
        case .critical: return min(preferredFPS, 30)
        case .serious: return min(preferredFPS, 60)
        default: return preferredFPS
        }
    }

    var gpuCapabilities: MetalControllerEngine.DeviceCapabilities? {
        computeEngine?.capabilities
    }

    init() {
        guard let dev = MTLCreateSystemDefaultDevice() else {
            fatalError("FrankenLife requires an Apple Metal GPU device.")
        }
        self.metalDevice = dev
        self.computeEngine = try? MetalControllerEngine(device: dev)
        do {
            self.renderer = try MetalWorldRenderer(device: dev)
        } catch {
            fatalError("Failed to initialize FrankenLife MetalWorldRenderer: \(error)")
        }

        self.renderer.onTelemetryUpdate = { [weak self] snap in
            self?.handleTelemetryUpdate(snap)
        }

        thermalObserver = NotificationCenter.default.addObserver(
            forName: ProcessInfo.thermalStateDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.thermalState = ProcessInfo.processInfo.thermalState
            }
        }

        loadBundledPresets()
        syncRenderStyle()

        // Launch directly into a rich, living ecosystem!
        let defaultId = presets.first(where: { $0.id == "tissue-world" })?.id
            ?? presets.first(where: { $0.id == "interactive-world" })?.id
            ?? presets.first?.id
        if let preset = presets.first(where: { $0.id == defaultId }) {
            loadPreset(preset)
        }
    }

    func shutdown() {
        if let obs = thermalObserver {
            NotificationCenter.default.removeObserver(obs)
            thermalObserver = nil
        }
        if let ptr = sessionPtr {
            renderer.attachSession(nil, worldWidth: 64, worldHeight: 64)
            flife_session_destroy(ptr)
            sessionPtr = nil
        }
    }

    // MARK: - Preset Discovery (Real Rust TOML Validation)

    private func loadBundledPresets() {
        let catalogMeta: [(String, String, String, PresetDescriptor.PresetCategory)] = [
            ("tissue-world", "Tissue World", "Continuous multicolored colonies & textured membranes", .featuredV1),
            ("interactive-world", "Interactive 25k", "Balanced 25k F16 ecology tuned for live touch play", .featuredV1),
            ("colony-world", "Colony World", "Single-color cellular colonies with distinct rims", .featuredV1),
            ("showcase-world", "Showcase World", "Classic bioluminescent multi-lineage clusters", .featuredV1),
            ("confluent-world", "Confluent World", "Dense contact-adhesion tissue filling the torus", .featuredV1),
            ("supercolony-v2-abundant", "Supercolony V2 (Abundant)", "V2 vascular arteries + persistent pheromone trails", .v2Morphogenesis),
            ("supercolony-v2", "Supercolony V2", "Pure-field INV-08 eusocial foraging networks", .v2Morphogenesis),
            ("morphogenesis-tissue-v2-abundant", "Morphogenesis Tissue V2+", "Epigenetic cell states h₀ + apical-basal polarity", .v2Morphogenesis),
            ("morphogenesis-tissue-v2", "Morphogenesis Tissue V2", "Clonal differentiation, cadherin sorting & PCP", .v2Morphogenesis),
            ("reference-small-world", "Reference 4k (F32)", "Canonical 4,096-founder F32 scientific baseline", .benchmarks),
            ("desktop-world", "Desktop 100k (Pro Max)", "Massive 100k-scale F16 stress world for Pro Max RAM", .benchmarks)
        ]

        var loaded: [PresetDescriptor] = []
        for (stem, title, subtitle, category) in catalogMeta {
            guard let url = Bundle.main.url(forResource: stem, withExtension: "toml", subdirectory: "Presets")
                ?? Bundle.main.url(forResource: stem, withExtension: "toml"),
                  let text = try? String(contentsOf: url, encoding: .utf8) else {
                continue
            }
            var summary = FlifePresetSummary()
            text.withCString { cStr in
                flife_inspect_preset(cStr, &summary)
            }
            guard summary.valid else { continue }

            loaded.append(PresetDescriptor(
                id: stem,
                fileName: "\(stem).toml",
                displayName: title,
                subtitle: subtitle,
                category: category,
                tomlContent: text,
                model: cArrayToString(summary.model),
                defaultColorMapping: cArrayToString(summary.color_mapping),
                weightDtype: cArrayToString(summary.weight_dtype).uppercased(),
                worldWidth: summary.world_width,
                worldHeight: summary.world_height,
                gridWidth: summary.grid_width,
                gridHeight: summary.grid_height,
                initialCount: summary.initial_count,
                maximumCount: summary.maximum_count,
                dtNumerator: summary.tick_numerator,
                dtDenominator: summary.tick_denominator,
                hasContact: summary.has_contact,
                hasV2Visuals: summary.has_v2_visuals,
                hasTrailCarrier: summary.has_trail_carrier,
                hasVascularField: summary.has_vascular_field,
                hasCellState: summary.has_cell_state,
                hasPolarity: summary.has_polarity,
                maxTotalBytes: summary.max_total_bytes
            ))
        }
        self.presets = loaded
    }

    func loadPreset(_ preset: PresetDescriptor) {
        isLoadingWorld = true
        statusBanner = nil

        if let old = sessionPtr {
            renderer.attachSession(nil, worldWidth: preset.worldWidth, worldHeight: preset.worldHeight)
            flife_session_destroy(old)
            sessionPtr = nil
        }

        selectedOrganism = nil
        regionMeasurement = nil
        isFollowingSelected = false
        populationHistory.removeAll(keepingCapacity: true)
        tpsHistory.removeAll(keepingCapacity: true)
        gpuMsHistory.removeAll(keepingCapacity: true)

        // Apply the preset's native visual mapping if sensible
        colorMapping = OrganismColorMapping.fromRegistry(preset.defaultColorMapping)
        if preset.hasTrailCarrier && fieldMode == .energy {
            // Keep Energy as default clean background; user can toggle Trails / Vascular in Style sheet
            fieldMode = .energy
        } else if !preset.hasTrailCarrier && (fieldMode == .trailCarrier || fieldMode == .vascularConductance) {
            fieldMode = .energy
        }

        // Adapt default dot radius cleanly to organism density so colonies keep crisp cellular texture
        pointRadiusPt = preset.initialCount >= 20_000 ? 1.55 : (preset.initialCount >= 8_000 ? 1.85 : 2.35)

        let wantMetal = useMetalCompute
        let workers = UInt32(max(1, cpuWorkers))
        let compute = self.computeEngine

        var errBuf = [CChar](repeating: 0, count: 512)
        let created: OpaquePointer? = preset.tomlContent.withCString { cToml in
            if wantMetal, let compute {
                var vtable = compute.makeVtable()
                return flife_session_create(cToml, &vtable, workers, &errBuf, errBuf.count)
            } else {
                return flife_session_create(cToml, nil, workers, &errBuf, errBuf.count)
            }
        }

        if let created {
            self.sessionPtr = created
            self.activePreset = preset
            self.isPlaying = true
            flife_session_set_playing(created, true)
            renderer.attachSession(created, worldWidth: preset.worldWidth, worldHeight: preset.worldHeight)
            syncRenderStyle()
            impactMedium.impactOccurred()
        } else {
            let msg = String(cString: errBuf)
            statusBanner = "Failed to found \(preset.displayName): \(msg)"
        }
        isLoadingWorld = false
    }

    func reloadActivePreset() {
        if let preset = activePreset {
            loadPreset(preset)
        }
    }

    // MARK: - Playback Controls

    func togglePlayPause() {
        guard let ptr = sessionPtr else { return }
        isPlaying.toggle()
        flife_session_set_playing(ptr, isPlaying)
        impactLight.impactOccurred()
    }

    func stepOnce() {
        guard let ptr = sessionPtr else { return }
        isPlaying = false
        flife_session_step_once(ptr)
        selectionHaptic.selectionChanged()
    }

    // MARK: - Camera Navigation

    func panCamera(dxPx: Double, dyPx: Double, viewportPx: CGSize) {
        if isFollowingSelected {
            stopFollowing()
        }
        let upp = renderer.camera.worldUnitsPerPixel(viewport: viewportPx)
        let lw = renderer.camera.worldWidth
        let lh = renderer.camera.worldHeight
        var nx = (renderer.camera.centerX - dxPx * upp).truncatingRemainder(dividingBy: lw)
        var ny = (renderer.camera.centerY - dyPx * upp).truncatingRemainder(dividingBy: lh)
        if nx < 0 { nx += lw }
        if ny < 0 { ny += lh }
        renderer.camera.centerX = nx
        renderer.camera.centerY = ny
    }

    func zoomCamera(by factor: Double, anchorPx: CGPoint, viewportPx: CGSize) {
        let before = renderer.camera.screenPointToWorld(anchorPx, viewportPx: viewportPx)
        renderer.camera.zoom = min(max(renderer.camera.zoom * factor, 0.65), 18.0)
        let after = renderer.camera.screenPointToWorld(anchorPx, viewportPx: viewportPx)
        let lw = renderer.camera.worldWidth
        let lh = renderer.camera.worldHeight
        var dx = before.x - after.x
        var dy = before.y - after.y
        dx -= lw * (dx / lw).rounded()
        dy -= lh * (dy / lh).rounded()
        var nx = (renderer.camera.centerX + dx).truncatingRemainder(dividingBy: lw)
        var ny = (renderer.camera.centerY + dy).truncatingRemainder(dividingBy: lh)
        if nx < 0 { nx += lw }
        if ny < 0 { ny += lh }
        renderer.camera.centerX = nx
        renderer.camera.centerY = ny
    }

    func resetCamera() {
        stopFollowing()
        renderer.camera.centerX = renderer.camera.worldWidth * 0.5
        renderer.camera.centerY = renderer.camera.worldHeight * 0.5
        renderer.camera.zoom = 1.0
        impactLight.impactOccurred()
    }

    // MARK: - Inspection, Follow & Scientific Interventions

    func handleCanvasTap(worldPos: SIMD2<Double>, pickRadius: Double) {
        if let pickedId = renderer.pickOrganism(near: worldPos, maxWorldRadius: pickRadius) {
            selectOrganism(id: pickedId)
            impactLight.impactOccurred()
        } else if selectedOrganism != nil {
            dismissInspector()
        }
    }

    func selectOrganism(id: UInt64) {
        renderer.camera.selectedOrganismId = id
        if let ptr = sessionPtr {
            flife_session_set_watch(ptr, true, id)
        }
        refreshSelectedOrganism()
    }

    func dismissInspector() {
        stopFollowing()
        renderer.camera.selectedOrganismId = nil
        renderer.camera.selectedPos = nil
        selectedOrganism = nil
        if let ptr = sessionPtr {
            flife_session_set_watch(ptr, false, 0)
        }
    }

    func toggleFollowSelected() {
        guard let ptr = sessionPtr, let org = selectedOrganism else { return }
        if isFollowingSelected {
            stopFollowing()
        } else {
            isFollowingSelected = true
            flife_session_set_follow(ptr, true, org.id)
            impactMedium.impactOccurred()
        }
    }

    func stopFollowing() {
        guard isFollowingSelected else { return }
        isFollowingSelected = false
        renderer.camera.followedPos = nil
        if let ptr = sessionPtr {
            flife_session_set_follow(ptr, false, 0)
        }
    }

    func adoptSelectedAffinityForBrush() {
        guard let org = selectedOrganism else { return }
        materialBrushDirection = org.affinity
        materialBrushLabel = "Organism #\(org.id)"
        activeTool = .materialBrush
        impactMedium.impactOccurred()
    }

    private func refreshSelectedOrganism() {
        guard let ptr = sessionPtr, let targetId = renderer.camera.selectedOrganismId else { return }
        var info = FlifeOrganismInfo()
        if flife_session_inspect_organism(ptr, targetId, &info), info.found {
            let widths = (0..<Int(min(info.hidden_layer_count, 4))).map { i -> UInt32 in
                switch i {
                case 0: return info.hidden_widths.0
                case 1: return info.hidden_widths.1
                case 2: return info.hidden_widths.2
                default: return info.hidden_widths.3
                }
            }
            let aff: [Double] = withUnsafeBytes(of: info.affinity) { raw in
                Array(raw.bindMemory(to: Double.self))
            }
            selectedOrganism = OrganismDetail(
                id: info.id,
                tick: info.tick,
                genomeId: info.genome_id,
                parentId: info.has_parent ? info.parent_id : nil,
                lineageId: info.lineage_id,
                generation: info.generation,
                birthTick: info.birth_tick,
                ageTicks: info.age_ticks,
                posX: info.pos_x,
                posY: info.pos_y,
                velX: info.vel_x,
                velY: info.vel_y,
                energy: info.energy,
                energyChangeRate: info.energy_change_rate,
                topologyIndex: info.topology_index,
                hiddenWidths: widths,
                parameterCount: info.parameter_count,
                localDensity: info.local_density,
                localRequest: info.local_request,
                localGain: info.local_gain,
                localToxicity: info.local_toxicity,
                affinity: aff,
                secretoryReadiness: info.has_secretory_readiness ? info.secretory_readiness : nil,
                neighborH0: info.has_neighbor_h0 ? info.neighbor_h0 : nil,
                childrenBorn: info.has_descendants ? info.children_born : nil,
                childrenAlive: info.has_descendants ? info.children_alive : nil,
                descendantsBorn: info.has_descendants ? info.descendants_born : nil,
                descendantsAlive: info.has_descendants ? info.descendants_alive : nil
            )
        } else if isFollowingSelected, let heirId = telemetry.followedOrganismId, telemetry.followedAlive, heirId != targetId {
            // Smoothly hand off inspector selection to followed heir!
            renderer.camera.selectedOrganismId = heirId
            flife_session_set_watch(ptr, true, heirId)
        }
    }

    func setBrushPreview(at worldPos: SIMD2<Double>, active: Bool) {
        if active {
            renderer.camera.brushCenter = SIMD2(Float(worldPos.x), Float(worldPos.y))
            renderer.camera.brushRadius = Float(activeTool == .removeDisk ? brushRadiusWorld : max(1.2, brushRadiusWorld * 0.5))
        } else {
            renderer.camera.brushRadius = 0
        }
    }

    func applyActiveTool(at worldPos: SIMD2<Double>) {
        guard let ptr = sessionPtr, let preset = activePreset else { return }
        let gx = UInt32(min(max(Int(worldPos.x / preset.worldWidth * Double(preset.gridWidth)), 0), Int(preset.gridWidth) - 1))
        let gy = UInt32(min(max(Int(worldPos.y / preset.worldHeight * Double(preset.gridHeight)), 0), Int(preset.gridHeight) - 1))

        switch activeTool {
        case .injectEnergy:
            _ = flife_session_inject_energy(ptr, gx, gy, energyDose)
            impactLight.impactOccurred(intensity: 0.6)
        case .materialBrush:
            materialBrushDirection.withUnsafeBufferPointer { buf in
                if let base = buf.baseAddress {
                    _ = flife_session_inject_material(ptr, gx, gy, energyDose, base)
                }
            }
            impactLight.impactOccurred(intensity: 0.6)
        case .introduce:
            _ = flife_session_introduce_organisms(ptr, gx, gy, UInt32(introduceCount), 2.5)
            impactMedium.impactOccurred()
        case .removeDisk:
            _ = flife_session_remove_organisms(ptr, worldPos.x, worldPos.y, brushRadiusWorld, removeDisposition)
            impactMedium.impactOccurred()
        case .explore, .measure:
            break
        }
    }

    func updateMeasurement(center: SIMD2<Double>, radius: Double) {
        guard let ptr = sessionPtr else { return }
        renderer.camera.measureCenter = SIMD2(Float(center.x), Float(center.y))
        renderer.camera.measureRadius = Float(radius)
        var m = FlifeRegionMeasure()
        if flife_session_measure_region(ptr, center.x, center.y, radius, &m), m.valid {
            regionMeasurement = RegionMeasureDetail(
                tick: m.tick,
                centerX: m.center_x,
                centerY: m.center_y,
                radius: m.radius,
                organisms: m.organisms,
                organismEnergy: m.organism_energy,
                lineages: m.lineages,
                cells: m.cells,
                fieldEnergy: m.field_energy
            )
        }
    }

    func clearMeasurement() {
        renderer.camera.measureCenter = nil
        renderer.camera.measureRadius = 0
        regionMeasurement = nil
    }

    // MARK: - Live Bit-Exact Metal Parity Self-Test (Gate G4)

    func runLiveParitySelfTest() {
        guard !isRunningParityTest,
              let compute = computeEngine,
              let preset = presets.first(where: { $0.id == "reference-small-world" }) ?? activePreset else {
            return
        }
        isRunningParityTest = true
        parityReport = nil
        let toml = preset.tomlContent
        let workers = UInt32(max(1, cpuWorkers))

        Task.detached(priority: .userInitiated) {
            var vtable = compute.makeVtable()
            var res = FlifeParityResult()
            toml.withCString { cToml in
                flife_verify_metal_parity(cToml, &vtable, 16, workers, &res)
            }
            let report = ParityVerificationReport(
                matched: res.success && res.digest_matched,
                ticksVerified: res.ticks_verified,
                liveFinal: res.live_final,
                metalBatched: res.metal_batched,
                metalDeferred: res.metal_deferred,
                metalDeviceMs: Double(res.metal_device_ns) * 1e-6,
                metalWallMs: Double(res.metal_wall_ns) * 1e-6,
                cpuWallMs: Double(res.cpu_wall_ns) * 1e-6,
                ledgerResidual: res.ledger_residual,
                metalDigestPrefix: String(cArrayToString(res.metal_digest_hex).prefix(16)),
                cpuDigestPrefix: String(cArrayToString(res.cpu_digest_hex).prefix(16)),
                error: res.success ? nil : cArrayToString(res.error)
            )
            await MainActor.run { [weak self] in
                self?.parityReport = report
                self?.isRunningParityTest = false
            }
        }
    }

    // MARK: - Internal Synchronization

    private func syncRenderStyle() {
        renderer.style = RenderStyleState(
            colorMapping: colorMapping.rawValue,
            lookMode: lookMode.rawValue,
            fieldMode: fieldMode.rawValue,
            fieldChannel: UInt8(clamping: fieldChannel),
            showField: showField,
            bloomStrength: Float(bloomStrength),
            pointRadiusPt: Float(pointRadiusPt),
            interpolate120Hz: interpolate120Hz
        )
    }

    private func handleTelemetryUpdate(_ snap: LiveTelemetrySnapshot) {
        let tickChanged = snap.tick != telemetry.tick
        self.telemetry = snap

        if tickChanged {
            appendSparkline(&populationHistory, Double(snap.liveCount))
            appendSparkline(&tpsHistory, snap.simTicksPerSecond)
            appendSparkline(&gpuMsHistory, snap.lastGpuMs)

            let now = CACurrentMediaTime()
            if selectedOrganism != nil && now - lastInspectorRefreshTime >= 0.2 {
                lastInspectorRefreshTime = now
                refreshSelectedOrganism()
            }
            if let m = regionMeasurement, activeTool == .measure {
                updateMeasurement(center: SIMD2(m.centerX, m.centerY), radius: m.radius)
            }
        }
    }

    private func appendSparkline(_ arr: inout [Double], _ value: Double) {
        arr.append(value)
        if arr.count > 90 {
            arr.removeFirst(arr.count - 90)
        }
    }
}

// MARK: - Utilities

func cArrayToString<T>(_ tuple: T) -> String {
    withUnsafeBytes(of: tuple) { raw in
        let bytes = raw.prefix { $0 != 0 }
        return String(decoding: bytes, as: UTF8.self)
    }
}

extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: r, green: g, blue: b, opacity: alpha)
    }
}
