use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_protocol::{
    GraphNode, GraphSpec, Guard, Join, NodeName, NodeRuntimeBinding, PolicyBinding, ProfileNode,
    RunProfileName, RuntimePlan, SteeringNetwork, WorkerRef, WriteBinding,
};
use openengine_cluster_server::graph_verifier::{
    FULL_V1_MAX_GRAPH_DEPTH, FULL_V1_MAX_GRAPH_NODES, graph_node_children,
};
use sha2::{Digest, Sha256};

use super::{NativeV2CliError, StoredProfiles, decode_profile};

pub(super) fn contains_imports(node: &GraphNode) -> bool {
    matches!(node, GraphNode::Profile(_))
        || graph_node_children(node).into_iter().any(contains_imports)
}

pub(super) fn expand(
    stored: &StoredProfiles,
    mut graph: GraphSpec,
    mut runtime: RuntimePlan,
    root_name: Option<&RunProfileName>,
) -> Result<(GraphSpec, RuntimePlan), NativeV2CliError> {
    Linker {
        stored,
        policy: &graph.policy,
        path: root_name.into_iter().cloned().collect(),
        names: BTreeSet::new(),
        imported_bytes: 0,
        networks: &mut graph.networks,
        runtime: &mut runtime,
    }
    .link(&mut graph.root, None, 1)?;
    Ok((graph, runtime))
}

struct Linker<'a> {
    stored: &'a StoredProfiles,
    policy: &'a PolicyBinding,
    path: Vec<RunProfileName>,
    names: BTreeSet<NodeName>,
    imported_bytes: usize,
    networks: &'a mut BTreeMap<NodeName, SteeringNetwork>,
    runtime: &'a mut RuntimePlan,
}

impl Linker<'_> {
    fn link(
        &mut self,
        node: &mut GraphNode,
        namespace: Option<&NodeName>,
        depth: u64,
    ) -> Result<(), NativeV2CliError> {
        if depth > FULL_V1_MAX_GRAPH_DEPTH || self.names.len() as u64 >= FULL_V1_MAX_GRAPH_NODES {
            return Err(invalid(
                "expanded graph exceeds the graph depth or node limit",
            ));
        }
        if let Some(namespace) = namespace {
            qualify_node(node, namespace)?;
        }
        if !self.names.insert(node.name().clone()) {
            return Err(invalid(format!(
                "duplicate expanded node name {}",
                node.name()
            )));
        }
        if let GraphNode::Profile(import) = node {
            return self.link_import(import, depth + 1);
        }
        for child in children_mut(node) {
            self.link(child, namespace, depth + 1)?;
        }
        Ok(())
    }

    fn link_import(
        &mut self,
        import: &mut ProfileNode,
        depth: u64,
    ) -> Result<(), NativeV2CliError> {
        let name = &import.profile;
        if import.body.is_some() || import.initial_input.is_some() {
            return Err(invalid(format!(
                "profile reference {} must omit its expansion",
                import.name
            )));
        }
        if self.path.contains(name) {
            let path = self
                .path
                .iter()
                .chain([name])
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            return Err(invalid(format!(
                "profile import cycle {}",
                path.join(" → ")
            )));
        }
        let raw = self.stored.profiles.get(name).ok_or_else(|| {
            invalid(format!(
                "profile {} references missing local profile {name}",
                import.name
            ))
        })?;
        self.imported_bytes = self.imported_bytes.saturating_add(raw.get().len());
        if self.imported_bytes > openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES {
            return Err(invalid(
                "expanded profile imports exceed the request size limit",
            ));
        }
        let mut child = decode_profile(raw, name, false)?;
        if child.graph.policy != *self.policy
            || child.graph.profile != openengine_cluster_protocol::GraphProfile::Full
        {
            return Err(invalid(format!(
                "profile {name} must use full/v1 and the parent graph policy"
            )));
        }
        self.merge_runtime(&import.name, child.runtime)?;
        self.merge_networks(&import.name, child.graph.networks)?;
        self.path.push(name.clone());
        self.link(&mut child.graph.root, Some(&import.name), depth)?;
        self.path.pop();
        import.initial_input = Some(child.graph.initial_input);
        import.body = Some(Box::new(child.graph.root));
        Ok(())
    }

    fn merge_runtime(
        &mut self,
        namespace: &NodeName,
        mut child: RuntimePlan,
    ) -> Result<(), NativeV2CliError> {
        let default_lane = child.lane();
        let parent_lane = self.runtime.lane();
        for (node, mut binding) in std::mem::take(child.nodes_mut()) {
            if let NodeRuntimeBinding::Agent { lane, .. } = &mut binding {
                let effective = lane.take().unwrap_or_else(|| default_lane.clone());
                *lane = (effective != parent_lane).then_some(effective);
            }
            let node = qualified(namespace, &node)?;
            if self
                .runtime
                .nodes_mut()
                .insert(node.clone(), binding)
                .is_some()
            {
                return Err(invalid(format!(
                    "duplicate expanded runtime binding {node}"
                )));
            }
        }
        Ok(())
    }

    fn merge_networks(
        &mut self,
        namespace: &NodeName,
        networks: BTreeMap<NodeName, SteeringNetwork>,
    ) -> Result<(), NativeV2CliError> {
        for (name, network) in networks {
            let name = qualified(namespace, &name)?;
            let network = SteeringNetwork {
                members: qualify_names(namespace, &network.members)?,
                publishers: qualify_names(namespace, &network.publishers)?,
            };
            if self.networks.insert(name.clone(), network).is_some() {
                return Err(invalid(format!("duplicate expanded network {name}")));
            }
        }
        Ok(())
    }
}

