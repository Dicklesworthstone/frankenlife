import SwiftUI

struct PresetPickerView: View {
    @Bindable var engine: FrankenLifeEngine
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                // Compute Engine & Apple Silicon Controls
                Section {
                    Toggle(isOn: $engine.useMetalCompute) {
                        HStack(spacing: 10) {
                            Image(systemName: "bolt.circle.fill")
                                .font(.title2)
                                .foregroundStyle(Color(hex: 0x9BF6FF))
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Apple Metal GPU Neural Compute")
                                    .font(.subheadline.weight(.semibold))
                                Text(engine.gpuCapabilities?.deviceName ?? "Bit-exact 0-ULP GPU inference")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    .tint(Color(hex: 0x2EC4B6))
                    .disabled(engine.computeEngine == nil)

                    HStack {
                        Label("CPU Workers", systemImage: "cpu")
                            .font(.subheadline)
                        Spacer()
                        Stepper("\(engine.cpuWorkers) threads", value: $engine.cpuWorkers, in: 1...8)
                            .font(.subheadline.monospacedDigit())
                    }

                    Button {
                        engine.reloadActivePreset()
                        dismiss()
                    } label: {
                        Label("Apply & Refound Current World", systemImage: "arrow.clockwise")
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(Color(hex: 0x2EC4B6))
                    }
                } header: {
                    Text("Execution Engine")
                } footer: {
                    Text("Metal executes FrankenLife's softfloat64 + controller kernels in IEEE-safe mode (.safe) overlapping with CPU chemistry phases. Identical SHA-256 world digests to CPU Reference.")
                }

                // Categorized Real TOML Presets
                ForEach(PresetDescriptor.PresetCategory.allCases, id: \.self) { category in
                    let items = engine.presets.filter { $0.category == category }
                    if !items.isEmpty {
                        Section(category.rawValue) {
                            ForEach(items) { preset in
                                let isActive = engine.activePreset?.id == preset.id
                                Button {
                                    engine.loadPreset(preset)
                                    dismiss()
                                } label: {
                                    VStack(alignment: .leading, spacing: 6) {
                                        HStack {
                                            Text(preset.displayName)
                                                .font(.headline)
                                                .foregroundStyle(.white)
                                            Spacer()
                                            if isActive {
                                                Text("ACTIVE")
                                                    .font(.system(size: 10, weight: .bold, design: .monospaced))
                                                    .foregroundStyle(.black)
                                                    .padding(.horizontal, 7)
                                                    .padding(.vertical, 3)
                                                    .background(Color(hex: 0x7AE582), in: Capsule())
                                            }
                                        }

                                        Text(preset.subtitle)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)

                                        // Live Rust-parsed TOML metadata badges
                                        HStack(spacing: 6) {
                                            SpecBadge(text: "\(preset.gridWidth)×\(preset.gridHeight)", color: Color(hex: 0x4EA8DE))
                                            SpecBadge(text: "\(compactCount(preset.initialCount))→\(compactCount(preset.maximumCount))", color: Color(hex: 0x2EC4B6))
                                            SpecBadge(text: preset.weightDtype, color: Color(hex: 0x9B5DE5))
                                            if preset.hasV2Visuals {
                                                SpecBadge(text: "V2", color: Color(hex: 0xF15BB5))
                                            }
                                            if preset.hasContact {
                                                SpecBadge(text: "Contact", color: Color(hex: 0xFFB627))
                                            }
                                        }
                                    }
                                    .padding(.vertical, 4)
                                }
                            }
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .navigationTitle("Worlds & Presets")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .fontWeight(.semibold)
                }
            }
        }
    }

    private func compactCount(_ n: UInt32) -> String {
        if n >= 1000 {
            return String(format: "%.0fk", Double(n) / 1000.0)
        }
        return "\(n)"
    }
}

private struct SpecBadge: View {
    let text: String
    let color: Color

    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold, design: .monospaced))
            .foregroundStyle(color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color.opacity(0.16), in: Capsule())
    }
}
