#!/usr/bin/env python3
"""Build the pull-request review graph from its prompt files.

The graph is a general code-review workflow. Triage summarises the change and
splits it into review units. Four reviewers examine every unit: two broad
reviews (one per model family), a depth review and a design review. Five more
reviewers each hunt one kind of failure across the whole change. Then, for
every unit, one agent merges the findings for that unit's files, tries to
disprove each one and rewrites the survivors; one more agent does the same for
findings outside every unit. A reviewer that fails leaves its findings empty
instead of failing the run. Nothing in the graph refers to a benchmark; the
adapter in ../image maps its output to a benchmark's file format.

    build_graph.py    # writes ../config/review.graph.json from prompts/
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
ERRORS = ["crash", "malformed", "refusal", "timeout"]

# Per-unit reviewer groups and their prompts; any other group repeats the broad review.
GROUPS = "abcd"
GROUP_PROMPTS = {"a": "unit_review", "b": "unit_depth", "d": "unit_design"}
# Whole-change reviewers, each with the prompt file of the same name.
WHOLE = ["hunt_logic", "hunt_state", "hunt_contracts", "hunt_errors", "hunt_claims"]
MAX_UNITS = 6 + 2  # triage is asked for at most 6 units; the maps allow 2 more
ATTEMPTS = 2
TRIAGE_MS = 4 * 60_000
REVIEW_MS = 13 * 60_000
FINALIZE_MS = 8 * 60_000


# --- payload types -----------------------------------------------------------


def string():
    return {"kind": "string"}


def integer():
    return {"kind": "integer"}


def array(items):
    return {"kind": "array", "items": items}


def enum(*values):
    return {"kind": "enum", "values": sorted(values)}


def record(**fields):
    """Fields map to a type (required) or to (type, required)."""
    out = {}
    for name in sorted(fields):
        value = fields[name]
        kind, required = value if isinstance(value, tuple) else (value, True)
        out[name] = {"type": kind, "required": required}
    return {"kind": "record", "fields": out}


FINDING = record(
    file=string(),
    startLine=integer(),
    endLine=integer(),
    severity=enum("high", "medium", "low"),
    title=string(),
    body=string(),
)
FINDINGS = array(FINDING)
CHANGED_FILE = record(path=string(), status=string(), added=integer(), removed=integer())
UNIT = record(name=string(), files=array(string()), focus=string())

INPUT_FIELDS = dict(
    title=string(),
    body=string(),
    repository=string(),
    baseRevision=string(),
    headRevision=string(),
    diffPath=string(),
    changedFiles=array(CHANGED_FILE),
)


def whole_field(name: str) -> str:
    """State field for a whole-change reviewer: `hunt_logic` -> `huntLogicFindings`."""
    head, *rest = name.split("_")
    return head + "".join(part.capitalize() for part in rest) + "Findings"


def make_state() -> dict:
    """Graph state. Every field is required, so each starts at its type's empty value and
    a reviewer that fails leaves its findings empty."""
    return record(
        **INPUT_FIELDS,
        summary=string(),
        units=array(UNIT),
        **{f"unitFindings{group.upper()}": array(FINDINGS) for group in GROUPS},
        **{whole_field(name): FINDINGS for name in WHOLE},
        unitFinal=array(FINDINGS),
        restFinal=FINDINGS,
    )


STATE = make_state()


# --- nodes -------------------------------------------------------------------


def state_value(*path):
    return {"source": "state", "path": list(path)}


def item_value(*path):
    return {"source": "item", "path": list(path)}


def agent(name, instructions, inputs, output_fields, writes, timeout_ms):
    """A read-only agent node.

    Every agent here inspects the checkout without changing it, so each one is a
    verifier with no signals: Zeroshot then gives it read-only runtime guidance
    instead of the worker guidance that installs dependencies.

    inputs: {target: (type, selector)}; writes: {state_path: output_field}.
    """
    return {
        "kind": "verifier",
        "name": name,
        "worker": f"review.{name}@1",
        "instructions": instructions,
        "input": record(**{target: kind for target, (kind, _) in inputs.items()}),
        "inputBindings": [
            {"target": [target], "value": selector} for target, (_, selector) in sorted(inputs.items())
        ],
        "output": record(**output_fields),
        "writeBindings": [
            {"value": {"node": name, "channel": "out", "path": [field]}, "target": [path]}
            for path, field in sorted(writes.items())
        ],
        "attempts": ATTEMPTS,
        "signals": {},
        "diagnostic": {"kind": "null"},
        "timeoutMs": timeout_ms,
    }


def seq(name, children, promoted=()):
    return {
        "kind": "seq",
        "name": name,
        "state": STATE,
        "children": children,
        "promotedStatePaths": [[p] for p in promoted],
    }


def par(name, branches, promoted):
    return {
        "kind": "par",
        "name": name,
        "state": STATE,
        "branches": branches,
        "promotedStatePaths": [[p] for p in promoted],
        "join": {"kind": "all"},
    }


def map_node(name, over, body, max_items, promoted):
    return {
        "kind": "map",
        "name": name,
        "state": STATE,
        "body": body,
        "over": state_value(over),
        "maxItems": max_items,
        "promotedStatePaths": [[p] for p in promoted],
    }


def on_error(name, guard, then):
    """Route a failure to an explicit terminal, otherwise continue."""
    return {
        "kind": "choice",
        "name": name,
        "state": STATE,
        "branches": [{"when": guard, "node": {"kind": "fail", "name": f"{name}_failed", "reason": f"{name}_failed"}}],
        "otherwise": then,
        "promotedStatePaths": [],
    }


def step_failed(node):
    return {"kind": "in", "value": {"name": node, "source": "error", "field": None}, "labels": ERRORS}


def map_failed(map_name, body_name):
    return {
        "kind": "any",
        "guards": [
            {"kind": "in", "value": {"name": map_name, "source": "group", "field": "overflow"}, "labels": ["overflow"]},
            {"kind": "k_of_map", "count": 1, "value": {"name": body_name, "source": "error", "field": None}, "labels": ERRORS},
        ],
    }


# --- prompts -----------------------------------------------------------------


def load_prompts(directory: Path) -> dict[str, str]:
    texts = {path.stem: path.read_text().strip() for path in sorted(directory.glob("*.md"))}

    def expand(text: str) -> str:
        return re.sub(r"\{\{(\w+)\}\}", lambda m: expand(texts[m.group(1)]), text)

    return {name: expand(text) for name, text in texts.items()}


# --- graph -------------------------------------------------------------------


def build(prompts: dict[str, str]) -> dict:
    pr_context = {
        "title": (string(), state_value("title")),
        "body": (string(), state_value("body")),
        "repository": (string(), state_value("repository")),
        "diffPath": (string(), state_value("diffPath")),
    }
    summary = (string(), state_value("summary"))

    triage = agent(
        "triage",
        prompts["triage"],
        dict(pr_context, changedFiles=(array(CHANGED_FILE), state_value("changedFiles"))),
        {"summary": string(), "units": array(UNIT)},
        {"summary": "summary", "units": "units"},
        TRIAGE_MS,
    )

    def unit_reviewer(group):
        return agent(
            f"unit_review_{group}",
            prompts[GROUP_PROMPTS.get(group, "unit_review")],
            dict(
                pr_context,
                summary=summary,
                unitName=(string(), item_value("name")),
                unitFiles=(array(string()), item_value("files")),
                unitFocus=(string(), item_value("focus")),
            ),
            {"findings": FINDINGS},
            {f"unitFindings{group.upper()}": "findings"},
            REVIEW_MS,
        )

    whole_inputs = dict(
        pr_context,
        summary=summary,
        changedFiles=(array(CHANGED_FILE), state_value("changedFiles")),
        units=(array(UNIT), state_value("units")),
    )
    whole_reviewers = [
        agent(name, prompts[name], whole_inputs, {"findings": FINDINGS}, {whole_field(name): "findings"}, REVIEW_MS)
        for name in WHOLE
    ]
    discover = par(
        "discover",
        [
            map_node(f"units_{g}", "units", unit_reviewer(g), MAX_UNITS, [f"unitFindings{g.upper()}"])
            for g in GROUPS
        ] + whole_reviewers,
        [f"unitFindings{g.upper()}" for g in GROUPS] + [whole_field(name) for name in WHOLE],
    )

    # Merge and verify each unit's files in parallel, plus the files outside every unit.
    all_findings = {
        **{f"unitFindings{g.upper()}": (array(FINDINGS), state_value(f"unitFindings{g.upper()}")) for g in GROUPS},
        **{whole_field(name): (FINDINGS, state_value(whole_field(name))) for name in WHOLE},
    }
    finalize_unit = agent(
        "finalize_unit",
        prompts["finalize"],
        dict(pr_context, summary=summary, unitName=(string(), item_value("name")),
             unitFiles=(array(string()), item_value("files")), **all_findings),
        {"findings": FINDINGS},
        {"unitFinal": "findings"},
        FINALIZE_MS,
    )
    finalize_rest = agent(
        "finalize_rest",
        prompts["finalize_rest"],
        dict(pr_context, summary=summary, units=(array(UNIT), state_value("units")), **all_findings),
        {"findings": FINDINGS},
        {"restFinal": "findings"},
        FINALIZE_MS,
    )
    done = {
        "kind": "succeed",
        "name": "done",
        "output": record(findings=array(FINDINGS), restFindings=FINDINGS),
        "bindings": [{"target": ["findings"], "value": state_value("unitFinal")},
                     {"target": ["restFindings"], "value": state_value("restFinal")}],
    }
    finalized = seq(
        "merged",
        [
            par("finalize", [
                map_node("finalize_units", "units", finalize_unit, MAX_UNITS, ["unitFinal"]),
                finalize_rest,
            ], ["unitFinal", "restFinal"]),
            on_error("finalize_units_result", map_failed("finalize_units", "finalize_unit"),
                     on_error("finalize_rest_result", step_failed("finalize_rest"), done)),
        ],
    )
    # No guard between the reviews and finalization: a reviewer that fails or times out
    # leaves its findings field empty, and finalization works with the others' findings.
    reviewed = seq("reviewed", [discover, finalized])
    root = seq("run", [triage, on_error("triage_result", step_failed("triage"), reviewed)])
    return {
        "profile": "openengine.graph.full/v1",
        "initialInput": record(**INPUT_FIELDS),
        "policy": {"policy": "policy.native-v2@1", "default": "deny"},
        "root": root,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--prompts", type=Path, default=HERE / "prompts")
    parser.add_argument("--out", type=Path, default=HERE.parent / "config" / "review.graph.json")
    args = parser.parse_args()
    graph = build(load_prompts(args.prompts))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(graph, indent=2) + "\n")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
