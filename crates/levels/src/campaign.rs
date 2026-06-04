//! The 10-level campaign: the authored curriculum and its per-level world `build` functions.
//!
//! Each level is a [`crate::Level`] carrying GUI-facing metadata (title / blurb / objective /
//! hints), the chosen enemy [`ai::Roster`], where the camera opens ([`crate::StartView`]),
//! whether basic automation is offered, the match horizon, and a bare
//! `build: fn(seed) -> (World, WorldParams)` world-builder. The player is always
//! [`Faction::Player`]; the enemy seat is [`Faction::Enemy`]; a level is **won** when
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

use layer1::{Faction, Vec2};
use world::{World, WorldParams};

use crate::builders::{
    authored_planet, default_world_params, diamond, neutral_planet, neutral_planet_res,
    stocked_planet, HOME_R, SUB_R,
};
use crate::{Level, StartView};
use ai::Roster;

// ======================================================================================
// L1 — "First Moves" (movement tutorial). StartView = Layer1.
// ======================================================================================

/// ONE planet, **3 sub-structures in a small triangle**: the Player owns the bottom-left anchor
/// (a decent garrison of 12), a **Passive** Enemy owns the bottom-right (a token 3), and the
/// apex is **neutral**. The three are spaced so they do **not** auto-fight at the start
/// (gap ≫ engagement radius) — the lesson is *movement and capture*, not combat: the player
/// selects the home, sends a fraction of its idle ships to take the neutral apex, then mops up
/// the inert enemy. With a Passive enemy this is trivially winnable.
fn build_l1(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    // A wide, shallow triangle. Subs are ~26-30 units apart so the start is peaceful (the
    // engagement radius is 7, the gap between any two subs is far larger), forcing the player
    // to actually move ships to make contact.
    let subs = [
        (Vec2::new(-15.0, -9.0), HOME_R, Faction::Player, 12usize), // your home (bottom-left)
        (Vec2::new(15.0, -9.0), SUB_R, Faction::Enemy, 3usize),     // inert foe (bottom-right)
        (Vec2::new(0.0, 16.0), SUB_R, Faction::Neutral, 0usize),    // neutral apex (the prize)
    ];
    let p = w.add_planet(authored_planet(seed, &subs, Vec2::new(0.0, 0.0), "Proving Ground"));
    let _ = p;
    (w, default_world_params())
}

// ======================================================================================
// L2 — "Contact" (combat tutorial). StartView = Layer1.
// ======================================================================================

/// ONE planet, **5 sub-structures in groups of 2 / 1 / 2** (a left pair, a central keep, a
/// right pair). The pairs are placed *close enough that adjacent sub-structures fight across
/// each other* through the engagement radius (the proximity battle bubbles), and the centre
/// keep sits in range of both inner posts. The Player owns the **outer-left** sub (well
/// stocked); a **GreedyLocal** Enemy owns the **outer-right** sub; the inner-left, the centre,
/// and the inner-right start neutral.
///
/// The lesson: *layout decides who fights whom, and concentration wins the brawl.* The player
/// must mass onto the inner-left post and then the contested centre rather than dribbling ships
/// in. Garrisons are set so a player who concentrates clearly beats the greedy foe, but a
/// player who splits force can stall.
fn build_l2(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    // x layout: left pair at -22 (home) and -8 (neutral inner-left); centre at 0; right pair at
    // +8 (neutral inner-right) and +22 (enemy home). Inner posts (-8 / 0 / +8) are 8 apart, so
    // with radius 4 and engagement radius 7 they engage across the gaps once ships garrison
    // them — the central keep is the contested flashpoint. The two homes (±22) are out of range
    // of each other, so the fight is decided in the middle.
    let subs = [
        (Vec2::new(-22.0, 0.0), HOME_R, Faction::Player, 18usize), // your home (outer-left)
        (Vec2::new(-8.0, 0.0), SUB_R, Faction::Neutral, 0usize),   // inner-left (neutral)
        (Vec2::new(0.0, 0.0), SUB_R, Faction::Neutral, 0usize),    // the keep (neutral centre)
        (Vec2::new(8.0, 0.0), SUB_R, Faction::Neutral, 0usize),    // inner-right (neutral)
        (Vec2::new(22.0, 0.0), HOME_R, Faction::Enemy, 10usize),   // enemy home (outer-right)
    ];
    let p = w.add_planet(authored_planet(seed, &subs, Vec2::new(0.0, 0.0), "The Crucible"));
    let _ = p;
    (w, default_world_params())
}

// ======================================================================================
// L3 — "Two Worlds" (Layer-2 + zoom + automation intro). StartView = Layer2.
// ======================================================================================

/// TWO planets + **one lane**. Planet 1 ("Homeworld") has **9 sub-structures**, of which the
/// Player owns **1** (the rest neutral — a big internal frontier to expand into). Planet 2
/// ("Outpost") has **5 sub-structures**, of which a **GreedyLocal** Enemy owns **1** (the rest
/// neutral). `automation_available = true`.
///
/// The lesson combines three new ideas: *send a fleet between planets* (the Layer-2 atomic
/// action), *zoom into a planet to micro its sub-structures*, and *enable basic automation* to
/// let a planet you are not actively flying expand/defend itself. The Player's homeworld is
/// rich enough that even with automation handling it, a fleet shipped to the outpost decides
/// the map.
fn build_l3(seed: u64) -> (World, WorldParams) {
    let mut w = World::new();
    // Homeworld: 9 subs, Player owns the centre one, the other 8 neutral. Seeded with a strong
    // home garrison so the player has surplus to both expand internally AND ship a fleet over.
    let home = nine_sub_player_home(seed, 16, Vec2::new(0.0, 0.0), "Homeworld");
    // Outpost: 5 subs, GreedyLocal Enemy owns the centre, the other 4 neutral.
    let outpost = five_sub_enemy_outpost(seed + 1, 8, Vec2::new(70.0, 0.0), "Outpost");
    let p = w.add_planet(home);
    let e = w.add_planet(outpost);
    w.add_lane(p, e, 70.0);
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
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Enemy, 4, 9, Vec2::new(95.0, 0.0), "Foundry"));
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
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Enemy, 3, 9, Vec2::new(100.0, 0.0), "Spire"));
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
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Enemy, 3, 9, Vec2::new(120.0, 0.0), "Citadel"));
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
    let e = w.add_planet(stocked_planet(seed + 1, Faction::Enemy, 1, 10, Vec2::new(28.0, 0.0), "Enemy Rear"));
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

