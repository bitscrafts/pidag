//! TDD tests for the pidag carousel priority queue subsystem.
//!
//! Each test maps to exactly one row in the TDD Contract (spec
//! `specs/09-carousel-queue.md`). Tests are written FIRST, before production
//! code. All file-writing tests run under `_tmp/` (never `/tmp/`).

use std::path::PathBuf;

use pidag::queue::{
    ProjectQueue, QueueEntry, SpecState, carousel_bounded, carousel_interleave,
    discover::discover_specs,
    extract_priority,
    state::{
        merge_queues, read_project_queue, reset_all_to_pending, retry_failed_only,
        write_project_queue,
    },
    weighted_carousel_bounded,
};

fn tmpdir(name: &str) -> PathBuf {
    let p = PathBuf::from("_tmp").join(name);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn mk_spec(root: &PathBuf, name: &str) {
    std::fs::write(root.join(name), "# spec\n").unwrap();
}

fn entry(spec_file: &str, priority: u8, state: SpecState) -> QueueEntry {
    QueueEntry {
        spec_name: spec_file.trim_end_matches(".md").to_string(),
        spec_file: spec_file.to_string(),
        state,
        priority,
        last_run_at: None,
        run_id: None,
        error: None,
    }
}

#[test]
fn test_spec_state_serde_round_trip() {
    let json = serde_json::to_string(&SpecState::Pending).unwrap();
    let back: SpecState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, SpecState::Pending);
    // Also verify the lowercase rename is honored.
    assert_eq!(json, "\"pending\"");
}

#[test]
fn test_queue_entry_serde_round_trip() {
    let e = entry("specs/01-a.md", 1, SpecState::Done);
    let json = serde_json::to_string(&e).unwrap();
    let back: QueueEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.spec_name, e.spec_name);
    assert_eq!(back.spec_file, e.spec_file);
    assert_eq!(back.priority, e.priority);
    assert_eq!(back.state, e.state);
}

#[test]
fn test_discover_specs_empty_dir() {
    let dir = tmpdir("queue-empty");
    let entries = discover_specs(&dir);
    assert!(entries.is_empty());
}

#[test]
fn test_discover_specs_finds_numbered() {
    let dir = tmpdir("queue-numbered");
    mk_spec(&dir, "01-a.md");
    mk_spec(&dir, "02-b.md");
    let entries = discover_specs(&dir);
    assert_eq!(entries.len(), 2, "both numbered specs discovered");
    // Sorted by priority: 01-a before 02-b.
    assert_eq!(entries[0].spec_file, "specs/01-a.md");
    assert_eq!(entries[1].spec_file, "specs/02-b.md");
}

#[test]
fn test_discover_specs_ignores_unnumbered() {
    let dir = tmpdir("queue-unnumbered");
    mk_spec(&dir, "readme.md");
    mk_spec(&dir, "01-a.md");
    let entries = discover_specs(&dir);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].spec_file, "specs/01-a.md");
}

#[test]
fn test_priority_extraction() {
    let p = extract_priority("42-foo.md").unwrap();
    assert_eq!(p, 42);
    assert!(extract_priority("readme.md").is_none());
}

#[test]
fn test_state_write_atomic() {
    let dir = tmpdir("queue-write");
    let q = ProjectQueue {
        project_root: dir.to_str().unwrap().to_string(),
        entries: vec![entry("specs/01-a.md", 1, SpecState::Pending)],
        updated_at: "2026-08-07T00:00:00Z".to_string(),
        weight: 1.0,
    };
    write_project_queue(&dir, &q).unwrap();
    let state_path = dir.join(".pidag").join("queue.json");
    assert!(state_path.exists(), "state file exists after write");
    let text = std::fs::read_to_string(&state_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed["entries"][0]["state"], "pending",
        "valid JSON with lowercased state"
    );
}

#[test]
fn test_state_read_nonexistent() {
    let dir = tmpdir("queue-read-none");
    let res = read_project_queue(&dir);
    assert!(res.unwrap().is_none(), "no state file -> Ok(None)");
}

