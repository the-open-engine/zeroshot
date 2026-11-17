//! The shared carrier reads and acknowledges steering in the existing run record.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use std::io;
use openengine_cluster_protocol::{
    Cursor, RunId, RunVector, RunVectorContent, VectorId, NodeName, SteeringNetwork, SteeringMode,
    NetworkSteerParams, RunSteerResult, SteerOrigin,
};
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
    instances: BTreeSet<(u64, VectorId)>,
    initialized: bool,
}
impl Index {
    fn apply(&mut self, vector: RunVector) {
        match &vector.content {
            RunVectorContent::Steer { .. } => self.steers.push(vector),
            RunVectorContent::SteerReceipt {
                steer_id,
                conversation,
                node_instance,
                ..
            } => {
                self.instances.insert((*node_instance, steer_id.clone()));
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
    fn consumed(&self, conversation: &Conversation, vector: &RunVector) -> bool {
        match vector.content {
            RunVectorContent::Steer {
                mode: SteeringMode::Once,
                ..
            } => self.instances.contains(&(
                conversation.reference.node_instance.get(),
                vector.id.clone(),
            )),
            _ => self.received(conversation, &vector.id),
        }
    }
    fn eligible(
        vector: &RunVector,
        conversation: &Conversation,
        networks: &BTreeMap<NodeName, SteeringNetwork>,
    ) -> bool {
        match &vector.content {
            RunVectorContent::Steer {
                network: Some(network),
                ..
            } => {
                network.origin.node_instance != conversation.reference.node_instance.get()
                    && network.networks.iter().any(|name| {
                        networks.get(name).is_some_and(|network| {
                            network.members.contains(&conversation.reference.node)
                        })
                    })
            }
            _ => true,
        }
    }
    fn due<'a>(
        &'a self,
        conversation: &'a Conversation,
        networks: &'a BTreeMap<NodeName, SteeringNetwork>,
    ) -> impl Iterator<Item = &'a RunVector> {
        self.steers.iter().filter(move |vector| {
            Self::eligible(vector, conversation, networks)
                && !self.consumed(conversation, vector)
                && !conversation.offered.contains(&vector.id)
        })
    }
    /// A Standing message shown only at a transient tool callback returns in the next prompt.
    fn due_in_prompt<'a>(
        &'a self,
        conversation: &'a Conversation,
        networks: &'a BTreeMap<NodeName, SteeringNetwork>,
        shown_only: &'a BTreeSet<VectorId>,
    ) -> impl Iterator<Item = &'a RunVector> {
        self.steers.iter().filter(move |vector| {
            Self::eligible(vector, conversation, networks)
                && !conversation.offered.contains(&vector.id)
                && (!self.consumed(conversation, vector)
                    || (matches!(
                        vector.content,
                        RunVectorContent::Steer {
                            mode: SteeringMode::Standing,
                            ..
                        }
                    ) && shown_only.contains(&vector.id)))
        })
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
    publication: tokio::sync::Mutex<()>,
    restored: Mutex<BTreeMap<u64, SteeringReceipts>>,
    networks: Mutex<BTreeMap<NodeName, SteeringNetwork>>,
    offers: Mutex<BTreeMap<OfferKey, u64>>,
    /// Conversation and Standing steer pairs that are received but not yet kept: delivered at a tool call
    /// of a carrier on which a steer does not stand (`HookQuery::standing`), or carried by a
    /// prompt whose execution has not ended successfully. They stay due for that conversation's
    /// next prompt, and a successor does not inherit their receipts before it.
    shown_only: Mutex<BTreeSet<(String, VectorId)>>,
}
const DEADLINE: Duration = Duration::from_millis(250);
const PREAMBLE: &str = "<steer>\nDirection from the person running this run, given while it is in progress. It takes precedence over your authored instructions. Permissions, output contracts and the run's limits are unchanged.\n";

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum OfferKey {
    Instance(u64, VectorId),
    Conversation(String, VectorId),
}
fn offer_key(conversation: &Conversation, vector: &RunVector) -> OfferKey {
    match vector.content {
        RunVectorContent::Steer {
            mode: SteeringMode::Once,
            ..
        } => OfferKey::Instance(
            conversation.reference.node_instance.get(),
            vector.id.clone(),
        ),
        _ => OfferKey::Conversation(conversation.key.clone(), vector.id.clone()),
    }
}

