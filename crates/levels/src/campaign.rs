//! The 7-level campaign: the authored curriculum and its per-level world `build` functions.
//!
//! Each level is a [`crate::Level`] carrying GUI-facing metadata (title / blurb / objective /
//! hints), the chosen enemy [`ai::Roster`], where the camera opens ([`crate::StartView`]),
//! whether basic automation is offered, the match horizon, and a bare
//! `build: fn(seed) -> (World, WorldParams)` world-builder. The player is always
//! [`Faction::Player`]; the enemy seats are `Faction::Ai(i)`, one per `enemies[i]` entry; a level
//! is **won** when [`world::World::outcome`] favours the Player (every rival eliminated).
//!
//! The campaign (see `LEVELS.md` for the full table, the tutorial-arc plan, and the
//! validation). L1 = Passive, L2 = the scripted Cycler, L3-L13 = SimpleColonize (L6 fields
//! two — a free-for-all; L7 adds a Passive third seat). All seven are hand-authored: L1-L6
//! single-struct, L7 the orbiting contested field (multi-struct, camera opens INSIDE). The
//! old L8-L13 placeholders were DELETED (owner, 2026-07-07 — no design intent survived them):
//!
//! | # | Title | View | Enemies |
//! |---|---|---|---|
//! | 1 | First steps | Layer1 | Passive |
//! | 2 | Command and Control | Layer1 | Cycler |
//! | 3 | Fire in the sky | Layer1 | Simple |
//! | 4 | The Sinews of War | Layer1 | Simple |
//! | 5 | Head of the Snake | Layer1 | Simple |
//! | 6 | Deliberation | Layer1 | Simple x2 |
//! | 7 | Far far away | Layer1! | Simple + Passive forts |

use layer1::{Faction, Interior, SubStructure, Vec2};
use world::{Structure, World, WorldParams};

use crate::builders::default_world_params;
use crate::{Level, StartView};
use ai::Roster;

// ======================================================================================
// L1 — "First steps" (movement tutorial). StartView = Layer1.
// ======================================================================================

/// ONE structure, **5 sub-structures in a square with a centre** — a **Layer-1-only** mission (Layer 2
/// is unavailable: with a single struct the game locks to the interior). The **centre** is a
/// **Passive** Enemy fortress (storage 100, production 3, **400 ships**); the four **corners** of the
/// square (storage 60, production 2) are one **Player** home (100 ships) and three **neutral** posts.
/// The square is wide enough that ships moving along its **outer edges** never enter the centre's
/// engagement range — so the player can safely expand corner-to-corner, build up, and only then
/// strike the centre. The reserve / patrol-zone node is added with its capacity raised to 10 000:
/// even with Layer 2 unavailable it is the struct's central staging buffer (over-cap corner
/// production auto-flows into it).
fn build_first_steps(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Interior::new(seed);
    let d = 40.0_f32; // corner offset; edges sit ≥ d from the centre (≫ the 3.5-unit engagement radius)

    // Centre: a passive Enemy garrison — big and deeply stocked.
    let centre = st.add_sub(
        SubStructure::new(Vec2::new(0.0, 0.0), 0.0, Faction::Ai(0))
            .with_storage_capacity(100)
            .with_production(3),
    );
    for _ in 0..400 {
        st.spawn_ship(Faction::Ai(0), centre);
    }

    // Four corners of the square: one Player home (100 ships), three neutral.
    let corners = [
        (Vec2::new(-d, -d), Faction::Player, 100usize),
        (Vec2::new(d, -d), Faction::Neutral, 0),
        (Vec2::new(d, d), Faction::Neutral, 0),
        (Vec2::new(-d, d), Faction::Neutral, 0),
    ];
    for &(pos, owner, ships) in &corners {
        let s = st.add_sub(
            SubStructure::new(pos, 0.0, owner)
                .with_storage_capacity(60)
                .with_production(2),
        );
        for _ in 0..ships {
            st.spawn_ship(owner, s);
        }
    }

    // A reserve / patrol-zone node (struct storage) with a large capacity of 10 000 — even though
    // Layer 2 is unavailable here, it acts as the struct's central staging buffer (the player's
    // over-cap corner production auto-flows into it). Capacity overridden from the default reserve cap.
    let stg = st.add_storage_sub();
    st.subs[stg].storage_capacity = 10_000;
    w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "Proving Ground"));
    (w, default_world_params())
}

