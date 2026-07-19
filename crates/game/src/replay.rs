//! `.mir` replay files — the parser side (the writer lives in `main.rs::write_replay`).
//!
//! Format v1, hand-rolled line-oriented text (see the CHANGELOG entry of 2026-07-10):
//! header lines (`mir 1`, `version`, `level`, `level_hash`, `seed`, `scale_bits`), one
//! `o` line per count-canonical order (all seats, tick-stamped, in issuance order), `h`
//! checkpoint lines (tick + state_hash — playback verifies these and flags divergence
//! loudly), and one `end` line (tick, winner, lost, killed, final hash). Parsing is
//! strict on structure and loud on failure (like the `.lvl` parser): a malformed replay
//! is a bug or a version mismatch, not something to guess around.

use layer1::{Faction, JournalEntry, OrderRecord};

/// One EXTENDED-replay frame: everything the recorder logged about a rendered frame —
/// see the writer (`capture_extended_frame`) for the field semantics.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FrameRecord {
    pub ms: u64,
    pub tick: u64,
    pub mx: f32,
    pub my: f32,
    /// bit0/1/2 = L/R/M pressed this frame; bit3/4/5 = L/R/M held down.
    pub btn: u8,
    pub wheel: i32,
    pub view_interior: bool,
    pub focus: usize,
    pub cam_t: f32,
    pub zoom: [f32; 3],
    pub pan: [(f32, f32); 2],
    pub speed_idx: usize,
    pub paused: bool,
    /// Orders this frame's input produced (0 = the click/keys attempted nothing).
    pub orders: u32,
    pub keys: Vec<String>,
    pub sel_struct: Option<usize>,
    pub sel_subs: Vec<usize>,
    /// The recorder's logical screen size (for mapping the ghost cursor onto a viewer
    /// window of a different size). Early files without the fields read as 1280×800.
    pub sw: f32,
    pub sh: f32,
}

/// A parsed replay file. Some fields are carried for future consumers (the stats screen,
/// the analysis pipeline) rather than read by playback itself.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReplayFile {
    pub git: String,
    pub pkg: String,
    pub level_id: u32,
    pub level_hash: u64,
    pub seed: u64,
    pub scale: f64,
    /// Every seat's orders, tick-stamped, in original issuance order.
    pub orders: Vec<JournalEntry>,
    /// `(tick, state_hash)` divergence checkpoints, ascending.
    pub checkpoints: Vec<(u64, u64)>,
    pub end_tick: u64,
    pub winner: Faction,
    pub lost: u64,
    pub killed: u64,
    pub final_hash: u64,
    /// The EXTENDED frame stream (empty for a plain `.mir`).
    pub frames: Vec<FrameRecord>,
}

fn seat(code: &str) -> Result<Faction, String> {
    match code {
        "P" => Ok(Faction::Player),
        "N" => Ok(Faction::Neutral),
        _ => code
            .strip_prefix('A')
            .and_then(|i| i.parse::<u8>().ok())
            .map(Faction::Ai)
            .ok_or_else(|| format!("bad seat code {code:?}")),
    }
}

