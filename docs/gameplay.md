# Playing the game

How to play, what the controls are, and how to read the board. For the rules underneath — how
capture and combat actually resolve — see [simulation.md](simulation.md).

## The goal

Each mission is a single structure of **sub-structures** that produce ships. You command the
**cyan** faction; the enemy is amber (a second same-kind enemy gets its own shade), and neutral
sites are grey. You win by **eliminating** every enemy — leaving them with no ships and no
sub-structures — before the match horizon. Capturing and holding ground grows your production,
which grows your fleet: the snowball is the game.

## Getting started

From the main menu, **Play** continues at your highest unlocked level; **Level Select** lists the
campaign (only unlocked levels are playable). Beating a level unlocks the next; progress is saved
next to the executable.

The match starts live, with a brief grace period to read the board (a count-up clock runs
top-right). Missions play out on one board under a freely zoomable, pannable camera.

## Controls

Keys are rebindable in Settings; these are the defaults.

**Commanding ships**
- **Left-click** one of your subs to select it, then **left-click a target sub** to send ships
  there (or **drag** from source to target). Orders move only *your* idle ships, and only idle
  ships move — a ship already flying is committed.
- **Left-drag a box** to multi-select every sub you command inside it; the next click orders them
  all at once. **Ctrl+drag** adds to (or toggles) the current selection.
- The top-bar **Send slider** sets what fraction of a source is sent (default 100%); snap it with
  **`1` / `2` / `3` / `4`** = 25 / 50 / 75 / 100%.
- **Right-click** clears the current selection.

**Camera**
- **Mouse-wheel** (or the right-side zoom slider) zooms; **right-drag** pans. Zoom and pan are
  unbounded — the view opens up empty space wherever you roam.

**Time**
- The top-bar **speed slider** steps **1× / 3× / 10× / 25×**; **`-` / `=`** step it too.
- **`Space`** toggles 1× ⇄ your last faster speed. **`Esc`** or **`P`** opens the pause menu
  (Resume / Restart / Main Menu); the sim freezes but the camera stays free.
- **`F3`** toggles a frame-timing overlay.

## Reading the board

Everything you see is the real simulation state — ship positions *are* the combat geometry.
Capture is a grind, and the board shows every part of it:

- **Ships** orbit their home sub as dots; a moving ship is a travelling triangle. Each present
  faction's **garrison count** is shown in its own colour (the owner's reads as `count / capacity`).
- **A resistance bar** under a sub is its capture meter. As an attacker grinds it down, the
  drained slice fills in the **attacker's colour** and the outline pulses; a sub being captured
  also wears a pulsing ring. When the owner sits alone and repairs it, the outline turns
  **green**. At zero the sub flips.
- **Production squares** mark where a sub mints ships; a sub being eroded undefended **stops
  producing** (its output is denied before it is even captured), so watch the squares go quiet.
- **Combat** reads through ship-death flashes rather than explicit battle bubbles.

The practical consequences: **concentrate force** (twice the ships is roughly four times the
power), **hold what you take** (a returning defender heals the bar, so hit-and-run is wasted),
and **park on an enemy sub to starve it** even when you can't yet capture it.

## The campaign

Seven hand-authored missions, in order. Difficulty is tuned by hand per level, not by a formula.

| # | Title | Opponent(s) | The idea |
|---|---|---|---|
| 1 | First Steps | Passive | Move ships, capture an unguarded site — the basic controls. |
| 2 | Fire in the Sky | Simple | Concentration of force; the contested middle decides it. |
| 3 | Command and Control | Cycler | Fleet command against a drillmaster that cycles and masses its ships. |
| 4 | The Sinews of War | Simple | Economy — out-produce before you out-fight. |
| 5 | Head of the Snake | Simple | A fortress line with a soft target behind it; decapitate rather than grind. |
| 6 | Deliberation | Simple × 2 | A three-way free-for-all; let your rivals spend themselves on each other. |
| 7 | Far Far Away | Simple + a passive watcher | Distance and expansion across a wide board. |

The **enemies** you face are the real product of the project — see
[architecture.md](architecture.md) for the roster of AI brains.
