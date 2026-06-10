//! The 10-level campaign: the authored curriculum and its per-level world `build` functions.
//!
//! Each level is a [`crate::Level`] carrying GUI-facing metadata (title / blurb / objective /
//! hints), the chosen enemy [`ai::Roster`], where the camera opens ([`crate::StartView`]),
//! whether basic automation is offered, the match horizon, and a bare
//! `build: fn(seed) -> (World, WorldParams)` world-builder. The player is always
//! [`Faction::Player`]; the enemy seat is [`Faction::Ai(0)`]; a level is **won** when
//! [`world::World::outcome`] favours the Player.
//!
//! The curriculum (see `LEVELS.md` for the full table and the measured validation):
//!
//! | # | Title | View | Enemy | Teaches |
//! |---|---|---|---|---|
//! | 1 | First Moves | Layer1 | Passive | select a sub, send a fraction, capture |
//! | 2 | Contact | Layer1 | GreedyLocal | concentration of force; layout decides who fights |
//! | 3 | Two Worlds | Layer2 | GreedyLocal | inter-planet fleets, zoom-to-micro, automation |
//! | 4 | Hold the Line | Layer2 | GreedyLocal | reinforce via automation across two bigger worlds |
//! | 5 | Three Fronts | Layer2 | GreedyLocal | multi-front concentration on a triangle |
//! | 6 | The Prize | Layer2 | GreedyLocal | expansion-vs-defense timing around a juicy neutral |
//! | 7 | The Seam | Layer2 | GreedyLocal | exploit greedy's undefended rear (a flank) |
//! | 8 | Overreach | Layer2 | Colonize | strike undefended production (attack > colonize) |
//! | 9 | The Turtle | Layer2 | Defend | out-expand a turtle (colonize > defend) |
//! | 10 | The Hammer | Layer2 | Attack | punish the over-committed stack (defend > attack) |

use layer1::{Faction, Structure, SubStructure, Vec2};
use world::{Planet, World, WorldParams};

use crate::builders::{
    default_world_params, diamond, neutral_planet, neutral_planet_res, stocked_planet,
};
use crate::{Level, StartView};
use ai::Roster;

// ======================================================================================
// L1 — "First Moves" (movement tutorial). StartView = Layer1.
// ======================================================================================

/// ONE planet, **5 sub-structures in a square with a centre** — a **Layer-1-only** mission (Layer 2
/// is unavailable: with a single planet the game locks to the interior). The **centre** is a
/// **Passive** Enemy fortress (storage 100, production 3, **400 ships**); the four **corners** of the
/// square (storage 60, production 2) are one **Player** home (100 ships) and three **neutral** posts.
/// The square is wide enough that ships moving along its **outer edges** never enter the centre's
/// engagement range — so the player can safely expand corner-to-corner, build up, and only then
/// strike the centre. No reserve / patrol-zone node (it is a Layer-2 concept, irrelevant here).
fn build_l1(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Structure::new(seed);
    let d = 20.0_f32; // corner offset; edges sit ≥ d from the centre (≫ the 7-unit engagement radius)

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
    // Layer 2 is unavailable here, it acts as the planet's central staging buffer (the player's
    // over-cap corner production auto-flows into it). Capacity overridden from the default reserve cap.
    let stg = st.add_storage_sub();
    st.subs[stg].storage_capacity = 10_000;
    w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "Proving Ground"));
    (w, default_world_params())
}

// ======================================================================================
// L2 — "Contact" (combat tutorial). StartView = Layer1.
// ======================================================================================

/// ONE planet, **Layer-1 only** (Layer 2 is locked, like Mission 1). **Six** sub-structures: **four
/// production posts in a square in the middle** (neutral, storage 60, production 3) and **two home
/// posts on opposite sides** — a Player home (left) and a **Simple** Enemy home (right), each
/// **60 ships, storage 60, production 1**. Both sides start even and race to seize the
/// high-production middle. (Plus the ownerless struct-storage staging node.)
fn build_l2(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Structure::new(seed);

    // Four production posts in a square in the middle: neutral, storage 60, production 3. Adjacent
    // corners sit close enough (~11 apart, < the ~13 engagement reach) to trade fire across the gap
    // once opposing sides garrison them.
    let m = 5.5_f32;
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
    // production 1. The homes (±24) are well clear of the middle, so the fight is decided in the centre.
    for &(pos, owner) in &[
        (Vec2::new(-24.0, 0.0), Faction::Player),
        (Vec2::new(24.0, 0.0), Faction::Ai(0)),
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
    w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "The Crucible"));
    (w, default_world_params())
}