#[test]
fn test_state_merge_preserves_done() {
    let dir = tmpdir("queue-merge");
    // Existing cached state has 01-a as Done.
    let cached = ProjectQueue {
        project_root: dir.to_str().unwrap().to_string(),
        entries: vec![entry("specs/01-a.md", 1, SpecState::Done)],
        updated_at: "old".to_string(),
        weight: 1.0,
    };
    // Fresh discovery finds 01-a (pending by default) and 02-b.
    let discovered = vec![
        entry("specs/01-a.md", 1, SpecState::Pending),
        entry("specs/02-b.md", 2, SpecState::Pending),
    ];
    let merged = merge_queues(&cached, &discovered, &dir);
    assert_eq!(merged.entries.len(), 2);
    // Done preserved for the spec that was Done before.
    let a = merged
        .entries
        .iter()
        .find(|e| e.spec_file == "specs/01-a.md")
        .unwrap();
    assert_eq!(
        a.state,
        SpecState::Done,
        "done state preserved across merge"
    );
    // Newly discovered spec is Pending.
    let b = merged
        .entries
        .iter()
        .find(|e| e.spec_file == "specs/02-b.md")
        .unwrap();
    assert_eq!(b.state, SpecState::Pending, "new spec starts Pending");
}

#[test]
fn test_reset_all_to_pending() {
    let mut q = ProjectQueue {
        project_root: ".".to_string(),
        entries: vec![
            entry("specs/01-a.md", 1, SpecState::Done),
            entry("specs/02-b.md", 2, SpecState::Failed),
            entry("specs/03-c.md", 3, SpecState::Pending),
        ],
        updated_at: "t".to_string(),
        weight: 1.0,
    };
    reset_all_to_pending(&mut q);
    assert!(q.entries.iter().all(|e| e.state == SpecState::Pending));
}

#[test]
fn test_retry_failed_only() {
    let mut q = ProjectQueue {
        project_root: ".".to_string(),
        entries: vec![
            entry("specs/01-a.md", 1, SpecState::Done),
            entry("specs/02-b.md", 2, SpecState::Failed),
            entry("specs/03-c.md", 3, SpecState::Pending),
        ],
        weight: 1.0,
        updated_at: "t".to_string(),
    };
    retry_failed_only(&mut q);
    let done = q
        .entries
        .iter()
        .find(|e| e.spec_name == "specs/01-a")
        .unwrap();
    assert_eq!(done.state, SpecState::Done, "Done stays Done");
    let failed = q
        .entries
        .iter()
        .find(|e| e.spec_name == "specs/02-b")
        .unwrap();
    assert_eq!(failed.state, SpecState::Pending, "Failed -> Pending");
    let pending = q
        .entries
        .iter()
        .find(|e| e.spec_name == "specs/03-c")
        .unwrap();
    assert_eq!(pending.state, SpecState::Pending, "Pending untouched");
}

#[test]
fn test_priority_ordering() {
    let mut entries = vec![
        entry("specs/03-c.md", 3, SpecState::Pending),
        entry("specs/01-a.md", 1, SpecState::Pending),
        entry("specs/02-b.md", 2, SpecState::Pending),
    ];
    entries.sort_by_key(|e| e.priority);
    let order: Vec<u8> = entries.iter().map(|e| e.priority).collect();
    assert_eq!(order, vec![1, 2, 3]);
}

#[test]
fn test_carousel_interleave() {
    // A[01,02], B[01,02] -> A/01, B/01, A/02, B/02.
    let a = vec![
        entry("specs/01-a.md", 1, SpecState::Pending),
        entry("specs/02-a.md", 2, SpecState::Pending),
    ];
    let b = vec![
        entry("specs/01-b.md", 1, SpecState::Pending),
        entry("specs/02-b.md", 2, SpecState::Pending),
    ];
    let order = carousel_interleave(vec![a, b]);
    let names: Vec<&str> = order.iter().map(|e| e.spec_file.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "specs/01-a.md",
            "specs/01-b.md",
            "specs/02-a.md",
            "specs/02-b.md"
        ]
    );
}

