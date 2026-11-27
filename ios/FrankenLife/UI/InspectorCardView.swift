import SwiftUI

/// Floating pure-readout (`INV-09`) Organism Inspector Card showing live
/// biological chemistry, neural topology, 32-channel affinity spectrum,
/// ancestry counts, and lineage-follow / material-brush actions.
struct InspectorCardView: View {
    @Bindable var engine: FrankenLifeEngine
    let organism: OrganismDetail

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            // Header row: Lineage swatch + ID + Generation + Close
            HStack(spacing: 10) {
                Circle()
                    .fill(organism.lineageColor)
                    .frame(width: 14, height: 14)
                    .shadow(color: organism.lineageColor.opacity(0.75), radius: 5)

                VStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 6) {
                        Text("Organism #\(organism.id)")
                            .font(.subheadline.weight(.bold).monospacedDigit())
                            .foregroundStyle(.white)
                        Text("Gen \(organism.generation)")
                            .font(.caption2.weight(.bold).monospacedDigit())
                            .foregroundStyle(organism.lineageColor)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(organism.lineageColor.opacity(0.18), in: Capsule())
                    }
                    Text("Lineage #\(organism.lineageId) • Age \(organism.ageTicks)t • \(topologyString)")
                        .font(.system(size: 10.5, weight: .medium, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.68))
                        .lineLimit(1)
                }

                Spacer()

                Button {
                    engine.dismissInspector()
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title3)
                        .foregroundStyle(.white.opacity(0.55))
                }
                .accessibilityLabel("Close Inspector")
            }

            // Key biophysical metrics grid
            HStack(spacing: 8) {
                MetricTile(
                    label: "ENERGY",
                    value: String(format: "%.2f", organism.energy),
                    delta: String(format: "%+.2f/s", organism.energyChangeRate),
                    deltaColor: organism.energyChangeRate >= 0 ? Color(hex: 0x7AE582) : Color(hex: 0xFF6F59)
                )
                MetricTile(
                    label: "SPEED",
                    value: String(format: "%.2f", organism.speed),
                    delta: String(format: "%.1f, %.1f", organism.posX, organism.posY),
                    deltaColor: .white.opacity(0.55)
                )
                MetricTile(
                    label: "LOCAL CHEM",
                    value: String(format: "+%.0f%%", organism.localGain * 100),
                    delta: String(format: "tox %.0f%%", organism.localToxicity * 100),
                    deltaColor: organism.localToxicity > 0.05 ? Color(hex: 0xFF6F59) : Color(hex: 0x9BF6FF)
                )
                MetricTile(
                    label: "BRAIN",
                    value: "\(organism.parameterCount)",
                    delta: organism.descendantsAlive.map { "\($0) kin" } ?? "params",
                    deltaColor: Color(hex: 0xC8B6FF)
                )
            }

            // 32-channel chemical affinity spectrum sparkbar
            VStack(alignment: .leading, spacing: 3) {
                HStack {
                    Text("32-CHANNEL CHEMICAL AFFINITY")
                        .font(.system(size: 9, weight: .bold, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.58))
                    Spacer()
                    if let h0 = organism.neighborH0 {
                        Text(String(format: "h̄₀ %.2f", h0))
                            .font(.system(size: 9, weight: .semibold, design: .monospaced))
                            .foregroundStyle(Color(hex: 0x9BF6FF))
                    }
                    if let sr = organism.secretoryReadiness {
                        Text(String(format: "sec %.2f", sr))
                            .font(.system(size: 9, weight: .semibold, design: .monospaced))
                            .foregroundStyle(Color(hex: 0xFEE440))
                    }
                }
                AffinitySpectrumView(values: organism.affinity)
                    .frame(height: 22)
            }

            // Action pills: Follow Lineage + Sample as Material Brush
            HStack(spacing: 8) {
                Button {
                    engine.toggleFollowSelected()
                } label: {
                    Label(
                        engine.isFollowingSelected ? "Following Lineage" : "Follow Lineage",
                        systemImage: engine.isFollowingSelected ? "scope" : "location.viewfinder"
                    )
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(engine.isFollowingSelected ? .black : .white)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
                    .background(
                        engine.isFollowingSelected ? Color(hex: 0xFEE440) : .white.opacity(0.12),
                        in: Capsule()
                    )
                }

                Button {
                    engine.adoptSelectedAffinityForBrush()
                } label: {
                    Label("Paint Chemistry", systemImage: "eyedropper.halffull")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                        .background(Color(hex: 0x9B5DE5).opacity(0.38), in: Capsule())
                        .overlay(Capsule().strokeBorder(Color(hex: 0xC8B6FF).opacity(0.4), lineWidth: 0.7))
                }
            }
        }
        .padding(13)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .strokeBorder(.white.opacity(0.20), lineWidth: 0.8)
        )
        .shadow(color: .black.opacity(0.5), radius: 18, y: 8)
    }

    private var topologyString: String {
        let hidden = organism.hiddenWidths.map(String.init).joined(separator: "→")
        return hidden.isEmpty ? "topo \(organism.topologyIndex)" : "[\(hidden)]"
    }
}

private struct MetricTile: View {
    let label: String
    let value: String
    let delta: String
    let deltaColor: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.system(size: 8.5, weight: .bold, design: .monospaced))
                .foregroundStyle(.white.opacity(0.52))
            Text(value)
                .font(.system(size: 13, weight: .bold, design: .monospaced))
                .foregroundStyle(.white)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
            Text(delta)
                .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                .foregroundStyle(deltaColor)
                .lineLimit(1)
                .minimumScaleFactor(0.75)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(.black.opacity(0.28), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}

private struct AffinitySpectrumView: View {
    let values: [Double]

    var body: some View {
        Canvas { ctx, size in
            let midY = size.height * 0.5
            // Baseline
            var base = Path()
            base.move(to: CGPoint(x: 0, y: midY))
            base.addLine(to: CGPoint(x: size.width, y: midY))
            ctx.stroke(base, with: .color(.white.opacity(0.18)), lineWidth: 0.6)

            guard !values.isEmpty else { return }
            let maxMag = max(values.map { abs($0) }.max() ?? 0.25, 0.15)
            let step = size.width / CGFloat(values.count)
            let barW = max(step - 1.5, 1.5)

            for (idx, val) in values.enumerated() {
                let norm = CGFloat(val / maxMag)
                let h = abs(norm) * (midY - 1.5)
                let x = CGFloat(idx) * step + 0.75
                let rect = norm >= 0
                    ? CGRect(x: x, y: midY - h, width: barW, height: h)
                    : CGRect(x: x, y: midY, width: barW, height: h)
                let color = norm >= 0 ? Color(hex: 0x2EC4B6) : Color(hex: 0xF15BB5)
                ctx.fill(Path(roundedRect: rect, cornerRadius: 1), with: .color(color))
            }
        }
    }
}