// ======================================================================================
// L2 — "Command and Control" (fleet-command tutorial). StartView = Layer1.
// ======================================================================================

/// ONE structure, **Layer-1 only** — the fleet-command mission between movement (L1) and the
/// Simple campaign enemy (L3), against the scripted **Cycler** (owner-designed; see
/// [`ai::cycler`]): a readable, telegraphed drillmaster. Its surplus rotates visibly between
/// its subs (and, in transit, dodges the idle attrition a parked garrison pays — the rotating
/// column outgrows the player's parked caps: the mission's built-in clock); an attacked sub
/// pulls its whole force into a massed defence (so a frontal wave meets everything at once —
/// feint one sub, strike the other); and once its total can overwhelm a target's defenders
/// (`max(3F, F+60)`, `F` = ships present + inbound at the target) it musters everything at one
/// sub — the visible tell — and launches all-in at a pseudo-random target. It is **blind to
/// ships staged in the reserve**: they count neither as defenders nor as threats, so the
/// reserve is both the player's hidden muster and the bait for the ambush (owner: intended).
///
/// Layout (mirrored pairs, a moderate gap between the sides):
/// * **West (player):** two 60-cap / 2-prod subs, **60 ships each**.
/// * **East (enemy):** two 60-cap / 2-prod subs, **50 ships each**.
fn build_command_and_control(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Interior::new(seed);

    // Player pair, west.
    for &pos in &[Vec2::new(-28.0, 14.0), Vec2::new(-28.0, -14.0)] {
        let s = st.add_sub(
            SubStructure::new(pos, 0.0, Faction::Player)
                .with_storage_capacity(60)
                .with_production(2),
        );
        for _ in 0..60 {
            st.spawn_ship(Faction::Player, s);
        }
    }

    // Enemy pair, east — the Cycler's drill ground.
    for &pos in &[Vec2::new(28.0, 14.0), Vec2::new(28.0, -14.0)] {
        let s = st.add_sub(
            SubStructure::new(pos, 0.0, Faction::Ai(0))
                .with_storage_capacity(60)
                .with_production(2),
        );
        for _ in 0..50 {
            st.spawn_ship(Faction::Ai(0), s);
        }
    }

    // Ownerless struct-storage staging node — the hidden muster the Cycler cannot see.
    st.add_storage_sub();
    w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "Command and Control"));
    (w, default_world_params())
}

// ======================================================================================
// L3 — "Fire in the sky" (combat tutorial). StartView = Layer1.
// ======================================================================================

/// ONE structure, **Layer-1 only** (Layer 2 is locked, like Mission 1). **Six** sub-structures: **four
/// production posts in a square in the middle** (neutral, storage 60, production 3) and **two home
/// posts on opposite sides** — a Player home (left) and a **Simple** Enemy home (right), each
/// **60 ships, storage 60, production 1**. Both sides start even and race to seize the
/// high-production middle. (Plus the ownerless struct-storage staging node.)
fn build_fire_in_the_sky(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Interior::new(seed);

    // Four production posts in a square in the middle: neutral, storage 60, production 3.
    // (The old ~11-apart flashpoint died when the engagement radius halved to 3.5; the middle is
    // now spaced to the corrected game scale — L2's combat lesson gets re-authored with the
    // tutorial arc.)
    let m = 11.0_f32;
    for &pos in &[
        Vec2::new(-m, -m),
        Vec2::new(m, -m),
        Vec2::new(m, m),
        Vec2::new(-m, m),
    ] {
        st.add_sub(
            SubStructure::new(pos, 0.0, Faction::Neutral)
                .with_storage_capacity(60)
                .with_production(3),
        );
    }

    // Two home posts on opposite sides: Player (left) and Enemy (right). 60 ships, storage 60,
    // production 1. The homes (±48) are well clear of the middle, so the fight is decided in the centre.
    for &(pos, owner) in &[
        (Vec2::new(-48.0, 0.0), Faction::Player),
        (Vec2::new(48.0, 0.0), Faction::Ai(0)),
    ] {
        let s = st.add_sub(
            SubStructure::new(pos, 0.0, owner)
                .with_storage_capacity(60)
                .with_production(1),
        );
        for _ in 0..60 {
            st.spawn_ship(owner, s);
        }
    }

    // Ownerless struct-storage staging node (over-cap production auto-flows here).
    st.add_storage_sub();
    w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "The Crucible"));
    (w, default_world_params())
}

