import SwiftUI

/// Floating top Liquid Glass HUD separating honest simulation clocks,
/// display FPS, Apple Metal GPU compute timing, and INV-03 ledger status.
struct TelemetryHUD: View {
    let engine: FrankenLifeEngine
    let onOpenPresets: () -> Void
    let onOpenTelemetry: () -> Void
    let onToggleCinema: () -> Void

    var body: some View {
        let t = engine.telemetry
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                // World / Preset Selector Pill
                Button(action: onOpenPresets) {
                    HStack(spacing: 7) {
                        Circle()
                            .fill(statusDotColor(t))
                            .frame(width: 7, height: 7)
                            .shadow(color: statusDotColor(t).opacity(0.8), radius: 4)

                        VStack(alignment: .leading, spacing: 1) {
                            Text(engine.activePreset?.displayName ?? "FrankenLife")
                                .font(.caption.weight(.bold))
                                .foregroundStyle(.white)
                                .lineLimit(1)

                            HStack(spacing: 5) {
                                Text("t \(t.tick.formatted())")
                                Text("•")
                                Text("\(t.liveCount.formatted()) live")
                            }
                            .font(.system(size: 10, weight: .medium, design: .monospaced))
                            .foregroundStyle(.white.opacity(0.74))
                        }

                        Image(systemName: "chevron.down")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(.white.opacity(0.55))
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(.ultraThinMaterial, in: Capsule())
                    .overlay(Capsule().strokeBorder(.white.opacity(0.18), lineWidth: 0.7))
                }

                Spacer(minLength: 4)

                // Honest Clocks & Metal Compute Telemetry Pill
                Button(action: onOpenTelemetry) {
                    HStack(spacing: 9) {
                        // Metal GPU / CPU badge
                        HStack(spacing: 3) {
                            Image(systemName: t.metalActive ? "bolt.fill" : "cpu")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundStyle(t.metalActive ? Color(hex: 0x9BF6FF) : Color(hex: 0xFFB627))
                            Text(t.metalActive ? String(format: "%.1fms", t.lastGpuMs) : "CPU")
                                .font(.system(size: 10, weight: .semibold, design: .monospaced))
                                .foregroundStyle(.white)
                        }

                        Divider().frame(height: 16).overlay(.white.opacity(0.2))

                        // Sim ticks/s & Display FPS (kept strictly distinct — AGENTS.md honesty rule)
                        VStack(alignment: .trailing, spacing: 1) {
                            Text(String(format: "%.0f t/s", t.simTicksPerSecond))
                                .font(.system(size: 10, weight: .bold, design: .monospaced))
                                .foregroundStyle(Color(hex: 0x7AE582))
                            Text(String(format: "%.0f fps", t.renderFPS))
                                .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                                .foregroundStyle(.white.opacity(0.68))
                        }

                        // INV-03 Energy Conservation Indicator
                        Image(systemName: t.faulted ? "exclamationmark.triangle.fill" : "checkmark.seal.fill")
                            .font(.system(size: 12))
                            .foregroundStyle(t.faulted ? .red : Color(hex: 0x2EC4B6))
                    }
                    .padding(.horizontal, 11)
                    .padding(.vertical, 6)
                    .background(.ultraThinMaterial, in: Capsule())
                    .overlay(Capsule().strokeBorder(.white.opacity(0.18), lineWidth: 0.7))
                }

                // Cinema Mode Hide Button
                Button(action: onToggleCinema) {
                    Image(systemName: "eye.slash")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.82))
                        .frame(width: 34, height: 34)
                        .background(.ultraThinMaterial, in: Circle())
                        .overlay(Circle().strokeBorder(.white.opacity(0.18), lineWidth: 0.7))
                }
                .accessibilityLabel("Hide HUD")
            }

            // Compact Color Legend & Active Mode Ribbon
            HStack(spacing: 8) {
                HStack(spacing: 4) {
                    HStack(spacing: 2) {
                        ForEach(Array(engine.colorMapping.swatches.prefix(6).enumerated()), id: \.offset) { _, color in
                            Circle()
                                .fill(color)
                                .frame(width: 6, height: 6)
                        }
                    }
                    Text(engine.colorMapping.title)
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.82))
                }
                .padding(.horizontal, 9)
                .padding(.vertical, 4)
                .background(.black.opacity(0.42), in: Capsule())
                .overlay(Capsule().strokeBorder(.white.opacity(0.12), lineWidth: 0.5))

                if engine.lookMode != .glow {
                    Text(engine.lookMode.title)
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(Color(hex: 0xFEE440))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(.black.opacity(0.45), in: Capsule())
                }

                if engine.isFollowingSelected, let folId = t.followedOrganismId {
                    HStack(spacing: 4) {
                        Image(systemName: "scope")
                            .font(.system(size: 9, weight: .bold))
                        Text("Following #\(folId) (\(t.followedHandoffs) handoffs)")
                            .font(.system(size: 10, weight: .semibold, design: .monospaced))
                    }
                    .foregroundStyle(Color(hex: 0xFEE440))
                    .padding(.horizontal, 9)
                    .padding(.vertical, 4)
                    .background(.black.opacity(0.55), in: Capsule())
                    .overlay(Capsule().strokeBorder(Color(hex: 0xFEE440).opacity(0.45), lineWidth: 0.6))
                }

                if engine.thermalState == .serious || engine.thermalState == .critical {
                    Label("Display \(engine.targetDisplayFPS)Hz", systemImage: "thermometer.high")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.orange)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(.black.opacity(0.55), in: Capsule())
                }
            }
        }
    }

    private func statusDotColor(_ t: LiveTelemetrySnapshot) -> Color {
        if t.faulted { return .red }
        if t.isCapacityPaused { return .orange }
        return engine.isPlaying ? Color(hex: 0x7AE582) : Color(hex: 0xFFB627)
    }
}