/// A 9-sub planet for the Player: the centre sub is Player-owned (seeded `home_ships` idle),
/// the surrounding 8 are neutral. A big internal frontier to expand into with automation /
/// micro while a fleet ships out.
fn nine_sub_player_home(seed: u64, home_ships: usize, pos: Vec2, name: &str) -> world::Planet {
    let mut subs: Vec<(Vec2, f32, Faction, usize)> = Vec::with_capacity(9);
    subs.push((Vec2::new(0.0, 0.0), HOME_R, Faction::Player, home_ships)); // centre = home
    // 8 neutral subs on a ring around the centre (within reach to expand into).
    for i in 0..8 {
        let ang = (i as f32) / 8.0 * std::f32::consts::TAU;
        let r = 16.0;
        subs.push((Vec2::new(r * ang.cos(), r * ang.sin()), SUB_R, Faction::Neutral, 0));
    }
    authored_planet(seed, &subs, pos, name)
}

/// A 5-sub planet for the Enemy: the centre sub is Enemy-owned (seeded `garrison` idle), the
/// surrounding 4 are neutral.
fn five_sub_enemy_outpost(seed: u64, garrison: usize, pos: Vec2, name: &str) -> world::Planet {
    let mut subs: Vec<(Vec2, f32, Faction, usize)> = Vec::with_capacity(5);
    subs.push((Vec2::new(0.0, 0.0), HOME_R, Faction::Enemy, garrison)); // centre = enemy seat
    for i in 0..4 {
        let ang = (i as f32) / 4.0 * std::f32::consts::TAU;
        let r = 14.0;
        subs.push((Vec2::new(r * ang.cos(), r * ang.sin()), SUB_R, Faction::Neutral, 0));
    }
    authored_planet(seed, &subs, pos, name)
}

// ======================================================================================
// The campaign.
// ======================================================================================

/// The 10 campaign levels, in play order. This is the single list the GUI consumes (it reads
/// each [`Level`]'s metadata to drive the UI, and calls `build(seed)` to instantiate the
/// world). The order is the intended difficulty/teaching progression.
pub fn campaign() -> Vec<Level> {
    vec![
        Level {
            id: 1,
            title: "First Moves".into(),
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
            enemy: Roster::Passive,
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 1200,
            build: build_l1,
        },
        Level {
            id: 2,
            title: "Contact".into(),
            blurb: "Five sites in a tight cluster — close enough that ships trade fire across the \
                    gaps. The shape of the ground decides the battle."
                .into(),
            objective: "Break the enemy: hold the central keep and capture the enemy's home site."
                .into(),
            hints: vec![
                "Sites that are close together fight across the gap — you do not need to be on the \
                 same site to engage."
                    .into(),
                "Concentrate: mass onto the inner-left site, then the centre keep, before pushing \
                 right."
                    .into(),
                "Feeding ships in a trickle loses the bubble; send them in waves.".into(),
            ],
            enemy: Roster::GreedyLocal,
            start_view: StartView::Layer1(0),
            automation_available: false,
            horizon: 1500,
            build: build_l2,
        },
        Level {
            id: 3,
            title: "Two Worlds".into(),
            blurb: "Two planets, one lane. Your homeworld is wide open to settle; a greedy outpost \
                    holds the other. Zoom out to ship a fleet across — and zoom in to fight."
                .into(),
            objective: "Take the enemy outpost. Settle your homeworld and land a fleet that wins it."
                .into(),
            hints: vec![
                "This is the Layer-2 view: planets and the lanes between them.".into(),
                "Select your homeworld and send a fleet down the lane to the outpost.".into(),
                "Zoom into a planet to micro its sub-structures directly.".into(),
                "Enable automation on your homeworld so it expands and defends itself while you fly \
                 the fleet."
                    .into(),
            ],
            enemy: Roster::GreedyLocal,
            start_view: StartView::Layer2,
            automation_available: true,
            horizon: 1500,
            build: build_l3,
        },
        Level {
            id: 4,
            title: "Hold the Line".into(),
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
            enemy: Roster::GreedyLocal,
            start_view: StartView::Layer2,
            automation_available: true,
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
            enemy: Roster::GreedyLocal,
            start_view: StartView::Layer2,
            automation_available: true,
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
            enemy: Roster::GreedyLocal,
            start_view: StartView::Layer2,
            automation_available: true,
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
            enemy: Roster::GreedyLocal,
            start_view: StartView::Layer2,
            automation_available: true,
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
            enemy: Roster::Colonize,
            start_view: StartView::Layer2,
            automation_available: true,
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
            enemy: Roster::Defend,
            start_view: StartView::Layer2,
            automation_available: true,
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
            enemy: Roster::Attack,
            start_view: StartView::Layer2,
            automation_available: true,
            horizon: 2000,
            build: build_l10,
        },
    ]
}