// ======================================================================================
// L4 — "The Sinews of War" (logistics + the fortress wall). StartView = Layer1.
// ======================================================================================

/// ONE struct, **Layer-1 only**. The economy/fortress mission (owner-designed layout):
///
/// * **Left (player):** a player-owned **shipyard** in the back (starts with **1 ship**; its
///   output pools at the yard up to the invisible virtual cap — the player's army lives there)
///   and two neutral **200-cap / 1-prod warehouses** (default resistance — a deliberate
///   midgame investment, not an opening move): forward storage, the "sinews".
/// * **Middle:** a vertical wall of **three fortresses covering one another** (20 apart, reach
///   ~21.7 at `FORTRESS_RANGE` 18). Only the **middle** starts enemy-owned — at full
///   **capacity (90)**, the wall's solid heart (Simple's fort doctrine — floor = capacity,
///   never evacuates — keeps it there); top and bottom are **neutral and empty** — harmless
///   until claimed, at which point their zones reach into the middle fort's own ground (the
///   wall can be turned). Above/below, inside the outer forts' dormant zones, two neutral
///   **60-cap / 2-prod** flank posts form the side corridors.
/// * **Right (enemy):** five 60-cap / 2-prod heartland subs placed asymmetrically — **one
///   enemy-owned with 60 ships** (it secures its side from there), four neutral — plus two
///   **enemy fortresses in the back thinly manned (10 each)**, out of overwatch range of the
///   heartland: they gate the eastern approach corridor toward the reserve ring, where enemy
///   remnants will stage — finishing the mission means paying their toll.
fn build_sinews_of_war(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Interior::new(seed);

    // Player shipyard (active — authored owned, default production 8), 1 starting ship.
    let yard = st.add_sub(SubStructure::shipyard(Vec2::new(-70.0, 0.0), Faction::Player));
    st.spawn_ship(Faction::Player, yard);

    // Two neutral warehouses: fat storage, token production, default (heavy) resistance.
    for &pos in &[Vec2::new(-45.0, 16.0), Vec2::new(-45.0, -16.0)] {
        st.add_sub(
            SubStructure::new(pos, 0.0, Faction::Neutral)
                .with_storage_capacity(200)
                .with_production(1),
        );
    }

    // The wall: three mutually covering fortresses. Only the MIDDLE starts enemy — at full
    // capacity (90): the wall's heart is solid from tick 0 (Simple's fort doctrine — floor =
    // capacity, no evacuation — keeps it that way). Top/bottom are neutral and EMPTY.
    st.add_sub(SubStructure::fortress(Vec2::new(0.0, 20.0), Faction::Neutral));
    let mid_fort = st.add_sub(SubStructure::fortress(Vec2::new(0.0, 0.0), Faction::Ai(0)));
    for _ in 0..90 {
        st.spawn_ship(Faction::Ai(0), mid_fort);
    }
    st.add_sub(SubStructure::fortress(Vec2::new(0.0, -20.0), Faction::Neutral));
    // Flank posts inside the outer forts' zones (the priced side corridors).
    for &pos in &[Vec2::new(0.0, 38.0), Vec2::new(0.0, -38.0)] {
        st.add_sub(
            SubStructure::new(pos, 0.0, Faction::Neutral)
                .with_storage_capacity(60)
                .with_production(2),
        );
    }

    // Enemy heartland: five asymmetric 60/2 subs — ONE starts owned, fully stocked (60 ships);
    // Simple expands across the rest from there.
    for &(pos, owned) in &[
        (Vec2::new(30.0, 8.0), true),
        (Vec2::new(42.0, -6.0), false),
        (Vec2::new(52.0, 20.0), false),
        (Vec2::new(62.0, -14.0), false),
        (Vec2::new(44.0, -32.0), false),
    ] {
        let owner = if owned { Faction::Ai(0) } else { Faction::Neutral };
        let s = st.add_sub(
            SubStructure::new(pos, 0.0, owner)
                .with_storage_capacity(60)
                .with_production(2),
        );
        if owned {
            for _ in 0..60 {
                st.spawn_ship(Faction::Ai(0), s);
            }
        }
    }

    // Back forts: the enemy's last line, gating the eastern approach (their zones stay clear
    // of the heartland subs at the widened reach, so the mid-game fight never triggers them).
    // Thinly manned (10 each) at start — Simple's manning tops them toward capacity over time.
    for &pos in &[Vec2::new(90.0, 26.0), Vec2::new(90.0, -26.0)] {
        let f = st.add_sub(SubStructure::fortress(pos, Faction::Ai(0)));
        for _ in 0..10 {
            st.spawn_ship(Faction::Ai(0), f);
        }
    }

    // A tighter reserve ring than the default game scale (0.6× — the level dial): this board
    // is one dense battlefield; the staging orbit should feel adjacent, not interplanetary.
    st.add_storage_sub_scaled(layer1::sim::STORAGE_RADIUS_SCALE * 0.6);
    w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "The Sinews of War"));
    (w, default_world_params())
}

