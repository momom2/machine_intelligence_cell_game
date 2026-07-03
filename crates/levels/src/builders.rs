//! Shared **world-authoring helpers** the 10 campaign `build` functions compose.
//!
//! Everything here is a plain, deterministic constructor over the substrate: a
//! [`layer1::Interior`] (a struct's internal sub-structures + garrison) wrapped in a
//! [`world::Structure`], placed on the Layer-2 map, and joined by [`world::Lane`]s. The level
//! `build` functions in [`crate::campaign`] use these to lay out exactly the topology each
//! lesson needs.
//!
//! Two authoring styles appear:
//!
//! * **Single-struct levels (L1/L2)** — one struct whose *sub-structures* are the playable
//!   pieces. The helpers here place subs at explicit local coordinates so the proximity
//!   battle-bubble geometry (`layer1`) reads clearly for the tutorial.
//! * **Multi-struct levels (L3-L10)** — several structs joined by lanes; each struct is built
//!   with [`stocked_struct`] / [`neutral_struct`] (a small owned/neutral cluster) so the
//!   Layer-1 greedy internals have room to play and Layer-2 fleets have somewhere to land.
//!
//! All randomness stays inside each struct's seeded `Interior`, so a given `seed` reproduces a
//! level bit-for-bit (the determinism the validation suite asserts).

use layer1::{Faction, Interior, SubStructure, Vec2};
use world::{Structure, World, WorldParams};

/// The legacy radius value threaded to [`layer1::SubStructure::new`] by the multi-structure
/// helpers. **Inert**: `SubStructure::new` ignores its radius argument (a sub's radius is
/// derived from its storage capacity); kept only because the call sites must pass something.
pub const SUB_R: f32 = 4.0;

// ======================================================================================
// Multi-struct authoring (L4-L10; the L1-L3 tutorials author their subs inline in
// `campaign.rs`).
// ======================================================================================

/// A struct whose `subs` sub-structures are all owned by `owner`, laid out in a ring (one at
/// the centre, the rest around a circle sized to the corrected game scale — subs are distinct
/// tactical positions, not one blob), each seeded with `per_sub` idle ships.
///
/// This mirrors the `ai` harness's `home_struct` so a struct built here behaves exactly like
/// the structs the validated diamond/seam measurements used.
pub fn stocked_struct(
    seed: u64,
    owner: Faction,
    subs: usize,
    per_sub: usize,
    pos: Vec2,
    name: &str,
) -> Structure {
    let mut st = Interior::new(seed);
    let ids: Vec<_> = (0..subs)
        .map(|i| {
            let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
            let r = if i == 0 { 0.0 } else { 18.0 };
            st.add_sub(SubStructure::new(
                Vec2::new(r * ang.cos(), r * ang.sin()),
                SUB_R,
                owner,
            ))
        })
        .collect();
    for &s in &ids {
        for _ in 0..per_sub {
            st.spawn_ship(owner, s);
        }
    }
    st.add_storage_sub();
    Structure::new(st, pos, name)
}

/// A neutral struct with `subs` empty neutral sub-structures (capturable production), laid out
/// in the same tight ring as [`stocked_struct`]. Mirrors the `ai` harness's `neutral_struct`.
pub fn neutral_struct(seed: u64, subs: usize, pos: Vec2, name: &str) -> Structure {
    neutral_struct_res(seed, subs, pos, name, None)
}

/// As [`neutral_struct`], but with an optional per-sub `max_resistance` override (the sanctioned
/// per-level capture-pace dial, [`layer1::SubStructure::with_max_resistance`]). `None` keeps the
/// capacity-derived default (`storage_capacity · `[`layer1::sim::RESISTANCE_PER_CAPACITY`]` = 3600` at
/// the default capacity 60). A lower value makes the struct a faster grab — used where a level
/// needs a contested objective to actually resolve within a sane horizon under the grind (e.g.
/// L6's fat central prize), without touching the global default.
pub fn neutral_struct_res(seed: u64, subs: usize, pos: Vec2, name: &str, max_res: Option<f32>) -> Structure {
    let mut st = Interior::new(seed);
    for i in 0..subs.max(1) {
        let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
        let r = if i == 0 { 0.0 } else { 18.0 };
        let sub = SubStructure::new(Vec2::new(r * ang.cos(), r * ang.sin()), SUB_R, Faction::Neutral);
        let sub = match max_res {
            Some(m) => sub.with_max_resistance(m),
            None => sub,
        };
        st.add_sub(sub);
    }
    st.add_storage_sub();
    Structure::new(st, pos, name)
}

/// The **diamond** topology the campaign's pure-Automaton showcases (L8-L10) are built on — the
/// map on which `AI.md` measured the rock-paper-scissors cycle closing cleanly (10-0 on every
/// edge, both seatings). Player home (left) and Enemy home (right), each with a **private flank
/// neutral** adjacent only to that home and the centre, plus a single shared **contested
/// centre** neutral that bridges both sides:
///
/// ```text
///        fP            fE          (private flank neutrals)
///       /  \          /  \
///   P-home --- centre --- E-home
/// ```
///
/// It is mirror-symmetric, so swapping seats is perfectly fair — exactly the property the
/// both-seatings validation relies on. `home_subs`/`home_ships` size each home's garrison;
/// `centre_subs` sizes the contested keep. The Player seat is `Faction::Player`, the Enemy seat
/// (the level's Automaton) is `Faction::Ai(0)`.
pub fn diamond(
    seed: u64,
    home_subs: usize,
    home_ships: usize,
    centre_subs: usize,
) -> World {
    let mut w = World::new();
    let p = w.add_struct(stocked_struct(seed, Faction::Player, home_subs, home_ships, Vec2::new(0.0, 0.0), "Home (you)"));
    let e = w.add_struct(stocked_struct(seed + 1, Faction::Ai(0), home_subs, home_ships, Vec2::new(120.0, 0.0), "Automaton"));
    let fp = w.add_struct(neutral_struct(seed + 11, 1, Vec2::new(30.0, 40.0), "West Reach"));
    let fe = w.add_struct(neutral_struct(seed + 12, 1, Vec2::new(90.0, 40.0), "East Reach"));
    let centre = w.add_struct(neutral_struct(seed + 13, centre_subs, Vec2::new(60.0, 0.0), "The Keep"));
    w.add_lane(p, fp, 35.0);
    w.add_lane(e, fe, 35.0);
    w.add_lane(p, centre, 45.0);
    w.add_lane(e, centre, 45.0);
    w.add_lane(fp, centre, 40.0);
    w.add_lane(fe, centre, 40.0);
    w
}

/// The default inter-struct dials for every level (the operating point the `world`/`ai` suites
/// validated). Levels return this alongside their `World` so the host drives both with one
/// consistent parameter set.
pub fn default_world_params() -> WorldParams {
    WorldParams::default()
}
