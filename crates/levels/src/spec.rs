//! **Data-driven level definitions** (owner ask, 2026-07-08: "make the move towards
//! data-driven levels" — a mission tweak must not cost a recompile).
//!
//! A level lives in one plain-text `.lvl` file under `assets/levels/`, parsed at startup by
//! [`parse`] (hand-rolled — the workspace's zero-external-dependency rule bans serde; the
//! same tradition as `mi_controls.cfg` / `mi_progress.json`) and built into a playable
//! [`world::World`] by [`LevelSpec::build`] — a deterministic interpreter that reproduces
//! exactly what the old hand-written `build_*` functions did: same construction order, same
//! RNG draws, same worlds bit-for-bit for a given seed.
//!
//! # The format
//!
//! Line-based `key = value` under section headers; `#` starts a comment; keys and section
//! order matter only as documented. Sub-structures belong to the most recent `[struct]`,
//! and their **file order is the sub id order** (ships spawn right after their sub is added,
//! matching the old builders' interleaving — determinism depends on it).
//!
//! ```text
//! [level]
//! id        = 7
//! title     = Far far away
//! blurb     = One paragraph; a literal \n escape breaks lines.
//! objective = ...
//! hint      = repeatable — one per line
//! enemy     = simple_adjacent 100      # passive | simple | cycler | simple_adjacent <range>
//! start     = layer1 0                 # layer1 <struct-index> | layer2
//! horizon   = 4800
//! zoom_min  = 0.8                      # optional per-mission out-zoom floor
//!
//! [struct]                             # repeatable
//! pos              = 0 0               # Layer-2 position
//! name             =                   # optional (empty = unnamed)
//! storage_scale    = 0.6               # optional reserve dial (× STORAGE_RADIUS_SCALE)
//! storage_capacity = 10000             # optional reserve capacity override
//!
//! [sub]                                # repeatable, belongs to the struct above
//! pos            = -60 0               # struct-local position
//! kind           = standard            # optional: standard | fortress | teleporter | shipyard
//! owner          = player              # player | enemy | enemy2 | neutral
//! cap            = 60                  # optional storage capacity (recouples resistance)
//! prod           = 2                   # optional production
//! ships          = 90                  # optional starting garrison (spawned for the owner)
//! max_resistance = 1800                # optional override (applied after cap)
//! keep_surplus   = true                # optional: over-cap production stays home
//! orbit          = 0 0 -1500           # optional: center_x center_y period (reference
//!                                      # ticks per revolution; negative = clockwise)
//!
//! [lane]                               # repeatable, after the structs it connects
//! between = 0 1                        # struct indices
//! length  = 170
//! ```

use layer1::{Faction, Interior, SubKind, SubStructure, Vec2};
use world::{Structure, World, WorldParams};

use crate::builders::default_world_params;
use ai::Roster;
use crate::StartView;

/// A parsed level definition: the [`crate::Level`] metadata plus the world recipe.
#[derive(Debug, Clone)]
pub struct LevelSpec {
    pub id: u32,
    pub title: String,
    pub blurb: String,
    pub objective: String,
    pub hints: Vec<String>,
    pub enemies: Vec<Roster>,
    pub start_view: StartView,
    pub horizon: u64,
    pub zoom_min: Option<f32>,
    pub structs: Vec<StructSpec>,
    pub lanes: Vec<(usize, usize, f32)>,
}

/// One structure: its Layer-2 placement, reserve dials, and sub-structures in id order.
#[derive(Debug, Clone)]
pub struct StructSpec {
    pub pos: Vec2,
    pub name: String,
    pub storage_scale: Option<f32>,
    pub storage_capacity: Option<u32>,
    pub subs: Vec<SubSpec>,
}

/// One sub-structure recipe (defaults mirror the [`SubStructure`] constructors).
#[derive(Debug, Clone)]
pub struct SubSpec {
    pub pos: Vec2,
    pub kind: SubKind,
    pub owner: Faction,
    pub cap: Option<u32>,
    pub prod: Option<u32>,
    pub ships: u32,
    pub max_resistance: Option<f32>,
    pub keep_surplus: bool,
    /// `(orbit centre, revolution period in reference ticks; negative = clockwise)`.
    pub orbit: Option<(Vec2, f32)>,
}

