import SwiftUI

struct StyleSheetView: View {
    @Bindable var engine: FrankenLifeEngine
    @Environment(\.dismiss) private var dismiss

    private let columns = [
        GridItem(.flexible(), spacing: 10),
        GridItem(.flexible(), spacing: 10)
    ]

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    // 1. Rendering Look
                    VStack(alignment: .leading, spacing: 10) {
                        SectionHeader(title: "Rendering Look", subtitle: "Pure presentation — never changes scientific state (INV-09)")
                        Picker("Look", selection: $engine.lookMode) {
                            ForEach(RenderLookMode.allCases) { mode in
                                Label(mode.title, systemImage: mode.systemImage).tag(mode)
                            }
                        }
                        .pickerStyle(.segmented)

                        if engine.lookMode == .raw {
                            Text("Gate G5 honest scientific view: unblended dots, zero bloom, linear greyscale field.")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                    }

                    // 2. All 9 Canonical FrankenLife Organism Color Mappings
                    VStack(alignment: .leading, spacing: 10) {
                        SectionHeader(title: "Organism Color Mapping", subtitle: "Canonical FrankenLife palettes & OKLab projections")
                        LazyVGrid(columns: columns, spacing: 10) {
                            ForEach(OrganismColorMapping.allCases) { mapping in
                                let isAvailable = isMappingAvailable(mapping)
                                let selected = engine.colorMapping == mapping
                                Button {
                                    engine.colorMapping = mapping
                                } label: {
                                    VStack(alignment: .leading, spacing: 6) {
                                        HStack {
                                            Text(mapping.title)
                                                .font(.subheadline.weight(.semibold))
                                                .foregroundStyle(isAvailable ? .white : .white.opacity(0.45))
                                                .lineLimit(1)
                                            Spacer()
                                            if selected {
                                                Image(systemName: "checkmark.circle.fill")
                                                    .foregroundStyle(Color(hex: 0x2EC4B6))
                                                    .font(.subheadline)
                                            }
                                        }

                                        HStack(spacing: 3) {
                                            ForEach(Array(mapping.swatches.enumerated()), id: \.offset) { _, color in
                                                RoundedRectangle(cornerRadius: 3)
                                                    .fill(color)
                                                    .frame(height: 8)
                                            }
                                        }

                                        Text(isAvailable ? mapping.shortDescription : "Enable a V2 preset to populate")
                                            .font(.caption2)
                                            .foregroundStyle(.secondary)
                                            .lineLimit(2)
                                            .multilineTextAlignment(.leading)
                                    }
                                    .padding(11)
                                    .background(
                                        selected ? Color(hex: 0x2EC4B6).opacity(0.16) : .white.opacity(0.06),
                                        in: RoundedRectangle(cornerRadius: 14, style: .continuous)
                                    )
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                                            .strokeBorder(
                                                selected ? Color(hex: 0x2EC4B6).opacity(0.8) : .white.opacity(0.10),
                                                lineWidth: selected ? 1.2 : 0.7
                                            )
                                    )
                                }
                                .disabled(!isAvailable)
                            }
                        }
                    }

