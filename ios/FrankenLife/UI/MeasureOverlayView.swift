import SwiftUI

/// Non-invasive (`INV-09`) floating disk measurement telemetry card.
struct MeasureOverlayView: View {
    let measure: RegionMeasureDetail
    let onClear: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Label("Region Measurement (Non-Invasive)", systemImage: "ruler")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(Color(hex: 0x9BF6FF))
                Spacer()
                Text(String(format: "r = %.1f u", measure.radius))
                    .font(.caption.monospacedDigit().weight(.semibold))
                    .foregroundStyle(.white.opacity(0.75))
                Button(action: onClear) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.white.opacity(0.55))
                }
            }

            HStack(spacing: 10) {
                MeasureStat(label: "ORGANISMS", value: measure.organisms.formatted())
                MeasureStat(label: "LINEAGES", value: measure.lineages.formatted())
                MeasureStat(label: "ORG ENERGY", value: String(format: "%.1f", measure.organismEnergy))
                MeasureStat(label: "FIELD ENERGY", value: String(format: "%.1f", measure.fieldEnergy))
            }
        }
        .padding(12)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .strokeBorder(Color(hex: 0x9BF6FF).opacity(0.35), lineWidth: 0.8)
        )
    }
}

private struct MeasureStat: View {
    let label: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.system(size: 8.5, weight: .bold, design: .monospaced))
                .foregroundStyle(.white.opacity(0.55))
            Text(value)
                .font(.system(size: 13, weight: .bold, design: .monospaced))
                .foregroundStyle(.white)
                .lineLimit(1)
                .minimumScaleFactor(0.75)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
