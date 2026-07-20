//! `.mir` replay files — the parser side (the writer lives in `main.rs::write_replay`).
//!
//! Format **v2** (the pure-L1 pivot, owner 2026-07-20 — v1's multi-struct lines live on
//! the `layer2` branch): hand-rolled line-oriented text — header lines (`mir 2`,
//! `version`, `level`, `level_hash`, `seed`, `scale_bits`), one `o` line per
//! count-canonical interior move (all seats, tick-stamped, in issuance order), `h`
//! checkpoint lines (tick + state_hash — playback verifies these and flags divergence
//! loudly), `s` persisted-snapshot lines (tick + blob FNV + base64 of the interior
//! snapshot blob), and one `end` line (tick, winner, lost, killed, final hash). EXTENDED
//! files (`.mirx`) append one `f` line per rendered frame. Parsing is strict on structure
//! and loud on failure (like the `.lvl` parser): a malformed replay is a bug or a version
//! mismatch, not something to guess around.

use layer1::{Faction, JournalEntry, OrderRecord};

/// One EXTENDED-replay frame (v2, 16 tokens): everything the recorder logged about a
/// rendered frame — see the writer (`capture_extended_frame`) for the field semantics.
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
    pub zoom: f32,
    pub pan: (f32, f32),
    pub speed_idx: usize,
    pub paused: bool,
    /// Orders this frame's input produced (0 = the click/keys attempted nothing).
    pub orders: u32,
    pub keys: Vec<String>,
    pub sel_sub: Option<usize>,
    pub sel_subs: Vec<usize>,
    /// The recorder's logical screen size (for mapping the ghost cursor onto a viewer
    /// window of a different size).
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
    /// Persisted interior SNAPSHOTS: `(tick, blob_fnv, base64)` per `s` line, ascending.
    /// The loader decodes + integrity-checks these lazily (bad ones are dropped, not
    /// fatal) — see `main.rs`'s snapshot restore.
    pub snaps: Vec<(u64, u64, String)>,
}

/// FNV-1a 64 over raw bytes — the snapshot lines' byte-integrity stamp (the same hash the
/// `levels` crate uses for content hashes; a text file mangled in transit fails here before
/// the blob is ever decoded).
pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (padded) — snapshot blobs travel inside the text `.mir` format.
/// Hand-rolled: the substrate stays dependency-free and this crate follows suit.
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        let enc = |i: u32| B64[(n >> (18 - 6 * i) & 63) as usize] as char;
        out.push(enc(0));
        out.push(enc(1));
        out.push(if chunk.len() > 1 { enc(2) } else { '=' });
        out.push(if chunk.len() > 2 { enc(3) } else { '=' });
    }
    out
}

