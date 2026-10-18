// Two build-time inputs (decision log D-034, D-035):
// - `sqlx::migrate!` embeds the migrations, so a change to them rebuilds the crate;
// - VOIDFORGE_ENGINE_BUILD, the running build's identity for snapshot headers: the workspace
//   version plus the git commit. VOIDFORGE_GIT_SHA wins when set (CI and images); otherwise
//   `git rev-parse` at build time; otherwise "unknown", an honest label rather than a guess.
//   Local builds are identified by their HEAD commit only, not by uncommitted edits.
// The codec never reads this constant itself: it takes the build as an input, so tests pass fixed
// values and stay independent of git state, and the core never sees it.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-env-changed=VOIDFORGE_GIT_SHA");
    let sha = match std::env::var("VOIDFORGE_GIT_SHA") {
        Ok(sha) if !sha.trim().is_empty() => Some(sha.trim().to_owned()),
        _ => {
            // Rebuild when HEAD moves: HEAD itself, the branch it names, and packed refs.
            for path in ["HEAD", "packed-refs"] {
                if let Some(p) = git(&["rev-parse", "--git-path", path]) {
                    println!("cargo:rerun-if-changed={p}");
                }
            }
            if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"])
                && let Some(p) = git(&["rev-parse", "--git-path", &branch])
            {
                println!("cargo:rerun-if-changed={p}");
            }
            git(&["rev-parse", "--short=12", "HEAD"])
        }
    };
    // Keep only what an engine build may contain (crate::snapshot::EngineBuild); anything else
    // in a hand-set variable becomes "unknown".
    let sha = sha
        .filter(|s| s.len() <= 40 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "unknown".to_owned());
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    println!("cargo:rustc-env=VOIDFORGE_ENGINE_BUILD={version}+{sha}");
}