/// Parse a `.mir` v1 replay. Returns a readable error naming the offending line.
pub fn parse(text: &str) -> Result<ReplayFile, String> {
    let mut lines = text.lines().enumerate();
    let mut need = |key: &str| -> Result<(usize, String), String> {
        for (i, l) in lines.by_ref() {
            let l = l.trim();
            if l.is_empty() {
                continue;
            }
            return match l.split_once(' ') {
                Some((k, rest)) if k == key => Ok((i + 1, rest.to_string())),
                _ => Err(format!("line {}: expected `{key} ...`, got {l:?}", i + 1)),
            };
        }
        Err(format!("missing `{key}` line"))
    };
    let (_, v) = need("mir")?;
    if v.trim() != "1" {
        return Err(format!("unsupported mir version {v:?} (this build reads 1)"));
    }
    let (_, version) = need("version")?;
    let (git, pkg) = version.split_once(' ').unwrap_or((version.as_str(), ""));
    let (git, pkg) = (git.to_string(), pkg.to_string());
    let (ln, v) = need("level")?;
    let level_id: u32 = v.trim().parse().map_err(|e| format!("line {ln}: level: {e}"))?;
    let (ln, v) = need("level_hash")?;
    let level_hash = u64::from_str_radix(v.trim(), 16).map_err(|e| format!("line {ln}: level_hash: {e}"))?;
    let (ln, v) = need("seed")?;
    let seed: u64 = v.trim().parse().map_err(|e| format!("line {ln}: seed: {e}"))?;
    let (ln, v) = need("scale_bits")?;
    let scale = f64::from_bits(
        u64::from_str_radix(v.trim(), 16).map_err(|e| format!("line {ln}: scale_bits: {e}"))?,
    );

    let mut orders = Vec::new();
    let mut checkpoints = Vec::new();
    let mut frames = Vec::new();
    let mut end = None;
    for (i, l) in lines {
        let ln = i + 1;
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        let toks: Vec<&str> = l.split_whitespace().collect();
        let int = |s: &str| -> Result<u64, String> {
            s.parse::<u64>().map_err(|e| format!("line {ln}: {s:?}: {e}"))
        };
        match toks[0] {
            "o" => {
                // o <tick> m <sid> <src> <tgt> <count> <seat> | o <tick> f <from> <to> <count> <seat>
                let tick = int(toks.get(1).ok_or(format!("line {ln}: truncated"))?)?;
                let record = match *toks.get(2).ok_or(format!("line {ln}: truncated"))? {
                    "m" if toks.len() == 8 => OrderRecord::Move {
                        sid: int(toks[3])? as usize,
                        source: int(toks[4])? as usize,
                        target: int(toks[5])? as usize,
                        count: int(toks[6])? as usize,
                        faction: seat(toks[7]).map_err(|e| format!("line {ln}: {e}"))?,
                    },
                    "f" if toks.len() == 7 => OrderRecord::Fleet {
                        from: int(toks[3])? as usize,
                        to: int(toks[4])? as usize,
                        count: int(toks[5])? as usize,
                        faction: seat(toks[6]).map_err(|e| format!("line {ln}: {e}"))?,
                    },
                    _ => return Err(format!("line {ln}: bad order line {l:?}")),
                };
                orders.push(JournalEntry { tick, record });
            }
            "f" if toks.len() == 22 || toks.len() == 24 => {
                let f32t = |s: &str| -> Result<f32, String> {
                    s.parse::<f32>().map_err(|e| format!("line {ln}: {s:?}: {e}"))
                };
                let sel: Vec<&str> = toks[21].splitn(2, ':').collect();
                let sel_struct = match sel.first().copied().unwrap_or("-") {
                    "-" => None,
                    v => Some(int(v)? as usize),
                };
                let sel_subs = match sel.get(1).copied().unwrap_or("-") {
                    "-" => Vec::new(),
                    v => v
                        .split(',')
                        .map(|x| int(x).map(|n| n as usize))
                        .collect::<Result<_, _>>()?,
                };
                frames.push(FrameRecord {
                    ms: int(toks[1])?,
                    tick: int(toks[2])?,
                    mx: f32t(toks[3])?,
                    my: f32t(toks[4])?,
                    btn: int(toks[5])? as u8,
                    wheel: toks[6].parse::<i32>().map_err(|e| format!("line {ln}: {e}"))?,
                    view_interior: toks[7] == "I",
                    focus: int(toks[8])? as usize,
                    cam_t: f32t(toks[9])?,
                    zoom: [f32t(toks[10])?, f32t(toks[11])?, f32t(toks[12])?],
                    pan: [(f32t(toks[13])?, f32t(toks[14])?), (f32t(toks[15])?, f32t(toks[16])?)],
                    speed_idx: int(toks[17])? as usize,
                    paused: toks[18] != "0",
                    orders: int(toks[19])? as u32,
                    keys: if toks[20] == "-" {
                        Vec::new()
                    } else {
                        toks[20].split(',').map(String::from).collect()
                    },
                    sel_struct,
                    sel_subs,
                    sw: if toks.len() == 24 { f32t(toks[22])? } else { 1280.0 },
                    sh: if toks.len() == 24 { f32t(toks[23])? } else { 800.0 },
                });
            }
            "h" if toks.len() == 3 => {
                let t = int(toks[1])?;
                let h = u64::from_str_radix(toks[2], 16).map_err(|e| format!("line {ln}: {e}"))?;
                checkpoints.push((t, h));
            }
            "end" if toks.len() == 6 => {
                end = Some((
                    int(toks[1])?,
                    seat(toks[2]).map_err(|e| format!("line {ln}: {e}"))?,
                    int(toks[3])?,
                    int(toks[4])?,
                    u64::from_str_radix(toks[5], 16).map_err(|e| format!("line {ln}: {e}"))?,
                ));
            }
            _ => return Err(format!("line {ln}: unrecognized line {l:?}")),
        }
    }
    let (end_tick, winner, lost, killed, final_hash) =
        end.ok_or("missing `end` line (unsealed/truncated replay)")?;
    Ok(ReplayFile {
        git,
        pkg,
        level_id,
        level_hash,
        seed,
        scale,
        orders,
        checkpoints,
        end_tick,
        winner,
        lost,
        killed,
        final_hash,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writer↔parser agreement on a hand-built file (the writer's format, verbatim).
    #[test]
    fn parses_the_writers_format() {
        let text = "mir 1\nversion abc123 0.1.0\nlevel 7\nlevel_hash 00000000deadbeef\n\
                    seed 1234\nscale_bits 4038000000000000\n\
                    o 0 m 0 1 2 100 P\no 480 f 0 1 50 A0\n\
                    h 600 0123456789abcdef\n\
                    end 900 P 10 20 fedcba9876543210\n";
        let r = parse(text).expect("parses");
        assert_eq!((r.level_id, r.seed), (7, 1234));
        assert_eq!(r.scale, 24.0);
        assert_eq!(r.orders.len(), 2);
        assert_eq!(
            r.orders[1].record,
            OrderRecord::Fleet { from: 0, to: 1, count: 50, faction: Faction::Ai(0) }
        );
        assert_eq!(r.checkpoints, vec![(600, 0x0123456789abcdef)]);
        assert_eq!((r.end_tick, r.winner, r.lost, r.killed), (900, Faction::Player, 10, 20));
        assert_eq!(r.final_hash, 0xfedcba9876543210);
    }

    /// EXTENDED frames round-trip: the writer's `f` line format, parsed back exactly —
    /// including a no-op click (orders 0), held buttons, keys, and a selection.
    #[test]
    fn parses_extended_frames()  {
        let text = "mir 1\nversion abc123 0.1.0\nlevel 1\nlevel_hash 00000000deadbeef\n\
                    seed 7\nscale_bits 4038000000000000\n\
                    f 16 42 512.5 300.0 9 -1 I 0 1.000 1.0000 2.5000 1.0000 12.50 -3.25 0.00 0.00 1 0 0 W,space 2:1,4\n\
                    f 33 43 512.5 300.0 0 0 L 0 0.000 1.0000 2.5000 1.0000 12.50 -3.25 0.00 0.00 1 1 3 - -:-\n\
                    end 900 P 10 20 fedcba9876543210\n";
        let r = parse(text).expect("parses");
        assert_eq!(r.frames.len(), 2);
        let f0 = &r.frames[0];
        assert_eq!((f0.ms, f0.tick, f0.btn, f0.wheel), (16, 42, 9, -1));
        assert!(f0.view_interior);
        assert_eq!(f0.orders, 0, "a click that produced nothing records 0 orders");
        assert_eq!(f0.keys, vec!["W".to_string(), "space".to_string()]);
        assert_eq!((f0.sel_struct, f0.sel_subs.as_slice()), (Some(2), &[1usize, 4][..]));
        let f1 = &r.frames[1];
        assert!(f1.paused && !f1.view_interior && f1.keys.is_empty());
        assert_eq!(f1.orders, 3);
        assert_eq!(f1.sel_struct, None);
        assert_eq!((f0.sw, f0.sh), (1280.0, 800.0), "22-token lines read as the default dims");
    }

    /// The current 24-token writer line (with logical screen dims) parses too.
    #[test]
    fn parses_frame_with_screen_dims() {
        let text = "mir 1\nversion abc123 0.1.0\nlevel 1\nlevel_hash 00000000deadbeef\n\
                    seed 7\nscale_bits 4038000000000000\n\
                    f 16 42 512.5 300.0 1 0 L 0 0.000 1.0000 1.0000 1.0000 0.00 0.00 0.00 0.00 0 0 1 - -:- 1600 900\n\
                    end 900 P 0 0 fedcba9876543210\n";
        let r = parse(text).expect("parses");
        assert_eq!((r.frames[0].sw, r.frames[0].sh), (1600.0, 900.0));
    }
}
