//! The shared carrier reads and acknowledges steering in the existing run record.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use openengine_cluster_protocol::{Cursor, RunId, RunVector, RunVectorContent, VectorId};
use crate::native_v2_runner::{DriverInvocation, conversations::SteeringReceipts};
use crate::v2_run_ledger::{RunEvent, RunLedger, RunLedgerError, SnapshotAndTail, vector_event};
use super::{ActiveMemorySource, HookDelivery, HookQuery};

pub(super) struct EmptySource;
impl ActiveMemorySource for EmptySource {
    fn tool_context(&self, _: &HookQuery) -> String {
        String::new()
    }
    fn graph_context(&self, _: &str, _: &str) -> String {
        String::new()
    }
}

#[derive(Clone)]
struct Record {
    run: RunId,
    ledger: Arc<dyn RunLedger>,
    runtime: tokio::runtime::Handle,
    index: Arc<ResidentRecord>,
}

// A derived view of the sole vector record. Completed pages and the in-flight bounded read
// survive a callback timeout; no callback has to restart a scan from the beginning.
#[derive(Default)]
struct ResidentRecord {
    scan: tokio::sync::Mutex<Scan>,
    data: Mutex<Index>,
}
#[derive(Default)]
struct Scan {
    after: Option<Cursor>,
    pending: Option<tokio::task::JoinHandle<Result<SnapshotAndTail, RunLedgerError>>>,
}
#[derive(Default)]
struct Index {
    steers: Vec<RunVector>,
    receipts: BTreeMap<(String, VectorId), BTreeMap<VectorId, RunId>>,
    // Acknowledged deliveries whose receipt is still being written. The conversation already
    // holds these steers; a failed write releases them so they are offered again.
    publishing: BTreeSet<(String, VectorId)>,
    initialized: bool,
}
impl Index {
    fn apply(&mut self, vector: RunVector) {
        match &vector.content {
            RunVectorContent::Steer { .. } => self.steers.push(vector),
            RunVectorContent::SteerReceipt {
                steer_id,
                conversation,
                ..
            } => {
                self.receipts
                    .entry((conversation.clone(), steer_id.clone()))
                    .or_default()
                    .insert(vector.id, vector.run_id);
            }
            _ => {}
        }
    }
    fn received(&self, conversation: &Conversation, steer: &VectorId) -> bool {
        self.receipts
            .get(&(conversation.key.clone(), steer.clone()))
            .is_some_and(|receipts| {
                receipts
                    .iter()
                    .any(|(id, run)| conversation.includes(id, run))
            })
    }
    fn held(&self, conversation: &Conversation, steer: &VectorId) -> bool {
        self.received(conversation, steer)
            || self
                .publishing
                .contains(&(conversation.key.clone(), steer.clone()))
    }
    fn due<'a>(&'a self, conversation: &'a Conversation) -> impl Iterator<Item = &'a RunVector> {
        self.steers
            .iter()
            .filter(|v| !self.held(conversation, &v.id) && !conversation.offered.contains(&v.id))
    }
}
impl Record {
    async fn refresh(&self) -> Result<(), RunLedgerError> {
        let mut through = None;
        let mut fresh_read = false;
        loop {
            let mut scan = self.index.scan.lock().await;
            if scan.pending.is_none() {
                if !fresh_read {
                    through = None;
                }
                fresh_read = true;
                let ledger = self.ledger.clone();
                let run = self.run.clone();
                let after = scan.after.clone();
                scan.pending = Some(tokio::spawn(async move {
                    tokio::time::timeout(DEADLINE, ledger.snapshot_and_tail(&run, after.as_ref()))
                        .await
                        .unwrap_or(Err(RunLedgerError::InvalidEvent(
                            "steer record read timed out",
                        )))
                }));
            }
            let Some(pending) = scan.pending.as_mut() else {
                return Err(RunLedgerError::Corrupt);
            };
            let result = pending.await;
            scan.pending = None;
            let tail =
                result.map_err(|_| RunLedgerError::InvalidEvent("steer record read failed"))??;
            // Concurrent executions may keep appending logs. Catch up to the snapshot this
            // refresh first observed rather than chasing a moving end of the record forever.
            let target = *through.get_or_insert(crate::v2_run_ledger::cursor_sequence(
                &tail.snapshot.cursor,
            )?);
            let mut index = self
                .index
                .data
                .lock()
                .map_err(|_| RunLedgerError::Corrupt)?;
            let empty = tail.events.is_empty();
            for stored in tail.events {
                scan.after = Some(stored.cursor);
                if let RunEvent::Vector { vector } = stored.event {
                    index.apply(vector);
                }
            }
            let complete = scan
                .after
                .as_ref()
                .map(crate::v2_run_ledger::cursor_sequence)
                .transpose()?
                .unwrap_or_default()
                >= target
                || empty;
            index.initialized |= complete;
            if complete && fresh_read {
                return Ok(());
            }
            if complete {
                // A retained read may predate this request. Follow its completed page with
                // a fresh snapshot so a newly recorded steer is visible at this delivery.
                through = None;
            }
        }
    }
}

