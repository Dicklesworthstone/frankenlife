// The commit a website manifest names as the source of its `flife` binary
// (scripts/media.ts, scripts/stills.ts).
//
// - Without `--revision`, the binary is taken to be built from HEAD, so the
//   Rust sources and presets must be committed: a dirty tree would make the
//   revision a guess.
// - With `--revision REV`, the binary was built earlier, from REV, an ancestor
//   of HEAD. The working tree's Rust sources are then not what runs, so they
//   may differ (another session may be editing them); the presets may not,
//   because the scripts read each world's configuration from the working tree.
//   The manifest records REV, and the binary's SHA-256 beside it.

import { spawnSync } from "node:child_process";

const SOURCES = ["crates", "presets", "Cargo.toml", "Cargo.lock"];

/** Run git in `repo`: its exit status and its standard output (a warning on stderr is not output). */
function git(repo: string, args: string[]): { ok: boolean; out: string } {
  const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
  return { ok: result.status === 0, out: (result.stdout ?? "").trim() };
}

/** The full commit id the manifest records, or an error saying why it cannot name one. */
export function sourceRevision(repo: string, revision: string | null): { revision: string } | { error: string } {
  if (revision === null) {
    const dirty = git(repo, ["status", "--porcelain", "--", ...SOURCES]);
    if (!dirty.ok) return { error: "git status failed; is this the repository?" };
    if (dirty.out !== "") {
      return {
        error: `commit the Rust sources and presets first, or pass --revision with the commit --flife was built from; uncommitted:\n${dirty.out}`,
      };
    }
    const head = git(repo, ["rev-parse", "--verify", "HEAD^{commit}"]);
    return head.ok ? { revision: head.out } : { error: "HEAD names no commit" };
  }
  const full = git(repo, ["rev-parse", "--verify", `${revision}^{commit}`]);
  if (!full.ok) return { error: `--revision ${revision} is not a commit` };
  if (!git(repo, ["merge-base", "--is-ancestor", full.out, "HEAD"]).ok) {
    return { error: `--revision ${revision} is not HEAD or an ancestor of it` };
  }
  // Working tree against REV, committed or not: the configurations must be REV's.
  const presets = git(repo, ["diff", "--name-only", full.out, "--", "presets"]);
  const untracked = git(repo, ["ls-files", "--others", "--exclude-standard", "--", "presets"]);
  if (!presets.ok || !untracked.ok) return { error: `git could not compare presets/ with ${revision}` };
  const changed = [presets.out, untracked.out].filter((text) => text !== "").join("\n");
  if (changed !== "") return { error: `presets/ differs from --revision ${revision}:\n${changed}` };
  return { revision: full.out };
}
