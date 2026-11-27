import SwiftUI
import MetalKit
import UIKit

/// Interactive touch tool mode routed cleanly to FrankenLife scientific interventions or pure inspection.
enum ObservatoryTool: String, CaseIterable, Identifiable, Sendable {
    case explore
    case injectEnergy
    case materialBrush
    case introduce
    case removeDisk
    case measure

    var id: String { rawValue }

    var label: String {
        switch self {
        case .explore: return "Explore"
        case .injectEnergy: return "Feed"
        case .materialBrush: return "Material"
        case .introduce: return "Seed"
        case .removeDisk: return "Remove"
        case .measure: return "Measure"
        }
    }

    var systemImage: String {
        switch self {
        case .explore: return "hand.draw"
        case .injectEnergy: return "bolt.fill"
        case .materialBrush: return "paintbrush.pointed.fill"
        case .introduce: return "sparkles"
        case .removeDisk: return "scissors.circle.fill"
        case .measure: return "ruler"
        }
    }

    var isScientificIntervention: Bool {
        switch self {
        case .explore, .measure: return false
        case .injectEnergy, .materialBrush, .introduce, .removeDisk: return true
        }
    }
}

/// Full-bleed 120Hz ProMotion HDR `MTKView` wrapped for SwiftUI with native
/// multi-touch gestures (pinch-to-zoom, toroidal pan, tap-to-inspect, double-tap reset,
/// and precision drag brushes/measurement disk).
struct MetalCanvasView: UIViewRepresentable {
    let engine: FrankenLifeEngine

    func makeCoordinator() -> Coordinator {
        Coordinator(engine: engine)
    }

