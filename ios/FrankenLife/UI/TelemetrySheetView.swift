import SwiftUI

/// Honest scientific telemetry, Apple Silicon Metal GPU diagnostics, sparklines,
/// INV-03 energy conservation ledger, and live on-device 0-ULP Metal vs CPU parity check.
struct TelemetrySheetView: View {
    @Bindable var engine: FrankenLifeEngine
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let t = engine.telemetry
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    // 1. Live Sparklines
                    VStack(alignment: .leading, spacing: 10) {
                        SectionHeader(title: "Live Observatory Trends", subtitle: "Simulation clock vs display clock kept strictly separate")
                        HStack(spacing: 10) {
                            SparklineCard(
                                title: "Population",
                                value: t.liveCount.formatted(),
                                color: Color(hex: 0x2EC4B6),
                                samples: engine.populationHistory
                            )
                            SparklineCard(
                                title: "Sim Rate",
                                value: String(format: "%.1f t/s", t.simTicksPerSecond),
                                color: Color(hex: 0x7AE582),
                                samples: engine.tpsHistory
                            )
                        }
                        SparklineCard(
                            title: "Metal Neural Compute Time / Tick",
                            value: t.metalActive ? String(format: "%.2f ms GPU", t.lastGpuMs) : "CPU Reference Active",
                            color: Color(hex: 0x9BF6FF),
                            samples: engine.gpuMsHistory
                        )
                    }

                    // 2. Apple Silicon Metal Hardware & Compute Telemetry
                    VStack(alignment: .leading, spacing: 8) {
                        SectionHeader(title: "Apple Silicon Metal Substrate", subtitle: "Native iOS Metal compute + 120Hz EDR render pipeline")
                        VStack(spacing: 8) {
                            if let cap = engine.gpuCapabilities {
                                InfoRow(label: "GPU Device", value: cap.deviceName)
                                InfoRow(label: "Apple GPU Family", value: cap.gpuFamilyLabel)
                                InfoRow(label: "Unified Memory / Dynamic Cache", value: "\(cap.hasUnifiedMemory ? "Unified" : "Discrete") • \(cap.supportsDynamicCaching ? "Dynamic Caching" : "Standard")")
                                InfoRow(label: "Recommended Working Set", value: formatBytes(cap.recommendedWorkingSetBytes))
                            }
                            InfoRow(label: "Controller Mode", value: t.metalActive ? "iOS Metal Compute (.safe 0-ULP)" : "CPU Reference")
                            InfoRow(label: "Metal Batched / Deferred", value: "\(t.metalBatched.formatted()) / \(t.metalDeferred.formatted())")
                            InfoRow(label: "Resident Weight Shards Peak", value: formatBytes(t.metalResidentPeakBytes))
                            InfoRow(label: "Display Pacing / EDR Headroom", value: String(format: "%.0f fps • %.2fx EDR (%@)", t.renderFPS, t.edrHeadroom, thermalLabel))
                        }
                        .padding(13)
                        .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                    }

                    // 3. INV-03 Energy Conservation Ledger
                    VStack(alignment: .leading, spacing: 8) {
                        SectionHeader(title: "INV-03 Energy Conservation Ledger", subtitle: "Measured every committed tick — zero hidden energy")
                        VStack(spacing: 8) {
                            InfoRow(label: "Committed Tick & Sim Time", value: String(format: "%@ (%.1fs)", t.tick.formatted(), Double(t.tick) * t.dtSeconds))
                            InfoRow(label: "Sim / Wall Ratio", value: String(format: "%.2fx realtime (%.1f ms/tick)", t.simTicksPerSecond * t.dtSeconds, t.lastTickWallMs))
                            InfoRow(label: "Organism / Field Energy", value: String(format: "%.1f / %.1f", t.organismEnergy, t.fieldEnergy))
                            InfoRow(label: "Cumulative Residual", value: String(format: "%.3e", t.ledgerResidual))
                            InfoRow(
                                label: "Normalized Residual",
                                value: t.ledgerNormalized.map { String(format: "%.3e", $0) } ?? "n/a"
                            )
                            InfoRow(label: "Births / Deaths / Max Gen", value: "\(t.births.formatted()) / \(t.deaths.formatted()) / G\(t.maxGeneration)")
                            InfoRow(label: "Active Neural Parameters", value: t.totalParameters.formatted())
                            if t.refusedBirths > 0 {
                                InfoRow(label: "Capacity-Refused Births (INV-12)", value: t.refusedBirths.formatted())
                            }
                        }
                        .padding(13)
                        .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                    }

