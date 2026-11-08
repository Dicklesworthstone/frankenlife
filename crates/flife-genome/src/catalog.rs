//! `dense-104-36-catalog-v1`: the topologies a controller may take (plan
//! §6.4, A.2).
//!
//! The catalog is exactly the five rows of §6.4's table, in that order:
//! `104 → 8 → 36`, `104 → 32 → 36`, `104 → 64 → 32 → 36`,
//! `104 → 96 → 64 → 36` and `104 → 128 → 96 → 36`. Another membership is
//! another catalog name. A member above the run's parameter cap stays
//! listed, so identities do not shift, but is not admitted: no genome may
//! take it and no transition may target it.

use flife_core::{CoreError, ValidatedConfig};
use flife_neural::DenseTopology;

use crate::transition::{OVERLAP_PLUS_ZERO_OUTPUT_V1, PREFIX_AND_IDENTITY_BRIDGE_V1};

/// The catalog this module implements.
pub const DENSE_104_36_CATALOG_V1: &str = "dense-104-36-catalog-v1";

/// Hidden widths of every member of `dense-104-36-catalog-v1`, in catalog
/// order.
pub const DENSE_104_36_MEMBERS: [&[u32]; 5] = [&[8], &[32], &[64, 32], &[96, 64], &[128, 96]];

/// A member's position in its catalog: the topology's identity within one
/// catalog version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TopologyId(u8);

impl TopologyId {
    /// The member's index in catalog order.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One run's topology catalog under its parameter cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyCatalog {
    members: Vec<DenseTopology>,
    maximum_parameters: u64,
    default: TopologyId,
}

impl TopologyCatalog {
    /// `dense-104-36-catalog-v1` under `maximum_parameters`, with the member
    /// whose hidden widths are `default_hidden` as the founders' topology.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when `default_hidden` is not a
    /// member, or is above the cap.
    pub fn dense_104_36_v1(
        maximum_parameters: u64,
        default_hidden: &[u32],
    ) -> Result<Self, CoreError> {
        let members = DENSE_104_36_MEMBERS
            .iter()
            .map(|hidden| DenseTopology::new(hidden))
            .collect::<Result<Vec<_>, _>>()?;
        let position = members
            .iter()
            .position(|member| member.hidden_widths() == default_hidden)
            .ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "controller.hidden_widths",
                detail: format!("{default_hidden:?} is not a member of {DENSE_104_36_CATALOG_V1}"),
            })?;
        let default = TopologyId(u8::try_from(position).map_err(|_| CoreError::Overflow {
            quantity: "topology id",
        })?);
        let catalog = Self {
            members,
            maximum_parameters,
            default,
        };
        if !catalog.admits(default) {
            return Err(CoreError::NumericalInvalid {
                quantity: "architecture_mutation.maximum_parameters",
                detail: format!(
                    "the default topology {default_hidden:?} is above the cap {maximum_parameters}"
                ),
            });
        }
        Ok(catalog)
    }

    /// The catalog of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another catalog, inheritance map
    /// or depth-change map, or a controller this build cannot evaluate;
    /// otherwise as [`TopologyCatalog::dense_104_36_v1`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let architecture = &config.config().architecture_mutation;
        for (role, name, implemented) in [
            (
                "architecture_mutation.catalog",
                &architecture.catalog,
                DENSE_104_36_CATALOG_V1,
            ),
            (
                "architecture_mutation.inheritance_map",
                &architecture.inheritance_map,
                OVERLAP_PLUS_ZERO_OUTPUT_V1,
            ),
            (
                "architecture_mutation.depth_change_map",
                &architecture.depth_change_map,
                PREFIX_AND_IDENTITY_BRIDGE_V1,
            ),
        ] {
            if name != implemented {
                return Err(CoreError::UnknownAlgorithm {
                    role,
                    name: name.clone(),
                });
            }
        }
        let default = DenseTopology::from_config(config)?;
        Self::dense_104_36_v1(architecture.maximum_parameters, default.hidden_widths())
    }

    /// Members, admitted or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the catalog has no member (never, for a constructed catalog).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Every member's identity, in catalog order.
    pub fn ids(&self) -> impl Iterator<Item = TopologyId> + '_ {
        (0..self.members.len()).filter_map(|index| u8::try_from(index).ok().map(TopologyId))
    }

    /// The member's topology.
    #[must_use]
    pub fn topology(&self, id: TopologyId) -> Option<&DenseTopology> {
        self.members.get(id.index())
    }

    /// The member with these hidden widths.
    #[must_use]
    pub fn find(&self, hidden_widths: &[u32]) -> Option<TopologyId> {
        self.ids().find(|&id| {
            self.topology(id)
                .is_some_and(|member| member.hidden_widths() == hidden_widths)
        })
    }

    /// The founders' topology.
    #[must_use]
    pub const fn default_topology(&self) -> TopologyId {
        self.default
    }

    /// The run's parameter cap.
    #[must_use]
    pub const fn maximum_parameters(&self) -> u64 {
        self.maximum_parameters
    }

    /// Whether `id` is a member within the parameter cap.
    #[must_use]
    pub fn admits(&self, id: TopologyId) -> bool {
        self.topology(id).is_some_and(|member| {
            u64::try_from(member.parameter_count())
                .is_ok_and(|count| count <= self.maximum_parameters)
        })
    }

    /// The admitted members other than `from`, in catalog order: the
    /// candidates of a topology change (A.3 item 4). Choosing among them is
    /// FL-010's `uniform-other-catalog-member`.
    pub fn transition_targets(&self, from: TopologyId) -> impl Iterator<Item = TopologyId> + '_ {
        self.ids().filter(move |&id| id != from && self.admits(id))
    }
}
