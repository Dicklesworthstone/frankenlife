//! `flife --list-features`: structured registry of all 21 `field-ecology-v2`
//! feature toggles, bundle defaults, `INV-08` status, and ABI impact (V2-01).

use flife_core::schema::Scope;
use flife_core::{V2Feature, V2Profile};
use serde::Serialize;

/// The schema of `flife --list-features`.
pub const FEATURES_SCHEMA: &str = "frankenlife.features.v1";

/// One entry in the feature registry listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureInfo {
    /// Canonical CLI and TOML slug.
    pub slug: &'static str,
    /// Stable bit index in the checkpoint `v2_feature_bitmask`.
    pub bit: u8,
    /// `"model"` (scientific) or `"presentation"` (`INV-09` display-only).
    pub scope: &'static str,
    /// Enabled by default under Bundle A (`auto` without `[contact]` / `supercolony`).
    pub bundle_a_default: bool,
    /// Enabled by default under Bundle B (`auto` with `[contact]` / `tissue`).
    pub bundle_b_default: bool,
    /// `INV-08` pure-field status: `"pure-field"`, `"contact-only"`,
    /// `"conditional-on-mode"`, or `"presentation-only"`.
    pub inv08_status: &'static str,
    /// Controller / field ABI width impact when active.
    pub abi_impact: &'static str,
}

/// Top-level record emitted by `flife --list-features`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureListing {
    /// [`FEATURES_SCHEMA`].
    pub schema: &'static str,
    /// All 21 registered `field-ecology-v2` features in canonical order.
    pub features: Vec<FeatureInfo>,
}

const fn inv08_status(feature: V2Feature) -> &'static str {
    if matches!(feature, V2Feature::V2VisualMappings) {
        "presentation-only"
    } else if feature.requires_contact_unconditionally() {
        "contact-only"
    } else if matches!(feature, V2Feature::HippoCrowding) {
        "conditional-on-mode"
    } else {
        "pure-field"
    }
}

const fn abi_impact(feature: V2Feature) -> &'static str {
    match feature {
        V2Feature::DualCarrierTrails => "+4 sensors, +1 field plane",
        V2Feature::HippoCrowding => "+1 sensor (when expose_sensor = true)",
        V2Feature::NotchDelta => "+1 sensor (when mode = sensor-neighbor-mean-v1)",
        V2Feature::So2Motor => "+4 actions",
        V2Feature::So2Budding => "+2 actions",
        V2Feature::EmtGate => "+1 action",
        V2Feature::WakeOffset => {
            "+1 action (when mode = polarized-pump-v1 and pump_direction_mode = neural-output-v1)"
        }
        V2Feature::EpigeneticRecurrence => "+S inputs, +2S raw outputs",
        _ => "none",
    }
}

/// Build the structured listing of all 21 `field-ecology-v2` feature flags.
#[must_use]
pub fn list_features() -> FeatureListing {
    let features = V2Feature::ALL
        .into_iter()
        .map(|feature| FeatureInfo {
            slug: feature.slug(),
            bit: feature.bit(),
            scope: match feature.scope() {
                Scope::Model => "model",
                Scope::Presentation => "presentation",
                Scope::Operational => "operational",
            },
            bundle_a_default: V2Profile::Auto.default_enabled(feature, false),
            bundle_b_default: V2Profile::Auto.default_enabled(feature, true),
            inv08_status: inv08_status(feature),
            abi_impact: abi_impact(feature),
        })
        .collect();
    FeatureListing {
        schema: FEATURES_SCHEMA,
        features,
    }
}
