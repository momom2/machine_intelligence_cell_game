//! The **narrative text engine** (owner feature, 2026-07-08): pre-mission briefings
//! (`<stem>_pre.brf`) and post-battle logs (`<stem>_post.glg`) are per-level asset files
//! (see `levels::campaign`), rendered through a shared TEMPLATE pass before the briefing
//! markup parser sees them. Everything here is gated behind the game's `--text` flag.
//!
//! # The template pass ([`render`])
//!
//! Line-based, applied to both formats:
//!
//! * `# …` lines are comments — dropped.
//! * `{result} {ticks} {lost} {killed} {ships}` substitute the context's metrics.
//! * **Logic-based text**: `?if <cond>` / `?elif <cond>` / `?else` / `?end` keep or drop the
//!   enclosed lines (one level deep — no nesting). A condition is `<key> <op> <value>` with
//!   keys `result | ticks | lost | killed | ships | notes`, ops `= != > < ~` (`~` =
//!   case-insensitive contains — `notes ~ teleporter` scans the player's notes). Numeric ops
//!   on non-numbers are false. Malformed directives pass through as visible text — loud by
//!   being ugly.
//!
//! A **briefing** renders against the PREVIOUS battle's metrics (read from the battle-log
//! history via [`last_metrics`]) — "comment on how the last battle went"; a **post-battle
//! log** renders against the match that just sealed. Rendered logs are appended to
//! `assets/notes/battle_logs.glg` behind a machine-readable `#metrics k=v …` header line,
//! which is exactly what [`last_metrics`] reads back — the whole flag pipeline is one
//! plain-text file the designer can inspect and edit.

/// The metrics/flags context a template renders against.
#[derive(Debug, Clone, Default)]
pub struct LogCtx {
    /// `"victory"` / `"defeat"` — or `"none"` when no battle has been fought yet.
    pub result: String,
    pub ticks: u64,
    /// The player's cumulative ship losses in the match.
    pub lost: u64,
    /// Enemy ships destroyed (all rival seats combined).
    pub killed: u64,
    /// The player's fleet strength when the match sealed.
    pub ships: u64,
    /// The player's persistent notes (for `notes ~ word` conditions). Not substitutable.
    pub notes: String,
}

impl LogCtx {
    fn lookup(&self, key: &str) -> Option<String> {
        Some(match key {
            "result" => self.result.clone(),
            "ticks" => self.ticks.to_string(),
            "lost" => self.lost.to_string(),
            "killed" => self.killed.to_string(),
            "ships" => self.ships.to_string(),
            "notes" => self.notes.clone(),
            _ => return None,
        })
    }
}

/// Evaluate one `<key> <op> <value>` condition against `ctx`. Unknown keys/ops → `false`.
fn eval_cond(cond: &str, ctx: &LogCtx) -> bool {
    let mut it = cond.split_whitespace();
    let (Some(key), Some(op)) = (it.next(), it.next()) else { return false };
    let value = it.collect::<Vec<_>>().join(" ");
    let Some(actual) = ctx.lookup(key) else { return false };
    match op {
        "=" => actual.eq_ignore_ascii_case(&value),
        "!=" => !actual.eq_ignore_ascii_case(&value),
        "~" => actual.to_lowercase().contains(&value.to_lowercase()),
        ">" | "<" => {
            let (Ok(a), Ok(b)) = (actual.parse::<f64>(), value.parse::<f64>()) else {
                return false;
            };
            if op == ">" { a > b } else { a < b }
        }
        _ => false,
    }
}

/// Substitute `{key}` metric references in one line (unknown keys pass through verbatim).
fn substitute(line: &str, ctx: &LogCtx) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if ctx.lookup(&after[..close]).is_some() && after[..close] != *"notes" => {
                out.push_str(&ctx.lookup(&after[..close]).expect("checked"));
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Render a `.brf` / `.glg` template against `ctx` (see the module doc for the grammar).
pub fn render(template: &str, ctx: &LogCtx) -> String {
    let mut out: Vec<String> = Vec::new();
    // Block state: None = outside any ?if; Some((taken_so_far, active)) inside one.
    let mut block: Option<(bool, bool)> = None;
    for line in template.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue; // comment
        }
        if let Some(cond) = t.strip_prefix("?if ") {
            let hit = eval_cond(cond, ctx);
            block = Some((hit, hit));
            continue;
        }
        if let Some(cond) = t.strip_prefix("?elif ") {
            if let Some((taken, _)) = block {
                let hit = !taken && eval_cond(cond, ctx);
                block = Some((taken || hit, hit));
                continue;
            }
        }
        if t == "?else" {
            if let Some((taken, _)) = block {
                block = Some((true, !taken));
                continue;
            }
        }
        if t == "?end" {
            if block.is_some() {
                block = None;
                continue;
            }
        }
        if let Some((_, active)) = block {
            if !active {
                continue;
            }
        }
        out.push(substitute(line, ctx));
    }
    out.join("\n")
}

