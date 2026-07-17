//! THE replay pin: a recorded order journal, fed back at its ticks on the same build +
//! level + seed, reproduces the match **bit-for-bit** — the property the whole replay
//! system (player replays, web replay collection) rests on. The recording run drives the
//! live enemy seats (the stateful Simple) plus a scripted player fleet order; the replay
//! run drives NO AI at all — every seat's orders come from the journal verbatim — and the
//! per-tick `state_hash` trace must match exactly.

use ai::harness::GAME_DECISION_BASE;
use ai::SeatController;
use layer1::{Faction, FractionBucket, SimParams};
use std::cell::RefCell;
use std::rc::Rc;
use world::FleetOrder;

#[test]
fn recorded_journal_replays_bit_identically() {
    // Far Far Away: two structs, a lane, an ADJACENT Simple and a Passive third seat —
    // exercises interior moves AND (via the scripted player order below) fleet records.
    let lvl = levels::campaign()
        .into_iter()
        .find(|l| l.id == 7)
        .expect("campaign has Far Far Away");
    let params = SimParams::default();
    let seed = 11u64;
    let horizon = 900u64;

    // --- RECORD: live seats + a scripted player fleet order, journal installed. ---
    let (mut w, wp) = lvl.world(seed);
    let journal: layer1::OrderJournal = Rc::new(RefCell::new(Vec::new()));
    w.set_journaling(Some(journal.clone()));
    let mut seats: Vec<SeatController> = lvl
        .enemies
        .iter()
        .enumerate()
        .map(|(i, &r)| SeatController::from_roster(Faction::Ai(i as u8), r))
        .collect();
    let mut hashes = Vec::with_capacity(horizon as usize);
    for t in 0..horizon {
        if t == 300 {
            // The scripted player action: an inter-struct fleet order (bucket form — it
            // must resolve to a count and land in the journal as the count-canonical atom).
            w.issue_fleet_order(FleetOrder::new(0, 1, FractionBucket::All), Faction::Player, &wp);
        }
        if t % GAME_DECISION_BASE == 0 {
            for e in &mut seats {
                e.decide_and_apply(&mut w, &params, &wp);
            }
        }
        w.step(&params, &wp);
        hashes.push(w.state_hash());
    }
    let log: Vec<layer1::JournalEntry> = journal.borrow().clone();
    assert!(
        log.iter().any(|e| matches!(e.record, layer1::OrderRecord::Move { .. })),
        "the recording run must journal interior moves (Simple acted)"
    );
    assert!(
        log.iter().any(|e| matches!(e.record, layer1::OrderRecord::Fleet { .. })),
        "the recording run must journal the scripted fleet order"
    );

    // --- REPLAY: no AI anywhere — the journal alone drives every seat. ---
    let (mut w2, wp2) = lvl.world(seed);
    let mut cursor = 0usize;
    for t in 0..horizon {
        while cursor < log.len() && log[cursor].tick == t {
            w2.apply_record(&log[cursor].record, &wp2);
            cursor += 1;
        }
        w2.step(&params, &wp2);
        assert_eq!(
            w2.state_hash(),
            hashes[t as usize],
            "replay diverged from the recording at tick {t}"
        );
    }
    assert_eq!(cursor, log.len(), "every journaled order must have been consumed");
}
