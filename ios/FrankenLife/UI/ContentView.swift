import SwiftUI

struct ContentView: View {
    @Bindable var engine: FrankenLifeEngine

    @State private var showPresetsSheet = false
    @State private var showStyleSheet = false
    @State private var showTelemetrySheet = false

    var body: some View {
        ZStack {
            // Edge-to-edge 120Hz ProMotion HDR Metal Ecosystem Canvas
            MetalCanvasView(engine: engine)
                .ignoresSafeArea()

            // Floating Observatory HUD Chrome
            if !engine.cinemaMode {
                VStack(spacing: 0) {
                    TelemetryHUD(
                        engine: engine,
                        onOpenPresets: { showPresetsSheet = true },
                        onOpenTelemetry: { showTelemetrySheet = true },
                        onToggleCinema: {
                            withAnimation(.spring(duration: 0.32, bounce: 0.12)) {
                                engine.cinemaMode = true
                            }
                        }
                    )
                    .padding(.horizontal, 14)
                    .padding(.top, 6)

                    if let banner = engine.statusBanner {
                        Text(banner)
                            .font(.caption.weight(.medium))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 8)
                            .background(.red.opacity(0.82), in: Capsule())
                            .padding(.top, 8)
                    }

                    Spacer()

                    // Contextual Inspector / Measurement floating card
                    if let org = engine.selectedOrganism {
                        InspectorCardView(engine: engine, organism: org)
                            .padding(.horizontal, 14)
                            .padding(.bottom, 10)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                    } else if let measure = engine.regionMeasurement {
                        MeasureOverlayView(measure: measure, onClear: { engine.clearMeasurement() })
                            .padding(.horizontal, 14)
                            .padding(.bottom, 10)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                    }

                    // Contextual brush tuning strip when an intervention tool is active
                    if engine.activeTool.isScientificIntervention {
                        ToolParameterStrip(engine: engine)
                            .padding(.horizontal, 14)
                            .padding(.bottom, 8)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                    }

                    // Bottom Floating Observatory Control Dock
                    BottomObservatoryDock(
                        engine: engine,
                        onOpenStyle: { showStyleSheet = true },
                        onOpenPresets: { showPresetsSheet = true }
                    )
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
                }
                .transition(.opacity)
            } else {
                // Cinema Mode: unobtrusive floating restore pill
                VStack {
                    HStack {
                        Spacer()
                        Button {
                            withAnimation(.spring(duration: 0.32, bounce: 0.12)) {
                                engine.cinemaMode = false
                            }
                        } label: {
                            Label("HUD", systemImage: "eye.fill")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.white.opacity(0.86))
                                .padding(.horizontal, 12)
                                .padding(.vertical, 7)
                                .background(.ultraThinMaterial, in: Capsule())
                                .overlay(Capsule().strokeBorder(.white.opacity(0.18), lineWidth: 0.6))
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 10)
                    Spacer()
                }
                .transition(.opacity)
            }
        }
        .animation(.spring(duration: 0.28, bounce: 0.14), value: engine.selectedOrganism?.id)
        .animation(.spring(duration: 0.28, bounce: 0.14), value: engine.regionMeasurement != nil)
        .animation(.spring(duration: 0.24, bounce: 0.12), value: engine.activeTool)
        .sheet(isPresented: $showStyleSheet) {
            StyleSheetView(engine: engine)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .presentationBackground(.ultraThinMaterial)
        }
        .sheet(isPresented: $showPresetsSheet) {
            PresetPickerView(engine: engine)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .presentationBackground(.ultraThinMaterial)
        }
        .sheet(isPresented: $showTelemetrySheet) {
            TelemetrySheetView(engine: engine)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .presentationBackground(.ultraThinMaterial)
        }
    }
}

// MARK: - Bottom Control Dock