// ======================================================================================
// L3 — "Deliberation" (two AI opponents — free-for-all). Layer-1 only (Layer 2 locked).
// ======================================================================================

/// ONE planet, **Layer-1 only** (Layer 2 locked, like Missions 1-2), with **two Simple AI
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
fn build_l3(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let mut st = Structure::new(seed);

    // A — the Player's starter at the left of the chain (full garrison).
    let a = st.add_sub(
        SubStructure::new(Vec2::new(-30.0, 0.0), 0.0, Faction::Player)
            .with_storage_capacity(60)
            .with_production(2),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Player, a);
    }
    // The neutral chain of 30-cap / 1-prod posts running right from A and on past the right cluster.
    for x in [-18.0_f32, -6.0, 6.0, 18.0, 30.0, 42.0] {
        st.add_sub(
            SubStructure::new(Vec2::new(x, 0.0), 0.0, Faction::Neutral)
                .with_storage_capacity(30)
                .with_production(1),
        );
    }
    // Upper branch: two richer 60-cap / 2-prod neutral posts, leading up to B (first Simple).
    for p in [Vec2::new(18.0, 13.0), Vec2::new(30.0, 13.0)] {
        st.add_sub(
            SubStructure::new(p, 0.0, Faction::Neutral)
                .with_storage_capacity(60)
                .with_production(2),
        );
    }
    let b = st.add_sub(
        SubStructure::new(Vec2::new(24.0, 26.0), 0.0, Faction::Ai(0))
            .with_storage_capacity(120)
            .with_production(4),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Ai(0), b);
    }
    // Lower branch: two lean 30-cap / 1-prod neutral posts, leading down to C (second Simple).
    for p in [Vec2::new(18.0, -13.0), Vec2::new(30.0, -13.0)] {
        st.add_sub(
            SubStructure::new(p, 0.0, Faction::Neutral)
                .with_storage_capacity(30)
                .with_production(1),
        );
    }
    let c = st.add_sub(
        SubStructure::new(Vec2::new(24.0, -26.0), 0.0, Faction::Ai(1))
            .with_storage_capacity(90)
            .with_production(3),
    );
    for _ in 0..60 {
        st.spawn_ship(Faction::Ai(1), c);
    }

    st.add_storage_sub();
    w.add_planet(Planet::new(st, Vec2::new(0.0, 0.0), "Deliberation"));
    (w, default_world_params())
}

// ======================================================================================
// L4 — "Hold the Line" (reinforce L3; lean on automation). StartView = Layer2.
// ======================================================================================

/// TWO **bigger** planets joined by one lane: a Player home and an Enemy home, each a fat
/// multi-sub base, with a longer lane between them. Reinforces L3 — the player leans on
/// automation to run both the home defence and the internal expansion while shipping the
/// decisive fleet across — but now the Enemy ([`Roster::GreedyLocal`]) actually pushes back, so
/// timing the cross-lane commitment matters. The Player home is given an edge so a competent
/// player wins comfortably.
fn build_l4(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    // Two 4-sub homes. Player starts slightly stronger (12/sub vs 9/sub) — this is still an
    // early teaching level, so the player should win clearly once they commit a fleet.
    let p = w.add_planet(stocked_planet(seed, Faction::Player, 4, 12, Vec2::new(0.0, 0.0), "Bastion"));
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Ai(0), 4, 9, Vec2::new(95.0, 0.0), "Foundry"));
    w.add_lane(p, e, 95.0);
    (w, default_world_params())
}

