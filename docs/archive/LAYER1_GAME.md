# LAYER1_GAME — the real-time PLAYABLE Layer-1 renderer (the *spectacle* layer)

`crates/layer1-game` is the macroquad GUI over the headless [`layer1`](crates/layer1) spatial
sim. It honours the design's signature principle (`00-overview.md`, *decouple computation from
spectacle*): **all** model logic lives in the `layer1` library; this binary only **draws** the
structure/ships/battle-bubbles and turns human input into `MoveOrder`s through the *same* public
API the AI uses. It does not modify the model — it constructs a GUI-specific `SimParams`
operating point (data it owns) purely to pace and prettify a human match.

The human commands the **Player** seat (cyan/blue); the Layer-1 `Automaton` commands the
**Enemy** seat (red/orange). Neutral sub-structures are grey until captured.

---

## How to run (RELEASE)

> Build and run **release**. Freshly linked Windows *debug* exes can be blocked by Smart App
> Control with `os error 4551`; the release binary runs fine. (`cargo run` without `--release`
> may hit that block on this machine.)

```powershell
# From the repo root (a Cargo workspace; layer1-game is a default member).
cargo run -p layer1-game --release
```

Or run the built binary directly:

```powershell
cargo build -p layer1-game --release
.\target\release\layer1-game.exe
```

Optional flags / env (see **Modes** below): `--auto`, `--shot <path>`, `--seed <hex|dec>`.

---

## Controls

| Input | Action |
|---|---|
| **Left-click a Player sub** | Select it as the order **source** (bright pulsing outline; its idle count shows in the disk). |
| **Left-click another sub** (with a source selected) | Issue `MoveOrder(source → target, current fraction)`. Source stays selected for rapid repeats. |
| **Left-click-drag** Player sub → other sub | Same as select-then-click: press on the source, release on the target. |
| **Right-click** / **Esc** | Clear the current selection. |
| **1 / 2 / 3 / 4** | Set the fraction bucket to **25 / 50 / 75 / 100 %** (default = 50 %). Shown in the HUD as `send NN%`. |
| **Space** | Pause / resume. |
| **`-` or `[`** | Slow the sim down one speed step. |
| **`=`/`+` or `]`** | Speed the sim up one step. |
| **R** | Restart: rebuild the scenario with `seed += 1` (each game differs but stays reproducible). |

The bottom HUD line restates these in-game. Number-row, keypad `+`/`-`, and the bracket keys all
work for speed/fraction.

Only **idle** ships move (matching `Structure::issue_order`): a sub showing `0` idle cannot send
anything. Ships already in transit are not redirected ("commit, then it's flying").

---

## What's on screen

- **Top HUD bar**: `PLAYER` ships/subs (left) vs `ENEMY` ships/subs (right); centre shows
  `tick N | mm:ss clock | send NN% | <speed>x` (or `PAUSED`).
- **Sub-structures**: filled disks scaled to their metre radius, ringed in the owner's colour,
  with the **idle ship count** drawn large and centred, and a thin **production arc** filling
  clockwise toward the next spawn (owned subs only).
- **Ships**: small **triangles** pointed along their velocity while moving, **dots** when idle
  (drawn at their real, interpolated positions, clustered around their home sub). Dead ships are
  skipped.
- **Battle bubbles**: a translucent **pulsing** disk at each bubble's centre/radius with the
  per-side engaged counts (`P vs E`) above it, plus cheap cosmetic **tracer** flashes between
  nearby opposing ships. Because combat is proximity-based, you'll see bubbles span the gap
  between two close sub-structures (e.g. the keep and a forward post).
- **A faint metre grid** for spatial reference.
- **End banner**: centred `VICTORY` / `DEFEAT` / `DRAW` (Player perspective) with the reason
  (by elimination / by lead at horizon) and `press R to restart`.

---

## Pacing & tuning constants

All live at the top of `crates/layer1-game/src/main.rs`.

| Constant | Value | Meaning |
|---|---|---|
| `BASE_TICKS_PER_SEC` | **1.8** | Sim ticks per real second at **1x**. |
| `SPEED_STEPS` | `[0.5, 1, 1.5, 2, 3, 4, 6]` | Speed multipliers (start at **1x**). |
| `DECISION_INTERVAL` | **5** ticks | How often each seat's Automaton re-plans (same cadence as the headless runner). |
| `MAX_TICKS_PER_FRAME` | **6** | Cap on sim ticks advanced in one rendered frame (anti-spiral). |
| `gui_params().fire_prob` | **0.020** | Softened from the library default **0.035** so brawls last long enough to read (spectacle only). All other `SimParams` = library defaults (`R=7`, `production_period=18`, `ship_speed=1.4`, …). |

