//! **Sound.** Every effect is synthesised at startup (no audio assets ship with the game) into
//! 16-bit mono WAV bytes and handed to macroquad's audio. Playback is fire-and-forget through
//! [`play`]; when the audio device or a decode fails the game stays silent instead of failing.
//!
//! The synthesis ([`render_cue`], [`render_ambience`], [`wav`]) is pure and unit-tested; only
//! [`init`] / [`play`] touch the audio device.

#![cfg_attr(not(has_sound), allow(dead_code))]


pub const SAMPLE_RATE: u32 = 22_050;

/// The game events that make a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cue {
    /// A source sub was selected.
    Select,
    /// An order launched ships.
    Order,
    /// The player captured a sub.
    Capture,
    /// The player lost a sub.
    Lost,
    /// Two other factions traded a sub.
    Flip,
    /// A burst of ships died.
    Combat,
    Victory,
    Defeat,
}

impl Cue {
    pub const ALL: [Cue; 8] = [
        Cue::Select, Cue::Order, Cue::Capture, Cue::Lost, Cue::Flip, Cue::Combat, Cue::Victory,
        Cue::Defeat,
    ];

    /// Minimum seconds between two plays of the cue (stops a battle from becoming a buzz).
    fn min_gap(self) -> f64 {
        match self {
            Cue::Combat => 0.35,
            Cue::Flip | Cue::Order => 0.08,
            _ => 0.05,
        }
    }
}

// --------------------------------------------------------------------------------------------
// Synthesis (pure).
// --------------------------------------------------------------------------------------------

fn n_samples(seconds: f32) -> usize {
    (seconds * SAMPLE_RATE as f32) as usize
}

/// A note: a sine with a touch of 2nd harmonic, a linear glide, and an attack/decay envelope.
fn note(freq0: f32, freq1: f32, seconds: f32, gain: f32, out: &mut Vec<f32>, start: f32) {
    let n = n_samples(seconds);
    let s0 = n_samples(start);
    if out.len() < s0 + n {
        out.resize(s0 + n, 0.0);
    }
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let f = freq0 + (freq1 - freq0) * t;
        phase += std::f32::consts::TAU * f / SAMPLE_RATE as f32;
        let attack = (i as f32 / (0.006 * SAMPLE_RATE as f32)).min(1.0);
        let env = attack * (1.0 - t).powf(1.6);
        out[s0 + i] += gain * env * (phase.sin() + 0.25 * (2.0 * phase).sin());
    }
}

/// Deterministic noise burst (xorshift), low-passed by a one-pole filter.
fn noise(seconds: f32, gain: f32, cutoff: f32, out: &mut Vec<f32>) {
    let n = n_samples(seconds);
    out.resize(n, 0.0);
    let mut x = 0x9E37_79B9u32;
    let mut lp = 0.0f32;
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let white = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
        lp += cutoff * (white - lp);
        let t = i as f32 / n as f32;
        out[i] = gain * lp * (1.0 - t).powi(2);
    }
}

/// The samples (`-1..1`) of one cue.
pub fn render_cue(cue: Cue) -> Vec<f32> {
    let mut v = Vec::new();
    match cue {
        Cue::Select => note(880.0, 900.0, 0.07, 0.35, &mut v, 0.0),
        Cue::Order => {
            note(440.0, 520.0, 0.06, 0.3, &mut v, 0.0);
            note(660.0, 780.0, 0.09, 0.3, &mut v, 0.05);
        }
        Cue::Capture => {
            for (k, f) in [523.25, 659.25, 783.99].into_iter().enumerate() {
                note(f, f, 0.16, 0.3, &mut v, 0.07 * k as f32);
            }
        }
        Cue::Lost => {
            note(392.0, 330.0, 0.18, 0.35, &mut v, 0.0);
            note(277.2, 220.0, 0.26, 0.35, &mut v, 0.11);
        }
        Cue::Flip => note(500.0, 480.0, 0.05, 0.2, &mut v, 0.0),
        Cue::Combat => noise(0.14, 0.55, 0.22, &mut v),
        Cue::Victory => {
            for (k, f) in [261.63, 329.63, 392.0, 523.25].into_iter().enumerate() {
                note(f, f, 0.55, 0.28, &mut v, 0.13 * k as f32);
            }
        }
        Cue::Defeat => {
            for (k, f) in [220.0, 174.6, 146.8].into_iter().enumerate() {
                note(f, f * 0.97, 0.7, 0.32, &mut v, 0.28 * k as f32);
            }
        }
    }
    v
}

/// Length of the ambience loop in seconds. Every partial completes a whole number of cycles in
/// it, so the loop is seamless.
pub const AMBIENCE_SECONDS: u32 = 20;

/// A slow drone: A1 + its fifth, breathing under a 0.1 Hz swell.
pub fn render_ambience() -> Vec<f32> {
    let n = (AMBIENCE_SECONDS * SAMPLE_RATE) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let swell = 0.65 + 0.35 * (std::f32::consts::TAU * 0.1 * t).sin();
            let a = (std::f32::consts::TAU * 55.0 * t).sin();
            let e = (std::f32::consts::TAU * 82.5 * t).sin();
            let o = (std::f32::consts::TAU * 110.0 * t + 1.0).sin();
            0.55 * swell * (a + 0.5 * e + 0.25 * o) / 1.75
        })
        .collect()
}

/// Encode `samples` (`-1..1`) as a 16-bit mono PCM WAV file.
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    b.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32000.0) as i16).to_le_bytes());
    }
    b
}

