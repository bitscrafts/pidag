//! Crash-recovery test aid for `RedbStore` durability (P0-1 follow-up,
//! HANDOFF 2026-08-03 hourly resume).
//!
//! Append-only child process: opens a redb vault at `argv[1]`, writes a
//! `RunMeta` and a sequence of `append_event` calls — each in its own
//! committed transaction (redb `Durability::Immediate` fsyncs on every
//! `commit()`). After each committed event it flushes a `COMMITTED <n>`
//! line to stdout so a parent test process can observe progress, then
//! parks forever (`sleep`) so the parent can `SIGKILL` it mid-stream —
//! modeling an abrupt host crash after batch 1 is durably committed but
//! before batch 2 would be written.
//!
//! Not a user-facing CLI; built only because `cargo test` builds every
//! `[[bin]]` of the package. Referenced from
//! `tests/crash_recovery_tests.rs` via `env!("CARGO_BIN_EXE_crash_writer")`,
//! mirroring the `mcp-adapter-rs` `mock-mcp-server` test-helper pattern.

use pidag::{Event, RedbStore, RunMeta, Store};
use std::path::PathBuf;

fn main() {
    // Minimal sync runtime — every `Store` method is async.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("crash_writer: build tokio runtime");

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("crash_writer: usage: crash_writer <vault_path>");
        std::process::exit(2);
    }
    let vault = PathBuf::from(&args[1]);

    rt.block_on(async move {
        let store = match RedbStore::open(&vault) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("crash_writer: open failed: {e:?}");
                std::process::exit(1);
            }
        };

        // Seed the run so per-run event appends are well-formed.
        let run = RunMeta {
            run_id: "crash-run".to_string(),
            dag_json: r#"{"nodes":[]}"#.to_string(),
            started_at: "2026-08-03T00:00:00Z".to_string(),
            completed_at: None,
            successful_nodes: 0,
            failed_nodes: 0,
        };
        if let Err(e) = store.put_run(&run).await {
            eprintln!("crash_writer: put_run failed: {e:?}");
            std::process::exit(1);
        }

        // Commit 3 events, each in its own fsync'd transaction. Flush a
        // notifier line after every commit so the parent can observe
        // exactly when durable data reaches disk.
        for i in 1..=3 {
            let ev = Event::NodeDispatched {
                node_id: format!("n{i}"),
                model: "test-model".to_string(),
                attempt: 1,
            };
            match store.append_event("crash-run", &ev).await {
                Ok(_seq) => {
                    println!("COMMITTED {i}");
                    // Force stdout to flush so the parent sees the line
                    // immediately (block buffering would hide progress).
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                Err(e) => {
                    eprintln!("crash_writer: append_event {i} failed: {e:?}");
                    std::process::exit(1);
                }
            }
        }

        // All intended batches committed and durably fsync'd. Park forever
        // so the parent (which observed `COMMITTED 1` and then SIGKILLed
        // us) is exercising the true "abrupt termination after some
        // committed batches" path. Even if the parent is slow and we
        // reach this park, the test still passes: it asserts >= 1 event
        // survived and the vault is usable after reopen.
        std::future::pending::<()>().await;
    });
}
