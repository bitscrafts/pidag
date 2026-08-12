//! TDD contract tests for spec-36: vault schema versioning and v1 migration.
//!
//! Fixture-based tests reuse `tests/fixtures/legacy_vault/legacy.redb` (the
//! genuine pre-spec-34 blob, hash-pinned by `test_fixture_hash_is_pinned`
//! below) by copying it into `_tmp/` first -- never opening it in place,
//! since `RedbStore::open` is read-write and would leave it dirty in git.
//!
//! The other tests build small synthetic v1-shaped vaults directly with
//! `redb`, bypassing `Store` entirely (which only ever writes the *current*
//! record shape). This is deliberate: it is the only way to test version
//! detection, atomicity, and the unknown-status-abort path without touching
//! the one genuine legacy artifact that exists (spec-36 G4).

use pidag::core::event::Event;
use pidag::store::legacy::NodeRecordV1;
use pidag::store::{NodeStatus, RedbStore, RunMeta, Store};
use redb::{Database, ReadableTable, TableDefinition};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const RUNS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("runs");
const NODES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");
const EVENTS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("events");
const SEQ_TABLE: TableDefinition<&str, &[u8; 8]> = TableDefinition::new("event_seq");
const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

/// A fresh, empty `_tmp/vault_migration_tests/<name>/` directory for this test.
fn work_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("_tmp")
        .join("vault_migration_tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create work dir");
    dir
}

/// Path to the committed fixture (never opened directly -- see module docs).
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_vault/legacy.redb")
}

/// Copy the committed fixture into `dest`, leaving the original untouched.
fn copy_fixture_into(dest: &Path) {
    std::fs::copy(fixture_path(), dest).expect("copy legacy fixture");
}

/// Write a raw v1-shaped vault directly via `redb`, bypassing `Store`
/// (which always writes the *current* record shape). Mirrors exactly what a
/// pre-spec-34 build wrote: no `meta` table, `NodeRecordV1` (plain `String`
/// state) in `nodes`. `events` uses the current `Event` type directly --
/// spec-36's V6 withdrawal established that spec-34 never changed the events
/// wire format, so a v1-era `events` table is byte-for-byte what `Event`
/// serializes today; there is no separate `EventV1` shape to mirror.
fn write_v1_vault(path: &Path, run_id: &str, nodes: &[NodeRecordV1], events: &[Event]) {
    let db = Database::create(path).expect("create v1 db");
    let write_txn = db.begin_write().expect("begin write");
    {
        let mut runs = write_txn.open_table(RUNS_TABLE).expect("open runs");
        let run = RunMeta {
            run_id: run_id.to_string(),
            dag_json: "{}".to_string(),
            started_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: None,
            successful_nodes: 0,
            failed_nodes: 0,
        };
        runs.insert(run_id, bincode::serialize(&run).unwrap().as_slice())
            .expect("insert run");

        let mut nodes_table = write_txn.open_table(NODES_TABLE).expect("open nodes");
        for n in nodes {
            let key = format!("{}\x00{}", run_id, n.node_id);
            nodes_table
                .insert(key.as_str(), bincode::serialize(n).unwrap().as_slice())
                .expect("insert node");
        }

        let mut events_table = write_txn.open_table(EVENTS_TABLE).expect("open events");
        for (i, e) in events.iter().enumerate() {
            let key = format!("{}\x00{:020}", run_id, i);
            events_table
                .insert(key.as_str(), bincode::serialize(e).unwrap().as_slice())
                .expect("insert event");
        }
        let mut seq = write_txn.open_table(SEQ_TABLE).expect("open seq");
        seq.insert(run_id, &(events.len() as u64).to_be_bytes())
            .expect("insert seq");
    }
    write_txn.commit().expect("commit v1 vault");
}

/// Stamp an arbitrary `schema_version` directly (used to construct a
/// too-new vault for V3c). Bypasses the migration entirely.
fn stamp_schema_version(path: &Path, version: u32) {
    let db = Database::open(path).expect("open db to stamp version");
    let write_txn = db.begin_write().expect("begin write");
    {
        let mut meta = write_txn.open_table(META_TABLE).expect("open meta");
        meta.insert(
            "schema_version",
            bincode::serialize(&version).unwrap().as_slice(),
        )
        .expect("insert schema_version");
    }
    write_txn.commit().expect("commit stamp");
}