impl LevelSpec {
    /// Build this level's world from `seed` — the deterministic interpreter. Construction
    /// order matches the old hand-written builders exactly: struct `i` seeds its interior
    /// with `seed + i`; each sub is added then immediately garrisoned; the reserve node comes
    /// last; lanes after all structs. Same seed ⇒ the same world, bit for bit.
    pub fn build(&self, seed: u64) -> (World, WorldParams) {
        let mut w = World::new();
        for (i, sp) in self.structs.iter().enumerate() {
            let mut st = Interior::new(seed.wrapping_add(i as u64));
            for sub in &sp.subs {
                let mut s = match sub.kind {
                    SubKind::Standard => SubStructure::new(sub.pos, 0.0, sub.owner),
                    SubKind::Fortress => SubStructure::fortress(sub.pos, sub.owner),
                    SubKind::Teleporter => SubStructure::teleporter(sub.pos, sub.owner),
                    SubKind::Shipyard { .. } => SubStructure::shipyard(sub.pos, sub.owner),
                };
                if let Some(c) = sub.cap {
                    s = s.with_storage_capacity(c); // recouples resistance, like the builders
                }
                if let Some(p) = sub.prod {
                    s = s.with_production(p);
                }
                if let Some(m) = sub.max_resistance {
                    s = s.with_max_resistance(m);
                }
                if sub.keep_surplus {
                    s = s.keep_surplus();
                }
                if let Some((centre, period)) = sub.orbit {
                    s = s.orbiting(centre, std::f32::consts::TAU / period);
                }
                let id = st.add_sub(s);
                for _ in 0..sub.ships {
                    st.spawn_ship(sub.owner, id);
                }
            }
            match sp.storage_scale {
                Some(x) => {
                    st.add_storage_sub_scaled(layer1::sim::STORAGE_RADIUS_SCALE * x);
                }
                None => {
                    st.add_storage_sub();
                }
            }
            if let Some(cap) = sp.storage_capacity {
                if let Some(stg) = st.storage_sub {
                    st.subs[stg].storage_capacity = cap;
                }
            }
            w.add_struct(Structure::new(st, sp.pos, &sp.name));
        }
        for &(a, b, len) in &self.lanes {
            w.add_lane(a, b, len);
        }
        (w, default_world_params())
    }
}