// ======================================================================================
// L5 — "Three Fronts" (triangle; multi-front + concentration). StartView = Layer2.
// ======================================================================================

/// THREE planets in a **triangle**: a Player home, an Enemy home, and a shared **neutral**
/// third planet that both can reach, with lanes forming the triangle. The lesson is *multi-front
/// concentration* — the player cannot be strong everywhere, so they must pick where to commit
/// (typically grab the neutral third to gain a 2-vs-1 production edge, then concentrate on the
/// enemy). Enemy is [`Roster::GreedyLocal`].
fn build_l5(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    let p = w.add_planet(stocked_planet(seed, Faction::Player, 3, 11, Vec2::new(0.0, 0.0), "Anvil"));
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Ai(0), 3, 9, Vec2::new(100.0, 0.0), "Spire"));
    let n = w.add_planet(neutral_planet(seed + 11, 2, Vec2::new(50.0, 70.0), "Crossroads"));
    // Triangle: both homes reach the neutral, and there is a long direct home-to-home lane.
    w.add_lane(p, n, 55.0);
    w.add_lane(e, n, 55.0);
    w.add_lane(p, e, 100.0);
    (w, default_world_params())
}

// ======================================================================================
// L6 — "The Prize" (a juicy neutral worth contesting). StartView = Layer2.
// ======================================================================================

/// FOUR planets: a Player home and an Enemy home, a small **forward neutral** near each side,
/// **plus one fat, central, high-production NEUTRAL planet** (the "prize") roughly equidistant
/// and reachable by both. The lesson is *expansion-vs-defense timing*: the prize is worth
/// rushing for its production, but over-committing to it leaves the home thin. Enemy is
/// [`Roster::GreedyLocal`]. Topology (a diamond with a fattened centre plus two short forward
/// spurs):
///
/// ```text
///   P-home --- prize(fat) --- E-home
///       \                    /
///        nP (fwd)     nE (fwd)
/// ```
fn build_l6(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    // Player starts with a clearer garrison edge (14/sub vs 9/sub). Under the resistance grind the
    // fat 3-sub prize is a long slog to take, so a competent player needs enough mass to both hold
    // home and out-grind the enemy for the centre — recalibrated up from 11/sub for winnability.
    let p = w.add_planet(stocked_planet(seed, Faction::Player, 3, 14, Vec2::new(0.0, 0.0), "Redoubt"));
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Ai(0), 3, 9, Vec2::new(120.0, 0.0), "Citadel"));
    // The prize: a fat 3-sub neutral in the centre, worth contesting for its production. Its subs
    // carry a REDUCED capture resistance (600 vs the default 1800) so the contest actually resolves
    // within the level horizon under the grind — a "rich but not impregnable" mine the Player's
    // garrison edge can convert. (Per-level `with_max_resistance`, the sanctioned pace dial.)
    let prize =
        w.add_planet(neutral_planet_res(seed + 11, 3, Vec2::new(60.0, 0.0), "Greatmine (prize)", Some(600.0)));
    // A small forward neutral spur off each home (cheap early expansion / a defensive buffer).
    let np = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(30.0, 45.0), "North Spur"));
    let ne = w.add_planet(neutral_planet(seed + 13, 1, Vec2::new(90.0, 45.0), "South Spur"));
    // Asymmetric approach: the Player sits a SHORTER hop from the prize (and its spur feeds the
    // prize faster) than the Enemy, so a competent Player out-tempos the greedy foe to the contested
    // mine and wins the freeze there. Under the grind a symmetric prize contest froze into a
    // coin-flip the Player lost on seed luck; the tempo edge converts its production lead instead.
    w.add_lane(p, prize, 35.0);
    w.add_lane(e, prize, 50.0);
    w.add_lane(p, np, 35.0);
    w.add_lane(e, ne, 35.0);
    w.add_lane(np, prize, 30.0);
    w.add_lane(ne, prize, 45.0);
    (w, default_world_params())
}

// ======================================================================================
// L7 — "The Seam" (exploit greedy's thin rear). StartView = Layer2.
// ======================================================================================

