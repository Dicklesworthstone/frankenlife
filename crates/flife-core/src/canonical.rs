//! Canonical serialization and content digests (plan B.1).
//!
//! Canonical TOML writes every key in schema order (top-level keys, then
//! each table in Appendix A order), one `key = value` per line. Floats use
//! Rust's shortest round-trip form, which TOML parses back to the same bits,
//! with `-0.0` written as `0.0`. Strings are TOML basic strings. Equal
//! configurations therefore serialize to equal bytes, and the bytes parse
//! back to an equal configuration.

use std::fmt::{self, Write as _};

use sha2::{Digest as _, Sha256};

use crate::config::RunConfig;
use crate::schema::{self, Value};

/// A SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut digest = [0_u8; 32];
        for (out, byte) in digest.iter_mut().zip(Sha256::digest(bytes).iter()) {
            *out = *byte;
        }
        Self(digest)
    }

    /// A digest read back from its raw bytes, as a checkpoint stores it.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }
}

/// SHA-256 over bytes fed in order: the same digest as
/// [`Sha256Digest::of`] their concatenation, without holding them.
#[derive(Debug, Clone, Default)]
pub struct Sha256Stream(Sha256);

impl Sha256Stream {
    /// An empty stream.
    #[must_use]
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    /// Feed `bytes`.
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The digest of everything fed.
    #[must_use]
    pub fn finish(self) -> Sha256Digest {
        let mut digest = [0_u8; 32];
        for (out, byte) in digest.iter_mut().zip(self.0.finalize().iter()) {
            *out = *byte;
        }
        Sha256Digest(digest)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

/// A float in canonical form: shortest round-trip, `-0.0` as `0.0`.
#[must_use]
pub fn canonical_float(value: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    format!("{value:?}")
}

fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if u32::from(c) < 0x20 || u32::from(c) == 0x7f => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The canonical TOML of `config`. Only meaningful for a validated
/// configuration (nonfinite floats are not TOML).
pub(crate) fn canonical_toml(config: &RunConfig) -> String {
    let mut out = String::with_capacity(6 * 1024);
    let mut section = "";
    for field in schema::fields(config) {
        if field.section != section {
            section = field.section;
            let _ = write!(out, "\n[{section}]\n");
        }
        out.push_str(field.key);
        out.push_str(" = ");
        match field.value {
            Value::Float(value) => out.push_str(&canonical_float(value)),
            Value::Int(value) => {
                let _ = write!(out, "{value}");
            }
            Value::Bool(value) => out.push_str(if value { "true" } else { "false" }),
            Value::Str(text) => push_string(&mut out, text),
            Value::StrList(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    push_string(&mut out, item);
                }
                out.push(']');
            }
            Value::IntList(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    let _ = write!(out, "{item}");
                }
                out.push(']');
            }
        }
        out.push('\n');
    }
    out
}
