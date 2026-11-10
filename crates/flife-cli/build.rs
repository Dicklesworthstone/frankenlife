//! Build facts for the run manifest's source group (FL-041; plan Appendix
//! B.1): the FrankenLife commit, the FrankenSuite commits `Cargo.lock`
//! resolves, the compiler, the target and the profile. A fact the build
//! cannot read (no git checkout, say) is recorded as unavailable, never
//! guessed.
//!
//! The commit is the one `HEAD` named when this script last ran; edits not
//! committed are not detected.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const UNAVAILABLE: &str = "unavailable";
const SUITE_PREFIX: &str = "git+https://github.com/Dicklesworthstone/";

fn output(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let line = text.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// `repo@commit` for each FrankenSuite git source, sorted.
fn suite(lock: &str) -> String {
    let mut pins = BTreeSet::new();
    for line in lock.lines() {
        let Some(source) = line
            .strip_prefix("source = \"")
            .and_then(|rest| rest.strip_suffix('"'))
        else {
            continue;
        };
        let Some(rest) = source.strip_prefix(SUITE_PREFIX) else {
            continue;
        };
        let Some((url, commit)) = rest.split_once('#') else {
            continue;
        };
        let repo = url.split(['?', '/']).next().unwrap_or_default();
        pins.insert(format!("{}@{commit}", repo.trim_end_matches(".git")));
    }
    if pins.is_empty() {
        return UNAVAILABLE.to_owned();
    }
    pins.into_iter().collect::<Vec<_>>().join(",")
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let root = Path::new(&manifest).join("../..");

    let git = root.join(".git");
    watch(&git.join("HEAD"));
    watch(&git.join("packed-refs"));
    if let Ok(head) = std::fs::read_to_string(git.join("HEAD"))
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        watch(&git.join(reference));
    }
    let revision = output(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root),
    )
    .filter(|commit| commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit()))
    .unwrap_or_else(|| UNAVAILABLE.to_owned());
    println!("cargo:rustc-env=FLIFE_REVISION={revision}");

    let lock_path = root.join("Cargo.lock");
    watch(&lock_path);
    let suite = std::fs::read_to_string(&lock_path)
        .map(|lock| suite(&lock))
        .unwrap_or_else(|_| UNAVAILABLE.to_owned());
    println!("cargo:rustc-env=FLIFE_SUITE={suite}");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let version =
        output(Command::new(rustc).arg("--version")).unwrap_or_else(|| UNAVAILABLE.to_owned());
    println!("cargo:rustc-env=FLIFE_RUSTC={version}");

    for (key, name) in [("TARGET", "FLIFE_TARGET"), ("PROFILE", "FLIFE_PROFILE")] {
        let value = std::env::var(key).unwrap_or_else(|_| UNAVAILABLE.to_owned());
        println!("cargo:rustc-env={name}={value}");
    }
}