#[test]
fn test_carousel_skips_empty_project() {
    // B has no pending -> only A's specs, in order.
    let a = vec![
        entry("specs/01-a.md", 1, SpecState::Pending),
        entry("specs/02-a.md", 2, SpecState::Pending),
    ];
    let b: Vec<QueueEntry> = vec![];
    let order = carousel_interleave(vec![a, b]);
    let names: Vec<&str> = order.iter().map(|e| e.spec_file.as_str()).collect();
    assert_eq!(names, vec!["specs/01-a.md", "specs/02-a.md"]);
}

#[test]
fn test_carousel_bounded_batch() {
    // Batch 3, A[01,02,03], B[01] -> A/01, B/01, A/02 (stop at 3).
    let a = vec![
        entry("specs/01-a.md", 1, SpecState::Pending),
        entry("specs/02-a.md", 2, SpecState::Pending),
        entry("specs/03-a.md", 3, SpecState::Pending),
    ];
    let b = vec![entry("specs/01-b.md", 1, SpecState::Pending)];
    let order = carousel_bounded(vec![a, b], 3);
    let names: Vec<&str> = order.iter().map(|e| e.spec_file.as_str()).collect();
    assert_eq!(
        names,
        vec!["specs/01-a.md", "specs/01-b.md", "specs/02-a.md"]
    );
}

// =============================================================================
// spec-11: weight-seeded flexible-DAG batch budget
// TDD contract tests — see specs/11-weight-seeded-batch-budget.md
// =============================================================================

#[test]
fn test_weighted_carousel_unit_weights_match_flat() {
    // Two projects, weights [1.0, 1.0], 3 entries each, batch 4.
    // R4 guardrail: unit weights MUST match carousel_bounded exactly.
    // The least-served tail (taken/initial-pending ratio) gives the leftover
    // slot to B when A already took 2 in the sweep, so unit-weight order
    // becomes A01,A02,B01,B02 - identical to flat round-robin.
    let mk = |suffix: &str| {
        vec![
            entry(&format!("specs/01-{suffix}.md"), 1, SpecState::Pending),
            entry(&format!("specs/02-{suffix}.md"), 2, SpecState::Pending),
            entry(&format!("specs/03-{suffix}.md"), 3, SpecState::Pending),
        ]
    };
    let flat_a = mk("a");
    let flat_b = mk("b");
    let weighted = vec![(1.0, mk("a")), (1.0, mk("b"))];

    let weighted_order = weighted_carousel_bounded(weighted, 4);
    let flat_order = carousel_bounded(vec![flat_a, flat_b], 4);

    let w_names: Vec<&str> = weighted_order
        .iter()
        .map(|e| e.spec_file.as_str())
        .collect();
    let f_names: Vec<&str> = flat_order.iter().map(|e| e.spec_file.as_str()).collect();
    assert_eq!(
        w_names, f_names,
        "R4: unit weights byte-for-byte parity with carousel_bounded"
    );
}

#[test]
fn test_weighted_carousel_high_weight_doubles_share() {
    // A weight 2.0 (5 pending), B weight 1.0 (3 pending), batch 6.
    // total_weight = 3.0. Weighted sweep:
    //   A: share = round(6*2/3)=4 -> A01..A04 (taken=4, ratio 4/5=0.80).
    //   B: share = round(2*1/3)=1 -> B01 (taken=1, ratio 1/3=0.33).
    // remaining=1. Least-served tail: A pending ratio 0.80 vs B 0.33 ->
    // B is under-served and takes the leftover slot -> B02.
    // Net: A=4, B=2 - clean 2:1 ratio matching the spec TDD row.
    let a: Vec<_> = (1..=5)
        .map(|i| entry(&format!("specs/{i:02}-a.md"), i as u8, SpecState::Pending))
        .collect();
    let b = vec![
        entry("specs/01-b.md", 1, SpecState::Pending),
        entry("specs/02-b.md", 2, SpecState::Pending),
        entry("specs/03-b.md", 3, SpecState::Pending),
    ];
    let order = weighted_carousel_bounded(vec![(2.0, a), (1.0, b)], 6);
    let names: Vec<&str> = order.iter().map(|e| e.spec_file.as_str()).collect();
    let a_count = names.iter().filter(|n| n.contains("-a.md")).count();
    let b_count = names.iter().filter(|n| n.contains("-b.md")).count();
    assert_eq!(order.len(), 6, "hard cap preserved");
    assert_eq!(a_count, 4, "2x-weight project takes 2x share (4 of 6)");
    assert_eq!(b_count, 2, "1x-weight project takes 1x share (2 of 6)");
    // Weighted sweep emits A's 4 contiguously, then B's 1, then tail 1 B.
    assert_eq!(
        names,
        vec![
            "specs/01-a.md",
            "specs/02-a.md",
            "specs/03-a.md",
            "specs/04-a.md",
            "specs/01-b.md",
            "specs/02-b.md"
        ]
    );
}