                    // 4. On-Device Bit-for-Bit Metal vs CPU Parity Verification (Gate G4)
                    VStack(alignment: .leading, spacing: 10) {
                        SectionHeader(title: "Gate G4: On-Device Metal Parity Check", subtitle: "Compares SHA-256 World::digest() bit-for-bit against CPU Reference")

                        Button {
                            engine.runLiveParitySelfTest()
                        } label: {
                            HStack {
                                if engine.isRunningParityTest {
                                    ProgressView()
                                        .tint(.black)
                                } else {
                                    Image(systemName: "checkmark.shield.fill")
                                }
                                Text(engine.isRunningParityTest ? "Verifying 16 Ticks on Metal GPU…" : "Run Bit-Exact Metal vs CPU Verification")
                                    .font(.subheadline.weight(.bold))
                            }
                            .foregroundStyle(.black)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                            .background(Color(hex: 0x9BF6FF), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                        }
                        .disabled(engine.isRunningParityTest || engine.computeEngine == nil)

                        if let report = engine.parityReport {
                            VStack(alignment: .leading, spacing: 6) {
                                HStack {
                                    Image(systemName: report.matched ? "checkmark.seal.fill" : "xmark.octagon.fill")
                                        .foregroundStyle(report.matched ? Color(hex: 0x7AE582) : .red)
                                    Text(report.matched ? "0-ULP BIT-EXACT MATCH (\(report.ticksVerified) ticks)" : "PARITY FAILURE")
                                        .font(.caption.weight(.bold).monospaced())
                                        .foregroundStyle(report.matched ? Color(hex: 0x7AE582) : .red)
                                }
                                if let err = report.error {
                                    Text(err).font(.caption).foregroundStyle(.red)
                                } else {
                                    InfoRow(label: "Metal Digest", value: report.metalDigestPrefix + "…")
                                    InfoRow(label: "CPU Digest", value: report.cpuDigestPrefix + "…")
                                    InfoRow(label: "GPU Batched / Deferred", value: "\(report.metalBatched) / \(report.metalDeferred)")
                                    InfoRow(label: "Metal GPU / Wall vs CPU", value: String(format: "%.1fms GPU | %.0fms vs %.0fms (%.2fx)", report.metalDeviceMs, report.metalWallMs, report.cpuWallMs, report.speedupRatio))
                                }
                            }
                            .padding(12)
                            .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                        }
                    }
                }
                .padding(18)
            }
            .navigationTitle("Telemetry & Metal")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .fontWeight(.semibold)
                }
            }
        }
    }

    private var thermalLabel: String {
        switch engine.thermalState {
        case .nominal: return "Nominal"
        case .fair: return "Fair"
        case .serious: return "Warm"
        case .critical: return "Hot"
        @unknown default: return "Unknown"
        }
    }

    private func formatBytes(_ bytes: UInt64) -> String {
        let mb = Double(bytes) / (1024.0 * 1024.0)
        if mb >= 1024.0 {
            return String(format: "%.2f GB", mb / 1024.0)
        }
        return String(format: "%.1f MB", mb)
    }
}

private struct InfoRow: View {
    let label: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer(minLength: 8)
            Text(value)
                .font(.caption.monospacedDigit().weight(.semibold))
                .foregroundStyle(.white)
                .multilineTextAlignment(.trailing)
        }
    }
}

private struct SparklineCard: View {
    let title: String
    let value: String
    let color: Color
    let samples: [Double]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(title)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Text(value)
                    .font(.caption.monospacedDigit().weight(.bold))
                    .foregroundStyle(color)
            }
            Canvas { ctx, size in
                guard samples.count >= 2 else { return }
                let lo = samples.min() ?? 0
                let hi = max(samples.max() ?? 1, lo + 1e-6)
                let span = hi - lo

                var line = Path()
                for (i, s) in samples.enumerated() {
                    let x = size.width * CGFloat(i) / CGFloat(samples.count - 1)
                    let y = size.height - (size.height - 4) * CGFloat((s - lo) / span) - 2
                    if i == 0 {
                        line.move(to: CGPoint(x: x, y: y))
                    } else {
                        line.addLine(to: CGPoint(x: x, y: y))
                    }
                }
                var fill = line
                fill.addLine(to: CGPoint(x: size.width, y: size.height))
                fill.addLine(to: CGPoint(x: 0, y: size.height))
                fill.closeSubpath()

                ctx.fill(
                    fill,
                    with: .linearGradient(
                        Gradient(colors: [color.opacity(0.32), color.opacity(0.0)]),
                        startPoint: CGPoint(x: 0, y: 0),
                        endPoint: CGPoint(x: 0, y: size.height)
                    )
                )
                ctx.stroke(line, with: .color(color), lineWidth: 1.6)
            }
            .frame(height: 36)
        }
        .padding(11)
        .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }
}
