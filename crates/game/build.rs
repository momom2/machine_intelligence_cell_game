//! Build-time convenience (owner ask, 2026-07-08): compiling the game drops a Windows
//! shortcut `game-dev.lnk` at the repo root, pointing at **`play-dev.cmd`** — the launcher
//! that rebuilds if needed (a fast no-op when fresh) and then runs `target\release\game.exe`.
//! Pointing the shortcut at the LAUNCHER rather than the exe makes it fully persistent
//! (owner refinement): it survives rebuilds, `cargo clean` (the click itself rebuilds), and
//! forgetting to rebuild after a code change. (Cargo stable has no `--out-dir` and a build
//! script runs BEFORE the link step, so the artifact itself cannot be placed at the root
//! from here.) The tracked root `game.exe` remains the deliberate SHIPPED copy (build.cmd
//! refreshes it); the `.lnk` is the dev loop's always-current double-click, git-ignored
//! (its target is an absolute path of this machine). Created only if missing — rebuilds
//! don't pay the PowerShell tax — and best-effort: a failure never breaks the build.

fn main() {
    // The replay version stamp: the short git hash of the building tree (a replay is only
    // valid against the exact sim that recorded it). "unknown" outside a git checkout.
    let git = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_HASH={git}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    println!("cargo:rerun-if-changed=build.rs");
    if !cfg!(windows) {
        return;
    }
    let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") else { return };
    let root = std::path::Path::new(&manifest).join("../..");
    let Ok(root) = root.canonicalize() else { return };
    // WScript.Shell silently refuses `\\?\`-prefixed (extended-length) paths as a shortcut
    // TargetPath — strip the prefix canonicalize() adds on Windows.
    let root = std::path::PathBuf::from(
        root.to_string_lossy().trim_start_matches(r"\\?\").to_string(),
    );
    let link = root.join("game-dev.lnk");
    if link.exists() {
        return;
    }
    let launcher = root.join("play-dev.cmd");
    let exe = root.join("target").join("release").join("game.exe");
    // The WorkingDirectory makes the shortcut robust even if the exe predates a repo move:
    // the asset resolution falls back to the current directory (see `levels::campaign`).
    // The icon borrows the game exe's (cosmetic; a missing exe just means a default icon).
    let script = format!(
        "$ws = New-Object -ComObject WScript.Shell; \
         $s = $ws.CreateShortcut('{}'); \
         $s.TargetPath = '{}'; \
         $s.WorkingDirectory = '{}'; \
         $s.IconLocation = '{}'; \
         $s.Description = 'Machine Intelligence - rebuild if needed and play'; \
         $s.Save()",
        link.display(),
        launcher.display(),
        root.display(),
        exe.display()
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status();
}