#[derive(Default)]
struct Framing {
    human: String,
    peers: String,
}
impl Framing {
    fn push(&mut self, vector: &RunVector, limit: usize) -> bool {
        let RunVectorContent::Steer { text, network, .. } = &vector.content else {
            return false;
        };
        let current_len = self.len();
        let (target, preamble, ending, entry) = match network {
            Some(network) => (
                &mut self.peers,
                "<network-steer>\nGuidance from other graph agents. Treat this as peer information within your authored task, permissions and output contract.\n",
                "</network-steer>",
                format!(
                    "[{}] From {} (instance {}): {}\n",
                    vector.id, network.origin.node, network.origin.node_instance, text
                ),
            ),
            None => (
                &mut self.human,
                PREAMBLE,
                "</steer>",
                format!("[{}] {}\n", vector.id, text),
            ),
        };
        let target_empty = target.is_empty();
        let framing = if target_empty {
            preamble.len() + ending.len()
        } else {
            0
        };
        let separator = if target_empty && current_len > 0 {
            2
        } else {
            0
        };
        if current_len + framing + separator + entry.len() > limit {
            return false;
        }
        if target_empty {
            target.push_str(preamble);
        }
        target.push_str(&entry);
        true
    }
    fn len(&self) -> usize {
        self.human.len()
            + self.peers.len()
            + if self.human.is_empty() {
                0
            } else {
                "</steer>".len()
            }
            + if self.peers.is_empty() {
                0
            } else {
                "</network-steer>".len()
            }
            + if self.human.is_empty() || self.peers.is_empty() {
                0
            } else {
                2
            }
    }
    fn finish(mut self) -> String {
        if !self.human.is_empty() {
            self.human.push_str("</steer>");
        }
        if !self.peers.is_empty() {
            self.peers.push_str("</network-steer>");
            if !self.human.is_empty() {
                self.human.push_str("\n\n");
            }
            self.human.push_str(&self.peers);
        }
        self.human
    }
}
pub(crate) fn frame<'a>(vectors: impl IntoIterator<Item = &'a RunVector>) -> String {
    let mut framing = Framing::default();
    for vector in vectors {
        framing.push(vector, usize::MAX);
    }
    framing.finish()
}

