//! The model descriptor (plan §18.3 `ModelDescriptor`, B.1).
//!
//! The descriptor identifies the scientific model a run uses: its schema,
//! the type and unit of every model and numerics parameter, the named
//! component chosen for each algorithm role, and the numerical profile. It
//! excludes parameter values (the configuration hash covers those), run
//! identity, execution resources and presentation. Two runs that differ
//! only in, say, `world.diffusion` or `presentation.width` share a model
//! descriptor and differ in configuration hash. Registry status, contract
//! references and owners are not part of the identity, so implementing a
//! component never changes the model.

use std::fmt::Write as _;

use crate::canonical::Sha256Digest;
use crate::config::CONFIG_SCHEMA;
use crate::registry::{self, AlgorithmEntry};
use crate::schema::{self, Kind, Scope, Value};
use crate::validate::ValidatedConfig;

/// The descriptor format version.
pub const DESCRIPTOR_SCHEMA: &str = "frankenlife.model-descriptor.v1";

/// A model descriptor in canonical text form, with its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    canonical: String,
    digest: Sha256Digest,
    components: Vec<(String, &'static AlgorithmEntry)>,
}

impl ModelDescriptor {
    /// Describe a validated configuration's model.
    #[must_use]
    pub fn of(config: &ValidatedConfig) -> Self {
        let mut canonical = String::with_capacity(8 * 1024);
        let mut components = Vec::new();
        let _ = writeln!(canonical, "{DESCRIPTOR_SCHEMA}");
        let _ = writeln!(canonical, "model\t{}", config.config().model);
        let _ = writeln!(canonical, "config-schema\t{CONFIG_SCHEMA}");
        for field in schema::fields(config.config()) {
            if !matches!(field.scope, Scope::Model | Scope::Numerics) {
                continue;
            }
            let path = field.path();
            let mut component = |name: &str, role| {
                let _ = writeln!(canonical, "component\t{path}\t{name}");
                if let Ok(entry) = registry::lookup(role, name) {
                    components.push((path.clone(), entry));
                }
            };
            match (field.kind, field.value) {
                (Kind::Name(role), Value::Str(name)) if path != "model" => component(name, role),
                (Kind::NameList(role), Value::StrList(names)) => {
                    for name in names {
                        component(name, role);
                    }
                }
                (Kind::Float(unit, _), _) => {
                    let _ = writeln!(canonical, "parameter\t{path}\tfloat\t{unit}");
                }
                (Kind::Int(unit, _), _) => {
                    let _ = writeln!(canonical, "parameter\t{path}\tinteger\t{unit}");
                }
                (Kind::IntList(unit), _) => {
                    let _ = writeln!(canonical, "parameter\t{path}\tinteger-list\t{unit}");
                }
                (Kind::Bool, _) => {
                    let _ = writeln!(canonical, "parameter\t{path}\tbool\tswitch");
                }
                _ => {}
            }
        }
        let digest = Sha256Digest::of(canonical.as_bytes());
        Self {
            canonical,
            digest,
            components,
        }
    }

    /// The canonical text (tab-separated lines).
    #[must_use]
    pub fn canonical_text(&self) -> &str {
        &self.canonical
    }

    /// SHA-256 of the canonical text: the model identity.
    #[must_use]
    pub const fn digest(&self) -> Sha256Digest {
        self.digest
    }

    /// Each model component with its registry entry, in schema order.
    #[must_use]
    pub fn components(&self) -> &[(String, &'static AlgorithmEntry)] {
        &self.components
    }
}