// --------------------------------------------------------------------------------------------
// Playback (compiled only with the audio backend; see `has_sound` in build.rs).
// --------------------------------------------------------------------------------------------

#[cfg(has_sound)]
mod backend {
    use super::*;
    use macroquad::audio::{load_sound_from_bytes, play_sound, stop_sound, PlaySoundParams, Sound};
    use std::cell::RefCell;

    struct Bank {
        cues: Vec<(Cue, Sound)>,
        ambience: Sound,
        ambience_on: bool,
        last_played: Vec<(Cue, f64)>,
        volume: f32,
    }

    thread_local! {
        static BANK: RefCell<Option<Bank>> = const { RefCell::new(None) };
    }

    /// Whether an audio device plausibly exists. The audio backend panics on its own thread (and the
    /// release profile aborts) when there is none, so headless Linux boxes must never reach it.
    fn audio_available() -> bool {
        if std::env::var_os("MI_NO_SOUND").is_some() {
            return false;
        }
        #[cfg(target_os = "linux")]
        if !std::path::Path::new("/dev/snd").exists() {
            return false;
        }
        true
    }

    /// Synthesise and load every sound. Silent no-op if `disabled` or no audio device exists.
    pub async fn init(volume: f32, disabled: bool) {
        if disabled || !audio_available() {
            return;
        }
        let mut cues = Vec::new();
        for cue in Cue::ALL {
            if let Ok(s) = load_sound_from_bytes(&wav(&render_cue(cue))).await {
                cues.push((cue, s));
            }
        }
        let Ok(ambience) = load_sound_from_bytes(&wav(&render_ambience())).await else { return };
        BANK.with(|b| {
            *b.borrow_mut() =
                Some(Bank { cues, ambience, ambience_on: false, last_played: Vec::new(), volume });
        });
    }

    /// Set the master volume, `0` = mute.
    pub fn set_volume(volume: f32) {
        BANK.with(|b| {
            if let Some(bank) = b.borrow_mut().as_mut() {
                bank.volume = volume.clamp(0.0, 1.0);
                if bank.ambience_on {
                    stop_sound(&bank.ambience);
                    bank.ambience_on = false;
                }
            }
        });
    }

    /// Play `cue` once, rate-limited per cue. `now` is wall-clock seconds.
    pub fn play(cue: Cue, now: f64) {
        BANK.with(|b| {
            let mut b = b.borrow_mut();
            let Some(bank) = b.as_mut() else { return };
            if bank.volume <= 0.0 {
                return;
            }
            if let Some(slot) = bank.last_played.iter_mut().find(|(c, _)| *c == cue) {
                if now - slot.1 < cue.min_gap() {
                    return;
                }
                slot.1 = now;
            } else {
                bank.last_played.push((cue, now));
            }
            if let Some((_, s)) = bank.cues.iter().find(|(c, _)| *c == cue) {
                play_sound(s, PlaySoundParams { looped: false, volume: bank.volume });
            }
        });
    }

    /// Start or stop the ambience loop (a match is running).
    pub fn ambience(on: bool) {
        BANK.with(|b| {
            let mut b = b.borrow_mut();
            let Some(bank) = b.as_mut() else { return };
            if on && !bank.ambience_on && bank.volume > 0.0 {
                play_sound(&bank.ambience, PlaySoundParams { looped: true, volume: bank.volume * 0.35 });
                bank.ambience_on = true;
            } else if !on && bank.ambience_on {
                stop_sound(&bank.ambience);
                bank.ambience_on = false;
            }
        });
    }

}

#[cfg(has_sound)]
pub use backend::{ambience, init, play, set_volume};

/// Silent stand-ins when the audio backend is not compiled in.
#[cfg(not(has_sound))]
mod silent {
    use super::Cue;
    pub async fn init(_volume: f32, _disabled: bool) {}
    pub fn set_volume(_volume: f32) {}
    pub fn play(_cue: Cue, _now: f64) {}
    pub fn ambience(_on: bool) {}
}
#[cfg(not(has_sound))]
pub use silent::{ambience, init, play, set_volume};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_is_audible_bounded_and_ends_quietly() {
        for cue in Cue::ALL {
            let s = render_cue(cue);
            assert!(s.len() > n_samples(0.04), "{cue:?} too short");
            let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.05, "{cue:?} inaudible (peak {peak})");
            assert!(peak <= 1.0, "{cue:?} clips (peak {peak})");
            let tail = s[s.len() - 8..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(tail < 0.05, "{cue:?} ends with a click ({tail})");
        }
    }

    #[test]
    fn wav_header_describes_the_data() {
        let s = render_cue(Cue::Order);
        let w = wav(&s);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize, s.len() * 2);
        assert_eq!(w.len(), 44 + s.len() * 2);
    }

    #[test]
    fn the_ambience_loops_seamlessly() {
        let a = render_ambience();
        assert_eq!(a.len(), (AMBIENCE_SECONDS * SAMPLE_RATE) as usize);
        // The step from the last sample back to the first is no bigger than a normal step.
        let wrap = (a[0] - a[a.len() - 1]).abs();
        let normal = a.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(wrap <= normal * 1.05, "loop seam {wrap} vs max step {normal}");
    }

    #[test]
    fn cues_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for cue in Cue::ALL {
            let s = render_cue(cue);
            let key: Vec<i32> = s.iter().step_by(97).map(|x| (x * 1000.0) as i32).collect();
            assert!(seen.insert(key), "{cue:?} sounds identical to another cue");
        }
    }
}
