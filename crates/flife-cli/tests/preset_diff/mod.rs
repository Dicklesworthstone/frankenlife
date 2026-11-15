//! Key-by-key comparison of preset TOML documents, shared by the profile
//! preset tests (FL-034): a preset is another configuration with exactly
//! its declared changes, each named in its header.

use std::collections::BTreeMap;

/// Every leaf of a TOML document by its dotted key.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn leaves(text: &str) -> BTreeMap<String, toml::Value> {
    fn walk(prefix: &str, table: &toml::Table, out: &mut BTreeMap<String, toml::Value>) {
        for (key, value) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match value {
                toml::Value::Table(inner) => walk(&path, inner, out),
                leaf => {
                    out.insert(path, leaf.clone());
                }
            }
        }
    }
    let table: toml::Table = toml::from_str(text).expect("TOML");
    let mut out = BTreeMap::new();
    walk("", &table, &mut out);
    out
}

/// One TOML value written as `text`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn value(text: &str) -> toml::Value {
    let table: toml::Table = toml::from_str(&format!("v = {text}")).expect("a TOML value");
    table.get("v").cloned().expect("v")
}

/// The keys whose values differ between `from` and `to`, which hold the
/// same keys, with both values, in key order.
pub(crate) fn changes(from: &str, to: &str) -> Vec<(String, toml::Value, toml::Value)> {
    let (from, to) = (leaves(from), leaves(to));
    assert_eq!(
        from.keys().collect::<Vec<_>>(),
        to.keys().collect::<Vec<_>>()
    );
    from.into_iter()
        .filter_map(|(key, before)| {
            let after = to.get(&key)?.clone();
            (after != before).then_some((key, before, after))
        })
        .collect()
}

/// `preset` is `from` with exactly the `declared` changes, given as
/// `(key, from's value, preset's value)` TOML text, and its header (its
/// leading `#` lines) declares each one as `key from → to`.
pub(crate) fn assert_exactly_declared(from: &str, preset: &str, declared: &[(&str, &str, &str)]) {
    let mut expected: Vec<(String, toml::Value, toml::Value)> = declared
        .iter()
        .map(|&(key, before, after)| (key.to_owned(), value(before), value(after)))
        .collect();
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(changes(from, preset), expected);
    let header: Vec<&str> = preset
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    for &(key, before, after) in declared {
        let (before, after) = (before.trim_matches('"'), after.trim_matches('"'));
        assert!(
            header.iter().any(|line| line.contains(key)
                && line.contains(before)
                && line.contains(after)
                && line.contains('→')),
            "the header does not declare {key}: {before} → {after}"
        );
    }
}
