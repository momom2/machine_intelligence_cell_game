//! Build-time convenience (owner ask, 2026-07-08): compiling the game drops a Windows
//! shortcut `game-dev.lnk` at the repo root, pointing at `target\release\game.exe` — a
//! SHORTCUT stays fresh across rebuilds (it resolves at click time), which a copied exe
//! cannot (cargo stable has no `--out-dir`; a build script runs BEFORE the link step, so
//! copying the artifact from here is impossible — pointing at its stable path is not).
//! The tracked root `game.exe` remains the deliberate SHIPPED copy (build.cmd refreshes it);
//! the `.lnk` is the dev loop's always-current double-click, git-ignored (its target is an
//! absolute path of this machine). Created only if missing — rebuilds don't pay the
//! PowerShell tax — and best-effort: a failure never breaks the build.

fn main() {
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
    let exe = root.join("target").join("release").join("game.exe");
    // The WorkingDirectory makes the shortcut robust even if the exe predates a repo move:
    // the asset resolution falls back to the current directory (see `levels::campaign`).
    let script = format!(
        "$ws = New-Object -ComObject WScript.Shell; \
         $s = $ws.CreateShortcut('{}'); \
         $s.TargetPath = '{}'; \
         $s.WorkingDirectory = '{}'; \
         $s.Description = 'Machine Intelligence - latest local build'; \
         $s.Save()",
        link.display(),
        exe.display(),
        root.display()
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status();
}