#[derive(Clone)]
struct Conversation {
    session: String,
    key: String,
    reference: crate::native_v2_contract::ExecutionRef,
    offered: Vec<VectorId>,
    run: RunId,
    inherited: Arc<BTreeSet<VectorId>>,
}
impl Conversation {
    fn includes(&self, receipt: &VectorId, run: &RunId) -> bool {
        run == &self.run || self.inherited.contains(receipt)
    }
}

pub(super) struct DeliverySource {
    memory: Arc<dyn ActiveMemorySource>,
    record: Mutex<Option<Record>>,
    conversations: Mutex<BTreeMap<u64, Conversation>>,
    restored: Mutex<BTreeMap<u64, SteeringReceipts>>,
}
const DEADLINE: Duration = Duration::from_millis(250);
const PREAMBLE: &str = "<steer>\nDirection from the person running this run, given while it is in progress. It takes precedence over your authored instructions. Permissions, output contracts and the run's limits are unchanged.\n";

pub(crate) fn frame<'a>(vectors: impl IntoIterator<Item = &'a RunVector>) -> String {
    let mut context = String::new();
    for vector in vectors {
        if let RunVectorContent::Steer { text } = &vector.content {
            if context.is_empty() {
                context.push_str(PREAMBLE);
            }
            context.push_str(&format!("[{}] {}\n", vector.id, text));
        }
    }
    if !context.is_empty() {
        context.push_str("</steer>");
    }
    context
}