    func makeUIView(context: Context) -> MTKView {
        let mtkView = MTKView()
        mtkView.device = engine.metalDevice
        mtkView.delegate = engine.renderer
        mtkView.preferredFramesPerSecond = engine.targetDisplayFPS
        mtkView.colorPixelFormat = .rgba16Float
        mtkView.framebufferOnly = true
        mtkView.isPaused = false
        mtkView.enableSetNeedsDisplay = false
        mtkView.clearColor = MTLClearColor(red: 0.006, green: 0.008, blue: 0.018, alpha: 1.0)
        mtkView.backgroundColor = UIColor(red: 0.008, green: 0.011, blue: 0.025, alpha: 1.0)

        // Enable Extended Dynamic Range (EDR) on iPhone Pro / Pro Max OLED displays!
        if let metalLayer = mtkView.layer as? CAMetalLayer {
            metalLayer.wantsExtendedDynamicRangeContent = true
            metalLayer.colorspace = CGColorSpace(name: CGColorSpace.extendedLinearDisplayP3)
            metalLayer.pixelFormat = .rgba16Float
        }

        let coord = context.coordinator
        coord.mtkView = mtkView

        let pan = UIPanGestureRecognizer(target: coord, action: #selector(Coordinator.handlePan(_:)))
        pan.minimumNumberOfTouches = 1
        pan.maximumNumberOfTouches = 2
        pan.delegate = coord

        let pinch = UIPinchGestureRecognizer(target: coord, action: #selector(Coordinator.handlePinch(_:)))
        pinch.delegate = coord

        let tap = UITapGestureRecognizer(target: coord, action: #selector(Coordinator.handleTap(_:)))
        tap.numberOfTapsRequired = 1

        let doubleTap = UITapGestureRecognizer(target: coord, action: #selector(Coordinator.handleDoubleTap(_:)))
        doubleTap.numberOfTapsRequired = 2
        tap.require(toFail: doubleTap)

        mtkView.addGestureRecognizer(pan)
        mtkView.addGestureRecognizer(pinch)
        mtkView.addGestureRecognizer(tap)
        mtkView.addGestureRecognizer(doubleTap)
        mtkView.isMultipleTouchEnabled = true

        return mtkView
    }

    func updateUIView(_ uiView: MTKView, context: Context) {
        if uiView.preferredFramesPerSecond != engine.targetDisplayFPS {
            uiView.preferredFramesPerSecond = engine.targetDisplayFPS
        }
        let shouldPause = engine.isAppBackgrounded
        if uiView.isPaused != shouldPause {
            uiView.isPaused = shouldPause
        }
    }

    @MainActor
    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        let engine: FrankenLifeEngine
        weak var mtkView: MTKView?
        private var lastPanPoint: CGPoint = .zero
        private var measureOriginWorld: SIMD2<Double>?
        private var lastBrushApplyTime: CFTimeInterval = 0
        private var wasNavigationPan: Bool = false
        private var lockedToNavigation: Bool = false

        init(engine: FrankenLifeEngine) {
            self.engine = engine
        }

        func gestureRecognizer(
            _ gestureRecognizer: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
        ) -> Bool {
            // Allow pinch + 2-finger pan simultaneously in Explore mode
            if (gestureRecognizer is UIPinchGestureRecognizer && otherGestureRecognizer is UIPanGestureRecognizer)
                || (gestureRecognizer is UIPanGestureRecognizer && otherGestureRecognizer is UIPinchGestureRecognizer) {
                return engine.activeTool == .explore || gestureRecognizer.numberOfTouches == 2
            }
            return false
        }

        private func viewPointToWorld(_ pt: CGPoint) -> SIMD2<Double>? {
            guard let view = mtkView, view.drawableSize.width > 0 else { return nil }
            let scale = view.contentScaleFactor
            let px = CGPoint(x: pt.x * scale, y: pt.y * scale)
            return engine.renderer.readCamera().screenPointToWorld(px, viewportPx: view.drawableSize)
        }

        @objc func handleTap(_ gr: UITapGestureRecognizer) {
            guard let view = mtkView, let worldPt = viewPointToWorld(gr.location(in: view)) else { return }
            let upp = engine.renderer.readCamera().worldUnitsPerPixel(viewport: view.drawableSize)
            let pickRadius = max(24.0 * Double(view.contentScaleFactor) * upp, 0.8)

            switch engine.activeTool {
            case .explore:
                engine.handleCanvasTap(worldPos: worldPt, pickRadius: pickRadius)
            case .injectEnergy, .materialBrush, .introduce, .removeDisk:
                engine.applyActiveTool(at: worldPt)
            case .measure:
                engine.updateMeasurement(center: worldPt, radius: engine.brushRadiusWorld)
            }
        }

        @objc func handleDoubleTap(_ gr: UITapGestureRecognizer) {
            engine.resetCamera()
        }

        @objc func handlePinch(_ gr: UIPinchGestureRecognizer) {
            guard let view = mtkView else { return }
            if gr.state == .changed {
                let scaleFactor = Double(gr.scale)
                gr.scale = 1.0
                let loc = gr.location(in: view)
                let scale = view.contentScaleFactor
                let px = CGPoint(x: loc.x * scale, y: loc.y * scale)
                engine.zoomCamera(by: scaleFactor, anchorPx: px, viewportPx: view.drawableSize)
            }
        }

        @objc func handlePan(_ gr: UIPanGestureRecognizer) {
            guard let view = mtkView else { return }
            let loc = gr.location(in: view)

            if gr.state == .began {
                lockedToNavigation = (engine.activeTool == .explore) || (gr.numberOfTouches >= 2)
                wasNavigationPan = lockedToNavigation
                lastPanPoint = loc
            } else if gr.numberOfTouches >= 2 {
                // Once a second finger joins, lock this gesture to navigation until release
                // so lifting one finger never accidentally fires Remove/Feed/Measure.
                lockedToNavigation = true
            }

            if gr.state == .ended || gr.state == .cancelled {
                engine.setBrushPreview(at: .zero, active: false)
                measureOriginWorld = nil
                wasNavigationPan = false
                lockedToNavigation = false
                return
            }

            if lockedToNavigation {
                if !wasNavigationPan {
                    // Transitioning mid-gesture from 1-finger brush to 2-finger pan:
                    // clear brush ring and re-anchor pan point so camera never teleports!
                    engine.setBrushPreview(at: .zero, active: false)
                    measureOriginWorld = nil
                    lastPanPoint = loc
                    wasNavigationPan = true
                    return
                }
                if gr.state == .changed {
                    let dxPt = loc.x - lastPanPoint.x
                    let dyPt = loc.y - lastPanPoint.y
                    lastPanPoint = loc
                    let s = view.contentScaleFactor
                    engine.panCamera(dxPx: Double(dxPt * s), dyPx: Double(dyPt * s), viewportPx: view.drawableSize)
                }
                return
            }

            guard let worldPt = viewPointToWorld(loc) else { return }
            let now = CACurrentMediaTime()

            if engine.activeTool == .measure {
                switch gr.state {
                case .began:
                    measureOriginWorld = worldPt
                    engine.updateMeasurement(center: worldPt, radius: 1.5)
                case .changed:
                    if let origin = measureOriginWorld {
                        let cam = engine.renderer.readCamera()
                        let lw = cam.worldWidth
                        let lh = cam.worldHeight
                        var dx = worldPt.x - origin.x
                        var dy = worldPt.y - origin.y
                        dx -= lw * (dx / lw).rounded()
                        dy -= lh * (dy / lh).rounded()
                        let r = max(0.5, min(hypot(dx, dy), 0.45 * min(lw, lh)))
                        engine.updateMeasurement(center: origin, radius: r)
                    }
                default:
                    break
                }
            } else {
                // Continuous brush painting throttled cleanly to ~20 Hz
                switch gr.state {
                case .began:
                    engine.setBrushPreview(at: worldPt, active: true)
                    engine.applyActiveTool(at: worldPt)
                    lastBrushApplyTime = now
                case .changed:
                    engine.setBrushPreview(at: worldPt, active: true)
                    if now - lastBrushApplyTime >= 0.05 {
                        engine.applyActiveTool(at: worldPt)
                        lastBrushApplyTime = now
                    }
                default:
                    break
                }
            }
        }
    }
}