**Why so slow, and why interpolation?** An Automaton-vs-Automaton match on the sample structure
resolves in only **~50–170 ticks** (measured over 8 seeds), and that length is dominated by the
AI's flank/elimination *seam*, not by combat lethality — so lowering `fire_prob` barely lengthens
it. To land a watchable **~1–3 minute** match we step the sim slowly (≈1.8 t/s ⇒ ~90 ticks ≈ 50 s;
longer games run 60–100 s, and a human reacting extends that further), and we keep motion fluid by
**interpolating ship positions between ticks** on the render side. The sim's determinism
explicitly supports this (`LAYER1_SIM.md`: "the renderer interpolates positions between calls if
it wants sub-tick smooth"); we snapshot each ship's pre-step position and lerp by the leftover
fractional tick accumulator. Impatient players can hold higher speeds; 0.5x studies a brawl.

The camera **fits the structure's coordinate bounds to the window** (with a 70 px margin, room
left for the HUD) and is **recomputed every frame**, so window resizes just work.

---

## Modes (the same binary)

| Invocation | Behaviour |
|---|---|
| *(default)* | **Human** plays Player, Automaton plays Enemy. |
| `--auto` (or `LAYER1_AUTO=1`) | **Both** seats driven by the Automaton; hands-off demo, never exits. |
| `--shot <path>` (or `LAYER1_SHOT=<path>`) | **Verification mode**: both seats AI, race the sim to tick `SHOT_TARGET_TICK` (≈48, where a battle bubble is live), render, capture the framebuffer with `get_screen_data()` → `Image::export_png(<path>)`, print a line, and **exit cleanly**. Creates the parent dir if needed. |

Extra env knobs (mainly for verification):
- `LAYER1_SEED=<hex|dec>` / `--seed <…>` — starting seed (default `0xC0FFEE1234`).
- `LAYER1_SHOT_TICK=<N>` — override the `--shot` capture tick. A large value (e.g. `9999`)
  captures the **end-of-match banner** (the match ends first).

### Screenshot examples (produced on this machine)

```powershell
# Mid-battle frame (proves subs + ships + a live battle bubble):
cargo run -p layer1-game --release -- --shot target\layer1_shot.png
#   -> "[layer1-game] shot written: target\layer1_shot.png (1279x799)  tick=48"
#   -> PNG ~130 KB

# End-of-match banner (VICTORY/DEFEAT):
$env:LAYER1_SHOT_TICK="9999"; cargo run -p layer1-game --release -- --shot target\layer1_end.png
```

---

## Build / test / verify

```powershell
cargo build -p layer1-game --release   # the renderer (release; see Smart App Control note)
cargo build --workspace                # everything still compiles (incl. set-aside architect)
cargo test                             # default member tests stay green (layer1-game adds no tests)
```

The `layer1` library stays **dependency-free** (macroquad lives only in this crate). On Windows,
macroquad pulls a windowing/audio/image dependency tree (miniquad, image/png, glam, fontdue, …);
the first build downloads and compiles them (~40 s here).

---

## Caveats / honest notes

- **Release only** on this machine: debug exes can be blocked by Smart App Control (`os error
  4551`). The release binary opens its window and runs fine — verified (the `--shot` run wrote a
  133 KB PNG and exited 0; the captured frame shows sub-structures, moving/idle ships, and a
  `3 vs 1` battle bubble).
- **`panic = "abort"`** is set in the workspace `release` profile. `Image::export_png` *panics*
  on failure (it returns `()`), so a failed PNG export would abort the process rather than unwind.
  In practice the export succeeded; the printed confirmation line and a non-empty file are the
  success signal.
- **`--shot` clock** reads `00:00`: the on-screen mm:ss clock only advances during wall-clock
  paced play; the shot path steps directly to a target **tick** (shown in the HUD), so the clock
  stays at zero in a screenshot. This is cosmetic and intended.
- **Pacing is approximate.** Match length is inherently variable (the AI seam can end a game
  early); 1x is tuned for a ~1–3 min feel on typical seeds, but a one-sided game can finish in
  ~30–40 s and a back-and-forth one can exceed it. Use Space and the speed steps to taste.
- The renderer never mutates the sim. It reads `subs`, `ships` (skipping `!alive`),
  `battle_bubbles`, the counts, and `outcome`, and writes only through `issue_order`.