// ======================================================================================
// L5 — "Head of the Snake" (teleporter + shipyard decapitation). StartView = Layer1.
// ======================================================================================

/// ONE struct, **Layer-1 only**. The mobility mission (the owner's Arc-1 plan): an
/// **impregnable enemy fortress line** — four mutually covering fortresses, all manned from
/// tick 0, zones overlapping with no seam, spanning every approach in the sub graph —
/// frontally hopeless and geometrically unflankable. The counter is not force but **mobility**:
/// a **neutral teleporter gate** stands on the player's side of the line; an owned gate's
/// departures arrive at their destination at undock-end, **no transit** — the crossing the wall
/// was built to price simply never happens. Behind the line, the enemy's **active shipyard** —
/// the *head of the snake* — feeds the wall; the deep strike through the gate decapitates the
/// economy and the line starves.
///
/// * **West (player):** home (60-cap / 2-prod, 60 ships) + two neutral 60-cap / 2-prod posts.
/// * **South-west:** the neutral **gate** (default 60-cap resistance — a deliberate midgame
///   investment, like L4's warehouses).
/// * **Middle:** the wall — four enemy fortresses 20 apart (reach ~21.7 at `FORTRESS_RANGE`
///   18), each manned with **60** (Simple's fort doctrine tops them toward capacity 90 — the
///   wall visibly thickens until the economy behind it dies).
/// * **East (enemy):** the **active shipyard** (40 ships pooled at the yard) + one owned 60/2
///   heartland sub (40 ships) + two neutral 60/2 subs for Simple to expand into. Once the
///   yard flips (an active yard keeps only a token resistance bar), its 8-prod output pools
///   *for the player, inside enemy ground* — the deep strike becomes a forward base.
fn build_head_of_the_snake(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Interior::new(seed);

    // Player home + two neutral posts, west of the wall.
    let home = st.add_sub(
        SubStructure::new(Vec2::new(-75.0, 0.0), 0.0, Faction::Player)
            .with_storage_capacity(60)
            .with_production(2),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Player, home);
    }
    for &pos in &[Vec2::new(-52.0, 22.0), Vec2::new(-52.0, -22.0)] {
        st.add_sub(
            SubStructure::new(pos, 0.0, Faction::Neutral)
                .with_storage_capacity(60)
                .with_production(2),
        );
    }

    // The gate: a neutral teleporter in the player's south-west corner — reachable without
    // crossing any fortress zone, priced at the default 60-cap grind.
    st.add_sub(SubStructure::teleporter(Vec2::new(-60.0, -42.0), Faction::Neutral));

    // The wall: four mutually covering enemy fortresses, manned 60 each (Simple tops them
    // toward capacity). Zones overlap (spacing 20 < reach ~21.7) and span every westward
    // approach between the authored subs — no seam, no flank.
    for &y in &[-30.0_f32, -10.0, 10.0, 30.0] {
        let f = st.add_sub(SubStructure::fortress(Vec2::new(-5.0, y), Faction::Ai(0)));
        for _ in 0..60 {
            st.spawn_ship(Faction::Ai(0), f);
        }
    }

    // The head of the snake: the enemy's ACTIVE shipyard (authored owned ⇒ active; output
    // pools at the yard, overflow feeds the reserve), plus its heartland.
    let yard = st.add_sub(SubStructure::shipyard(Vec2::new(45.0, 0.0), Faction::Ai(0)));
    for _ in 0..40 {
        st.spawn_ship(Faction::Ai(0), yard);
    }
    let heart = st.add_sub(
        SubStructure::new(Vec2::new(30.0, 22.0), 0.0, Faction::Ai(0))
            .with_storage_capacity(60)
            .with_production(2),
    );
    for _ in 0..40 {
        st.spawn_ship(Faction::Ai(0), heart);
    }
    for &pos in &[Vec2::new(30.0, -22.0), Vec2::new(60.0, -18.0)] {
        st.add_sub(
            SubStructure::new(pos, 0.0, Faction::Neutral)
                .with_storage_capacity(60)
                .with_production(2),
        );
    }

    // A tighter reserve ring (0.6× dial, as L4): the board is one dense battlefield.
    st.add_storage_sub_scaled(layer1::sim::STORAGE_RADIUS_SCALE * 0.6);
    w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "Head of the Snake"));
    (w, default_world_params())
}

