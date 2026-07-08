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
//! zoom_min         = 0.1               # optional PER-STRUCT interior out-zoom floor
//! zoom_max         = 7.0               # optional PER-STRUCT interior in-zoom ceiling
//!                                      # (defaults: the [level] zoom_min / the global bounds)
//!
//! [sub]                                # repeatable, belongs to the struct above
//! pos            = -60 0               # struct-local position. UNIFORM NOISE (owner ask,
//!                                      # 2026-07-08): any pos component may be `A+-X` —
//!                                      # e.g. `pos = -14+-1 14+-2` draws x in [-15,-13],
//!                                      # y in [12,16]. Drawn ONCE at world build from the
//!                                      # match seed (fixed for the match; same seed ⇒ the
//!                                      # same layout, so replays/validation stay
//!                                      # bit-identical). [struct] pos accepts it too.
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
//! [orbit]                              # the RING CONSTRUCTOR (owner ask, 2026-07-08):
//! center = 0 0                         # every [sub] that follows — until the next
//! radius = 72.576                      # [orbit]/[struct]/[lane] — is a MEMBER of this
//! period = -1500                       # ring: it takes `angle = <degrees>` (0° = +x,
//!                                      # counter-clockwise) INSTEAD of `pos`/`orbit`.
//!                                      # Omit every angle and the members are spaced
//!                                      # regularly in file order starting at 0°. The ring
//!                                      # compiles down to per-sub pos + orbit at parse
//!                                      # time. Plain positioned subs must come BEFORE the
//!                                      # struct's rings (a ring runs to the next header).
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
    /// Per-component uniform noise half-width for `pos` (the `+-X` syntax; 0 = exact).
    pub pos_noise: Vec2,
    pub name: String,
    pub storage_scale: Option<f32>,
    pub storage_capacity: Option<u32>,
    /// PER-STRUCT interior zoom bounds (owner mechanism, 2026-07-08): override the level /
    /// global out-zoom floor and in-zoom ceiling while THIS struct's interior is focused —
    /// e.g. Far far away's rear yard-only struct needs a much lower floor before the wheel
    /// exits to the lens, so its huge reserve ring can actually be seen. `None` = the
    /// level's `zoom_min` (floor) / the global ceiling.
    pub zoom_min: Option<f32>,
    pub zoom_max: Option<f32>,
    pub subs: Vec<SubSpec>,
}

/// One sub-structure recipe (defaults mirror the [`SubStructure`] constructors).
#[derive(Debug, Clone)]
pub struct SubSpec {
    pub pos: Vec2,
    /// Per-component uniform noise half-width for `pos` (the `+-X` syntax; 0 = exact).
    pub pos_noise: Vec2,
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

/// The dedicated RNG for the `+-X` position noise (splitmix64 — hand-rolled, zero-dep).
/// One stream per world build, seeded from the match seed (xor-decorrelated from the
/// interiors' `seed + i` streams), consumed in strict struct/sub file order and ONLY for
/// non-zero spreads — so a given spec + seed always draws the same layout, bit for bit.
struct NoiseRng(u64);

impl NoiseRng {
    fn new(seed: u64) -> NoiseRng {
        NoiseRng(seed ^ 0xA0B4_2C43_58F1_D7D3)
    }
    /// Next uniform f32 in `[0, 1)` (top 24 bits of a splitmix64 step — exact in f32).
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        ((z >> 40) as f32) / (1u64 << 24) as f32
    }
    /// `base ± spread` per component, uniform; a zero spread draws nothing (keeps the
    /// stream stable when only some components are noisy).
    fn jitter(&mut self, base: Vec2, spread: Vec2) -> Vec2 {
        let mut p = base;
        if spread.x != 0.0 {
            p.x += (2.0 * self.next_f32() - 1.0) * spread.x;
        }
        if spread.y != 0.0 {
            p.y += (2.0 * self.next_f32() - 1.0) * spread.y;
        }
        p
    }
}

