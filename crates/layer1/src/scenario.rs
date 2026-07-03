//! Ready-made Layer-1 scenarios — chiefly the **sample structure** the headless runner and
//! the future GUI start from.
//!
//! The sample is a single structure of 7 sub-structures in a deliberately *interesting*
//! layout: two opposing home bases, two flanking forward posts each side can fight over, and
//! a contested neutral keep in the middle. Several sub-structures sit close enough that
//! ships fight **across** them through the engagement radius (the core Layer-1 property) —
//! e.g. each side's two forward posts and the central keep form one proximity neighbourhood.

use crate::sim::{SimParams, Interior, SubStructure};
use crate::types::{Faction, SubId, Vec2};

/// The named ids of the sample structure's sub-structures, returned alongside it so callers
/// (runner, tests, GUI) can refer to them meaningfully.
#[derive(Debug, Clone, Copy)]
pub struct SampleLayout {
    /// Player home base (rear-left), starts well garrisoned.
    pub player_home: SubId,
    /// Enemy home base (rear-right), starts well garrisoned.
    pub enemy_home: SubId,
    /// Player's forward post (upper-left of centre).
    pub player_post: SubId,
    /// Enemy's forward post (upper-right of centre).
    pub enemy_post: SubId,
    /// A neutral flank post low-left, near the player.
    pub neutral_left: SubId,
    /// A neutral flank post low-right, near the enemy.
    pub neutral_right: SubId,
    /// The contested neutral keep in the dead centre — the prize both sides want.
    pub neutral_keep: SubId,
}

/// Build the sample structure seeded with `seed`, plus its [`SampleLayout`].
///
/// Layout (x grows right, y grows up), 7 sub-structures:
/// ```text
///        player_post(.)         neutral_keep(N)        enemy_post(.)
///   player_home(P)                                          enemy_home(E)
///        neutral_left(N)                              neutral_right(N)
/// ```
/// Player starts owning `player_home` (+8 ships) and `player_post` (+3); Enemy mirrors with
/// `enemy_home`/`enemy_post`. The three `neutral_*` sub-structures start unowned and empty,
/// giving both AIs an immediate reason to expand and contest. The central keep is roughly
/// equidistant and within proximity range of both forward posts, so the first brawl tends to
/// erupt around it — ships from a post can fire on the keep's defenders without being
/// garrisoned there.
pub fn sample_structure(seed: u64) -> (Interior, SampleLayout) {
    let mut st = Interior::new(seed);

    // Homes are large (more garrison room, strong defender edge); posts/keep are medium.
    let home_r = 5.0;
    let post_r = 4.0;

    let player_home = st.add_sub(SubStructure::new(Vec2::new(-26.0, 0.0), home_r, Faction::Player));
    let enemy_home = st.add_sub(SubStructure::new(Vec2::new(26.0, 0.0), home_r, Faction::Ai(0)));

    let player_post = st.add_sub(SubStructure::new(Vec2::new(-9.0, 8.0), post_r, Faction::Player));
    let enemy_post = st.add_sub(SubStructure::new(Vec2::new(9.0, 8.0), post_r, Faction::Ai(0)));

    let neutral_left = st.add_sub(SubStructure::new(Vec2::new(-12.0, -9.0), post_r, Faction::Neutral));
    let neutral_right = st.add_sub(SubStructure::new(Vec2::new(12.0, -9.0), post_r, Faction::Neutral));
    let neutral_keep = st.add_sub(SubStructure::new(Vec2::new(0.0, 6.0), post_r, Faction::Neutral));

    // Starting garrisons: a healthy home stack plus a small forward post each, so both
    // sides can expand AND fight without the match being decided by a single opening clash.
    for _ in 0..12 {
        st.spawn_ship(Faction::Player, player_home);
    }
    for _ in 0..4 {
        st.spawn_ship(Faction::Player, player_post);
    }
    for _ in 0..12 {
        st.spawn_ship(Faction::Ai(0), enemy_home);
    }
    for _ in 0..4 {
        st.spawn_ship(Faction::Ai(0), enemy_post);
    }

    let layout = SampleLayout {
        player_home,
        enemy_home,
        player_post,
        enemy_post,
        neutral_left,
        neutral_right,
        neutral_keep,
    };
    (st, layout)
}

/// The [`SimParams`] the sample scenario is tuned for (currently the defaults). Provided as
/// a named function so the runner/tests/GUI all share one operating point.
pub fn sample_params() -> SimParams {
    SimParams::default()
}
