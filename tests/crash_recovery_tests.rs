//! Crash-recovery integration test for `RedbStore` durability (P0-1 follow-up,
//! HANDOFF 2026-08-03 hourly resume).
//!
//! Substantiates the P0-1 "no-op claim" recorded in the 2026-08-02 audit:
//! redb 1.5.2's `WriteTransaction` defaults to `Durability::Immediate`
//! (every `commit()` fsyncs), so a host/process crash that happens AFTER a
//! committed batch and BEFORE the next batch cannot lose the committed rows,
//! and the vault file is still openable and writable on restart.
//!
//! Mechanism: spawn the `crash_writer` bin (a test aid, located via
//! `CARGO_BIN_EXE_crash_writer`), let it commit one event batch, then
//! `SIGKILL` it (`tokio` Child::kill sends SIGKILL on Unix) while it is
//! still alive and about to write more. Reopen the vault from this test
//! process and assert:
//!   1. The committed event(s) survived the abrupt kill.
//!   2. The run metadata (`put_run`) survived.
//!   3. The reopened vault is fully usable — accepts new commits and reads
//!      them back (proves the file is not corrupt / can be recovered).
//!
//! This is a genuine abrupt-termination test: the child is killed while
//! still running (its `RedbStore` is NOT dropped gracefully), so the
//! redb on-disk recovery path is exercised — not just a clean handle drop.

use pidag::{Event, RedbStore, RunMeta, Store};
use std::path::PathBuf;
use std::time::Duration;

/// Locate the `crash_writer` test-aid binary. `cargo test` builds every
/// `[[bin]]` of the package and sets `CARGO_BIN_EXE_<name>` for each.
fn crash_writer_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_crash_writer"))
}

#[tokio::test]
async fn test_vault_survives_sigkill_after_committed_batch() {
    let tmpdir = PathBuf::from("_tmp/phase3/test_crash_recovery_sigkill");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let vault = tmpdir.join("crash.redb");

    // Spawn the child. Inherit stdout via piped so we can read progress lines
    // (`COMMITTED <n>`) and know exactly when batch 1 has been fsync'd.
    let mut child = tokio::process::Command::new(crash_writer_bin())
        .arg(&vault)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn crash_writer");

    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdout = child.stdout.take().expect("crash_writer stdout piped");
    let mut reader = BufReader::new(stdout).lines();

    // Read committed-batch notifications until we see COMMITTED 1.
    // (We expect at least 1; the child may race ahead and commit more
    // before we kill it — that is fine, the assertions are >= 1.)
    let mut committed_count = 0usize;
    let read_deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let next = tokio::time::timeout_at(
            tokio::time::Instant::from_std(read_deadline),
            reader.next_line(),
        )
        .await;
        match next {
            Ok(Ok(Some(line))) => {
                if let Some(rest) = line.strip_prefix("COMMITTED ") {
                    committed_count = rest.trim().parse::<usize>().expect("COMMITTED <n> parse");
                    // We have at least one durable commit; proceed to kill.
                    if committed_count >= 1 {
                        break;
                    }
                }
                // Ignore any other lines (none expected, but stay robust).
            }
            Ok(Ok(None)) => {
                // Child stdout closed early without committing — fail loudly.
                child.wait().await.ok();
                panic!(
                    "crash_writer exited before committing any batch (commits seen: {committed_count})"
                );
            }
            Ok(Err(e)) => {
                panic!("crash_writer stdout read error: {e:?}");
            }
            Err(_elapsed) => {
                // Timeout: child never committed within 15s. Kill it and fail.
                let _ = child.kill().await;
                child.wait().await.ok();
                panic!("crash_writer did not commit within 15s (commits seen: {committed_count})");
            }
        }
    }

    // At least one batch is fsync'd on disk. SIGKILL the child while it is
    // still alive (it parks forever after its commits, so it is definitely
    // still running here). This exercises redb's abrupt-termination
    // recovery path — the child's `RedbStore` is NOT dropped gracefully.
    child.kill().await.expect("SIGKILL crash_writer");
    // Reap to avoid a zombie; ignore the exit status (SIGKILL -> not 0).
    let _ = child.wait().await;

    // Give the OS a brief moment to finalize the killed process and release
    // the file lock (redb uses an exclusive lock; reopening too soon on some
    // platforms races with the dying process releasing it).
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Reopen the vault from THIS process — this is the "restart" being
    // substantiated. If redb could not recover the file, open() errors.
    let store = RedbStore::open(&vault).expect("reopen vault after SIGKILL");

    // (1) Run metadata survived the kill.
    let run = store
        .get_run("crash-run")
        .await
        .expect("get_run after reopen");
    let run = run.expect("crash-run RunMeta must have survived SIGKILL");
    assert_eq!(run.run_id, "crash-run");

    // (2) Events survived. We committed at least 1; the child may have
    // committed up to 3 before we killed it. Assert >= 1 and that every
    // loaded event is well-formed (proves the event log is not torn).
    let events = store
        .load_events("crash-run")
        .await
        .expect("load_events after reopen");
    assert!(
        !events.is_empty(),
        "at least 1 committed event must survive SIGKILL; got 0"
    );
    // Every loaded event should be a NodeDispatched for n<digit>.
    for (i, ev) in events.iter().enumerate() {
        match ev {
            Event::NodeDispatched { node_id, model, .. } => {
                assert!(
                    node_id.starts_with('n'),
                    "event {i}: unexpected node_id {node_id}"
                );
                assert_eq!(model, "test-model", "event {i}: model drift");
            }
            other => panic!("event {i}: expected NodeDispatched, got {other:?}"),
        }
    }

    // (3) The reopened vault is fully usable — not just readable, but able
    // to accept a NEW committed write and read it back. This proves the
    // file is not corrupt (a torn redb file would refuse new writes).
    let new_ev = Event::NodeDone {
        node_id: "post-restart".to_string(),
        model: "test-model".to_string(),
        output: "reopened-ok".to_string(),
    };
    let seq = store
        .append_event("crash-run", &new_ev)
        .await
        .expect("append_event after reopen");
    // Sequencing must continue past the recovered event count — proves the
    // per-run SEQ_TABLE (P0-2) was recovered, not reset to 0.
    assert_eq!(
        seq,
        events.len() as u64,
        "post-restart append must continue seq numbering from {} recovered events; got {seq}",
        events.len()
    );

    let reloaded = store
        .load_events("crash-run")
        .await
        .expect("load_events after post-restart append");
    assert_eq!(
        reloaded.len(),
        events.len() + 1,
        "reopened vault accepted new event"
    );
    // The last event must be the one we just appended.
    match reloaded.last() {
        Some(Event::NodeDone { node_id, .. }) => {
            assert_eq!(node_id, "post-restart");
        }
        other => panic!("last event after reopen-append: {other:?}"),
    }
}