/// Read `meta/schema_version` directly, without going through `RedbStore`.
/// Returns `None` if the `meta` table (or the key within it) is absent.
fn read_schema_version_raw(path: &Path) -> Option<u32> {
    let db = Database::open(path).expect("open db to read version");
    let read_txn = db.begin_read().expect("begin read");
    let meta = read_txn.open_table(META_TABLE).ok()?;
    meta.get("schema_version")
        .expect("get schema_version")
        .map(|v| bincode::deserialize(v.value()).expect("decode schema_version"))
}

/// Read a single `nodes` record's raw bytes back as a `NodeRecordV1` --
/// i.e. assert it is STILL in the old (String state) shape. Used to prove
/// atomicity: an aborted migration must not have touched even the records
/// that would have converted cleanly.
fn read_node_raw_v1(path: &Path, run_id: &str, node_id: &str) -> NodeRecordV1 {
    let db = Database::open(path).expect("open db to read node");
    let read_txn = db.begin_read().expect("begin read");
    let nodes = read_txn.open_table(NODES_TABLE).expect("open nodes");
    let key = format!("{}\x00{}", run_id, node_id);
    let bytes = nodes
        .get(key.as_str())
        .expect("get node")
        .expect("node exists")
        .value()
        .to_vec();
    bincode::deserialize(&bytes).expect("decode as NodeRecordV1")
}

/// Snapshot every `events` table entry as raw (key, value) bytes, in
/// iteration order. Used to prove the migration issues no write to the
/// events table (spec-36 V6'): if this is byte-identical before and after
/// `RedbStore::open`, nothing in the table changed.
fn read_events_raw(path: &Path) -> Vec<(String, Vec<u8>)> {
    let db = Database::open(path).expect("open db to read events");
    let read_txn = db.begin_read().expect("begin read");
    let events = read_txn.open_table(EVENTS_TABLE).expect("open events");
    let mut out = Vec::new();
    for entry in events.iter().expect("iterate events") {
        let (k, v) = entry.expect("read entry");
        out.push((k.value().to_string(), v.value().to_vec()));
    }
    out
}

/// The comparable subset of `redb::TableStats` (which has no `PartialEq` of
/// its own). A second, storage-layer signal alongside `read_events_raw`
/// that the events table's on-disk B-tree was never touched: page counts /
/// tree height / stored & fragmented bytes are exactly what an insert or
/// remove -- even of already-identical bytes -- would be expected to move.
#[derive(Debug, PartialEq)]
struct EventsTableStats {
    tree_height: u32,
    leaf_pages: u64,
    branch_pages: u64,
    stored_bytes: u64,
    metadata_bytes: u64,
    fragmented_bytes: u64,
}

fn read_events_table_stats(path: &Path) -> EventsTableStats {
    let db = Database::open(path).expect("open db to read events stats");
    let read_txn = db.begin_read().expect("begin read");
    let events = read_txn.open_table(EVENTS_TABLE).expect("open events");
    let stats = events.stats().expect("events table stats");
    EventsTableStats {
        tree_height: stats.tree_height(),
        leaf_pages: stats.leaf_pages(),
        branch_pages: stats.branch_pages(),
        stored_bytes: stats.stored_bytes(),
        metadata_bytes: stats.metadata_bytes(),
        fragmented_bytes: stats.fragmented_bytes(),
    }
}