                    // 3. Shared Chemical Field Layer
                    VStack(alignment: .leading, spacing: 10) {
                        HStack {
                            SectionHeader(title: "Chemical Field Layer", subtitle: "Shared torus energy & 32-channel chemistry")
                            Spacer()
                            Toggle("", isOn: $engine.showField)
                                .labelsHidden()
                                .tint(Color(hex: 0x2EC4B6))
                        }

                        if engine.showField {
                            ScrollView(.horizontal, showsIndicators: false) {
                                HStack(spacing: 8) {
                                    ForEach(FieldLayerMode.allCases) { mode in
                                        let available = isFieldModeAvailable(mode)
                                        let selected = engine.fieldMode == mode
                                        Button {
                                            engine.fieldMode = mode
                                        } label: {
                                            Text(mode.title)
                                                .font(.caption.weight(.semibold))
                                                .foregroundStyle(selected ? .black : (available ? .white : .white.opacity(0.35)))
                                                .padding(.horizontal, 12)
                                                .padding(.vertical, 8)
                                                .background(
                                                    selected ? Color(hex: 0x2EC4B6) : .white.opacity(0.08),
                                                    in: Capsule()
                                                )
                                        }
                                        .disabled(!available)
                                    }
                                }
                            }

                            if engine.fieldMode == .channel {
                                HStack {
                                    Text("Moment Channel \(engine.fieldChannel)")
                                        .font(.caption.monospacedDigit().weight(.semibold))
                                    Spacer()
                                    Stepper("", value: $engine.fieldChannel, in: 0...31)
                                        .labelsHidden()
                                }
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                                .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 12))
                            }
                        }
                    }

                    // 4. ProMotion 120Hz & HDR Glow Tuning
                    VStack(alignment: .leading, spacing: 12) {
                        SectionHeader(title: "ProMotion & HDR Display", subtitle: "Display-only pacing & optical tuning")

                        VStack(spacing: 12) {
                            HStack {
                                Text("Organism Radius")
                                    .font(.subheadline)
                                Spacer()
                                Text(String(format: "%.1f pt", engine.pointRadiusPt))
                                    .font(.caption.monospacedDigit())
                                    .foregroundStyle(.secondary)
                            }
                            Slider(value: $engine.pointRadiusPt, in: 1.2...7.5, step: 0.1)
                                .tint(Color(hex: 0x2EC4B6))

                            HStack {
                                Text("HDR Bloom Strength")
                                    .font(.subheadline)
                                    .foregroundStyle(engine.lookMode == .glow ? .primary : .secondary)
                                Spacer()
                                Text(engine.lookMode == .glow ? "\(Int(engine.bloomStrength * 100))%" : "Off in \(engine.lookMode.title)")
                                    .font(.caption.monospacedDigit())
                                    .foregroundStyle(.secondary)
                            }
                            Slider(value: $engine.bloomStrength, in: 0.0...1.6, step: 0.05)
                                .tint(Color(hex: 0x9B5DE5))
                                .disabled(engine.lookMode != .glow)

                            Toggle("GPU 120Hz Toroidal Interpolation", isOn: $engine.interpolate120Hz)
                                .font(.subheadline)
                                .tint(Color(hex: 0x2EC4B6))

                            HStack {
                                Text("Display Refresh Target")
                                    .font(.subheadline)
                                Spacer()
                                Picker("", selection: $engine.preferredFPS) {
                                    Text("120 Hz").tag(120)
                                    Text("60 Hz").tag(60)
                                    Text("30 Hz").tag(30)
                                }
                                .pickerStyle(.segmented)
                                .frame(width: 190)
                            }
                        }
                        .padding(14)
                        .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                    }
                }
                .padding(18)
            }
            .navigationTitle("Visual Style")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .fontWeight(.semibold)
                }
            }
        }
    }

    private func isMappingAvailable(_ mapping: OrganismColorMapping) -> Bool {
        switch mapping {
        case .cellState: return engine.telemetry.hasCellStates || (engine.activePreset?.hasCellState ?? false)
        case .polarity: return engine.telemetry.hasPolarities || (engine.activePreset?.hasPolarity ?? false)
        default: return true
        }
    }

    private func isFieldModeAvailable(_ mode: FieldLayerMode) -> Bool {
        switch mode {
        case .trailCarrier: return engine.telemetry.hasTrailEnergy || (engine.activePreset?.hasTrailCarrier ?? false)
        case .vascularConductance: return engine.telemetry.hasVascular || (engine.activePreset?.hasVascularField ?? false)
        default: return true
        }
    }
}

struct SectionHeader: View {
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.headline)
                .foregroundStyle(.white)
            Text(subtitle)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}