/// Parse one `.lvl` file (see the module doc for the format). Errors carry the 1-based line
/// number and are meant to be `panic!`ed with the file name at load time — a malformed level
/// is a content bug that must be loud.
pub fn parse(text: &str) -> Result<LevelSpec, String> {
    #[derive(PartialEq, Clone, Copy)]
    enum Section {
        None,
        Level,
        Struct,
        Sub,
        Lane,
    }
    let mut sec = Section::None;

    // The spec under construction (metadata defaults are "missing" and checked at the end).
    let mut id: Option<u32> = None;
    let mut title = String::new();
    let mut blurb = String::new();
    let mut objective = String::new();
    let mut hints: Vec<String> = Vec::new();
    let mut enemies: Vec<Roster> = Vec::new();
    let mut start_view: Option<StartView> = None;
    let mut horizon: Option<u64> = None;
    let mut zoom_min: Option<f32> = None;
    let mut structs: Vec<StructSpec> = Vec::new();
    let mut lanes: Vec<(usize, usize, f32)> = Vec::new();
    let mut lane_between: Option<(usize, usize)> = None;
    let mut lane_length: Option<f32> = None;

    fn vec2(v: &str, ln: usize) -> Result<Vec2, String> {
        let mut it = v.split_whitespace();
        let x = it.next().and_then(|s| s.parse::<f32>().ok());
        let y = it.next().and_then(|s| s.parse::<f32>().ok());
        match (x, y) {
            (Some(x), Some(y)) => Ok(Vec2::new(x, y)),
            _ => Err(format!("line {ln}: expected two numbers, got `{v}`")),
        }
    }
    fn num<T: std::str::FromStr>(v: &str, ln: usize) -> Result<T, String> {
        v.parse::<T>().map_err(|_| format!("line {ln}: bad number `{v}`"))
    }

    let flush_lane =
        |between: &mut Option<(usize, usize)>, length: &mut Option<f32>, lanes: &mut Vec<(usize, usize, f32)>, ln: usize| -> Result<(), String> {
            match (between.take(), length.take()) {
                (Some((a, b)), Some(l)) => {
                    lanes.push((a, b, l));
                    Ok(())
                }
                (None, None) => Ok(()),
                _ => Err(format!("line {ln}: [lane] needs both `between` and `length`")),
            }
        };

    for (i, raw) in text.lines().enumerate() {
        let ln = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line {
            "[level]" => {
                flush_lane(&mut lane_between, &mut lane_length, &mut lanes, ln)?;
                sec = Section::Level;
                continue;
            }
            "[struct]" => {
                flush_lane(&mut lane_between, &mut lane_length, &mut lanes, ln)?;
                sec = Section::Struct;
                structs.push(StructSpec {
                    pos: Vec2::new(0.0, 0.0),
                    name: String::new(),
                    storage_scale: None,
                    storage_capacity: None,
                    subs: Vec::new(),
                });
                continue;
            }
            "[sub]" => {
                flush_lane(&mut lane_between, &mut lane_length, &mut lanes, ln)?;
                sec = Section::Sub;
                let st = structs.last_mut().ok_or(format!("line {ln}: [sub] before any [struct]"))?;
                st.subs.push(SubSpec {
                    pos: Vec2::new(0.0, 0.0),
                    kind: SubKind::Standard,
                    owner: Faction::Neutral,
                    cap: None,
                    prod: None,
                    ships: 0,
                    max_resistance: None,
                    keep_surplus: false,
                    orbit: None,
                });
                continue;
            }
            "[lane]" => {
                flush_lane(&mut lane_between, &mut lane_length, &mut lanes, ln)?;
                sec = Section::Lane;
                continue;
            }
            _ => {}
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("line {ln}: expected `key = value`, got `{line}`"));
        };
        let (k, v) = (k.trim(), v.trim());
        match sec {
            Section::None => return Err(format!("line {ln}: `{k}` before any section header")),
            Section::Level => match k {
                "id" => id = Some(num(v, ln)?),
                "title" => title = v.to_string(),
                "blurb" => blurb = v.replace("\\n", "\n"),
                "objective" => objective = v.replace("\\n", "\n"),
                "hint" => hints.push(v.replace("\\n", "\n")),
                "enemy" => {
                    let mut it = v.split_whitespace();
                    let name = it.next().unwrap_or("");
                    enemies.push(match name {
                        "passive" => Roster::Passive,
                        "simple" => Roster::SimpleColonize,
                        "cycler" => Roster::Cycler,
                        "simple_adjacent" => {
                            let r = it
                                .next()
                                .and_then(|s| s.parse::<f32>().ok())
                                .ok_or(format!("line {ln}: simple_adjacent needs a range"))?;
                            Roster::SimpleAdjacent { range: r }
                        }
                        other => return Err(format!("line {ln}: unknown enemy `{other}`")),
                    });
                }
                "start" => {
                    let mut it = v.split_whitespace();
                    start_view = Some(match it.next().unwrap_or("") {
                        "layer2" => StartView::Layer2,
                        "layer1" => StartView::Layer1(
                            it.next()
                                .and_then(|s| s.parse::<usize>().ok())
                                .ok_or(format!("line {ln}: layer1 needs a struct index"))?,
                        ),
                        other => return Err(format!("line {ln}: unknown start `{other}`")),
                    });
                }
                "horizon" => horizon = Some(num(v, ln)?),
                "zoom_min" => zoom_min = Some(num(v, ln)?),
                other => return Err(format!("line {ln}: unknown [level] key `{other}`")),
            },
            Section::Struct => {
                let st = structs.last_mut().expect("section guarantees a struct");
                match k {
                    "pos" => st.pos = vec2(v, ln)?,
                    "name" => st.name = v.to_string(),
                    "storage_scale" => st.storage_scale = Some(num(v, ln)?),
                    "storage_capacity" => st.storage_capacity = Some(num(v, ln)?),
                    other => return Err(format!("line {ln}: unknown [struct] key `{other}`")),
                }
            }
            Section::Sub => {
                let sub = structs
                    .last_mut()
                    .and_then(|s| s.subs.last_mut())
                    .expect("section guarantees a sub");
                match k {
                    "pos" => sub.pos = vec2(v, ln)?,
                    "kind" => {
                        sub.kind = match v {
                            "standard" => SubKind::Standard,
                            "fortress" => SubKind::Fortress,
                            "teleporter" => SubKind::Teleporter,
                            // The active flag is derived from the owner by the constructor.
                            "shipyard" => SubKind::Shipyard { active: false },
                            other => return Err(format!("line {ln}: unknown kind `{other}`")),
                        }
                    }
                    "owner" => {
                        sub.owner = match v {
                            "player" => Faction::Player,
                            "enemy" => Faction::Ai(0),
                            "enemy2" => Faction::Ai(1),
                            "neutral" => Faction::Neutral,
                            other => return Err(format!("line {ln}: unknown owner `{other}`")),
                        }
                    }
                    "cap" => sub.cap = Some(num(v, ln)?),
                    "prod" => sub.prod = Some(num(v, ln)?),
                    "ships" => sub.ships = num(v, ln)?,
                    "max_resistance" => sub.max_resistance = Some(num(v, ln)?),
                    "keep_surplus" => sub.keep_surplus = v == "true",
                    "orbit" => {
                        let mut it = v.split_whitespace();
                        let x = it.next().and_then(|s| s.parse::<f32>().ok());
                        let y = it.next().and_then(|s| s.parse::<f32>().ok());
                        let period = it.next().and_then(|s| s.parse::<f32>().ok());
                        match (x, y, period) {
                            (Some(x), Some(y), Some(t)) if t != 0.0 => {
                                sub.orbit = Some((Vec2::new(x, y), t))
                            }
                            _ => {
                                return Err(format!(
                                    "line {ln}: orbit needs `center_x center_y period` (period ≠ 0)"
                                ))
                            }
                        }
                    }
                    other => return Err(format!("line {ln}: unknown [sub] key `{other}`")),
                }
            }
            Section::Lane => match k {
                "between" => {
                    let mut it = v.split_whitespace();
                    let a = it.next().and_then(|s| s.parse::<usize>().ok());
                    let b = it.next().and_then(|s| s.parse::<usize>().ok());
                    match (a, b) {
                        (Some(a), Some(b)) => lane_between = Some((a, b)),
                        _ => return Err(format!("line {ln}: between needs two struct indices")),
                    }
                }
                "length" => lane_length = Some(num(v, ln)?),
                other => return Err(format!("line {ln}: unknown [lane] key `{other}`")),
            },
        }
    }
    flush_lane(&mut lane_between, &mut lane_length, &mut lanes, text.lines().count())?;

    if structs.is_empty() {
        return Err("no [struct] section".into());
    }
    for &(a, b, _) in &lanes {
        if a >= structs.len() || b >= structs.len() {
            return Err(format!("lane between {a} and {b}: no such struct"));
        }
    }
    if let Some(StartView::Layer1(s)) = start_view {
        if s >= structs.len() {
            return Err(format!("start layer1 {s}: no such struct"));
        }
    }
    Ok(LevelSpec {
        id: id.ok_or("missing [level] id")?,
        title,
        blurb,
        objective,
        hints,
        enemies,
        start_view: start_view.ok_or("missing [level] start")?,
        horizon: horizon.ok_or("missing [level] horizon")?,
        zoom_min,
        structs,
        lanes,
    })
}