fn sha256_hex(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read file for hashing");
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------
// V1: test_new_vault_stamps_version_2
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_new_vault_stamps_version_2() {
    let dir = work_dir("new_vault_stamps_v2");
    let db_path = dir.join("vault.redb");

    {
        let _store = RedbStore::open(&db_path).expect("open fresh vault");
    } // drop before reading the file directly -- redb holds an exclusive lock

    let version = read_schema_version_raw(&db_path);
    assert_eq!(
        version,
        Some(2),
        "a freshly created vault must stamp meta/schema_version = 2"
    );
}

// ---------------------------------------------------------------------
// V2: test_absent_version_is_v1
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_absent_version_is_v1() {
    let dir = work_dir("absent_version_is_v1");
    let db_path = dir.join("vault.redb");

    // A hand-built v1 vault: base tables exist (as any pre-spec-36 build
    // would have left them), but there is no `meta` table at all.
    write_v1_vault(
        &db_path,
        "run1",
        &[NodeRecordV1 {
            node_id: "solo".to_string(),
            state: "Done".to_string(),
            model: None,
            attempt: 1,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
        }],
        &[],
    );
    assert_eq!(
        read_schema_version_raw(&db_path),
        None,
        "precondition: hand-built vault has no meta table yet"
    );

    // Opening it must recognise the absence as version 1 and migrate --
    // observable as the vault reading back at version 2 afterward, and the
    // node decoding cleanly as the current typed NodeRecord.
    let nodes = {
        let store = RedbStore::open(&db_path).expect("open v1 vault");
        store.list_nodes("run1").await.expect("list_nodes")
    }; // drop store before reading the file directly
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].state, NodeStatus::Done);

    assert_eq!(
        read_schema_version_raw(&db_path),
        Some(2),
        "absent meta/schema_version must be treated as v1 and migrated to v2"
    );
}

// ---------------------------------------------------------------------
// V3a: test_legacy_vault_still_loads is in tests/typed_state_tests.rs
// (already applied ahead of this spec; this is the acceptance test).
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// V3b: test_v2_vault_opens_without_writing
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_v2_vault_opens_without_writing() {
    let dir = work_dir("v2_opens_without_writing");
    let db_path = dir.join("vault.redb");

    // First open stamps version 2 (V1).
    {
        let _store = RedbStore::open(&db_path).expect("create + stamp v2");
    }
    assert_eq!(read_schema_version_raw(&db_path), Some(2));

    let hash_before = sha256_hex(&db_path);
    let mtime_before = std::fs::metadata(&db_path)
        .expect("metadata")
        .modified()
        .expect("mtime");

    // Second open: already v2, must not write at all.
    {
        let store = RedbStore::open(&db_path).expect("reopen v2 vault");
        // Exercise a read too, to be sure opening is actually usable.
        let _ = store.list_runs().await.expect("list_runs");
    } // drop before reading the file directly -- redb holds an exclusive lock

    let hash_after = sha256_hex(&db_path);
    let mtime_after = std::fs::metadata(&db_path)
        .expect("metadata")
        .modified()
        .expect("mtime");

    // Content is the meaningful signal: byte-identical means nothing was
    // written. (mtime is captured too, per the TDD contract, but redb's
    // `Database::open`/`create` touches the file's mtime on every open --
    // even a bare `redb::Database::open()` with zero transactions bumps it,
    // confirmed independently of this migration -- so mtime alone cannot
    // distinguish "opened" from "written" for this storage engine. Logged
    // below rather than asserted on, so a real content write still shows up
    // via the hash check.)
    assert_eq!(
        hash_before, hash_after,
        "opening an already-v2 vault must not modify its bytes"
    );
    println!(
        "mtime_before={:?} mtime_after={:?} (informational: see comment above)",
        mtime_before, mtime_after
    );
}

// ---------------------------------------------------------------------
// V3c: test_future_version_is_rejected
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_future_version_is_rejected() {
    let dir = work_dir("future_version_is_rejected");
    let db_path = dir.join("vault.redb");

    // Create a valid vault, then stamp an unsupported future version.
    {
        let _store = RedbStore::open(&db_path).expect("create vault");
    }
    stamp_schema_version(&db_path, 99);

    let hash_before = sha256_hex(&db_path);

    let result = RedbStore::open(&db_path);
    assert!(result.is_err(), "schema_version 99 must be rejected");
    let msg = format!("{}", result.err().unwrap());
    assert!(
        msg.contains("99"),
        "error must name the found version 99: {}",
        msg
    );
    assert!(
        msg.contains('2'),
        "error must name the supported version 2: {}",
        msg
    );

    let hash_after = sha256_hex(&db_path);
    assert_eq!(
        hash_before, hash_after,
        "rejecting a too-new vault must not write to it"
    );
}

