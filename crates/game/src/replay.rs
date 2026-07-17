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
}
