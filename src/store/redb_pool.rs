//! `RedbStorePool`: a per-operation redb vault wrapper for concurrent access.
//!
//! This module provides `RedbStorePool`, a `Store` implementation that opens
//! the vault per-operation rather than holding the lock for the process
//! lifetime. This allows the UI and an SDD run to share the same vault.

use super::{NodeRecord, NodeTiming, RedbStore, RunMeta, Store};
use crate::core::error::PidagError;
use crate::core::event::Event;
use async_trait::async_trait;
use std::path::PathBuf;

/// A `Store` wrapper that opens the redb vault **per-operation**, releasing the
/// exclusive file lock between calls. This lets the UI and an SDD run share
/// the same vault without one blocking the other: each holds the lock only
/// for the duration of a single read/write (milliseconds), not for the
/// entire process lifetime.
///
/// Trade-off: every method call pays an `open` cost (file open + flock +
/// table checks). For the UI's low request rate (a handful of GETs per poll)
/// and the SDD run's event rate (a few events per node), this is negligible.
/// If a future use case needs high-throughput writes, use `RedbStore`
/// directly (persistent lock).
pub struct RedbStorePool {
    path: PathBuf,
}

impl RedbStorePool {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Open a short-lived `RedbStore` for a single operation. The lock is
    /// released when the returned `RedbStore` is dropped.
    fn open(&self) -> Result<RedbStore, PidagError> {
        RedbStore::open(&self.path)
    }
}

#[async_trait]
impl Store for RedbStorePool {
    async fn put_run(&self, run: &RunMeta) -> Result<(), PidagError> {
        self.open()?.put_run(run).await
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunMeta>, PidagError> {
        self.open()?.get_run(run_id).await
    }

    async fn put_node_state(
        &self,
        run_id: &str,
        node_id: &str,
        record: &NodeRecord,
    ) -> Result<(), PidagError> {
        self.open()?.put_node_state(run_id, node_id, record).await
    }

    async fn list_nodes(&self, run_id: &str) -> Result<Vec<NodeRecord>, PidagError> {
        self.open()?.list_nodes(run_id).await
    }

    async fn terminal_set(&self, run_id: &str) -> Result<Vec<(String, String)>, PidagError> {
        self.open()?.terminal_set(run_id).await
    }

    async fn append_event(&self, run_id: &str, ev: &Event) -> Result<u64, PidagError> {
        self.open()?.append_event(run_id, ev).await
    }

    async fn load_events(&self, run_id: &str) -> Result<Vec<Event>, PidagError> {
        self.open()?.load_events(run_id).await
    }

    async fn list_runs(&self) -> Result<Vec<RunMeta>, PidagError> {
        self.open()?.list_runs().await
    }

    async fn load_events_since(
        &self,
        run_id: &str,
        since: u64,
    ) -> Result<Vec<(u64, Event)>, PidagError> {
        self.open()?.load_events_since(run_id, since).await
    }

    async fn put_artifact(
        &self,
        run_id: &str,
        node_id: &str,
        artifact: &str,
    ) -> Result<(), PidagError> {
        self.open()?.put_artifact(run_id, node_id, artifact).await
    }

    async fn get_artifact(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<String>, PidagError> {
        self.open()?.get_artifact(run_id, node_id).await
    }

    async fn put_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
        timing: &NodeTiming,
    ) -> Result<(), PidagError> {
        self.open()?.put_node_timing(run_id, node_id, timing).await
    }

    async fn get_node_timing(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeTiming>, PidagError> {
        self.open()?.get_node_timing(run_id, node_id).await
    }

    async fn list_node_timings(
        &self,
        run_id: &str,
    ) -> Result<Vec<(String, NodeTiming)>, PidagError> {
        self.open()?.list_node_timings(run_id).await
    }

    async fn get_budget(&self, run_id: &str) -> Result<super::BudgetCounters, PidagError> {
        self.open()?.get_budget(run_id).await
    }

    async fn put_budget(
        &self,
        run_id: &str,
        counters: &super::BudgetCounters,
    ) -> Result<(), PidagError> {
        self.open()?.put_budget(run_id, counters).await
    }
}