// ---------------------------------------------------------------------
// V4: test_migration_is_atomic
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_migration_is_atomic() {
    let dir = work_dir("migration_is_atomic");
    let db_path = dir.join("vault.redb");

    // Two nodes that would convert cleanly, and one that cannot -- the
    // migration must fail before committing ANY of the three.
    write_v1_vault(
        &db_path,
        "run1",
        &[
            NodeRecordV1 {
                node_id: "alpha".to_string(),
                state: "Done".to_string(),
                model: None,
                attempt: 1,
                timestamp: "2026-01-01T00:00:01Z".to_string(),
            },
            NodeRecordV1 {
                node_id: "beta".to_string(),
                state: "Failed".to_string(),
                model: None,
                attempt: 1,
                timestamp: "2026-01-01T00:00:02Z".to_string(),
            },
            NodeRecordV1 {
                node_id: "zorp_node".to_string(),
                state: "Zorp".to_string(),
                model: None,
                attempt: 1,
                timestamp: "2026-01-01T00:00:03Z".to_string(),
            },
        ],
        &[],
    );

    let result = RedbStore::open(&db_path);
    assert!(
        result.is_err(),
        "migration must abort when any record has an unrecognised status"
    );

    // The vault must still read as version 1 -- no partial stamp.
    assert_eq!(
        read_schema_version_raw(&db_path),
        None,
        "an interrupted migration must leave the vault reading as v1 (no partial stamp)"
    );

    // Every record -- INCLUDING the ones that would have converted cleanly
    // -- must still be in the untouched v1 (String state) shape.
    let alpha = read_node_raw_v1(&db_path, "run1", "alpha");
    assert_eq!(alpha.state, "Done");
    let beta = read_node_raw_v1(&db_path, "run1", "beta");
    assert_eq!(beta.state, "Failed");

    // The vault is still cleanly openable as v1 by a plain v1 reader: a
    // second migration attempt (on a copy) must succeed once the bad
    // record is fixed, proving nothing was corrupted by the aborted try.
    let db_path2 = dir.join("vault2.redb");
    write_v1_vault(
        &db_path2,
        "run1",
        &[NodeRecordV1 {
            node_id: "alpha".to_string(),
            state: "Done".to_string(),
            model: None,
            attempt: 1,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
        }],
        &[],
    );
    let store = RedbStore::open(&db_path2).expect("clean v1 vault still migrates fine");
    let nodes = store.list_nodes("run1").await.expect("list_nodes");
    assert_eq!(nodes[0].state, NodeStatus::Done);
}

// ---------------------------------------------------------------------
// V5: test_node_fields_roundtrip
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_node_fields_roundtrip() {
    let dir = work_dir("node_fields_roundtrip");
    let db_path = dir.join("legacy.redb");
    copy_fixture_into(&db_path);

    let store = RedbStore::open(&db_path).expect("open + migrate fixture");
    let nodes = store.list_nodes("legacy1").await.expect("list_nodes");

    let alpha = nodes.iter().find(|n| n.node_id == "alpha").expect("alpha");
    assert_eq!(alpha.node_id, "alpha");
    assert_eq!(alpha.model, None);
    assert_eq!(alpha.attempt, 1);
    assert_eq!(alpha.timestamp, "2026-08-12T02:24:41.191472004+00:00");

    let beta = nodes.iter().find(|n| n.node_id == "beta").expect("beta");
    assert_eq!(beta.node_id, "beta");
    assert_eq!(beta.model, None);
    assert_eq!(beta.attempt, 1);
    assert_eq!(beta.timestamp, "2026-08-12T02:24:41.195904977+00:00");

    let gamma = nodes.iter().find(|n| n.node_id == "gamma").expect("gamma");
    assert_eq!(gamma.node_id, "gamma");
    assert_eq!(gamma.model, None);
    assert_eq!(gamma.attempt, 0);
    assert_eq!(gamma.timestamp, "2026-08-12T02:24:41.198276321+00:00");
}

// ---------------------------------------------------------------------
// V6'a: test_events_untouched_by_migration
// ---------------------------------------------------------------------
//
// spec-36's V6 withdrawal: spec-34 never changed the `Event` wire format --
// no persisted variant carries a `state` field -- so there is nothing to
// migrate in the events table. This test proves both halves of V6' against
// the genuine committed fixture: every event still decodes as the current
// `Event`, and the migration issues NO write to the events table at all.

