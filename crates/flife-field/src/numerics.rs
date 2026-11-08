//! Host-independent elementary functions, re-exported from
//! `flife_core::numerics` so that the field, the engine and the neural
//! evaluator share one definition of each.

pub use flife_core::numerics::{exp_neg, expm1_neg, log2, sigmoid, tanh};
