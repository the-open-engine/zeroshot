use super::*;

#[derive(Clone, Copy)]
pub(super) enum PromotionRule {
    Definite,
    EveryAlternative,
    Map,
}

#[derive(Clone, Default)]
pub(super) struct OutcomeRefinement {
    pub(super) success: BTreeSet<NodeName>,
    pub(super) failed: BTreeSet<NodeName>,
}

impl OutcomeRefinement {
    pub(super) fn apply(&self, flow: &mut Flow) {
        for name in &self.success {
            flow.failed.remove(name);
        }
        flow.failed.extend(self.failed.clone());
        flow.resolve_successful_writes();
    }
}

pub(super) struct ChoiceControl {
    pub(super) branches: Vec<OutcomeRefinement>,
    pub(super) branch_reachable: Vec<bool>,
    pub(super) branch_completion: Vec<CompletionPredicate>,
    pub(super) otherwise: OutcomeRefinement,
    pub(super) otherwise_reachable: bool,
    pub(super) otherwise_completion: CompletionPredicate,
    pub(super) exhaustive: bool,
}

impl ChoiceControl {
    pub(super) fn unknown(choice: &ChoiceNode) -> Self {
        Self {
            branches: vec![OutcomeRefinement::default(); choice.branches.as_slice().len()],
            branch_reachable: vec![true; choice.branches.as_slice().len()],
            branch_completion: vec![CompletionPredicate::Always; choice.branches.as_slice().len()],
            otherwise: OutcomeRefinement::default(),
            otherwise_reachable: choice.otherwise.is_some(),
            otherwise_completion: CompletionPredicate::Always,
            exhaustive: choice.otherwise.is_some(),
        }
    }

    pub(super) fn reachability(&self) -> ChoiceReachability {
        ChoiceReachability {
            branches: self.branch_reachable.clone(),
            otherwise_reachable: self.otherwise_reachable,
        }
    }
}

pub(super) fn choice_branch_completion_predicates(choice: &ChoiceNode) -> Vec<CompletionPredicate> {
    let mut earlier = Vec::new();
    choice
        .branches
        .as_slice()
        .iter()
        .map(|branch| {
            let current = CompletionPredicate::Guard(branch.when.clone());
            let residual = CompletionPredicate::all(
                earlier
                    .iter()
                    .cloned()
                    .map(CompletionPredicate::not)
                    .chain(std::iter::once(current.clone())),
            );
            earlier.push(current);
            residual
        })
        .collect()
}

pub(super) fn choice_otherwise_completion_predicate(choice: &ChoiceNode) -> CompletionPredicate {
    CompletionPredicate::all(
        choice.branches.as_slice().iter().map(|branch| {
            CompletionPredicate::not(CompletionPredicate::Guard(branch.when.clone()))
        }),
    )
}

#[derive(Clone)]
pub(super) struct ChoiceReachability {
    pub(super) branches: Vec<bool>,
    pub(super) otherwise_reachable: bool,
}

impl ChoiceReachability {
    pub(super) fn branch_reachable(&self, index: usize) -> bool {
        self.branches.get(index).copied().unwrap_or(true)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SelectorKey {
    pub(super) name: NodeName,
    pub(super) source: ControlSourceKey,
    pub(super) field: Option<FieldName>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum ControlSourceKey {
    Signal,
    Error,
    Group,
}

impl From<&ControlSelector> for SelectorKey {
    fn from(selector: &ControlSelector) -> Self {
        Self {
            name: selector.name.clone(),
            source: match selector.source {
                ControlSource::Signal => ControlSourceKey::Signal,
                ControlSource::Error => ControlSourceKey::Error,
                ControlSource::Group => ControlSourceKey::Group,
            },
            field: selector.field.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct AssignedControl {
    pub(super) counts: BTreeMap<EnumLabel, u64>,
}

pub(super) type Assignment = BTreeMap<SelectorKey, AssignedControl>;