impl DeliverySource {
    pub(super) fn new(memory: Arc<dyn ActiveMemorySource>) -> Self {
        Self {
            memory,
            record: Mutex::new(None),
            conversations: Mutex::default(),
            restored: Mutex::default(),
        }
    }
    pub(super) fn bind(&self, run: RunId, ledger: Arc<dyn RunLedger>) {
        if let Ok(mut record) = self.record.lock() {
            *record = Some(Record {
                run,
                ledger,
                runtime: tokio::runtime::Handle::current(),
                index: Arc::default(),
            });
        }
    }
    pub(super) fn restore(&self, instance: u64, receipts: SteeringReceipts) {
        if let Ok(mut restored) = self.restored.lock() {
            restored.insert(instance, receipts);
        }
    }
    pub(super) fn capture(
        &self,
    ) -> BTreeMap<crate::native_v2_contract::NodeInstanceId, SteeringReceipts> {
        let Some(record) = self.record() else {
            return BTreeMap::new();
        };
        let Ok(index) = record.index.data.lock() else {
            return BTreeMap::new();
        };
        let Ok(conversations) = self.conversations.lock() else {
            return BTreeMap::new();
        };
        conversations
            .values()
            .map(|conversation| {
                let receipts = index
                    .receipts
                    .iter()
                    .filter(|((key, _), _)| key == &conversation.key)
                    .flat_map(|(_, receipts)| receipts.iter())
                    .filter(|(id, run)| conversation.includes(id, run))
                    .map(|(id, _)| id.clone())
                    .collect();
                (
                    conversation.reference.node_instance,
                    SteeringReceipts {
                        conversation: conversation.key.clone(),
                        receipts,
                    },
                )
            })
            .collect()
    }
    fn record(&self) -> Option<Record> {
        self.record.lock().ok()?.clone()
    }
    pub(super) async fn prepare(
        &self,
        invocation: &mut DriverInvocation,
        continued: bool,
    ) -> Option<u64> {
        let record = self.record()?;
        let reference = invocation.node.reference.clone();
        let execution = reference.execution.get();
        let session = format!(
            "{}/{}",
            record.run.as_str(),
            super::provider_session_key(invocation)
        );
        let mut conversation = {
            let mut conversations = self.conversations.lock().ok()?;
            let conversation = conversations
                .get(&execution)
                .filter(|c| c.session == session)
                .cloned()
                .or_else(|| {
                    conversations
                        .values()
                        .find(|c| c.session == session)
                        .cloned()
                        .map(|mut c| {
                            c.reference = reference.clone();
                            c.offered.clear();
                            c
                        })
                })
                .unwrap_or_else(|| {
                    // A readable snapshot alone proves nothing: adapters confirm native restoration.
                    let saved = continued
                        .then(|| {
                            self.restored
                                .lock()
                                .ok()?
                                .get(&reference.node_instance.get())
                                .cloned()
                        })
                        .flatten();
                    Conversation {
                        key: saved
                            .as_ref()
                            .map_or_else(|| session.clone(), |s| s.conversation.clone()),
                        inherited: Arc::new(saved.map(|s| s.receipts).unwrap_or_default()),
                        run: record.run.clone(),
                        session,
                        reference,
                        offered: Vec::new(),
                    }
                });
            conversations.insert(execution, conversation.clone());
            conversation
        };
        // Prompt starts need the complete standing direction. Each page's I/O is bounded;
        // large valid records may take several pages, without discarding completed work.
        record.refresh().await.ok()?;
        let index = record.index.data.lock().ok()?;
        let due: Vec<_> = index.due(&conversation).collect();
        let context = frame(due.iter().copied());
        let ids: Vec<_> = due.iter().map(|v| v.id.clone()).collect();
        conversation.offered.extend(ids);
        if !context.is_empty() {
            invocation.host_context = Some(match invocation.host_context.take() {
                Some(host) if !host.is_empty() => format!("{context}\n\n{host}"),
                _ => context,
            });
        }
        self.conversations
            .lock()
            .ok()?
            .insert(execution, conversation);
        Some(execution)
    }
    pub(super) async fn finish(&self, execution: u64, success: bool) {
        let ids = self
            .conversations
            .lock()
            .ok()
            .and_then(|c| c.get(&execution).map(|c| c.offered.clone()))
            .unwrap_or_default();
        if success {
            self.receipt_ids(execution, &ids).await;
        }
        if let Ok(mut conversations) = self.conversations.lock()
            && let Some(conversation) = conversations.get_mut(&execution)
        {
            conversation.offered.clear();
        }
    }
    pub(super) async fn delivery(&self, query: &HookQuery) -> HookDelivery {
        let mut delivery = HookDelivery {
            context: self.memory.tool_context(query),
            steers: Vec::new(),
        };
        let Some(record) = self.record() else {
            return delivery;
        };
        let conversation = self
            .conversations
            .lock()
            .ok()
            .and_then(|c| c.get(&query.execution_id).cloned());
        let Some(conversation) = conversation else {
            return delivery;
        };
        // Leave time within the carrier's existing deadline for framing and transport. A busy
        // record keeps progressing, and already-read directions remain usable between pages.
        let _ = tokio::time::timeout(DEADLINE / 2, record.refresh()).await;
        let Ok(index) = record.index.data.lock() else {
            return delivery;
        };
        // Read all inherited receipts before deciding whether a continued conversation is due.
        if !index.initialized {
            return delivery;
        }
        let mut selected = Vec::new();
        let mut bytes = PREAMBLE.len()
            + "</steer>".len()
            + delivery.context.len()
            + if delivery.context.is_empty() { 0 } else { 2 };
        for vector in index.due(&conversation) {
            let RunVectorContent::Steer { text } = &vector.content else {
                continue;
            };
            let added = vector.id.as_str().len() + text.len() + 4;
            if bytes + added > super::native::MAX_CONTEXT_BYTES {
                break;
            }
            bytes += added;
            delivery.steers.push(vector.id.clone());
            selected.push(vector);
        }
        let context = frame(selected);
        if !context.is_empty() {
            delivery.context = if delivery.context.is_empty() {
                context
            } else {
                format!("{context}\n\n{}", delivery.context)
            };
        }
        delivery
    }
    pub(super) async fn receipt(&self, query: &HookQuery, delivery: &HookDelivery) {
        self.receipt_ids(query.execution_id, &delivery.steers).await;
    }
    async fn receipt_ids(&self, execution: u64, ids: &[VectorId]) {
        if ids.is_empty() {
            return;
        }
        let Some(record) = self.record() else {
            return;
        };
        let conversation = self
            .conversations
            .lock()
            .ok()
            .and_then(|c| c.get(&execution).cloned());
        let Some(conversation) = conversation else {
            return;
        };
        let Some(publication) = record.publish(&conversation, ids) else {
            return;
        };
        // The callback waits only within its deadline. A slower write keeps running so the
        // same conversation is not offered the steer again while its receipt is in flight.
        let _ = tokio::time::timeout(DEADLINE, publication).await;
    }
}
impl Record {
    fn publish(
        &self,
        conversation: &Conversation,
        ids: &[VectorId],
    ) -> Option<tokio::task::JoinHandle<()>> {
        let execution = conversation.reference.public_execution().ok()?;
        // Selection and reservation happen without an await, so a cancelled callback either
        // reserves nothing or hands its reservation to the publication task below.
        let (held, events) = {
            let mut index = self.index.data.lock().ok()?;
            let mut held = Vec::new();
            let mut events = Vec::new();
            for id in ids {
                if index.held(conversation, id) || held.iter().any(|(_, held)| held == id) {
                    continue;
                }
                events.push(
                    vector_event(
                        &self.run,
                        RunVectorContent::SteerReceipt {
                            steer_id: id.clone(),
                            conversation: conversation.key.clone(),
                            execution: execution.clone(),
                            node_instance: conversation.reference.node_instance.get(),
                        },
                    )
                    .ok()?,
                );
                held.push((conversation.key.clone(), id.clone()));
            }
            if events.is_empty() {
                return None;
            }
            index.publishing.extend(held.iter().cloned());
            (held, events)
        };
        let ledger = self.ledger.clone();
        let run = self.run.clone();
        let resident = self.index.clone();
        Some(tokio::spawn(async move {
            let stored = ledger.append(&run, events.clone()).await.is_ok();
            let Ok(mut index) = resident.data.lock() else {
                return;
            };
            for key in &held {
                index.publishing.remove(key);
            }
            if stored {
                for event in events {
                    if let RunEvent::Vector { vector } = event {
                        index.apply(vector);
                    }
                }
            }
        }))
    }
}
impl ActiveMemorySource for DeliverySource {
    fn graph_context(&self, node: &str, session: &str) -> String {
        self.memory.graph_context(node, session)
    }
    fn tool_context(&self, query: &HookQuery) -> String {
        self.tool_delivery(query).context
    }
    fn tool_delivery(&self, query: &HookQuery) -> HookDelivery {
        match self.record() {
            Some(record) => record.runtime.block_on(self.delivery(query)),
            None => self.memory.tool_delivery(query),
        }
    }
    fn acknowledged(&self, query: &HookQuery, delivery: &HookDelivery) {
        if let Some(record) = self.record() {
            record.runtime.block_on(self.receipt(query, delivery));
        }
        self.memory.acknowledged(query, delivery);
    }
    fn unavailable(&self, reason: &str) {
        self.memory.unavailable(reason);
    }
}

#[cfg(test)]
mod tests;
