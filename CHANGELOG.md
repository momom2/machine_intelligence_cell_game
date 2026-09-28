# Changelog

## 0.2.0
- **AI: the Opportunist** (`crates/ai/src/opportunist.rs`, `enemy = opportunist`). Plays Simple v1 for its economy and
  adds a read of the opponent: it tracks the share of the opponent's fleet in flight and strikes any sub whose
  garrison (idle + inbound) it can beat at a ratio that falls from 1.8x to 1.0x as that share rises. Snapshot judgement,
  cooldown, home garrison floor. 6 unit tests.
- **New mission: Tell** (arc 2, index 0, id 9) fielding it.
- **Lore/copy:** every placeholder blurb/objective/hint filled; pre- and post-battle briefings written for missions
  2-7 and Tell (the humans + Blotto framing; foreshadows the evolution cluster). Old mislabeled pre-briefings replaced.
- **Tests:** briefings render cleanly for every level and battle result; no placeholder copy may ship. The
  "exactly 8 levels" assertion became "at least 9". Tests: 176 passing.
- Docs: campaign table and roster updated.