// ======================================================================================
// L6 — "Deliberation" (two AI opponents — free-for-all). Layer-1 only (Layer 2 locked).
// ======================================================================================

/// ONE structure, **Layer-1 only** (Layer 2 locked, like Missions 1-2), with **two Simple AI
/// opponents** — a three-way free-for-all (every real seat fights the others). Layout:
///
/// ```text
///                           B
///                       1  -  1
/// A  -  0  -  0  -  0  -  0  -  0  -  0
///                       0  -  0
///                            C
/// ```
///
/// * **A** — Player start (60 ships, storage 60, prod 2), at the left of a horizontal chain.
/// * The chain (now running on past the cluster) + lower branch are neutral **`0`** posts (storage 30, prod 1).
/// * The upper branch is two neutral **`1`** posts (storage 60, prod 2) — the richer road.
/// * **B** — the first Simple's home (60 ships, storage 120, prod 4), atop the rich upper branch.
/// * **C** — the second Simple's home (60 ships, storage 90, prod 3), below the lean lower branch.
///
/// The two AIs sit on opposite branches of the right cluster; the Player must cross the chain to
/// contest it, while the two Simples grind each other as much as the Player.
fn build_deliberation(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Interior::new(seed);

    // A — the Player's starter at the left of the chain (full garrison).
    let a = st.add_sub(
        SubStructure::new(Vec2::new(-60.0, 0.0), 0.0, Faction::Player)
            .with_storage_capacity(60)
            .with_production(2),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Player, a);
    }
    // The neutral chain of 30-cap / 1-prod posts running right from A and on past the right cluster.
    for x in [-36.0_f32, -12.0, 12.0, 36.0, 60.0, 84.0] {
        st.add_sub(
            SubStructure::new(Vec2::new(x, 0.0), 0.0, Faction::Neutral)
                .with_storage_capacity(30)
                .with_production(1),
        );
    }
    // Upper branch: two richer 60-cap / 2-prod neutral posts, leading up to B (first Simple).
    for p in [Vec2::new(36.0, 26.0), Vec2::new(60.0, 26.0)] {
        st.add_sub(
            SubStructure::new(p, 0.0, Faction::Neutral)
                .with_storage_capacity(60)
                .with_production(2),
        );
    }
    let b = st.add_sub(
        SubStructure::new(Vec2::new(48.0, 52.0), 0.0, Faction::Ai(0))
            .with_storage_capacity(120)
            .with_production(4),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Ai(0), b);
    }
    // Lower branch: two lean 30-cap / 1-prod neutral posts, leading down to C (second Simple).
    for p in [Vec2::new(36.0, -26.0), Vec2::new(60.0, -26.0)] {
        st.add_sub(
            SubStructure::new(p, 0.0, Faction::Neutral)
                .with_storage_capacity(30)
                .with_production(1),
        );
    }
    let c = st.add_sub(
        SubStructure::new(Vec2::new(48.0, -52.0), 0.0, Faction::Ai(1))
            .with_storage_capacity(90)
            .with_production(3),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Ai(1), c);
    }

    st.add_storage_sub();
    w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), "Deliberation"));
    (w, default_world_params())
}

