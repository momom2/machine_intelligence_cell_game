//! # layer1 — the headless, deterministic Layer-1 spatial micro-simulation
//!
//! Layer 1 of the cell game (`03-ui-layers.md`, "embodied / micro") is a **single
//! structure** rendered with the ships and sub-structures the player commands at the
//! operator level. This crate is the **simulation only** — no graphics — honouring the
//! design's signature principle (`00-overview.md`): *decouple computation from spectacle*.
//! The sim is the computation; a future macroquad renderer is the spectacle, built directly
//! on the types here.
//!
//! ## What it models (the project owner's Layer-1 spec)
//!
//! * **One [`Structure`]** = several [`SubStructure`]s placed at 2D positions, each owned by
//!   a [`Faction`] (`Player`/`Enemy`) or `Neutral`, each slowly **producing** ships for its
//!   owner (a reason to hold ground; the square-law snowball).
//! * **Discrete [`Ship`]s** garrison at a sub-structure or **move** to another at a fixed
//!   speed with per-ship spread.
//! * **Proximity battle bubbles**: combat is purely positional. Any ship with a living enemy
//!   within the [`SimParams::engagement_radius`] is engaged — so ships near the border
//!   between two close sub-structures fight *across* them; being in the same sub-structure
//!   is not required. The layout decides who fights whom.
//! * **Stochastic Lanchester square-law combat** (`01-mechanics.md`, the Layer-1/spectacle
//!   model): each engaged ship is a stochastic emitter that one-shots a random in-range
//!   enemy when it fires; loss rate scales with engaged-ship count, so `2x ships => ~4x`
//!   relative advantage emerges. All randomness comes from one seeded, in-crate PRNG
//!   ([`rng::Rng`]) so runs are bit-reproducible from a seed.
//! * **Capture & win**: a sub-structure flips to a faction that holds it uncontested; a
//!   faction is eliminated with zero ships and zero sub-structures. [`Structure::outcome`]
//!   reports the winner (by elimination, or lead at a horizon).
//!
//! ## The renderer/GUI API at a glance
//!
//! * Build the sample world: [`scenario::sample_structure`] + [`scenario::sample_params`].
//! * Issue a fraction-bucket move order: [`Structure::issue_order`] with a [`MoveOrder`].
//! * Step one frame: [`Structure::step`] (deterministic; `dt` is one tick — call N times for
//!   N ticks; the renderer interpolates positions between calls if it wants sub-tick smooth).
//! * Query for drawing: [`Structure::subs`], [`Structure::ships`],
//!   [`Structure::battle_bubbles`], plus counts ([`Structure::ship_count`],
//!   [`Structure::sub_count`]).
//! * Outcome: [`Structure::outcome`].
//! * The enemy mind: [`ai::Automaton`] (drive either seat with [`ai::drive`]).
//!
//! ## Module map
//! * [`rng`] — the seeded, dependency-free PRNG (xorshift64*).
//! * [`types`] — plain value types ([`Vec2`], [`Faction`], [`MoveOrder`], ids, buckets).
//! * [`sim`] — the [`Structure`], its tick loop, combat, battle bubbles, capture, outcome.
//! * [`ai`] — the Layer-1 [`Automaton`] reactive micro-policy (one documented seam).
//! * [`scenario`] — the sample structure both the runner and the GUI start from.

pub mod ai;
pub mod rng;
pub mod scenario;
pub mod sim;
pub mod types;

// Flat re-exports of the most-used items for renderer/host convenience.
pub use ai::{drive, Automaton};
pub use rng::Rng;
pub use scenario::{sample_params, sample_structure, SampleLayout};
pub use sim::{BattleBubble, Outcome, SimParams, Ship, Structure, SubKind, SubStructure};
pub use types::{Faction, FractionBucket, MoveOrder, ShipId, SubId, Vec2};

/// Run an **Automaton-vs-Automaton** match on `st` to elimination or `horizon` ticks,
/// invoking each Automaton's decision every `decision_interval` ticks. Returns the final
/// [`Outcome`]. Used by the headless runner and tests.
///
/// `decision_interval` (>= 1) throttles re-planning so the AIs commit forces over time
/// rather than re-issuing every tick; `1` means decide every tick. Both seats decide on the
/// *same* pre-step snapshot (Player's orders issued first — a fixed, documented tie-break;
/// the sim is otherwise seat-symmetric).
///
/// `on_tick` is an optional callback invoked **after** each tick with the current tick
/// number and the structure, so a runner can print a periodic summary without this function
/// knowing anything about I/O.
pub fn run_auto_vs_auto(
    st: &mut Structure,
    params: &SimParams,
    player: &Automaton,
    enemy: &Automaton,
    horizon: u64,
    decision_interval: u64,
    mut on_tick: impl FnMut(u64, &Structure),
) -> Outcome {
    let interval = decision_interval.max(1);
    while st.tick < horizon {
        if st.is_eliminated(Faction::Player) || st.is_eliminated(Faction::Ai(0)) {
            break;
        }
        if st.tick % interval == 0 {
            // Both decide on the same snapshot; Player issues first (documented tie-break).
            let p_orders = player.decide(st, params);
            let e_orders = enemy.decide(st, params);
            for o in p_orders {
                st.issue_order(o, player.seat);
            }
            for o in e_orders {
                st.issue_order(o, enemy.seat);
            }
        }
        st.step(params);
        on_tick(st.tick, st);
    }
    st.outcome()
}
