# Changelog

## 0.3.0
- **Bred opponents.** New `Evolved` brain (`ai::evolved`): a 15-dial policy. `crates/ai/examples/evolve.rs`
  breeds it by (mu + lambda) evolution against the roster and its own earlier champions, on five symmetric
  arena boards (`ai::arena`); five frozen lineages are shipped in `ai::lineages` (`enemy = evolved N`).
  `crates/ai/examples/bench.rs` prints the round-robin. Write-up and the generation log: `docs/evolution.md`.
- **Arc 3 "Lineage"** (five missions: First Generation, Selection Pressure, Recombination, Crossover,
  Convergence) fielding lineages 0-4; the finale is a three-way free-for-all against 3 and 4.
- **Arc 2 grows** to three missions (Tell, Decoy, Crossed Wires); **arc 0** gains Tempo (partial sends,
  time controls) with in-game captions (`EventAction::Caption`).
- **Sound**: every effect and a drone are synthesised at startup (`game::sfx`); `F4` mutes (a rebindable action). On for Windows,
  macOS and the browser; on Linux opt in with `--features sound` (the audio backend aborts without a
  device); `--nosound` skips it.
- **UI**: HUD names the adversary; the pause screen shows objective, tips and adversary description;
  capture rings; arc titles in the level select.
- **Copy**: every placeholder blurb/objective/hint replaced; pre/post briefings for missions 2-7 and all
  new ones; `docs/lore.md` records voices and threads.
- **Tools**: `crates/levels/examples/measure.rs` plays a stand-in player against a level's enemies.
- Opportunist counts its cooldown in decisions (the game's tick rate is 24x the reference).
- **Tests:** 190 passing (briefing rendering, ASCII/placeholder copy, sound synthesis, brains, lineage order).

## 0.2.0
- The Opportunist brain and the mission Tell; placeholder copy and briefings for missions 1-7.