/// FOUR planets shaped so the win is to **exploit the greedy Automaton's documented thin-rear
/// seam**. The Enemy ([`Roster::GreedyLocal`]) holds a **single-sub rear** one short lane from
/// the Player home, with a **neutral bait corridor** dangling off the rear. Greedy always ships
/// its surplus toward the nearest uncontested grab and never posts a reserve, so it streams its
/// garrison down the bait corridor and leaves the rear defended only by the flat floor — then a
/// concentrated Player strike across the short lane overruns it and the captured rear snowballs.
///
/// ```text
///   P-home === E-rear --- bait1 --- bait2     (=== the short strike lane)
/// ```
///
/// This is the level-scale version of the seam `AI.md` validated; the campaign validation
/// re-confirms a rear-flanking proxy beats greedy here.
fn build_l7(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    // Player home: a strong 3-sub base to build the strike stack from.
    let p = w.add_planet(stocked_planet(seed, Faction::Player, 3, 14, Vec2::new(0.0, 0.0), "Forward Base"));
    // Enemy rear: a SINGLE sub (low production, low defender mass) — the thin rear that, once
    // greedy bleeds it toward the floor, a concentrated strike captures.
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Ai(0), 1, 10, Vec2::new(28.0, 0.0), "Enemy Rear"));
    // The bait corridor greedy ships its surplus down (it never keeps a reserve at the rear).
    let b1 = w.add_planet(neutral_planet(seed + 11, 1, Vec2::new(64.0, 0.0), "Lure I"));
    let b2 = w.add_planet(neutral_planet(seed + 12, 1, Vec2::new(100.0, 0.0), "Lure II"));
    w.add_lane(p, e, 28.0); // the short strike lane
    w.add_lane(e, b1, 36.0); // greedy's bait corridor
    w.add_lane(b1, b2, 36.0);
    (w, default_world_params())
}

// ======================================================================================
// L8-L10 — one PURE Automaton each, on the validated DIAMOND (where the cycle closes).
// ======================================================================================

/// L8 — "Overreach", vs a pure [`Roster::Colonize`] Automaton on the symmetric **diamond**.
/// Colonize out-expands but leaves its production undefended; the counter the player learns is
/// the **timed strike** (Attack > Colonize). Same diamond the `ai` suite measured the cycle on.
fn build_l8(seed: u64) -> (World, WorldParams) {
    (diamond(seed, 3, 10, 2), default_world_params())
}

/// L9 — "The Turtle", vs a pure [`Roster::Defend`] Automaton on the diamond. The turtle
/// concentrates on its own ground and barely expands; the counter is to **out-expand and starve
/// it** (Colonize > Defend).
fn build_l9(seed: u64) -> (World, WorldParams) {
    (diamond(seed, 3, 10, 2), default_world_params())
}

/// L10 — "The Hammer", vs a pure [`Roster::Attack`] Automaton on the diamond. Attack masses and
/// over-commits a spearhead; the counter is to **hold and punish the emptied rear** (Defend >
/// Attack).
fn build_l10(seed: u64) -> (World, WorldParams) {
    (diamond(seed, 3, 10, 2), default_world_params())
}