impl LevelSpec {
    /// Build this level's world from `seed` — the deterministic interpreter. Construction
    /// order matches the old hand-written builders exactly: struct `i` seeds its interior
    /// with `seed + i`; each sub is added then immediately garrisoned; the reserve node comes
    /// last; lanes after all structs. `+-X` position noise is drawn here (one [`NoiseRng`]
    /// stream, file order). Same seed ⇒ the same world, bit for bit.
    pub fn build(&self, seed: u64) -> (World, WorldParams) {
        let mut w = World::new();
        let mut noise = NoiseRng::new(seed);
        for (i, sp) in self.structs.iter().enumerate() {
            let spos = noise.jitter(sp.pos, sp.pos_noise);
            let mut st = Interior::new(seed.wrapping_add(i as u64));
            for sub in &sp.subs {
                let pos = noise.jitter(sub.pos, sub.pos_noise);
                let mut s = match sub.kind {
                    SubKind::Standard => SubStructure::new(pos, 0.0, sub.owner),
                    SubKind::Fortress => SubStructure::fortress(pos, sub.owner),
                    SubKind::Teleporter => SubStructure::teleporter(pos, sub.owner),
                    SubKind::Shipyard { .. } => SubStructure::shipyard(pos, sub.owner),
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
            w.add_struct(Structure::new(st, spos, &sp.name));
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
        Orbit,
        Lane,
    }
    let mut sec = Section::None;

    // The ring constructor under construction ([orbit] — see the module doc). Members are
    // recorded as (sub index in the current struct, explicit angle in degrees if any) and
    // resolved to pos + orbit when the ring closes at the next section header (or EOF).
    struct Ring {
        opened_at: usize,
        center: Option<Vec2>,
        radius: Option<f32>,
        period: Option<f32>,
        members: Vec<(usize, Option<f32>)>,
    }
    let mut ring: Option<Ring> = None;

    fn close_ring(ring: &mut Option<Ring>, structs: &mut [StructSpec]) -> Result<(), String> {
        let Some(rg) = ring.take() else { return Ok(()) };
        let ln = rg.opened_at;
        let center = rg.center.ok_or(format!("line {ln}: [orbit] needs `center`"))?;
        let radius = rg.radius.ok_or(format!("line {ln}: [orbit] needs `radius`"))?;
        let period = rg.period.ok_or(format!("line {ln}: [orbit] needs `period`"))?;
        if rg.members.is_empty() {
            return Err(format!("line {ln}: [orbit] has no member [sub]s"));
        }
        let given = rg.members.iter().filter(|(_, a)| a.is_some()).count();
        if given != 0 && given != rg.members.len() {
            return Err(format!(
                "line {ln}: [orbit] members must ALL have an angle, or none (regular spacing)"
            ));
        }
        let st = structs.last_mut().expect("members imply a struct");
        let n = rg.members.len() as f32;
        for (k, (idx, angle)) in rg.members.into_iter().enumerate() {
            let deg = angle.unwrap_or(k as f32 * 360.0 / n);
            let rad = deg.to_radians();
            let sub = &mut st.subs[idx];
            sub.pos = Vec2::new(center.x + radius * rad.cos(), center.y + radius * rad.sin());
            sub.orbit = Some((center, period));
        }
        Ok(())
    }

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
    /// One number token, optionally noisy: `A` or `A+-X` → `(A, X)` with `X ≥ 0` (`X = 0` for
    /// the plain form). The uniform-noise syntax (owner ask, 2026-07-08).
    fn noisy_num(tok: &str, ln: usize) -> Result<(f32, f32), String> {
        match tok.split_once("+-") {
            Some((b, s)) => {
                let base = b
                    .parse::<f32>()
                    .map_err(|_| format!("line {ln}: bad number `{b}` in `{tok}`"))?;
                let spread = s
                    .parse::<f32>()
                    .map_err(|_| format!("line {ln}: bad noise width `{s}` in `{tok}`"))?;
                if spread < 0.0 {
                    return Err(format!("line {ln}: noise width must be ≥ 0, got `{tok}`"));
                }
                Ok((base, spread))
            }
            None => tok
                .parse::<f32>()
                .map(|n| (n, 0.0))
                .map_err(|_| format!("line {ln}: bad number `{tok}`")),
        }
    }
    /// A position value: two noisy tokens → `(base, per-component noise half-width)`.
    fn noisy_vec2(v: &str, ln: usize) -> Result<(Vec2, Vec2), String> {
        let mut it = v.split_whitespace();
        let (Some(xt), Some(yt), None) = (it.next(), it.next(), it.next()) else {
            return Err(format!("line {ln}: expected two numbers, got `{v}`"));
        };
        let (x, nx) = noisy_num(xt, ln)?;
        let (y, ny) = noisy_num(yt, ln)?;
        Ok((Vec2::new(x, y), Vec2::new(nx, ny)))
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
                close_ring(&mut ring, &mut structs)?;
                sec = Section::Level;
                continue;
            }
            "[struct]" => {
                flush_lane(&mut lane_between, &mut lane_length, &mut lanes, ln)?;
                close_ring(&mut ring, &mut structs)?;
                sec = Section::Struct;
                structs.push(StructSpec {
                    pos: Vec2::new(0.0, 0.0),
                    pos_noise: Vec2::new(0.0, 0.0),
                    name: String::new(),
                    storage_scale: None,
                    storage_capacity: None,
                    zoom_min: None,
                    zoom_max: None,
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
                    pos_noise: Vec2::new(0.0, 0.0),
                    kind: SubKind::Standard,
                    owner: Faction::Neutral,
                    cap: None,
                    prod: None,
                    ships: 0,
                    max_resistance: None,
                    keep_surplus: false,
                    orbit: None,
                });
                if let Some(rg) = ring.as_mut() {
                    rg.members.push((st.subs.len() - 1, None));
                }
                continue;
            }
            "[orbit]" => {
                flush_lane(&mut lane_between, &mut lane_length, &mut lanes, ln)?;
                close_ring(&mut ring, &mut structs)?;
                if structs.is_empty() {
                    return Err(format!("line {ln}: [orbit] before any [struct]"));
                }
                sec = Section::Orbit;
                ring = Some(Ring {
                    opened_at: ln,
                    center: None,
                    radius: None,
                    period: None,
                    members: Vec::new(),
                });
                continue;
            }
            "[lane]" => {
                flush_lane(&mut lane_between, &mut lane_length, &mut lanes, ln)?;
                close_ring(&mut ring, &mut structs)?;
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
                    "pos" => (st.pos, st.pos_noise) = noisy_vec2(v, ln)?,
                    "name" => st.name = v.to_string(),
                    "storage_scale" => st.storage_scale = Some(num(v, ln)?),
                    "storage_capacity" => st.storage_capacity = Some(num(v, ln)?),
                    "zoom_min" => st.zoom_min = Some(num(v, ln)?),
                    "zoom_max" => st.zoom_max = Some(num(v, ln)?),
                    other => return Err(format!("line {ln}: unknown [struct] key `{other}`")),
                }
            }
            Section::Sub => {
                let sub = structs
                    .last_mut()
                    .and_then(|s| s.subs.last_mut())
                    .expect("section guarantees a sub");
                match k {
                    "pos" if ring.is_some() => {
                        return Err(format!(
                            "line {ln}: ring members take `angle`, not `pos` (the [orbit] places them)"
                        ))
                    }
                    "orbit" if ring.is_some() => {
                        return Err(format!("line {ln}: the [orbit] section sets the orbit"))
                    }
                    "angle" => match ring.as_mut() {
                        Some(rg) => {
                            rg.members.last_mut().expect("member registered at [sub]").1 =
                                Some(num(v, ln)?)
                        }
                        None => {
                            return Err(format!(
                                "line {ln}: `angle` only applies to [orbit] members"
                            ))
                        }
                    },
                    "pos" => (sub.pos, sub.pos_noise) = noisy_vec2(v, ln)?,
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
            Section::Orbit => {
                let rg = ring.as_mut().expect("section guarantees a ring");
                match k {
                    "center" => rg.center = Some(vec2(v, ln)?),
                    "radius" => {
                        let r: f32 = num(v, ln)?;
                        if r <= 0.0 {
                            return Err(format!("line {ln}: radius must be positive"));
                        }
                        rg.radius = Some(r);
                    }
                    "period" => {
                        let t: f32 = num(v, ln)?;
                        if t == 0.0 {
                            return Err(format!("line {ln}: period must be non-zero"));
                        }
                        rg.period = Some(t);
                    }
                    other => return Err(format!("line {ln}: unknown [orbit] key `{other}`")),
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
    close_ring(&mut ring, &mut structs)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "[level]\nid = 1\nstart = layer2\nhorizon = 100\n[struct]\npos = 0 0\n";

    fn sub(extra: &str) -> String {
        format!("[sub]\n{extra}owner = neutral\n")
    }

    /// The ring constructor's contract: members get pos on the circle + the ring's orbit;
    /// no angles means regular spacing in file order starting at 0° (counter-clockwise).
    #[test]
    fn orbit_places_members_regularly_when_no_angles_given() {
        let text = format!("{HEAD}[orbit]\ncenter = 10 0\nradius = 4\nperiod = -1500\n{}{}{}{}", sub(""), sub(""), sub(""), sub(""));
        let sp = parse(&text).expect("parses");
        let subs = &sp.structs[0].subs;
        let want = [(14.0, 0.0), (10.0, 4.0), (6.0, 0.0), (10.0, -4.0)]; // 0°, 90°, 180°, 270°
        assert_eq!(subs.len(), 4);
        for (s, &(x, y)) in subs.iter().zip(&want) {
            assert!((s.pos.x - x).abs() < 1e-4 && (s.pos.y - y).abs() < 1e-4, "got {:?}", s.pos);
            let (c, t) = s.orbit.expect("ring sets the orbit");
            assert_eq!((c.x, c.y, t), (10.0, 0.0, -1500.0));
        }
    }

    /// The `+-X` noise contract: parsed into (base, spread); drawn at build within bounds;
    /// the same seed rebuilds the same layout bit for bit (replay/validation determinism);
    /// different seeds may (and here do) differ; plain numbers stay exact.
    #[test]
    fn position_noise_is_bounded_and_seed_deterministic() {
        let text = format!(
            "{HEAD}[sub]\npos = 10+-2 -5+-1\nowner = neutral\n[sub]\npos = 3 4\nowner = neutral\n"
        );
        let sp = parse(&text).expect("parses");
        let s0 = &sp.structs[0].subs[0];
        assert_eq!((s0.pos.x, s0.pos.y), (10.0, -5.0));
        assert_eq!((s0.pos_noise.x, s0.pos_noise.y), (2.0, 1.0));

        let mut layouts = Vec::new();
        for seed in 0..8u64 {
            let (w, _) = sp.build(seed);
            let p = w.structs[0].interior.subs[0].pos;
            assert!((p.x - 10.0).abs() <= 2.0 && (p.y + 5.0).abs() <= 1.0, "out of bounds: {p:?}");
            let exact = w.structs[0].interior.subs[1].pos;
            assert_eq!((exact.x, exact.y), (3.0, 4.0), "a plain pos is never jittered");
            // Same seed => the same draw, bit for bit.
            let (w2, _) = sp.build(seed);
            let p2 = w2.structs[0].interior.subs[0].pos;
            assert_eq!((p.x, p.y), (p2.x, p2.y), "seed {seed} must rebuild identically");
            layouts.push((p.x, p.y));
        }
        layouts.dedup();
        assert!(layouts.len() > 1, "eight seeds all drew the same layout: {layouts:?}");

        let bad = format!("{HEAD}[sub]\npos = 10+--2 4\nowner = neutral\n");
        assert!(parse(&bad).is_err(), "a malformed noise width must be rejected");
    }

    #[test]
    fn orbit_honours_explicit_angles_and_rejects_mixed_or_positioned_members() {
        let text = format!("{HEAD}[orbit]\ncenter = 0 0\nradius = 2\nperiod = 500\n{}{}", sub("angle = 90\n"), sub("angle = 270\n"));
        let sp = parse(&text).expect("parses");
        let subs = &sp.structs[0].subs;
        assert!((subs[0].pos.y - 2.0).abs() < 1e-4 && subs[0].pos.x.abs() < 1e-4);
        assert!((subs[1].pos.y + 2.0).abs() < 1e-4);

        let mixed = format!("{HEAD}[orbit]\ncenter = 0 0\nradius = 2\nperiod = 500\n{}{}", sub("angle = 90\n"), sub(""));
        assert!(parse(&mixed).unwrap_err().contains("ALL"), "mixed angles must be rejected");

        let positioned = format!("{HEAD}[orbit]\ncenter = 0 0\nradius = 2\nperiod = 500\n{}", sub("pos = 1 1\n"));
        assert!(parse(&positioned).unwrap_err().contains("angle"), "pos in a member must be rejected");
    }
}