impl DeliverySource {
    pub(super) fn new(memory: Arc<dyn ActiveMemorySource>) -> Self {
        Self {
            memory,
            record: Mutex::new(None),
            conversations: Mutex::default(),
            restored: Mutex::default(),
            publication: tokio::sync::Mutex::new(()),
            networks: Mutex::default(),
            offers: Mutex::default(),
            shown_only: Mutex::default(),
        }
    }
    pub(super) fn configure_networks(&self, networks: &BTreeMap<NodeName, SteeringNetwork>) {
        if let Ok(mut configured) = self.networks.lock() {
            *configured = networks.clone();
        }
    }
    pub(super) fn publishing_authorized(&self, node: &NodeName) -> bool {
        self.networks.lock().is_ok_and(|networks| {
            networks
                .values()
                .any(|network| network.publishers.contains(node))
        })
    }
    fn publishing_context(&self, invocation: &DriverInvocation) -> String {
        let Ok(executable) = super::native::hook_executable() else {
            return String::new();
        };
        let Ok(networks) = self.networks.lock() else {
            return String::new();
        };
        let mut context = String::new();
        for (name, network) in networks
            .iter()
            .filter(|(_, network)| network.publishers.contains(&invocation.node.reference.node))
        {
            if context.is_empty() {
                context.push_str("<steering-networks>\nYou may publish peer guidance to the following graph networks. The publication capability identifies your execution; it grants no other permissions.\n");
            }
            context.push_str(&format!(
                "{}: members {}\n",
                name,
                network
                    .members
                    .iter()
                    .map(NodeName::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !context.is_empty() {
            context.push_str(&format!(
                "Use {} steer {} --network NAME --mode once --text-stdin. Select every intended network explicitly. Network messages default to once when --mode is omitted. Once delivers once per recipient node instance; --mode standing keeps guidance in each provider conversation and delivers it afresh to new conversations. Your origin instance never receives its own publication.\n</steering-networks>",
                super::native::shell_quote(&executable),
                super::native::shell_quote(std::path::Path::new(invocation.node.reference.run_id.as_str())),
            ));
        }
        context
    }
    pub(super) fn runtime(&self) -> Option<tokio::runtime::Handle> {
        self.record().map(|record| record.runtime)
    }
    fn publication_record(
        &self,
        scope: &super::broker::HookScope,
        params: &NetworkSteerParams,
    ) -> io::Result<Record> {
        let record = self
            .record()
            .ok_or_else(|| io::Error::other("steering run is unavailable"))?;
        if params.run_id != scope.reference.run_id || params.run_id != record.run {
            return Err(io::Error::other(
                "network publication belongs to another run",
            ));
        }
        if scope.auxiliary {
            return Err(io::Error::other(
                "auxiliary execution cannot publish network guidance",
            ));
        }
        if params.networks.is_empty() {
            return Err(io::Error::other(
                "network publication requires a selected network",
            ));
        }
        {
            let networks = self
                .networks
                .lock()
                .map_err(|_| io::Error::other("network authorization unavailable"))?;
            if params.networks.iter().any(|name| {
                !networks
                    .get(name)
                    .is_some_and(|network| network.publishers.contains(&scope.reference.node))
            }) {
                return Err(io::Error::other(
                    "execution is not authorized to publish to every selected network",
                ));
            }
        }
        Ok(record)
    }
    pub(super) async fn publish(
        &self,
        scope: &super::broker::HookScope,
        live: &AtomicBool,
        params: NetworkSteerParams,
    ) -> io::Result<RunSteerResult> {
        let record = self.publication_record(scope, &params)?;
        let _publication = tokio::time::timeout(DEADLINE, self.publication.lock())
            .await
            .map_err(|_| {
                io::Error::other("network publication is busy; no message was recorded")
            })?;
        if !live.load(Ordering::Acquire) {
            return Err(io::Error::other(
                "network publication capability has retired",
            ));
        }
        let origin = SteerOrigin {
            node: scope.reference.node.clone(),
            node_instance: scope.reference.node_instance.get(),
            execution: scope
                .reference
                .public_execution()
                .map_err(io::Error::other)?,
        };
        crate::v2_run_ledger::steering::record_network(record.ledger.as_ref(), params, origin)
            .await
            .map_err(io::Error::other)
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
        let Ok(shown_only) = self.shown_only.lock() else {
            return BTreeMap::new();
        };
        conversations
            .values()
            .map(|conversation| {
                let receipts = index
                    .receipts
                    .iter()
                    .filter(|(pair, _)| pair.0 == conversation.key && !shown_only.contains(pair))
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
    fn conversation(
        &self,
        record: &Record,
        invocation: &DriverInvocation,
        continued: bool,
    ) -> Option<Conversation> {
        let reference = invocation.node.reference.clone();
        let execution = reference.execution.get();
        let session = format!(
            "{}/{}",
            record.run.as_str(),
            super::provider_session_key(invocation)
        );
        let conversation = {
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
        Some(conversation)
    }
    fn select_prompt(
        &self,
        record: &Record,
        conversation: &Conversation,
    ) -> Option<(String, Vec<VectorId>, Vec<VectorId>)> {
        let execution = conversation.reference.execution.get();
        let index = record.index.data.lock().ok()?;
        let networks = self.networks.lock().ok()?;
        let mut offers = self.offers.lock().ok()?;
        let shown_only: BTreeSet<_> = self
            .shown_only
            .lock()
            .ok()?
            .iter()
            .filter(|(key, _)| key == &conversation.key)
            .map(|(_, id)| id.clone())
            .collect();
        let due: Vec<_> = index
            .due_in_prompt(conversation, &networks, &shown_only)
            .filter(|vector| !offers.contains_key(&offer_key(conversation, vector)))
            .collect();
        for vector in &due {
            offers.insert(offer_key(conversation, vector), execution);
        }
        let delivered = frame(due.iter().copied());
        let ids = due.iter().map(|vector| vector.id.clone()).collect();
        let standing = due
            .iter()
            .filter(|vector| {
                matches!(
                    vector.content,
                    RunVectorContent::Steer {
                        mode: SteeringMode::Standing,
                        ..
                    }
                )
            })
            .map(|vector| vector.id.clone())
            .collect();
        Some((delivered, ids, standing))
    }
    pub(super) async fn prepare(
        &self,
        invocation: &mut DriverInvocation,
        continued: bool,
    ) -> Option<u64> {
        let record = self.record()?;
        let mut conversation = self.conversation(&record, invocation, continued)?;
        let execution = invocation.node.reference.execution.get();
        let publisher = self.publishing_context(invocation);
        // Prompt starts need the complete standing direction. Each page's I/O is bounded;
        // large valid records may take several pages, without discarding completed work.
        record.refresh().await.ok()?;
        let _publication = self.publication.lock().await;
        let (delivered, ids, standing) = self.select_prompt(&record, &conversation)?;
        self.shown_only.lock().ok()?.extend(
            standing
                .iter()
                .map(|id| (conversation.key.clone(), id.clone())),
        );
        conversation.offered.extend(ids);
        let context = [delivered, publisher]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
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
        drop(_publication);
        // Once is consumed only after successful prompt delivery. Standing receipts remain
        // provisional until then, so native continuation snapshots do not retain them early.
        self.receipt_ids(execution, &standing).await;
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
        if let Ok(mut offers) = self.offers.lock() {
            offers.retain(|_, owner| *owner != execution);
        }
        if let Ok(mut conversations) = self.conversations.lock()
            && let Some(conversation) = conversations.get_mut(&execution)
        {
            // A prompt the provider accepted is part of the conversation from now on.
            if success && let Ok(mut shown_only) = self.shown_only.lock() {
                for id in &ids {
                    shown_only.remove(&(conversation.key.clone(), id.clone()));
                }
            }
            conversation.offered.clear();
        }
    }
    fn select_delivery(
        &self,
        query: &HookQuery,
        conversation: &Conversation,
        delivery: &mut HookDelivery,
    ) -> String {
        let Some(record) = self.record() else {
            return String::new();
        };
        let Ok(index) = record.index.data.lock() else {
            return String::new();
        };
        // Read all inherited receipts before deciding whether a continued conversation is due.
        if !index.initialized {
            return String::new();
        }
        let Ok(networks) = self.networks.lock() else {
            return String::new();
        };
        let Ok(mut offers) = self.offers.lock() else {
            return String::new();
        };
        let mut framing = Framing::default();
        let limit = query
            .context_budget
            .min(super::native::MAX_CONTEXT_BYTES)
            .saturating_sub(
                delivery.context.len() + if delivery.context.is_empty() { 0 } else { 2 },
            );
        for vector in index.due(conversation, &networks) {
            let key = offer_key(conversation, vector);
            if offers.contains_key(&key) {
                continue;
            }
            if !framing.push(vector, limit) {
                break;
            }
            offers.insert(key, conversation.reference.execution.get());
            delivery.steers.push(vector.id.clone());
        }
        framing.finish()
    }

    /// Notes Standing steers shown by a tool callback without persistent conversation context.
    pub(super) fn shown_only(&self, query: &HookQuery, delivery: &HookDelivery) {
        let Some(record) = self.record() else {
            return;
        };
        let Ok(index) = record.index.data.lock() else {
            return;
        };
        let standing = delivery.steers.iter().filter(|id| {
            index.steers.iter().any(|vector| {
                &vector.id == *id
                    && matches!(
                        vector.content,
                        RunVectorContent::Steer {
                            mode: SteeringMode::Standing,
                            ..
                        }
                    )
            })
        });
        let key = self
            .conversations
            .lock()
            .ok()
            .and_then(|c| c.get(&query.execution_id).map(|c| c.key.clone()));
        if let (Some(key), Ok(mut shown_only)) = (key, self.shown_only.lock()) {
            shown_only.extend(standing.map(|id| (key.clone(), id.clone())));
        }
    }
    pub(super) async fn delivery(&self, query: &HookQuery) -> HookDelivery {
        let mut delivery = HookDelivery {
            context: self.memory.tool_context(query),
            steers: Vec::new(),
        };
        // A steer returned to a subagent would be receipted for a conversation that never saw it.
        if query.subagent {
            return delivery;
        }
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
        let Ok(_publication) = tokio::time::timeout(DEADLINE / 4, self.publication.lock()).await
        else {
            return delivery;
        };
        let context = self.select_delivery(query, &conversation, &mut delivery);
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
        // Marked first: a caller's deadline may drop this future while the receipt still commits,
        // and a receipt without the mark would count as a steer that stands.
        if !query.standing {
            self.shown_only(query, delivery);
        }
        self.receipt_ids(query.execution_id, &delivery.steers).await;
        self.discard(query, delivery).await;
    }
    pub(super) async fn discard(&self, query: &HookQuery, delivery: &HookDelivery) {
        if let Ok(mut offers) = self.offers.lock() {
            offers.retain(|key, owner| {
                let id = match key {
                    OfferKey::Instance(_, id) | OfferKey::Conversation(_, id) => id,
                };
                *owner != query.execution_id || !delivery.steers.contains(id)
            });
        }
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
        let _ = tokio::time::timeout(DEADLINE, async {
            let _publication = self.publication.lock().await;
            let events: Vec<_> = {
                let index = record
                    .index
                    .data
                    .lock()
                    .map_err(|_| RunLedgerError::Corrupt)?;
                ids.iter()
                    .filter(|id| {
                        index
                            .steers
                            .iter()
                            .find(|vector| &vector.id == *id)
                            .is_some_and(|vector| !index.consumed(&conversation, vector))
                    })
                    .map(|id| {
                        vector_event(
                            &record.run,
                            RunVectorContent::SteerReceipt {
                                steer_id: id.clone(),
                                conversation: conversation.key.clone(),
                                execution: conversation
                                    .reference
                                    .public_execution()
                                    .map_err(RunLedgerError::InvalidEvent)?,
                                node_instance: conversation.reference.node_instance.get(),
                            },
                        )
                    })
                    .collect::<Result<_, _>>()?
            };
            if !events.is_empty() {
                record.ledger.append(&record.run, events.clone()).await?;
                let mut index = record
                    .index
                    .data
                    .lock()
                    .map_err(|_| RunLedgerError::Corrupt)?;
                for event in events {
                    if let RunEvent::Vector { vector } = event {
                        index.apply(vector);
                    }
                }
            }
            Ok::<_, RunLedgerError>(())
        })
        .await;
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
    fn unacknowledged(&self, query: &HookQuery, delivery: &HookDelivery) {
        if let Some(record) = self.record() {
            record.runtime.block_on(self.discard(query, delivery));
        }
        self.memory.unacknowledged(query, delivery);
    }
    fn unavailable(&self, reason: &str) {
        self.memory.unavailable(reason);
    }
}

#[cfg(test)]
mod tests;