// ======================================================================================
// L7 — "Far far away" (orbiting contested field + the unseen source). StartView = Layer1!
// ======================================================================================

/// TWO **unnamed** structs, one lane — but the camera opens **inside** the contested struct
/// (`StartView::Layer1` on a multi-struct map — the owner's rework, 2026-07-07): the enemy's
/// reinforcements pour in from *somewhere out there*, the off-screen arrows pile up at the
/// frame edge, and wheeling out past the reserve IS the discovery. No struct names anywhere —
/// nothing labels the world as bigger than the box.
///
/// **The contested struct** (the whole board moves — the owner's orbital mechanic):
/// * **Six 90-cap / 3-prod** subs spaced 60° apart (R = 75.6), all **orbiting the centre
///   clockwise** (τ/1500 per reference tick): **west = Player** (90 ships), **east = the
///   Simple enemy** (90 ships), the other four neutral.
/// * A **neutral shipyard** dead centre at the default **token 1.0 resistance** (reaching it
///   is the whole cost; the forts, not a grind, are its defence).
/// * Three **fortresses** on an inner ring (R = 14, 120° apart), orbiting
///   **counter-clockwise, slower** (τ/3000) — owned by a third, **Passive** seat hostile to
///   both players, manned **60 each**: their zones (reach ≈ 21.7) sweep over the yard at all
///   times. The watchers must fall before the prize can be worked.
/// * Ships ordered to a moving sub **lead the target** (the dispatch intercept solves
///   time-to-arrival against the orbit — no chasing).
/// * Victory requires eliminating BOTH rival seats; the fort seat produces nothing, so
///   killing its garrisons suffices (production-0 subs keep no shipless seat alive).
///
/// **The enemy struct** (far away down the lane): a single **active shipyard** — the source.
/// Simple funnels its output to the front; the stream only stops when the yard falls.
fn build_far_far_away(seed: u64) -> (World, WorldParams) {
    use std::f32::consts::TAU;
    let mut w = World::new();

    // --- The contested struct (unnamed). -----------------------------------------------
    let mut st = Interior::new(seed);
    let centre = Vec2::new(0.0, 0.0);
    let r_subs = 75.6_f32; // 42 × 1.8 (owner: distance the ring out)
    // Six subs, 60° apart: the enemy east (k = 0), the player west (k = 3), four neutral.
    for k in 0..6u32 {
        let a = k as f32 * TAU / 6.0;
        let (owner, ships) = match k {
            0 => (Faction::Ai(0), 90usize), // east — the enemy beachhead
            3 => (Faction::Player, 90),     // west — the player
            _ => (Faction::Neutral, 0),
        };
        let pos = Vec2::new(centre.x + r_subs * a.cos(), centre.y + r_subs * a.sin());
        let s = st.add_sub(
            SubStructure::new(pos, 0.0, owner)
                .with_storage_capacity(90)
                .with_production(3)
                .orbiting(centre, -TAU / 1500.0), // clockwise on screen
        );
        for _ in 0..ships {
            st.spawn_ship(owner, s);
        }
    }
    // The prize: a neutral shipyard dead centre — static; the field turns around it. The
    // default token bar (owner rule: zero capacity ⇒ no resistance): whoever reaches it under
    // the watchers' guns takes it on contact.
    st.add_sub(SubStructure::shipyard(centre, Faction::Neutral));
    // The watchers: three Passive-seat fortresses on the slow counter-rotating inner ring,
    // each manned 60 — their zones cover the yard from every bearing.
    let r_forts = 14.0_f32;
    for k in 0..3u32 {
        let a = TAU * 0.25 + k as f32 * TAU / 3.0; // one starts at the top
        let pos = Vec2::new(centre.x + r_forts * a.cos(), centre.y + r_forts * a.sin());
        let f = st.add_sub(
            SubStructure::fortress(pos, Faction::Ai(1)).orbiting(centre, TAU / 3000.0),
        );
        for _ in 0..60 {
            st.spawn_ship(Faction::Ai(1), f);
        }
    }
    st.add_storage_sub();
    let front = w.add_struct(Structure::new(st, Vec2::new(0.0, 0.0), ""));

    // --- The enemy struct (unnamed, far away): a single active shipyard — the source. ---
    let mut rear = Interior::new(seed + 1);
    let yard = rear.add_sub(SubStructure::shipyard(Vec2::new(0.0, 0.0), Faction::Ai(0)));
    for _ in 0..40 {
        rear.spawn_ship(Faction::Ai(0), yard);
    }
    rear.add_storage_sub();
    let source = w.add_struct(Structure::new(rear, Vec2::new(170.0, 0.0), ""));

    w.add_lane(front, source, 170.0);
    (w, default_world_params())
}

