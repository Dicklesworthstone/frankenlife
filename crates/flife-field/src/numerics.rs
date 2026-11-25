//! Host-independent elementary functions, re-exported from
//! `flife_core::numerics` so that the field, the engine and the neural
//! evaluator share one definition of each.

pub use flife_core::numerics::{
    exp_neg, exp_neg_4, expm1_neg, expm1_neg_4, log2, sigmoid, sigmoid_4, tanh, tanh_4,
};