#[tokio::test]
async fn test_events_untouched_by_migration() {
    let dir = work_dir("events_untouched_by_migration");
    let db_path = dir.join("legacy.redb");
    copy_fixture_into(&db_path);

    // Snapshot the events table's raw entries and storage stats BEFORE
    // migration -- read directly via redb, never through `RedbStore` (which
    // would trigger the very migration this test is trying to observe).
    let before_entries = read_events_raw(&db_path);
    let before_stats = read_events_table_stats(&db_path);
    assert!(
        !before_entries.is_empty(),
        "precondition: the fixture must actually have events for this test to mean anything"
    );

    // Every event must already decode as the CURRENT `Event` type, even
    // before migration -- this is the empirical claim the V6 withdrawal
    // rests on: the events wire format never changed.
    for (key, bytes) in &before_entries {
        let _: Event = bincode::deserialize(bytes)
            .unwrap_or_else(|e| panic!("event {:?} must decode as current Event: {}", key, e));
    }

    {
        let store = RedbStore::open(&db_path).expect("open + migrate fixture");
        // Exercise the read path too, through the typed Store API.
        let events = store.load_events("legacy1").await.expect("load_events");
        assert_eq!(events.len(), before_entries.len());
    } // drop before reading the file directly -- redb holds an exclusive lock

    // Confirm the vault actually migrated (nodes/meta DID change) -- so
    // "the events table didn't change" below is a real claim about a
    // migration that ran, not a vacuous one about a no-op open.
    assert_eq!(
        read_schema_version_raw(&db_path),
        Some(2),
        "precondition: the vault must actually have migrated to v2"
    );

    // The events table itself must be byte-for-byte identical: same keys,
    // same values, same order -- direct proof at the data level.
    let after_entries = read_events_raw(&db_path);
    assert_eq!(
        before_entries, after_entries,
        "migration must not alter a single byte of the events table"
    );

    // The storage layer agrees: no page-level churn in the events table's
    // B-tree (tree height / page counts / stored & fragmented bytes all
    // unchanged) -- the strongest signal available through redb's public
    // API that no insert/remove was ever issued against this table.
    let after_stats = read_events_table_stats(&db_path);
    assert_eq!(
        before_stats, after_stats,
        "events table storage stats must be unchanged -- no write was issued to it"
    );

    // And most directly of all: the migration's own source contains no
    // write call site for the events table. `migrate_v1_to_v2` only ever
    // opens EVENTS_TABLE to ensure it exists for a brand-new vault; it
    // never binds it mutably and never calls `.insert`/`.remove` on it.
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/store/redb_store.rs"),
    )
    .expect("read src/store/redb_store.rs");
    let fn_start = src
        .find("fn migrate_v1_to_v2")
        .expect("migrate_v1_to_v2 must exist");
    let fn_end = src[fn_start..]
        .find("\nfn ")
        .map(|off| fn_start + off)
        .unwrap_or(src.len());
    let fn_body = &src[fn_start..fn_end];
    assert!(
        !fn_body.contains("mut events"),
        "migrate_v1_to_v2 must never bind the events table mutably: {}",
        fn_body
    );
}

// ---------------------------------------------------------------------
// V6'b: test_event_seq_preserved
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_event_seq_preserved() {
    let dir = work_dir("event_seq_preserved");
    let db_path = dir.join("vault.redb");

    let v1_events: Vec<Event> = (0..20)
        .map(|i| Event::NodeRetry {
            node_id: format!("node{}", i),
            reason: format!("reason{}", i),
        })
        .collect();
    write_v1_vault(&db_path, "run1", &[], &v1_events);

    let store = RedbStore::open(&db_path).expect("open + migrate");
    let seqed = store
        .load_events_since("run1", 0)
        .await
        .expect("load_events_since");

    // since=0 excludes seq 0 itself (see `load_events_since` semantics), so
    // expect seq 1..=19 in order.
    assert_eq!(seqed.len(), 19);
    for (idx, (seq, ev)) in seqed.iter().enumerate() {
        let expected_seq = (idx + 1) as u64;
        assert_eq!(*seq, expected_seq, "seq must be preserved in order");
        match ev {
            Event::NodeRetry { node_id, .. } => {
                assert_eq!(node_id, &format!("node{}", expected_seq));
            }
            other => panic!("unexpected event variant: {:?}", other),
        }
    }

    // And explicitly from the start too.
    let all = store.load_events("run1").await.expect("load_events");
    assert_eq!(all.len(), 20);
    for (idx, ev) in all.iter().enumerate() {
        match ev {
            Event::NodeRetry { node_id, .. } => assert_eq!(node_id, &format!("node{}", idx)),
            other => panic!("unexpected event variant: {:?}", other),
        }
    }
}