// --------------------------------------------------------------------------------------
// Local multi-sub planet helpers for L3 (explicit owned-centre + neutral ring layouts).
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
            build: build_l1,
        },
        Level {
            id: 2,
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
            build: build_l2,
        },
        Level {
            id: 3,
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
            build: build_l3,
        },
        Level {
            id: 4,
            title: "Far far away".into(),
            blurb: "Two fortified worlds face off across a long lane. Let automation run the home \
                    while you time the blow that breaks the foundry."
                .into(),
            objective: "Defeat the enemy world: defend your base and commit the fleet that tips it."
                .into(),
            hints: vec![
                "Turn on automation for your home so its internal defence runs itself.".into(),
                "Build a surplus before you commit — a half-strength fleet just feeds the enemy.".into(),
                "Watch the lane: a fleet takes time to undock and cross.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer2,
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            horizon: 1800,
            build: build_l4,
        },
        Level {
            id: 5,
            title: "Three Fronts".into(),
            blurb: "Three worlds in a triangle. You cannot be strong everywhere — take the \
                    crossroads, then turn its production against the enemy."
                .into(),
            objective: "Win the map: secure the neutral crossroads, then overwhelm the enemy.".into(),
            hints: vec![
                "Grab the neutral crossroads early for a two-to-one production edge.".into(),
                "Do not split your army three ways — concentrate where you intend to win.".into(),
                "Automation can hold a quiet front while you mass on the live one.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer2,
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            horizon: 1800,
            build: build_l5,
        },
        Level {
            id: 6,
            title: "The Prize".into(),
            blurb: "A great mine sits in the middle of the map, unclaimed and immensely productive. \
                    Whoever holds it pulls ahead — if they can still defend home."
                .into(),
            objective: "Out-produce and defeat the enemy: contest the central mine without losing your base."
                .into(),
            hints: vec![
                "The central mine is fat with production — taking it compounds fast.".into(),
                "But over-committing to the mine leaves your home thin; keep a garrison.".into(),
                "The short forward spurs are cheap buffers — useful, but not the prize.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer2,
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            // Raised for the resistance grind: contesting the fat 3-sub prize is a long slog, so the
            // map needs more ticks for a competent player's production edge to convert.
            horizon: 3000,
            build: build_l6,
        },
        Level {
            id: 7,
            title: "The Seam".into(),
            blurb: "The greedy mind always reaches forward for the next easy world — and never \
                    looks behind it. Its rear is one short lane away, and it will not be guarded."
                .into(),
            objective: "Exploit the seam: flank and capture the enemy's thinly-held rear, then snowball."
                .into(),
            hints: vec![
                "Greedy ships every spare ship toward the next neutral — its rear keeps only a token guard."
                    .into(),
                "Let it commit down the lure corridor, then mass and strike its rear across the short lane."
                    .into(),
                "The captured rear keeps producing for you — the flank snowballs.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer2,
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            horizon: 1800,
            build: build_l7,
        },
        Level {
            id: 8,
            title: "Overreach".into(),
            blurb: "This Automaton expands relentlessly, planting colonies faster than you can — \
                    but it barely guards what it grabs. Fat, undefended production is a target."
                .into(),
            objective: "Punish the colonizer: strike a fat, thinly-held enemy world before its growth compounds."
                .into(),
            hints: vec![
                "It out-expands you — do not try to win the land-grab race.".into(),
                "Its new colonies are held by a skeleton garrison. Mass and strike one.".into(),
                "A timed assault on undefended production beats a colonizer.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer2,
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            horizon: 2000,
            build: build_l8,
        },
        Level {
            id: 9,
            title: "The Turtle".into(),
            blurb: "This Automaton digs in and reinforces, refusing to over-extend. It will not \
                    come to you. Every tick it sits still is ground it is not taking."
                .into(),
            objective: "Starve the turtle: out-expand it and win on territory before it can break out."
                .into(),
            hints: vec![
                "A turtle pays an opportunity cost — it holds, but it does not grow.".into(),
                "Claim the neutral worlds it ignores and out-produce it.".into(),
                "You do not have to crack its shell — lead on territory at the horizon.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer2,
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            horizon: 2000,
            build: build_l9,
        },
        Level {
            id: 10,
            title: "The Hammer".into(),
            blurb: "This Automaton masses everything into one hammer-blow and swings it at your \
                    most valuable world. It hits hard — and empties its own rear to do it."
                .into(),
            objective: "Break the assault: survive the hammer, then counter-punch its stripped rear."
                .into(),
            hints: vec![
                "It concentrates and over-commits — its home goes thin behind the spearhead.".into(),
                "Hold and reinforce your threatened world; let the assault break on your garrison.".into(),
                "Then counter-attack the emptied rear it left behind.".into(),
            ],
            enemies: vec![Roster::SimpleColonize],
            start_view: StartView::Layer2,
            automation_available: false, // PARKED: basic automation quarantined pending redesign
            horizon: 2000,
            build: build_l10,
        },
    ]
}
