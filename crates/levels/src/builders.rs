//! Shared **world-authoring helpers** the 10 campaign `build` functions compose.
//!
//! Everything here is a plain, deterministic constructor over the substrate: a
//! [`layer1::Structure`] (a planet's internal sub-structures + garrison) wrapped in a
//! [`world::Planet`], placed on the Layer-2 map, and joined by [`world::Lane`]s. The level
//! `build` functions in [`crate::campaign`] use these to lay out exactly the topology each
//! lesson needs.
//!
//! Two authoring styles appear:
//!
//! * **Single-planet levels (L1/L2)** — one planet whose *sub-structures* are the playable
//!   pieces. The helpers here place subs at explicit local coordinates so the proximity
//!   battle-bubble geometry (`layer1`) reads clearly for the tutorial.
//! * **Multi-planet levels (L3-L10)** — several planets joined by lanes; each planet is built
//!   with [`stocked_planet`] / [`neutral_planet`] (a small owned/neutral cluster) so the
//!   Layer-1 greedy internals have room to play and Layer-2 fleets have somewhere to land.
//!
//! All randomness stays inside each planet's seeded `Structure`, so a given `seed` reproduces a
//! level bit-for-bit (the determinism the validation suite asserts).

use layer1::{Faction, Structure, SubStructure, Vec2};
use world::{Planet, World, WorldParams};

/// The standard sub-structure radius used across the campaign. Matches the radius the `ai`
/// harness and `world` tests use, so capture/engagement geometry behaves as those suites
/// measured.
pub const SUB_R: f32 = 4.0;

/// The standard home/keep radius (slightly larger — a home base has more garrison room and a
/// stronger defender edge). Used for the single-planet tutorials where one larger anchor reads
/// well.
pub const HOME_R: f32 = 5.0;

// ======================================================================================
// Single-planet authoring (L1 / L2 tutorials).
// ======================================================================================

/// A planet authored sub-by-sub: each `(local_pos, radius, owner, garrison)` becomes one
/// sub-structure on the planet, seeded with `garrison` idle ships of that sub's owner (a
/// neutral sub gets none — ships are never neutral). The planet sits at Layer-2 `pos` with
/// `name`. This is the explicit-layout constructor the tutorials use so the engagement-radius
/// geometry is exactly as designed.
pub fn authored_planet(
    seed: u64,
    subs: &[(Vec2, f32, Faction, usize)],
    pos: Vec2,
    name: &str,
) -> Planet {
    let mut st = Structure::new(seed);
    for &(p, r, owner, garrison) in subs {
        let id = st.add_sub(SubStructure::new(p, r, owner));
        if owner.is_real() {
            for _ in 0..garrison {
                st.spawn_ship(owner, id);
            }
        }
    }
    Planet::new(st, pos, name)
}

// ======================================================================================
// Multi-planet authoring (L3-L10).
// ======================================================================================

/// A planet whose `subs` sub-structures are all owned by `owner`, laid out in a tight ring
/// (one at the centre, the rest around a small circle) so they sit within engagement proximity
/// and the Layer-1 greedy can shuffle between them, each seeded with `per_sub` idle ships.
///
/// This mirrors the `ai` harness's `home_planet` so a planet built here behaves exactly like
/// the planets the validated diamond/seam measurements used.
pub fn stocked_planet(
    seed: u64,
    owner: Faction,
    subs: usize,
    per_sub: usize,
    pos: Vec2,
    name: &str,
) -> Planet {
    let mut st = Structure::new(seed);
    let ids: Vec<_> = (0..subs)
        .map(|i| {
            let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
            let r = if i == 0 { 0.0 } else { 9.0 };
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
    Planet::new(st, pos, name)
}

/// A neutral planet with `subs` empty neutral sub-structures (capturable production), laid out
/// in the same tight ring as [`stocked_planet`]. Mirrors the `ai` harness's `neutral_planet`.
pub fn neutral_planet(seed: u64, subs: usize, pos: Vec2, name: &str) -> Planet {
    let mut st = Structure::new(seed);
    for i in 0..subs.max(1) {
        let ang = (i as f32) / (subs.max(1) as f32) * std::f32::consts::TAU;
        let r = if i == 0 { 0.0 } else { 9.0 };
        st.add_sub(SubStructure::new(
            Vec2::new(r * ang.cos(), r * ang.sin()),
            SUB_R,
            Faction::Neutral,
        ));
    }
    Planet::new(st, pos, name)
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
/// (the level's Automaton) is `Faction::Enemy`.
pub fn diamond(
    seed: u64,
    home_subs: usize,
    home_ships: usize,
    centre_subs: usize,
) -> World {
    let mut w = World::new();
    let p = w.add_planet(stocked_planet(seed, Faction::Player, home_subs, home_ships, Vec2::new(0.0, 0.0), "Home (you)"));
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Enemy, home_subs, home_ships, Vec2::new(120.0, 0.0), "Automaton"));
    let fp = w.add_planet(neutral_planet(seed + 11, 1, Vec2::new(30.0, 40.0), "West Reach"));
    let fe = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(90.0, 40.0), "East Reach"));
    let centre = w.add_planet(neutral_planet(seed + 13, centre_subs, Vec2::new(60.0, 0.0), "The Keep"));
    w.add_lane(p, fp, 35.0);
    w.add_lane(e, fe, 35.0);
    w.add_lane(p, centre, 45.0);
    w.add_lane(e, centre, 45.0);
    w.add_lane(fp, centre, 40.0);
    w.add_lane(fe, centre, 40.0);
    w
}

/// The default inter-planet dials for every level (the operating point the `world`/`ai` suites
/// validated). Levels return this alongside their `World` so the host drives both with one
/// consistent parameter set.
pub fn default_world_params() -> WorldParams {
    WorldParams::default()
}