// ---------------------------------------------------------------------
// V7: test_unknown_status_aborts
// ---------------------------------------------------------------------

#[tokio::test]
async fn test_unknown_status_aborts() {
    let dir = work_dir("unknown_status_aborts");
    let db_path = dir.join("vault.redb");

    write_v1_vault(
        &db_path,
        "my_run",
        &[NodeRecordV1 {
            node_id: "weird_node".to_string(),
            state: "Zorp".to_string(),
            model: None,
            attempt: 1,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
        }],
        &[],
    );

    let result = RedbStore::open(&db_path);
    let err = match result {
        Ok(_) => panic!("an unrecognised legacy status must abort the migration"),
        Err(e) => e,
    };
    let msg = format!("{}", err);

    assert!(msg.contains("my_run"), "error must name the run: {}", msg);
    assert!(
        msg.contains("weird_node"),
        "error must name the node: {}",
        msg
    );
    assert!(
        msg.contains("Zorp"),
        "error must name the offending status string: {}",
        msg
    );

    // And, crucially: not defaulted to Pending. Since the migration aborted,
    // there is no NodeRecord to inspect at all via the typed Store API --
    // the raw bytes on disk are still the untouched v1 String, never a
    // fabricated `NodeStatus::Pending`.
    let raw = read_node_raw_v1(&db_path, "my_run", "weird_node");
    assert_eq!(raw.state, "Zorp", "the offending record must be untouched");
}

// ---------------------------------------------------------------------
// V8a: test_fixture_hash_is_pinned
// ---------------------------------------------------------------------

#[test]
fn test_fixture_hash_is_pinned() {
    const PINNED_SHA256: &str = "0e321fefd51c5d4f80aad9cc4d436dc0e47a4c145a44a8aca2be9969d8ef20aa";
    let actual = sha256_hex(&fixture_path());
    assert_eq!(
        actual, PINNED_SHA256,
        "tests/fixtures/legacy_vault/legacy.redb must stay byte-identical to the genuine \
         pre-spec-34 blob -- if this fails, the fixture was regenerated (spec-36 G4)"
    );
}

// ---------------------------------------------------------------------
// V8b: test_generator_is_ignored
// ---------------------------------------------------------------------

#[test]
fn test_generator_is_ignored() {
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gen_legacy_vault.rs"),
    )
    .expect("read gen_legacy_vault.rs");

    // The #[ignore] attribute must appear directly above (modulo blank
    // lines/comments) the generator test function, so it never runs on a
    // plain `cargo test` and silently regenerates the fixture again.
    let fn_pos = src
        .find("async fn gen_legacy_vault_fixture")
        .expect("generator function must exist");
    let before_fn = &src[..fn_pos];
    let ignore_pos = before_fn
        .rfind("#[ignore]")
        .expect("#[ignore] must be present on the generator");
    let between = &before_fn[ignore_pos + "#[ignore]".len()..];
    assert!(
        between.trim().is_empty() || between.trim_start().starts_with("#["),
        "#[ignore] must sit directly on gen_legacy_vault_fixture, not on something else: {:?}",
        between
    );
}

// ---------------------------------------------------------------------
// V9: test_no_false_compat_claim
// ---------------------------------------------------------------------

#[test]
fn test_no_false_compat_claim() {
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/scheduler/mod.rs"),
    )
    .expect("read src/scheduler/mod.rs");
    assert!(
        !src.contains("wire-format compatibility to existing strings"),
        "the false wire-compatibility claim must be gone from NodeStatus's doc comment"
    );
}