/// Decode [`b64_encode`]'s output. `None` on any malformed input (bad char, bad length,
/// misplaced padding) — snapshot loading treats that as "drop this snapshot".
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if s.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (ci, chunk) in s.chunks(4).enumerate() {
        let last = (ci + 1) * 4 == s.len();
        let pads = chunk.iter().filter(|&&c| c == b'=').count();
        if pads > 2 || (pads > 0 && (!last || chunk[..4 - pads].iter().any(|&c| c == b'='))) {
            return None;
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pads] {
            n = n << 6 | val(c)?;
        }
        n <<= 6 * pads as u32;
        out.push((n >> 16) as u8);
        if pads < 2 {
            out.push((n >> 8) as u8);
        }
        if pads < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
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

/// Parse a `.mir` v2 replay. Returns a readable error naming the offending line.
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
    if v.trim() != "2" {
        return Err(format!(
            "unsupported mir version {v:?} (this build reads 2; v1 multi-struct replays \
             belong to the layer2 branch)"
        ));
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
    let mut snaps = Vec::new();
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
            // o <tick> m <src> <tgt> <count> <seat> — the single interior's move atom.
            "o" if toks.len() == 7 && toks[2] == "m" => {
                let tick = int(toks[1])?;
                orders.push(JournalEntry {
                    tick,
                    record: OrderRecord::Move {
                        source: int(toks[3])? as usize,
                        target: int(toks[4])? as usize,
                        count: int(toks[5])? as usize,
                        faction: seat(toks[6]).map_err(|e| format!("line {ln}: {e}"))?,
                    },
                });
            }
            "o" => return Err(format!("line {ln}: bad order line {l:?}")),
            // f v2: the tag + 16 fields = 17 tokens — ms(1) tick(2) mx(3) my(4) btn(5)
            // wheel(6) zoom(7) panx(8) pany(9) spd(10) flags(11) orders(12) keys(13)
            // sel(14) sw(15) sh(16).
            "f" if toks.len() == 17 => {
                let f32t = |s: &str| -> Result<f32, String> {
                    s.parse::<f32>().map_err(|e| format!("line {ln}: {s:?}: {e}"))
                };
                let sel: Vec<&str> = toks[14].splitn(2, ':').collect();
                let sel_sub = match sel.first().copied().unwrap_or("-") {
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
                    zoom: f32t(toks[7])?,
                    pan: (f32t(toks[8])?, f32t(toks[9])?),
                    speed_idx: int(toks[10])? as usize,
                    paused: toks[11] != "0",
                    orders: int(toks[12])? as u32,
                    keys: if toks[13] == "-" {
                        Vec::new()
                    } else {
                        toks[13].split(',').map(String::from).collect()
                    },
                    sel_sub,
                    sel_subs,
                    sw: f32t(toks[15])?,
                    sh: f32t(toks[16])?,
                });
            }
            "h" if toks.len() == 3 => {
                let t = int(toks[1])?;
                let h = u64::from_str_radix(toks[2], 16).map_err(|e| format!("line {ln}: {e}"))?;
                checkpoints.push((t, h));
            }
            // s <tick> <blob_fnv:016x> <base64> — a persisted interior snapshot.
            "s" if toks.len() == 4 => {
                let t = int(toks[1])?;
                let fnv = u64::from_str_radix(toks[2], 16).map_err(|e| format!("line {ln}: {e}"))?;
                snaps.push((t, fnv, toks[3].to_string()));
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
        snaps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writer↔parser agreement on a hand-built v2 file (the writer's format, verbatim).
    #[test]
    fn parses_the_writers_format() {
        let text = "mir 2\nversion abc123 0.1.0\nlevel 7\nlevel_hash 00000000deadbeef\n\
                    seed 1234\nscale_bits 4038000000000000\n\
                    o 0 m 1 2 100 P\no 480 m 0 3 50 A0\n\
                    h 600 0123456789abcdef\n\
                    end 900 P 10 20 fedcba9876543210\n";
        let r = parse(text).expect("parses");
        assert_eq!((r.level_id, r.seed), (7, 1234));
        assert_eq!(r.scale, 24.0);
        assert_eq!(r.orders.len(), 2);
        assert_eq!(
            r.orders[1].record,
            OrderRecord::Move { source: 0, target: 3, count: 50, faction: Faction::Ai(0) }
        );
        assert_eq!(r.checkpoints, vec![(600, 0x0123456789abcdef)]);
        assert_eq!((r.end_tick, r.winner, r.lost, r.killed), (900, Faction::Player, 10, 20));
        assert_eq!(r.final_hash, 0xfedcba9876543210);
    }

    /// A v1 file is rejected loudly with the layer2-branch pointer.
    #[test]
    fn rejects_v1_files() {
        let text = "mir 1\nversion abc123 0.1.0\nlevel 1\nlevel_hash 0\nseed 7\n\
                    scale_bits 4038000000000000\nend 900 P 0 0 0\n";
        let err = parse(text).unwrap_err();
        assert!(err.contains("layer2"), "the error should point at the layer2 branch: {err}");
    }

    /// EXTENDED v2 frames round-trip: the writer's 16-token `f` line, parsed back exactly
    /// — including a no-op click (orders 0), held buttons, keys, and a selection.
    #[test]
    fn parses_extended_frames() {
        let text = "mir 2\nversion abc123 0.1.0\nlevel 1\nlevel_hash 00000000deadbeef\n\
                    seed 7\nscale_bits 4038000000000000\n\
                    f 16 42 512.5 300.0 9 -1 2.5000 12.50 -3.25 1 0 0 W,space 2:1,4 1280 800\n\
                    f 33 43 512.5 300.0 0 0 1.0000 0.00 0.00 1 1 3 - -:- 1600 900\n\
                    end 900 P 10 20 fedcba9876543210\n";
        let r = parse(text).expect("parses");
        assert_eq!(r.frames.len(), 2);
        let f0 = &r.frames[0];
        assert_eq!((f0.ms, f0.tick, f0.btn, f0.wheel), (16, 42, 9, -1));
        assert_eq!((f0.zoom, f0.pan), (2.5, (12.5, -3.25)));
        assert_eq!(f0.orders, 0, "a click that produced nothing records 0 orders");
        assert_eq!(f0.keys, vec!["W".to_string(), "space".to_string()]);
        assert_eq!((f0.sel_sub, f0.sel_subs.as_slice()), (Some(2), &[1usize, 4][..]));
        assert_eq!((f0.sw, f0.sh), (1280.0, 800.0));
        let f1 = &r.frames[1];
        assert!(f1.paused && f1.keys.is_empty());
        assert_eq!(f1.orders, 3);
        assert_eq!(f1.sel_sub, None);
        assert_eq!((f1.sw, f1.sh), (1600.0, 900.0));
    }

    /// Base64 round-trips every length mod 3 (padding paths) and rejects malformed input.
    #[test]
    fn b64_roundtrip_and_rejection() {
        for n in 0..10usize {
            let bytes: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
            let enc = b64_encode(&bytes);
            assert_eq!(b64_decode(&enc).as_deref(), Some(bytes.as_slice()), "len {n}");
        }
        assert_eq!(b64_decode("AAA"), None, "bad length");
        assert_eq!(b64_decode("A=AA"), None, "misplaced padding");
        assert_eq!(b64_decode("AA!A"), None, "bad alphabet");
    }

    /// `s` snapshot lines parse into (tick, fnv, payload) and ride alongside everything else.
    #[test]
    fn parses_snapshot_lines() {
        let text = "mir 2\nversion abc123 0.1.0\nlevel 1\nlevel_hash 00000000deadbeef\n\
                    seed 7\nscale_bits 4038000000000000\n\
                    h 3600 0123456789abcdef\n\
                    s 3600 00000000000000ff AQID\n\
                    end 4000 P 0 0 fedcba9876543210\n";
        let r = parse(text).expect("parses");
        assert_eq!(r.snaps, vec![(3600, 0xff, "AQID".to_string())]);
    }
}
