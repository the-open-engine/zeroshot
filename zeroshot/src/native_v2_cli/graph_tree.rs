//! Read-only presentation of the authored graph, independent of template selection.

use std::io::{self, Write};

use openengine_cluster_protocol::{
    ControlSelector, ControlSource, DataSelector, FieldPath, GraphNode, GraphSpec, Guard, Join,
    NonEmptyEnumSet, PositiveInteger,
};

pub(super) fn write_graph_tree(graph: &GraphSpec, output: &mut impl Write) -> io::Result<()> {
    let root = tree_row(&graph.root);
    writeln!(output, "{}", root.label)?;
    root.write_children("", output)?;
    writeln!(
        output,
        "\n* executable node: RuntimePlan binding (template delivery bindings are automatic)"
    )
}

struct TreeRow {
    label: String,
    children: Vec<Self>,
}

impl TreeRow {
    fn write_children(&self, prefix: &str, output: &mut impl Write) -> io::Result<()> {
        for (index, child) in self.children.iter().enumerate() {
            let last = index + 1 == self.children.len();
            let connector = if last { "└─ " } else { "├─ " };
            writeln!(output, "{prefix}{connector}{}", child.label)?;
            let continuation = if last { "   " } else { "│  " };
            child.write_children(&format!("{prefix}{continuation}"), output)?;
        }
        Ok(())
    }
}

fn tree_row(node: &GraphNode) -> TreeRow {
    let mut children = Vec::new();
    let details = match node {
        GraphNode::Step(node) => format!("step  {}", count(node.attempts, "attempt")),
        GraphNode::Verifier(node) => {
            let signals = node
                .signals
                .iter()
                .map(|(field, values)| format!("{field}: {}", labels(values)))
                .collect::<Vec<_>>()
                .join("; ");
            format!("verifier  {}  {signals}", count(node.attempts, "attempt"))
        }
        GraphNode::Seq(node) => {
            children = node.children.as_slice().iter().map(tree_row).collect();
            "seq".to_owned()
        }
        GraphNode::Par(node) => {
            children = node.branches.as_slice().iter().map(tree_row).collect();
            format!("par  {}", join_label(&node.join))
        }
        GraphNode::Loop(node) => {
            children.push(tree_row(&node.body));
            let mut details = format!("loop  max {}", count(node.max_iterations, "iteration"));
            if let Some(path) = &node.max_iterations_input {
                details.push_str(&format!(" (input: {})", field_path(path)));
            }
            if let Some(until) = &node.until {
                details.push_str(&format!("; until {}", guard_label(until)));
            }
            details
        }
        GraphNode::Choice(node) => {
            for branch in node.branches.as_slice() {
                let mut child = tree_row(&branch.node);
                child.label = format!("{} → {}", guard_label(&branch.when), child.label);
                children.push(child);
            }
            if let Some(otherwise) = &node.otherwise {
                let mut child = tree_row(otherwise);
                child.label = format!("otherwise → {}", child.label);
                children.push(child);
            }
            "choice  first matching branch".to_owned()
        }
        GraphNode::Map(node) => {
            children.push(tree_row(&node.body));
            format!(
                "map  over {}; max {}",
                data_label(&node.over),
                count(node.max_items, "item")
            )
        }
        GraphNode::Succeed(_) => "succeed".to_owned(),
        GraphNode::Fail(node) => format!("fail  reason: {}", node.reason.as_label()),
    };
    let marker = if matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_)) {
        " *"
    } else {
        ""
    };
    TreeRow {
        label: format!("{}{marker}  {details}", node.name()),
        children,
    }
}

fn count(value: PositiveInteger, noun: &str) -> String {
    format!(
        "{} {noun}{}",
        value.get(),
        if value.get() == 1 { "" } else { "s" }
    )
}

fn labels(values: &NonEmptyEnumSet) -> String {
    values
        .values()
        .iter()
        .map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn field_path(path: &FieldPath) -> String {
    path.segments()
        .iter()
        .map(|field| field.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn data_label(selector: &DataSelector) -> String {
    match selector {
        DataSelector::State { path } => format!("state.{}", field_path(path)),
        DataSelector::Item { path } => format!("item.{}", field_path(path)),
    }
}

fn control_label(selector: &ControlSelector) -> String {
    let source = match selector.source {
        ControlSource::Signal => "signal",
        ControlSource::Error => "error",
        ControlSource::Group => "group",
    };
    let mut label = format!("{}.{source}", selector.name);
    if let Some(field) = &selector.field {
        label.push('.');
        label.push_str(field.as_str());
    }
    label
}

fn guard_label(guard: &Guard) -> String {
    match guard {
        Guard::In {
            value,
            labels: values,
        } => format!("{} in [{}]", control_label(value), labels(values)),
        Guard::All { guards } | Guard::Any { guards } => {
            let separator = if matches!(guard, Guard::All { .. }) {
                " and "
            } else {
                " or "
            };
            format!(
                "({})",
                guards
                    .as_slice()
                    .iter()
                    .map(guard_label)
                    .collect::<Vec<_>>()
                    .join(separator)
            )
        }
        Guard::Not { guard } => format!("not ({})", guard_label(guard)),
        Guard::KOfN {
            count,
            values,
            labels: accepted,
        } => format!(
            "at least {} of ({}) in [{}]",
            count.get(),
            values
                .as_slice()
                .iter()
                .map(control_label)
                .collect::<Vec<_>>()
                .join(", "),
            labels(accepted)
        ),
        Guard::KOfMap {
            count,
            value,
            labels: accepted,
        } => format!(
            "at least {} mapped {} in [{}]",
            count.get(),
            control_label(value),
            labels(accepted)
        ),
    }
}

fn join_label(join: &Join) -> String {
    match join {
        Join::All {} => "wait for all".to_owned(),
        Join::Any {} => "wait for any".to_owned(),
        Join::Quorum { count } => format!("wait for {}", count.get()),
        Join::First { when } => format!("first matching {}", guard_label(when)),
    }
}

#[cfg(test)]
#[path = "graph_tree/tests.rs"]
mod tests;