// --------------------------------------------------------------------------------------
// Local multi-sub struct helpers for L3 (explicit owned-centre + neutral ring layouts).
// --------------------------------------------------------------------------------------

// ======================================================================================
// The campaign.
// ======================================================================================

/// The campaign levels, in play order. This is the single list the GUI consumes (it reads each
/// [`Level`]'s metadata to drive the UI, and calls `build(seed)` to instantiate the world).
///
/// **DEPRECATED — legacy placeholder content.** Every level and mission briefing below
/// (titles / blurbs / objectives / hints / layouts) is the *old* teaching campaign and is slated
/// for replacement by the authored **narrative** campaign, redone **one level at a time** on the
/// designer's instruction (awakening machine-strategist arc). It is kept functional only so the
/// game still builds and runs while the new campaign is written; do not treat any of this copy or
/// layout as final. Replace entries here as each new mission is specified.
pub fn campaign() -> Vec<Level> {
    vec![
        Level {
            id: 1,
            title: "First steps".into(),
            blurb: "A quiet corner of the cluster. One structure, three sites, and an enemy too \
                    dormant to fight back. Learn to move."
                .into(),
            objective: "Capture every site: take the neutral apex, then the dormant enemy.".into(),
            hints: vec![
                "Click your home sub-structure to select it.".into(),
                "Use 25 / 50 / 75 / 100% to choose how many idle ships to send.".into(),
                "Click a destination to send that fraction; ships capture an unguarded site on arrival."
                    .into(),
                "Send a wave to the neutral apex, then to the dormant enemy site.".into(),
            ],
            enemies: vec![Roster::Passive],
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 1200,
            zoom_min: None,
            build: build_first_steps,
        },
        Level {
            id: 2,
            title: "Command and Control".into(),
            // PLACEHOLDER copy — the owner writes the final blurb/objective/hints.
            blurb: "The opposing commander drills its fleet in endless rotation, and it will \
                    not fight fair: attack one post and everything answers at once. Watch the \
                    pattern. Then break it."
                .into(),
            objective: "Eliminate the enemy: read the rotation, split its attention, and never \
                        meet its massed strength head-on."
                .into(),
            hints: vec![
                "Ctrl+click adds a sub-structure to your selection — command several at once.".into(),
                "Threaten one of its posts and its whole force converges there — the other post \
                 stands alone."
                    .into(),
                "Ships in the patrol ring around the structure are beneath its notice. Muster \
                 there unseen."
                    .into(),
                "Ships rotating between its posts never wear out; parked garrisons do. Waiting \
                 is not free — its column grows."
                    .into(),
                "When it gathers everything at one post, the all-in strike is coming.".into(),
            ],
            enemies: vec![Roster::Cycler],
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 2400,
            zoom_min: None,
            build: build_command_and_control,
        },
        Level {
            id: 3,
            title: "Fire in the sky".into(),
            blurb: "Two homes face off across four neutral production posts. Whoever seizes the \
                    middle out-builds the other — and this enemy is awake."
                .into(),
            objective: "Out-produce and break the enemy: seize the central posts, then take the enemy home."
                .into(),
            hints: vec![
                "Both sides start even — 60 units each. The four middle posts produce 3× your home; \
                 grab them first."
                    .into(),
                "Adjacent middle posts trade fire across the gap — hold a cluster to fight from \
                 strength."
                    .into(),
                "Send in waves; a trickle loses the brawl.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 1500,
            zoom_min: None,
            build: build_fire_in_the_sky,
        },
        Level {
            id: 4,
            title: "The Sinews of War".into(),
            // PLACEHOLDER copy — the owner writes the final blurb/objective/hints.
            blurb: "An armoured wall splits the field, and behind it an economy that will not \
                    wait for you. Build from nothing; make every hull count."
                .into(),
            objective: "Break the wall and eliminate the enemy: build up from the yard, stock \
                        the depots, and take the fortress line."
                .into(),
            hints: vec![
                "Your shipyard hoards its own output — your army musters at the yard itself.".into(),
                "The big depots store hundreds close to the front. Production wins wars; storage \
                 positions them."
                    .into(),
                "A fortress's garrison fires far beyond its ground. Crossing a manned zone has a \
                 price — count it before you pay it."
                    .into(),
                "The wall's outer forts are unclaimed. A fortress you man is a fortress they \
                 must answer."
                    .into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 3600,
            zoom_min: None,
            build: build_sinews_of_war,
        },
        Level {
            id: 5,
            title: "Head of the Snake".into(),
            // PLACEHOLDER copy — the owner writes the final blurb/objective/hints.
            blurb: "A fortress line you cannot break and cannot go around, fed by a shipyard \
                    deep in its shadow. Walls do not starve. Economies do."
                .into(),
            objective: "Decapitate: seize the old gate, strike through it, take their shipyard \
                        — then dismantle the starving wall."
                .into(),
            hints: vec![
                "The wall's zones overlap; there is no seam and no flank. Do not pay what it asks."
                    .into(),
                "The old gate stands unclaimed on your side of the line. Fleets leaving a gate \
                 you own arrive instantly — the crossing never happens."
                    .into(),
                "Their shipyard feeds the wall. An active yard barely resists once its defenders \
                 fall — take it, and its output is yours, behind their line."
                    .into(),
                "A starving wall is still a wall: it must fall, but it can no longer answer.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 4800,
            zoom_min: None,
            build: build_head_of_the_snake,
        },
        Level {
            id: 6,
            title: "Deliberation".into(),
            blurb: "Two minds, not one. A pair of rivals hold the far cluster — and they are as \
                    much each other's problem as yours. Cross the chain and let them deliberate."
                .into(),
            objective: "Outlast both rivals: take the chain, then break B and C while they grind each other."
                .into(),
            hints: vec![
                "Two enemies this time (the yellows). They fight you AND each other — a free-for-all."
                    .into(),
                "The upper road (the bigger posts) is richer but leads to the stronger rival, B."
                    .into(),
                "Let them trade blows over the right cluster, then arrive in force to clean up.".into(),
            ],
            enemies: vec![Roster::SimpleColonize, Roster::SimpleColonize],
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 1800,
            zoom_min: None,
            build: build_deliberation,
        },
        Level {
            id: 7,
            title: "Far far away".into(),
            // PLACEHOLDER copy — the owner writes the final blurb/objective/hints. Deliberately
            // DIEGETIC: no mention of layers, lenses, or other structs — the discovery that the
            // sandbox is bigger than the box is the mission (zoom out, follow the arrows).
            blurb: "A turning field: every post rides its orbit, a dead foundry waits at the \
                    hub, and the watchers on the inner ring answer to no one. The enemy never \
                    seems to run out of ships. Where do they keep coming from?"
                .into(),
            objective: "Eliminate all hostiles — take the turning field, silence the watchers, \
                        and cut the enemy off at the source."
                .into(),
            hints: vec![
                "The ground itself moves. Your ships lead a moving target — launch and trust \
                 the intercept."
                    .into(),
                "The watchers fire on everyone in reach, and the foundry never leaves their \
                 kill zone. They must fall before it can be worked."
                    .into(),
                "Count their ships. Count yours. The arithmetic doesn't close — their \
                 reinforcements come from somewhere beyond the field."
                    .into(),
                "Follow the arrows at the edge of the screen. Zoom out. Farther.".into(),
            ],
            enemies: vec![Roster::SimpleColonize, Roster::Passive],
            start_view: StartView::Layer1(0),
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            horizon: 4800,
            // Tighter out-zoom floor (owner-tuned): the interior frames the
            // turning field; the reveal beyond it belongs to the lens.
            zoom_min: Some(0.8),
            build: build_far_far_away,
        },
    ]
}
