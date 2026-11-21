//! Which scope reads each selector: the check's own, or the items of an enclosing map chain.
use super::*;

#[derive(Default)]
pub(in crate::graph_verifier) struct ScopePlan<'a> {
    pub(super) owner: Option<NodeName>,
    pub(super) local: BTreeMap<NodeName, Vec<SelectorKey>>,
    pub(super) maps: Vec<(&'a MapNode, ScopePlan<'a>)>,
    pub(in crate::graph_verifier) keys: BTreeSet<SelectorKey>,
}

pub(in crate::graph_verifier) fn scope_plan<'a>(
    selectors: &[ControlSelector],
    inputs: DimensionInputs<'_, 'a>,
) -> ScopePlan<'a> {
    let mut plan = ScopePlan::default();
    for selector in selectors {
        let key = SelectorKey::from(selector);
        let chain = map_chain(&key, inputs);
        insert_key(&mut plan, &chain, key);
    }
    sort_maps(&mut plan, inputs);
    plan
}

fn map_chain<'a>(key: &SelectorKey, inputs: DimensionInputs<'_, 'a>) -> Vec<&'a MapNode> {
    if !inputs.map_aggregates.contains_key(key) {
        return vec![];
    }
    inputs
        .nodes
        .get(&key.name)
        .map(|info| {
            info.scope_path()
                .iter()
                .filter_map(|part| {
                    let DiagnosticPathSegment::Node { name } = part else {
                        return None;
                    };
                    if name == &key.name {
                        return None;
                    }
                    match inputs.nodes.get(name)?.node {
                        GraphNode::Map(map) => Some(map),
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn insert_key<'a>(plan: &mut ScopePlan<'a>, chain: &[&'a MapNode], key: SelectorKey) {
    plan.keys.insert(key.clone());
    let Some((map, rest)) = chain.split_first() else {
        let keys = plan.local.entry(key.name.clone()).or_default();
        if !keys.contains(&key) {
            keys.push(key);
        }
        return;
    };
    let index = plan
        .maps
        .iter()
        .position(|(existing, _)| existing.name == map.name)
        .unwrap_or_else(|| {
            plan.maps.push((
                map,
                ScopePlan {
                    owner: Some(map.name.clone()),
                    ..ScopePlan::default()
                },
            ));
            plan.maps.len() - 1
        });
    insert_key(&mut plan.maps[index].1, rest, key);
}

fn sort_maps(plan: &mut ScopePlan<'_>, inputs: DimensionInputs<'_, '_>) {
    plan.maps
        .sort_by_key(|(map, _)| inputs.nodes[&map.name].ordinal);
    for (_, child) in &mut plan.maps {
        sort_maps(child, inputs);
    }
}
