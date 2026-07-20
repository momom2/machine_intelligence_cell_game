//! THE replay pin: a recorded order journal, fed back at its ticks on the same build +
//! level + seed, reproduces the match **bit-for-bit** — the property the whole replay
//! system (player replays, web replay collection) rests on. Pure-L1 (owner pivot,
//! 2026-07-20): the match is ONE interior; the journal records count-canonical interior
//! moves only. The recording run drives the live enemy seats (the stateful Simple); the
//! replay run drives NO AI at all — every seat's orders come from the journal verbatim —
//! and the per-tick `state_hash` trace must match exactly.

use ai::harness::GAME_DECISION_BASE;
use ai::SeatController;
use layer1::{Faction, OrderRecord, SimParams};
use std::cell::RefCell;
use std::rc::Rc;

/// Feed one journal record back into the interior (the playback atom).
fn apply_record(st: &mut layer1::Interior, r: &OrderRecord) {
    match *r {
        OrderRecord::Move { source, target, count, faction, .. } => {
            st.issue_order_count(source, target, count, faction);
        }
        // Fleet records no longer exist in pure-L1 recordings; ignore defensively.
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

#[test]
fn recorded_journal_replays_bit_identically() {
    // Far Far Away: the ring board with the ADJACENT Simple + a Passive second seat —
    // exercises the stateful campaign brain plus a scripted player order.
    let lvl = levels::campaign()
        .into_iter()
        .find(|l| l.id == 7)
        .expect("campaign has Far Far Away");
    let params = SimParams::default();
    let seed = 11u64;
    let horizon = 900u64;

    // --- RECORD: live seats + a scripted player move, journal installed. ---
    let mut w = lvl.interior(seed);
    let journal: layer1::OrderJournal = Rc::new(RefCell::new(Vec::new()));
    w.journal = Some(journal.clone());
    let mut seats: Vec<SeatController> = lvl
        .enemies
        .iter()
        .enumerate()
        .map(|(i, &r)| SeatController::from_roster(Faction::Ai(i as u8), r))
        .collect();
    let mut hashes = Vec::with_capacity(horizon as usize);
    for t in 0..horizon {
        if t == 300 {
            // The scripted player action: an ordinary count-canonical interior move.
            let n = w.idle_count_at(0, Faction::Player).min(20);
            w.issue_order_count(0, 1, n, Faction::Player);
        }
        if t % GAME_DECISION_BASE == 0 {
            for e in &mut seats {
                e.decide_and_apply(&mut w, &params);
            }
        }
        w.step(&params);
        hashes.push(w.state_hash());
    }
    let log: Vec<layer1::JournalEntry> = journal.borrow().clone();
    assert!(
        log.iter().any(|e| matches!(e.record, OrderRecord::Move { .. })),
        "the recording run must journal interior moves (the seats acted)"
    );

    // --- REPLAY: no AI anywhere — the journal alone drives every seat. ---
    let mut w2 = lvl.interior(seed);
    let mut cursor = 0usize;
    for t in 0..horizon {
        while cursor < log.len() && log[cursor].tick == t {
            apply_record(&mut w2, &log[cursor].record);
            cursor += 1;
        }
        w2.step(&params);
        assert_eq!(
            w2.state_hash(),
            hashes[t as usize],
            "replay diverged from the recording at tick {t}"
        );
    }
    assert_eq!(cursor, log.len(), "every journaled order must have been consumed");
}

/// The SCRUBBER's foundation stone: a cloned interior, resumed with the same remaining
/// orders, continues bit-identically — so playback can keep periodic snapshots and seek
/// to any tick by "restore nearest snapshot + fast-forward" with perfect fidelity.
#[test]
fn snapshot_resume_is_bit_exact() {
    let lvl = levels::campaign()
        .into_iter()
        .find(|l| l.id == 7)
        .expect("campaign has Far Far Away");
    let params = SimParams::default();
    let mut w = lvl.interior(23);
    let journal: layer1::OrderJournal = Rc::new(RefCell::new(Vec::new()));
    w.journal = Some(journal.clone());
    let mut seats: Vec<SeatController> = lvl
        .enemies
        .iter()
        .enumerate()
        .map(|(i, &r)| SeatController::from_roster(Faction::Ai(i as u8), r))
        .collect();
    // Record 600 ticks, cloning the interior at tick 300 (between ticks, like the scrubber).
    let mut fork: Option<layer1::Interior> = None;
    let mut hashes = Vec::new();
    for t in 0..600u64 {
        if t == 300 {
            let mut c = w.clone();
            c.journal = None; // playback copies never record
            fork = Some(c);
        }
        if t % GAME_DECISION_BASE == 0 {
            for e in &mut seats {
                e.decide_and_apply(&mut w, &params);
            }
        }
        w.step(&params);
        hashes.push(w.state_hash());
    }
    // Resume the fork with the journaled orders from tick 300 on: identical trace.
    let log: Vec<layer1::JournalEntry> = journal.borrow().clone();
    let mut w2 = fork.expect("forked at 300");
    let mut cursor = log.partition_point(|e| e.tick < 300);
    for t in 300..600u64 {
        while cursor < log.len() && log[cursor].tick == t {
            apply_record(&mut w2, &log[cursor].record);
            cursor += 1;
        }
        w2.step(&params);
        assert_eq!(
            w2.state_hash(),
            hashes[t as usize],
            "snapshot resume diverged at tick {t}"
        );
    }
}

/// The PERSISTED-snapshot pin: an interior round-tripped through the snap serializer is
/// not merely hash-equal at the restore point — resumed with the same remaining orders it
/// continues **bit-identically**. Strictly stronger than the clone pin above: it proves
/// the serializer captures everything evolution-relevant that `state_hash` itself does
/// not fold (the RNG stream, the cached pacing, the cross-tick derived flags).
#[test]
fn serialized_snapshot_resume_is_bit_exact() {
    let lvl = levels::campaign()
        .into_iter()
        .find(|l| l.id == 7)
        .expect("campaign has Far Far Away");
    let params = SimParams::default();
    let mut w = lvl.interior(41);
    let journal: layer1::OrderJournal = Rc::new(RefCell::new(Vec::new()));
    w.journal = Some(journal.clone());
    let mut seats: Vec<SeatController> = lvl
        .enemies
        .iter()
        .enumerate()
        .map(|(i, &r)| SeatController::from_roster(Faction::Ai(i as u8), r))
        .collect();
    let mut blob: Option<Vec<u8>> = None;
    let mut hash_at_300 = 0u64;
    let mut hashes = Vec::new();
    for t in 0..600u64 {
        if t == 300 {
            let mut c = w.clone();
            c.journal = None;
            let mut wr = layer1::snap::SnapWriter::new();
            c.snap_write(&mut wr);
            blob = Some(wr.buf);
            hash_at_300 = c.state_hash();
        }
        if t % GAME_DECISION_BASE == 0 {
            for e in &mut seats {
                e.decide_and_apply(&mut w, &params);
            }
        }
        w.step(&params);
        hashes.push(w.state_hash());
    }
    let blob = blob.expect("serialized at 300");
    let mut r = layer1::snap::SnapReader::new(&blob);
    let mut w2 = layer1::Interior::snap_read(&mut r).expect("blob deserializes");
    assert!(r.exhausted(), "the blob must be fully consumed");
    assert_eq!(w2.tick, 300, "restored interior resumes at the snapshot tick");
    assert_eq!(
        w2.state_hash(),
        hash_at_300,
        "restored interior must hash identically to its source"
    );
    let log: Vec<layer1::JournalEntry> = journal.borrow().clone();
    let mut cursor = log.partition_point(|e| e.tick < 300);
    for t in 300..600u64 {
        while cursor < log.len() && log[cursor].tick == t {
            apply_record(&mut w2, &log[cursor].record);
            cursor += 1;
        }
        w2.step(&params);
        assert_eq!(
            w2.state_hash(),
            hashes[t as usize],
            "serialized-snapshot resume diverged at tick {t}"
        );
    }
}