#[tokio::test]
async fn test_vault_clean_restart_roundtrip() {
    // Companion to the SIGKILL test: a GRACEFUL restart (drop the store,
    // reopen the same file) must obviously also preserve data and
    // continue sequence numbering. This isolates "graceful restart"
    // semantics so the SIGKILL test's failure modes are unambiguous.
    let tmpdir = PathBuf::from("_tmp/phase3/test_crash_recovery_graceful");
    let _ = std::fs::remove_dir_all(&tmpdir);
    std::fs::create_dir_all(&tmpdir).unwrap();
    let vault = tmpdir.join("graceful.redb");

    {
        let store = RedbStore::open(&vault).unwrap();
        store
            .put_run(&RunMeta {
                run_id: "graceful-run".to_string(),
                dag_json: "{}".to_string(),
                started_at: "2026-08-03T00:00:00Z".to_string(),
                completed_at: None,
                successful_nodes: 0,
                failed_nodes: 0,
            })
            .await
            .unwrap();
        let _s0 = store
            .append_event("graceful-run", &Event::DagSubmitted)
            .await
            .unwrap();
        let s1 = store
            .append_event("graceful-run", &Event::DagSubmitted)
            .await
            .unwrap();
        assert_eq!(s1, 1);
    } // store dropped gracefully here

    let store = RedbStore::open(&vault).unwrap();
    let run = store.get_run("graceful-run").await.unwrap().unwrap();
    assert_eq!(run.run_id, "graceful-run");
    let events = store.load_events("graceful-run").await.unwrap();
    assert_eq!(events.len(), 2);

    // Seq continues from where the previous handle left off (SEQ_TABLE
    // persisted + recovered).
    let s2 = store
        .append_event("graceful-run", &Event::DagSubmitted)
        .await
        .unwrap();
    assert_eq!(s2, 2);
}
