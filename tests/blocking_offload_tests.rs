//! spec-33 O1/O4: blocking work must not run on the async runtime.
//!
//! Every `Store` method is `async fn`, but `RedbStore` calls redb synchronously
//! and redb's `WriteTransaction` defaults to `Durability::Immediate`, so
//! `commit()` fsyncs before returning. Each call therefore parked a Tokio worker
//! thread on a disk sync. With one worker per core, once that many nodes were in
//! flight nothing was left to poll timers, serve the UI, or drive the event
//! consumer.

use pidag::store::Store;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// O1, stated as the property that matters: a store write must not stop the
/// runtime from making progress on anything else.
///
/// Runs on a **single-worker** runtime, which is the configuration that makes
/// the defect visible — with more workers a blocked one is masked by its peers.
/// A timer is armed, a batch of store writes is issued, and the timer must still
/// fire close to schedule. Before the change the writes held the only worker and
/// the timer was starved.
#[test]
fn test_store_writes_do_not_starve_the_runtime() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");

    rt.block_on(async {
        let dir = std::path::PathBuf::from("_tmp/blocking_offload/store_starve");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let store = Arc::new(pidag::store::RedbStore::open(&dir.join("v.redb")).expect("vault"));

        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = Arc::clone(&fired);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        // Enough fsyncing writes to hold a single worker for well over 50ms if
        // they run on it.
        let start = Instant::now();
        for i in 0..40 {
            let run = pidag::RunMeta {
                run_id: format!("r{i}"),
                dag_json: "{}".to_string(),
                started_at: "t".to_string(),
                completed_at: None,
                successful_nodes: 0,
                failed_nodes: 0,
            };
            let _ = store.put_run(&run).await;
        }
        let write_elapsed = start.elapsed();

        timer.await.expect("timer task");
        assert!(
            fired.load(std::sync::atomic::Ordering::SeqCst),
            "the timer never fired: {} store writes on a single-worker runtime \
             starved it (writes took {:?})",
            40,
            write_elapsed
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// O5, the guard that keeps the property from rotting. It is invisible in review:
/// a future `async fn` calling redb directly reintroduces the defect and nothing
/// about the code will look wrong.
///
/// The property is **not** "every async fn contains spawn_blocking" — that was the
/// first version of this test and it produced a false positive on `terminal_set`,
/// which does no redb work at all and simply delegates to `list_nodes`. What
/// matters is that no redb *primitive* is touched outside a blocking closure.
#[test]
fn test_no_bare_redb_call_in_async_store_methods() {
    const REDB_PRIMITIVES: &[&str] = &["begin_write(", "begin_read(", "open_table("];

    let src = std::fs::read_to_string("src/store/redb_store.rs").expect("read store");

    let mut offenders: Vec<String> = Vec::new();
    let mut fn_name = String::new();
    let mut in_async_fn = false;
    let mut blocking_depth: i32 = -1;
    let mut depth: i32 = 0;

    for line in src.lines() {
        let t = line.trim_start();

        if t.starts_with("async fn ") || t.starts_with("pub async fn ") {
            in_async_fn = true;
            blocking_depth = -1;
            depth = 0;
            fn_name = t
                .trim_start_matches("pub ")
                .trim_start_matches("async fn ")
                .split('(')
                .next()
                .unwrap_or("?")
                .to_string();
        }

        if in_async_fn {
            if t.contains("spawn_blocking") && blocking_depth < 0 {
                blocking_depth = depth;
            }
            // A redb primitive is only safe inside the blocking closure.
            if blocking_depth < 0 && REDB_PRIMITIVES.iter().any(|p| t.contains(p)) {
                let entry = fn_name.clone();
                if !offenders.contains(&entry) {
                    offenders.push(entry);
                }
            }
            depth += line.matches('{').count() as i32;
            depth -= line.matches('}').count() as i32;
            if depth <= 0 {
                in_async_fn = false;
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these async store methods touch redb outside a blocking closure, so each \
         parks a runtime worker thread on an fsync: {:?}",
        offenders
    );
}
