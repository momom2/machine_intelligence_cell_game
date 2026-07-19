//! # snap — byte-exact state (de)serialization primitives (replay snapshots)
//!
//! The persisted-snapshot feature (owner, 2026-07-19: "let the snapshots be part of the
//! replay") needs the full sim state written to bytes and restored **bit-exactly** — a
//! restored world must evolve identically to the clone it came from, or the replay
//! checkpoint verifier trips DIVERGED. These are the dependency-free primitives (the
//! substrate crates take no serde): a little-endian writer and a bounds-checked reader.
//! Floats travel as raw IEEE bits (`to_bits`/`from_bits`) — exactness is the whole point.
//!
//! Robustness stance: the reader never panics on malformed input — every read is checked
//! and `None` bubbles up, so a corrupt or version-drifted snapshot line is *dropped* by
//! the loader (falling back to background indexing) rather than crashing the viewer.

use crate::types::{Faction, Vec2};

/// Shared [`Faction`] encoding (used by both this crate's interior blobs and `world`'s).
pub fn w_faction(w: &mut SnapWriter, f: Faction) {
    match f {
        Faction::Neutral => w.u8(0),
        Faction::Player => w.u8(1),
        Faction::Ai(i) => {
            w.u8(2);
            w.u8(i);
        }
    }
}

pub fn r_faction(r: &mut SnapReader) -> Option<Faction> {
    Some(match r.u8()? {
        0 => Faction::Neutral,
        1 => Faction::Player,
        2 => Faction::Ai(r.u8()?),
        _ => return None,
    })
}

/// Shared [`Vec2`] encoding (raw IEEE bits, like every float here).
pub fn w_vec2(w: &mut SnapWriter, v: Vec2) {
    w.f32(v.x);
    w.f32(v.y);
}

pub fn r_vec2(r: &mut SnapReader) -> Option<Vec2> {
    Some(Vec2::new(r.f32()?, r.f32()?))
}

/// Little-endian byte writer for snapshot blobs.
pub struct SnapWriter {
    pub buf: Vec<u8>,
}

impl SnapWriter {
    pub fn new() -> SnapWriter {
        SnapWriter { buf: Vec::new() }
    }
    #[inline]
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    #[inline]
    pub fn bool(&mut self, v: bool) {
        self.buf.push(u8::from(v));
    }
    #[inline]
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    /// `usize` travels as `u64` (wasm32 ↔ native width difference must not change the format).
    #[inline]
    pub fn uz(&mut self, v: usize) {
        self.u64(v as u64);
    }
    #[inline]
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    #[inline]
    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    pub fn str(&mut self, v: &str) {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v.as_bytes());
    }
}

impl Default for SnapWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounds-checked little-endian reader over a snapshot blob.
pub struct SnapReader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> SnapReader<'a> {
    pub fn new(b: &'a [u8]) -> SnapReader<'a> {
        SnapReader { b, pos: 0 }
    }
    /// True when every byte has been consumed (loaders require this — trailing garbage
    /// means the format drifted).
    pub fn exhausted(&self) -> bool {
        self.pos == self.b.len()
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    #[inline]
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    #[inline]
    pub fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    #[inline]
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    #[inline]
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    #[inline]
    pub fn uz(&mut self) -> Option<usize> {
        usize::try_from(self.u64()?).ok()
    }
    #[inline]
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    #[inline]
    pub fn f64(&mut self) -> Option<f64> {
        Some(f64::from_bits(self.u64()?))
    }
    pub fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}