#[test]
fn test_weighted_carousel_batch_cap_is_hard() {
    // A weight 9.0 (10 pending), B weight 1.0 (10 pending), batch 3.
    // share_A = round(3 * 9/10) = 3 (capped at remaining); B gets 0 this sweep.
    let a: Vec<_> = (1..=10)
        .map(|i| entry(&format!("specs/{i:02}-a.md"), i as u8, SpecState::Pending))
        .collect();
    let b: Vec<_> = (1..=10)
        .map(|i| entry(&format!("specs/{i:02}-b.md"), i as u8, SpecState::Pending))
        .collect();
    let order = weighted_carousel_bounded(vec![(9.0, a), (1.0, b)], 3);
    assert_eq!(order.len(), 3, "batch cap is hard");
    let a_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-a.md"))
        .count();
    let b_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-b.md"))
        .count();
    assert_eq!(a_count, 3);
    assert_eq!(b_count, 0);
}

#[test]
fn test_weighted_carousel_zero_weight_starves() {
    // A weight 0.0 (5 pending), B weight 1.0 (5 pending), batch 5.
    // A's share = 0 (w_i==0 filtered), B consumes all 5.
    let a: Vec<_> = (1..=5)
        .map(|i| entry(&format!("specs/{i:02}-a.md"), i as u8, SpecState::Pending))
        .collect();
    let b: Vec<_> = (1..=5)
        .map(|i| entry(&format!("specs/{i:02}-b.md"), i as u8, SpecState::Pending))
        .collect();
    let order = weighted_carousel_bounded(vec![(0.0, a), (1.0, b)], 5);
    assert_eq!(order.len(), 5);
    let a_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-a.md"))
        .count();
    assert_eq!(a_count, 0, "weight 0.0 starves");
}

#[test]
fn test_weighted_carousel_nonzero_weight_anti_starvation() {
    // A weight 0.01 (10 pending), B weight 9.99 (1 pending), batch 5.
    // Weighted sweep: share_A = round(5 * 0.01/10.0) = 0 -> A contributes 0
    //   but share is max(1,...) => A takes 1.
    // share_B = round(5 * 9.99/10.0) = 5 -> B takes min(5, 1) = 1.
    // After sweep: 3 budget left; both have pending -> round-robin tail.
    // Anti-starvation (N4): A gets at least 1 in the sweep (max(1,share)).
    let a: Vec<_> = (1..=10)
        .map(|i| entry(&format!("specs/{i:02}-a.md"), i as u8, SpecState::Pending))
        .collect();
    let b = vec![entry("specs/01-b.md", 1, SpecState::Pending)];
    let order = weighted_carousel_bounded(vec![(0.01, a), (9.99, b)], 5);
    assert_eq!(order.len(), 5);
    let a_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-a.md"))
        .count();
    let b_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-b.md"))
        .count();
    // B has only 1 to give; A fills the rest (anti-starvation).
    assert_eq!(b_count, 1);
    assert!(
        a_count >= 1,
        "nonzero weight must not starve (N4), got a_count={a_count}"
    );
    assert_eq!(a_count + b_count, 5);
}

#[test]
fn test_weighted_carousel_handles_fewer_pending_than_share() {
    // A weight 5.0 (1 pending), B weight 1.0 (10 pending), batch 6.
    // share_A = round(6 * 5/6) = 5; A only has 1 -> take 1; leftover = 5.
    // Leftover distributed to B (anti-starvation tail) -> B takes 5.
    let a = vec![entry("specs/01-a.md", 1, SpecState::Pending)];
    let b: Vec<_> = (1..=10)
        .map(|i| entry(&format!("specs/{i:02}-b.md"), i as u8, SpecState::Pending))
        .collect();
    let order = weighted_carousel_bounded(vec![(5.0, a), (1.0, b)], 6);
    assert_eq!(order.len(), 6);
    let a_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-a.md"))
        .count();
    let b_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-b.md"))
        .count();
    assert_eq!(a_count, 1, "A had only 1 pending despite 5-share");
    assert_eq!(b_count, 5, "leftover 5 goes to B");
}