// ======================================================================================
// The battle-log history file (assets/notes/battle_logs.glg) + the player notes path.
// ======================================================================================

/// The `assets/notes/` directory (owner reorg: player-facing text lives with the assets):
/// next to the executable in a shipped layout, else the workspace tree; created on demand.
pub fn notes_dir() -> std::path::PathBuf {
    let base = if let Ok(exe) = std::env::current_exe() {
        exe.parent()
            .map(|d| d.join("assets"))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
            })
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    };
    let dir = base.join("notes");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The player's persistent notes file: `assets/notes/notes.glg`.
pub fn notes_path() -> std::path::PathBuf {
    notes_dir().join("notes.glg")
}

/// The battle-log history: every rendered post-battle log appended behind its `#metrics`
/// header — designer-inspectable, and the source [`last_metrics`] reads flags back from.
pub fn battle_log_path() -> std::path::PathBuf {
    notes_dir().join("battle_logs.glg")
}

/// One `#metrics` header line for `ctx` (level id included for future per-level flags).
fn metrics_line(level_id: u32, ctx: &LogCtx) -> String {
    format!(
        "#metrics level={} result={} ticks={} lost={} killed={} ships={}",
        level_id, ctx.result, ctx.ticks, ctx.lost, ctx.killed, ctx.ships
    )
}

/// Append a rendered post-battle log (plus its metrics header) to the history file.
pub fn append_battle_log(level_id: u32, ctx: &LogCtx, rendered: &str) {
    use std::io::Write;
    let path = battle_log_path();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", metrics_line(level_id, ctx));
        let _ = writeln!(f, "{rendered}");
        let _ = writeln!(f);
    }
}

/// The PREVIOUS battle's metrics — the flags a briefing's logic-based text reads: parse the
/// LAST `#metrics` line of the history file. No history ⇒ the default ctx (`result = none`,
/// zeros), so conditions simply evaluate false-ish on a fresh profile.
pub fn last_metrics(notes: &str) -> LogCtx {
    let mut ctx = LogCtx { result: "none".into(), notes: notes.to_string(), ..LogCtx::default() };
    let Ok(text) = std::fs::read_to_string(battle_log_path()) else { return ctx };
    let Some(line) = text.lines().rev().find(|l| l.starts_with("#metrics ")) else { return ctx };
    for kv in line.trim_start_matches("#metrics ").split_whitespace() {
        let Some((k, v)) = kv.split_once('=') else { continue };
        match k {
            "result" => ctx.result = v.to_string(),
            "ticks" => ctx.ticks = v.parse().unwrap_or(0),
            "lost" => ctx.lost = v.parse().unwrap_or(0),
            "killed" => ctx.killed = v.parse().unwrap_or(0),
            "ships" => ctx.ships = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> LogCtx {
        LogCtx {
            result: "victory".into(),
            ticks: 4200,
            lost: 120,
            killed: 310,
            ships: 85,
            notes: "remember the Teleporter trick".into(),
        }
    }

    #[test]
    fn substitutes_metrics_and_drops_comments() {
        let out = render("# a comment\nLost {lost} of {ships}, {unknown} stays.", &ctx());
        assert_eq!(out, "Lost 120 of 85, {unknown} stays.");
    }

    #[test]
    fn logic_blocks_keep_and_drop_lines() {
        let t = "?if lost > 100\nbloody\n?elif lost = 0\nflawless\n?else\nordinary\n?end\ntail";
        assert_eq!(render(t, &ctx()), "bloody\ntail");
        let calm = LogCtx { lost: 0, ..ctx() };
        assert_eq!(render(t, &calm), "flawless\ntail");
        let mid = LogCtx { lost: 50, ..ctx() };
        assert_eq!(render(t, &mid), "ordinary\ntail");
    }

    #[test]
    fn notes_keyword_and_result_conditions() {
        let t = "?if notes ~ teleporter\nseen\n?end\n?if result = victory\nwon\n?end";
        assert_eq!(render(t, &ctx()), "seen\nwon");
        let lost = LogCtx { result: "defeat".into(), notes: String::new(), ..ctx() };
        assert_eq!(render(t, &lost), "");
    }
}