fn children_mut(node: &mut GraphNode) -> Vec<&mut GraphNode> {
    match node {
        GraphNode::Seq(group) => group.children.as_mut_slice().iter_mut().collect(),
        GraphNode::Par(group) => group.branches.as_mut_slice().iter_mut().collect(),
        GraphNode::Choice(group) => group
            .branches
            .as_mut_slice()
            .iter_mut()
            .map(|branch| &mut branch.node)
            .chain(group.otherwise.as_deref_mut())
            .collect(),
        GraphNode::Loop(group) => vec![&mut group.body],
        GraphNode::Map(group) => vec![&mut group.body],
        GraphNode::Step(_)
        | GraphNode::Verifier(_)
        | GraphNode::Profile(_)
        | GraphNode::Succeed(_)
        | GraphNode::Fail(_) => Vec::new(),
    }
}

fn qualify_node(node: &mut GraphNode, namespace: &NodeName) -> Result<(), NativeV2CliError> {
    let name = match node {
        GraphNode::Step(node) => {
            qualify_worker(namespace, &mut node.worker, &mut node.write_bindings)?;
            &mut node.name
        }
        GraphNode::Verifier(node) => {
            qualify_worker(namespace, &mut node.worker, &mut node.write_bindings)?;
            &mut node.name
        }
        GraphNode::Seq(node) => &mut node.name,
        GraphNode::Choice(node) => {
            for branch in node.branches.as_mut_slice() {
                qualify_guard(namespace, &mut branch.when)?;
            }
            &mut node.name
        }
        GraphNode::Par(node) => {
            if let Join::First { when } = &mut node.join {
                qualify_guard(namespace, when)?;
            }
            &mut node.name
        }
        GraphNode::Loop(node) => {
            if let Some(guard) = &mut node.until {
                qualify_guard(namespace, guard)?;
            }
            &mut node.name
        }
        GraphNode::Map(node) => &mut node.name,
        GraphNode::Profile(node) => &mut node.name,
        GraphNode::Succeed(node) => &mut node.name,
        GraphNode::Fail(node) => &mut node.name,
    };
    *name = qualified(namespace, name)?;
    Ok(())
}

fn qualify_worker(
    namespace: &NodeName,
    worker: &mut WorkerRef,
    writes: &mut [WriteBinding],
) -> Result<(), NativeV2CliError> {
    let reference = worker.as_str();
    if !is_builtin_worker(worker) {
        let encoded = format!("{namespace}/{reference}");
        *worker = WorkerRef::new(format!("profile.{:x}@1", Sha256::digest(encoded)))
            .map_err(|error| invalid(error.to_string()))?;
    }
    for write in writes {
        write.value.node = qualified(namespace, &write.value.node)?;
    }
    Ok(())
}

fn is_builtin_worker(worker: &WorkerRef) -> bool {
    matches!(
        worker.as_str(),
        crate::native_v2_wait::WAIT_WORKER_REF
            | crate::native_v2_request::ASK_WORKER_REF
            | crate::native_v2_comment_delivery::COMMENT_DELIVERY_WORKER_REF
    ) || crate::native_v2_delivery::DeliveryMode::from_worker(worker).is_some()
}

fn qualify_guard(namespace: &NodeName, guard: &mut Guard) -> Result<(), NativeV2CliError> {
    match guard {
        Guard::In { value, .. } | Guard::KOfMap { value, .. } => {
            value.name = qualified(namespace, &value.name)?;
        }
        Guard::KOfN { values, .. } => {
            for value in values.as_mut_slice() {
                value.name = qualified(namespace, &value.name)?;
            }
        }
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_mut_slice() {
                qualify_guard(namespace, guard)?;
            }
        }
        Guard::Not { guard } => qualify_guard(namespace, guard)?,
    }
    Ok(())
}

fn qualify_names(
    namespace: &NodeName,
    names: &BTreeSet<NodeName>,
) -> Result<BTreeSet<NodeName>, NativeV2CliError> {
    names
        .iter()
        .map(|name| qualified(namespace, name))
        .collect()
}

fn qualified(namespace: &NodeName, name: &NodeName) -> Result<NodeName, NativeV2CliError> {
    let joined = format!("{namespace}.{name}");
    NodeName::new(joined.as_str())
        .or_else(|_| NodeName::new(format!("profile_{:x}", Sha256::digest(joined))))
        .map_err(|error| invalid(error.to_string()))
}

fn invalid(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Usage(message.into())
}
