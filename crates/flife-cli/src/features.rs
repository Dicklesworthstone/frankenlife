//! `flife --list-features`: structured registry of all 21 `field-ecology-v2`
//! feature toggles, bundle defaults, `INV-08` status, and ABI impact (V2-01).

use flife_core::V2Feature;
use flife_core::schema::Scope;
use serde::Serialize;

/// The schema of `flife --list-features`.
pub const FEATURES_SCHEMA: &str = "frankenlife.features.v1";

/// One entry in the feature registry listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureInfo {
    /// Canonical CLI and TOML slug.
    pub slug: &'static str,
    /// Stable bit index (`0..21`) in the checkpoint `v2_feature_bitmask`.
    pub bit: u32,
    /// `"model"` (scientific) or `"presentation"` (`INV-09` display-only).
    pub scope: &'static str,
    /// Enabled by default under Bundle A (`tissue` / `auto` with `[contact]`).
    pub bundle_a_default: bool,
    /// Enabled by default under Bundle B (`supercolony` / `auto` without `[contact]`).
    pub bundle_b_default: bool,
    /// `INV-08` pure-field status summary.
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

/// Build the structured listing of all 21 `field-ecology-v2` feature flags.
#[must_use]
pub fn list_features() -> FeatureListing {
    let features = V2Feature::ALL
        .into_iter()
        .map(|feature| FeatureInfo {
            slug: feature.slug(),
            bit: feature.bit().trailing_zeros(),
            scope: match feature.scope() {
                Scope::Model => "model",
                Scope::Presentation => "presentation",
                Scope::Run => "run",
                Scope::Numerics => "numerics",
                Scope::Execution => "execution",
            },
            bundle_a_default: feature.default_in_bundle_a(),
            bundle_b_default: feature.default_in_bundle_b(),
            inv08_status: feature.inv08_status(),
            abi_impact: feature.abi_impact(),
        })
        .collect();
    FeatureListing {
        schema: FEATURES_SCHEMA,
        features,
    }
}