private struct BottomObservatoryDock: View {
    @Bindable var engine: FrankenLifeEngine
    let onOpenStyle: () -> Void
    let onOpenPresets: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            // Play / Pause primary transport button
            Button {
                engine.togglePlayPause()
            } label: {
                Image(systemName: engine.isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 16, weight: .bold))
                    .foregroundStyle(.black)
                    .frame(width: 40, height: 40)
                    .background(
                        LinearGradient(
                            colors: [Color(hex: 0x2EC4B6), Color(hex: 0x7AE582)],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        ),
                        in: Circle()
                    )
                    .shadow(color: Color(hex: 0x2EC4B6).opacity(0.42), radius: 8, y: 2)
            }
            .accessibilityLabel(engine.isPlaying ? "Pause Simulation" : "Play Simulation")

            if !engine.isPlaying {
                Button {
                    engine.stepOnce()
                } label: {
                    Image(systemName: "forward.frame.fill")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 34, height: 34)
                        .background(.white.opacity(0.14), in: Circle())
                }
                .accessibilityLabel("Step One Tick")
            }

            // Interactive Tool Rail — all 6 tools fit cleanly on Pro Max without clipping
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 2) {
                    ForEach(ObservatoryTool.allCases) { tool in
                        let selected = engine.activeTool == tool
                        Button {
                            engine.activeTool = tool
                        } label: {
                            HStack(spacing: 4) {
                                Image(systemName: tool.systemImage)
                                    .font(.system(size: 12, weight: .semibold))
                                if selected {
                                    Text(tool.label)
                                        .font(.caption.weight(.semibold))
                                        .lineLimit(1)
                                }
                            }
                            .foregroundStyle(selected ? .white : .white.opacity(0.68))
                            .padding(.horizontal, selected ? 10 : 7.5)
                            .padding(.vertical, 8)
                            .background(
                                selected
                                    ? AnyShapeStyle(tool.isScientificIntervention
                                        ? Color(hex: 0xFF6F59).opacity(0.32)
                                        : Color(hex: 0x2EC4B6).opacity(0.28))
                                    : AnyShapeStyle(.clear),
                                in: Capsule()
                            )
                            .overlay(
                                Capsule().strokeBorder(
                                    selected ? .white.opacity(0.30) : .clear,
                                    lineWidth: 0.7
                                )
                            )
                        }
                    }
                }
            }
            .frame(height: 36)

            // Style & World Buttons
            HStack(spacing: 5) {
                Button(action: onOpenStyle) {
                    Image(systemName: "paintpalette.fill")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 34, height: 34)
                        .background(.white.opacity(0.12), in: Circle())
                }
                .accessibilityLabel("Visual Style")

                Button(action: onOpenPresets) {
                    Image(systemName: "globe.americas.fill")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 34, height: 34)
                        .background(.white.opacity(0.12), in: Circle())
                }
                .accessibilityLabel("Worlds and Presets")
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(.ultraThinMaterial, in: Capsule())
        .overlay(
            Capsule().strokeBorder(
                LinearGradient(
                    colors: [.white.opacity(0.26), .white.opacity(0.08)],
                    startPoint: .top,
                    endPoint: .bottom
                ),
                lineWidth: 0.8
            )
        )
        .shadow(color: .black.opacity(0.45), radius: 16, y: 8)
    }
}

// MARK: - Contextual Scientific Tool Parameter Strip

private struct ToolParameterStrip: View {
    @Bindable var engine: FrankenLifeEngine

    var body: some View {
        HStack(spacing: 12) {
            Label(engine.activeTool.label, systemImage: engine.activeTool.systemImage)
                .font(.caption.weight(.bold))
                .foregroundStyle(Color(hex: 0xFFB627))

            switch engine.activeTool {
            case .injectEnergy:
                Slider(value: $engine.energyDose, in: 20...600, step: 10)
                    .tint(Color(hex: 0xFFB627))
                Text("+\(Int(engine.energyDose)) E")
                    .font(.caption.monospacedDigit().weight(.semibold))
                    .foregroundStyle(.white.opacity(0.9))
                    .frame(width: 58, alignment: .trailing)

            case .materialBrush:
                Text(engine.materialBrushLabel)
                    .font(.caption2.weight(.medium))
                    .foregroundStyle(.white.opacity(0.75))
                    .lineLimit(1)
                Slider(value: $engine.energyDose, in: 20...600, step: 10)
                    .tint(Color(hex: 0xF15BB5))
                Text("+\(Int(engine.energyDose)) E")
                    .font(.caption.monospacedDigit().weight(.semibold))
                    .foregroundStyle(.white.opacity(0.9))

            case .introduce:
                Slider(
                    value: Binding(
                        get: { Double(engine.introduceCount) },
                        set: { engine.introduceCount = Int($0) }
                    ),
                    in: 8...512,
                    step: 8
                )
                .tint(Color(hex: 0x7AE582))
                Text("+\(engine.introduceCount) orgs")
                    .font(.caption.monospacedDigit().weight(.semibold))
                    .foregroundStyle(.white.opacity(0.9))
                    .frame(width: 68, alignment: .trailing)

            case .removeDisk:
                Slider(value: $engine.brushRadiusWorld, in: 1.5...16.0, step: 0.5)
                    .tint(Color(hex: 0xFF6F59))
                Picker("", selection: $engine.removeDisposition) {
                    Text("Recycle").tag(UInt32(2))
                    Text("Remove").tag(UInt32(0))
                    Text("Dissipate").tag(UInt32(1))
                }
                .pickerStyle(.menu)
                .tint(.white)

            case .explore, .measure:
                EmptyView()
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .strokeBorder(.white.opacity(0.18), lineWidth: 0.7)
        )
    }
}