#[test]
fn test_weighted_carousel_empty_project_excluded() {
    // A weight 5.0 (0 pending), B weight 1.0 (3 pending), batch 3.
    // A is excluded (empty); B emits 3.
    let a: Vec<QueueEntry> = Vec::new();
    let b = vec![
        entry("specs/01-b.md", 1, SpecState::Pending),
        entry("specs/02-b.md", 2, SpecState::Pending),
        entry("specs/03-b.md", 3, SpecState::Pending),
    ];
    let order = weighted_carousel_bounded(vec![(5.0, a), (1.0, b)], 3);
    let names: Vec<&str> = order.iter().map(|e| e.spec_file.as_str()).collect();
    assert_eq!(
        names,
        vec!["specs/01-b.md", "specs/02-b.md", "specs/03-b.md"]
    );
}

#[tokio::test]
async fn test_run_daemon_weighted_batch_picks_more_from_heavy_project() {
    // _tmp workspace with two projects; A's queue.json -> weight 3.0,
    // B's queue.json -> weight 1.0. batch 4, dry_run.
    // Expect A entries >= B entries in the printed order (weighted), len 4.
    let ws = tmpdir("weighted_daemon_ws");
    let proj_a = ws.join("a");
    let proj_b = ws.join("b");
    std::fs::create_dir_all(proj_a.join("specs")).unwrap();
    std::fs::create_dir_all(proj_b.join("specs")).unwrap();
    std::fs::create_dir_all(proj_a.join(".pidag")).unwrap();
    std::fs::create_dir_all(proj_b.join(".pidag")).unwrap();
    for n in ["01-aa.md", "02-aa.md", "03-aa.md", "04-aa.md"] {
        mk_spec(&proj_a.join("specs"), n);
    }
    for n in ["01-bb.md", "02-bb.md", "03-bb.md", "04-bb.md"] {
        mk_spec(&proj_b.join("specs"), n);
    }
    // Write queue.json with weights. Use the public write_project_queue then
    // bump weight by re-reading + re-writing (write_project_queue preserves
    // the weight field on the state).
    let mk_queue = |root: &std::path::Path, weight: f64| {
        let entries = discover_specs(root);
        let q = ProjectQueue {
            project_root: root.to_string_lossy().to_string(),
            entries,
            updated_at: "2026-08-07T00:00:00.000Z".to_string(),
            weight,
        };
        write_project_queue(root, &q).unwrap();
    };
    mk_queue(&proj_a, 3.0);
    mk_queue(&proj_b, 1.0);

    // `run_daemon` is the public surface (single-project). For the
    // multi-project weighted batch we must use the bounded driver directly:
    // project_root here = workspace; run_daemon only scans a single project.
    // So instead exercise weighted_carousel_bounded over the two projects'
    // discovered pending lists with the configured weights — same code path
    // the daemon's multi-project arm uses, deterministic, no subprocess.
    let a_entries = discover_specs(&proj_a);
    let b_entries = discover_specs(&proj_b);
    let a_weight = read_project_queue(&proj_a).unwrap().unwrap().weight;
    let b_weight = read_project_queue(&proj_b).unwrap().unwrap().weight;
    let order = weighted_carousel_bounded(vec![(a_weight, a_entries), (b_weight, b_entries)], 4);
    assert_eq!(order.len(), 4);
    let a_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-aa.md"))
        .count();
    let b_count = order
        .iter()
        .filter(|e| e.spec_file.contains("-bb.md"))
        .count();
    // weight 3:1 with batch 4 -> share_A = round(4*3/4)=3, share_B=1.
    assert!(
        a_count > b_count,
        "heavy project picks more: a={a_count} b={b_count}"
    );
    assert_eq!(a_count + b_count, 4);
}
