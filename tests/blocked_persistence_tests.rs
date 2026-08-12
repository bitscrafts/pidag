//! Blocked must survive a round trip through the vault.
//!
//! Found 2026-08-12 while generating a fixture, not by any test: a run reported
//! `gamma ⛔ Blocked` and `pidag show` read it back as `Pending`.
//!
//! Two linked defects. `RedbSink::emit` had no arm for `Event::NodeBlocked`, so
//! the state was never written; and `terminal_set` filtered to `Done | Failed`,
//! excluding Blocked even had it been written. Together they made
//! `load_checkpoint`'s `"Blocked" => blocked_nodes.insert(..)` branch **dead
//! code** — it could not execute. On resume, a node blocked by a failed
//! dependency was seen as Pending and re-dispatched.

use pidag::core::event::{Event, EventSink, RedbSink};
use pidag::store::{NodeStatus, RedbStore, Store};
use std::sync::Arc;

fn vault(name: &str) -> (Arc<RedbStore>, std::path::PathBuf) {
    let dir = std::path::PathBuf::from(format!("_tmp/blocked_persist/{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmpdir");
    let store = Arc::new(RedbStore::open(&dir.join("v.redb")).expect("vault"));
    (store, dir)
}

#[tokio::test]
async fn test_blocked_state_is_persisted() {
    let (store, dir) = vault("persisted");
    let mut sink = RedbSink::new(Arc::clone(&store) as Arc<dyn Store>, "r1".to_string());

    sink.emit(&Event::NodeBlocked {
        node_id: "gamma".to_string(),
    })
    .await
    .expect("emit");

    let nodes = store.list_nodes("r1").await.expect("list");
    let gamma = nodes.iter().find(|n| n.node_id == "gamma");
    assert!(
        gamma.is_some(),
        "NodeBlocked was not written to the vault at all"
    );
    assert_eq!(
        gamma.unwrap().state,
        NodeStatus::Blocked,
        "Blocked round-tripped as something else; `pidag show` reported Pending"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_terminal_set_reports_blocked() {
    let (store, dir) = vault("terminal");
    let mut sink = RedbSink::new(Arc::clone(&store) as Arc<dyn Store>, "r2".to_string());

    sink.emit(&Event::NodeBlocked {
        node_id: "gamma".to_string(),
    })
    .await
    .expect("emit");

    let terminal = store.terminal_set("r2").await.expect("terminal_set");
    assert!(
        terminal
            .iter()
            .any(|(id, st)| id == "gamma" && st == "Blocked"),
        "terminal_set omitted Blocked, which is what made load_checkpoint's \
         Blocked branch unreachable: {terminal:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
