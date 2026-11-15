#!/usr/bin/env python3
"""Regenerate explicitly simulated replay examples from frozen profile definitions.

Normally uses the graph/runtime snapshots already in history-examples.json.
For the initial capture, pass --profiles /absolute/path/to/profiles.json.
These are rendering fixtures, not execution or reducer-conformance evidence.
"""

import argparse
import copy
import json
from pathlib import Path


OUTPUT = Path(__file__).resolve().parents[1] / "public" / "history-examples.json"
BASE_TIME = 1789646400000
NAMES = [
    "api-compatibility-release", "conference-planning-packet",
    "customer-onboarding-plan", "customer-support-drafting",
    "database-migration-package", "editorial-evidence-review",
    "flaky-test-investigation", "product-launch-content-kit",
    "security-patch-merge", "service-incident-handoff",
]
# Derived from security-patch-merge on every regeneration: its acceptance review becomes a person.
APPROVAL = "security-patch-approval"
REQUEST_WORKER = "builtin.request.ask@1"
HOUR = 3600000


def walk(node):
    yield node
    for child in node.get("children", []):
        yield from walk(child)
    for branch in node.get("branches", []):
        yield from walk(branch.get("node", branch))
    for key in ["body", "otherwise"]:
        if key in node:
            yield from walk(node[key])


def value(schema, values, key=""):
    if key in values:
        return copy.deepcopy(values[key])
    kind = schema["kind"]
    if kind == "record":
        return {name: value(field["type"], values, name)
                for name, field in schema["fields"].items()
                if field.get("required", False) or name in values}
    if kind == "array":
        return []
    if kind == "enum":
        return schema["values"][0]
    if kind == "string":
        return ""
    if kind in ["number", "integer"]:
        return 0
    if kind == "boolean":
        return False
    return None


def step(name, text, output=None, signals=None, inputs=None, item=None, error=None, tool=None, ask=None):
    return dict(name=name, text=text, output=output or {}, signals=signals,
                inputs=inputs or {}, item=item, error=error, tool=tool, ask=ask)


def ask(name, question):
    """A request node's execution: it records the question and the run pauses for a person."""
    return step(name, question, inputs={"text": question}, ask=question)


def approval_profile(merge):
    """The security patch graph with its acceptance review asked of a person instead of an agent."""
    profile = copy.deepcopy(merge)

    def rename(value):
        if isinstance(value, dict):
            return {key: ("approval" if key in ("name", "node") and item == "acceptance" else rename(item))
                    for key, item in value.items()}
        if isinstance(value, list):
            return [rename(item) for item in value]
        return value

    profile["graph"] = rename(profile["graph"])
    question = {"type": {"kind": "string"}, "required": True}
    profile["graph"]["initialInput"]["fields"]["approvalQuestion"] = question
    for node in walk(profile["graph"]["root"]):
        if node.get("state", {}).get("kind") == "record":
            node["state"]["fields"]["approvalQuestion"] = copy.deepcopy(question)
    node = next(node for node in walk(profile["graph"]["root"]) if node["name"] == "approval")
    node.pop("instructions", None)
    node.update(
        worker=REQUEST_WORKER, attempts=1,
        input={"kind": "record", "fields": {"text": copy.deepcopy(question)}},
        output={"kind": "record", "fields": {"feedback": copy.deepcopy(question)}},
        inputBindings=[{"target": ["text"], "value": {"source": "state", "path": ["approvalQuestion"]}}],
        writeBindings=[{"value": {"node": "approval", "channel": "out", "path": ["feedback"]},
                        "target": ["acceptanceFeedback"]}])
    runtime = profile["runtime"]["nodes"]
    runtime.pop("acceptance", None)
    runtime["approval"] = {"kind": "request"}
    return profile


def review(name, text, verdict="accepted", output=None):
    return step(name, text, output=output, signals={"verdict": verdict})


class Fixture:
    def __init__(self, name, profile, initial, description, index, profile_name=None, created=None):
        self.name = name
        self.profile_name = profile_name or name
        self.profile = profile
        self.run_id = "example-" + name
        self.initial = initial
        self.description = description
        self.created = created if created is not None else BASE_TIME + index * 3600000
        # The ledger's clock: every event records when it was appended (`recordedAt`).
        self.clock = self.created
        self.settled = []
        self.held = None
        self.successor = None
        self.nodes = {node["name"]: node for node in walk(profile["graph"]["root"])}
        self.values = copy.deepcopy(initial)
        self.events = []
        self.executions = 0
        self.instances = {}
        self.add(kind="run_started")
        self.add(kind="safe_log", execution=None, stream="system",
                 line="Simulated example. No agent, repository change, or delivery operation was executed.")

    def now(self):
        return self.clock

    def add(self, advance=2000, **event):
        if event["kind"] == "safe_log":
            event["timestamp"] = self.clock
        self.events.append({"cursor": f"v2:{len(self.events) + 1}", "recordedAt": self.clock, "event": event})
        self.clock += advance

    def work(self, entry):
        """A deterministic working time per step, so examples show believable durations."""
        seed = sum(ord(char) for char in entry["name"] + entry["text"])
        return (25 + seed % 420) * 1000

    def usage(self, node, ref, entry):
        """Provider-reported tokens for agent steps; other workers report none."""
        if self.profile["runtime"].get("nodes", {}).get(node["name"], {}).get("kind") != "agent":
            return
        size = len(entry["text"])
        inputs = 6000 + (size * 911) % 42000
        self.add(kind="token_usage_observed", execution=ref["execution"], advance=0,
                 usage={"inputTokens": inputs, "outputTokens": 400 + (size * 67) % 5200,
                        "cacheReadInputTokens": inputs * 3 // 5})

    def batch(self, *entries):
        active = []
        for entry in entries:
            node = self.nodes[entry["name"]]
            assert node["kind"] in ["step", "verifier"], entry["name"]
            self.executions += 1
            indices = [] if entry["item"] is None else [entry["item"]]
            occurrence = (node["name"], tuple(indices))
            self.instances.setdefault(occurrence, f"instance-{len(self.instances) + 1}")
            ref = {"runId": self.run_id, "node": node["name"],
                   "nodeInstance": self.instances[occurrence],
                   "execution": f"execution-{self.executions}"}
            inputs = value(node["input"], {**self.values, **entry["inputs"]})
            # Steps started together are one append batch, so they share a recorded time.
            self.add(kind="node_started", reference=ref, advance=0,
                     occurrence={"node": node["name"], "mapIndices": indices},
                     attempt=1, input=inputs)
            active.append((entry, node, ref, indices, inputs))
        for entry, node, ref, *_ in active:
            if entry["ask"]:
                continue
            self.clock += self.work(entry)
            self.add(kind="safe_log", execution=ref["execution"], stream="output", line=entry["text"])
            if entry["tool"]:
                self.add(kind="safe_log", execution=ref["execution"], stream="system", line=entry["tool"])
            self.usage(node, ref, entry)
        # Reverse completion order deliberately exercises overlapping execution histories.
        for entry, node, ref, indices, inputs in reversed(active):
            if entry["ask"]:
                self.request(entry, node, ref, indices, inputs)
                continue
            if entry["error"]:
                outcome = {"status": "error", "code": entry["error"], "reason": "declared_failure"}
                self.add(kind="safe_log", execution=ref["execution"], stream="error",
                         line=f"Simulated {entry['error']}: {entry['text']}")
            else:
                output = value(node["output"], entry["output"])
                outcome = {"status": "verified", "output": output, "artifacts": []}
                if node["kind"] == "verifier":
                    signals = entry["signals"] or {key: labels[0] for key, labels in node["signals"].items()}
                    assert all(label in node["signals"][key] for key, label in signals.items())
                    outcome.update(status="verifier", signals=signals,
                                   diagnostic=value(node["diagnostic"], {"message": entry["text"]}))
                    self.values.update(signals)
                self.values.update(entry["output"])
                # Capture explicit output/diagnostic writes as well as public output labels.
                for binding in node.get("writeBindings", []):
                    source = binding.get("value", {})
                    current = outcome.get(source.get("channel"))
                    for key in source.get("path", []):
                        current = current.get(key) if isinstance(current, dict) else None
                    if current is not None and len(binding["target"]) == 1:
                        self.values[binding["target"][0]] = current
            self.add(kind="node_completed", completion={"reference": ref, "outcome": outcome})
            self.settled.append((ref, node, indices, inputs, outcome))
        if self.held:
            request = self.held["request"]
            self.add(kind="suspended", suspension={
                "reason": "request", "node": request["node"], "createdAt": request["createdAt"],
                "request": {key: request[key] for key in ["id", "text", "signals", "output", "expiresAt"]}})

    def request(self, entry, node, ref, indices, inputs):
        """The request node defers, records its question, and the run pauses until an answer."""
        assert node["worker"] == REQUEST_WORKER, node["name"]
        self.add(kind="request_deferred", reference=ref)
        request = {"kind": "request", "id": f"{self.run_id}-request", "runId": self.run_id,
                   "createdAt": self.now(), "node": node["name"], "text": entry["ask"],
                   "signals": node["signals"], "output": node["output"],
                   "expiresAt": self.now() + 7 * 24 * HOUR}
        self.add(kind="vector", vector=request)
        self.held = {"ref": ref, "node": node, "indices": indices, "inputs": inputs, "request": request}

    def pause(self, successor):
        """The paused run's detail: suspended on its question, linked to the run an answer started."""
        request = self.held["request"]
        cursor = self.events[-1]["cursor"]
        detail = self.detail(cursor, "suspended", None)
        detail["suspension"] = self.events[-1]["event"]["suspension"]
        detail["workspaceRecovery"] = {"successorRunId": successor.run_id}
        return {"detail": detail, "events": self.events}

    def continue_with(self, name, signals, feedback, waited, description):
        """The successor an answer starts: earlier work carried over, then the held request settled."""
        request = self.held["request"]
        answered = request["createdAt"] + waited
        successor = Fixture(name, self.profile, self.initial, description, 0,
                            profile_name=self.profile_name, created=answered)
        successor.events = []
        successor.values = copy.deepcopy(self.values)
        successor.executions = self.executions
        successor.instances = dict(self.instances)
        # The ledger's order for a continued run: inherited vectors and the continuation record,
        # then the carried-over executions, then the run starts.
        for event in self.events:
            if event["event"]["kind"] == "vector":
                successor.add(**copy.deepcopy(event["event"]))
        successor.add(kind="vector", vector={"kind": "continuation", "id": f"{name}-continuation",
                                             "runId": successor.run_id, "createdAt": answered,
                                             "resumedFrom": self.run_id})
        for ref, node, indices, inputs, outcome in self.settled:
            successor.carry(ref, node, indices, inputs, {"Settled": {"position": 1, "outcome": outcome}})
        held = self.held
        successor.carry(held["ref"], held["node"], held["indices"], held["inputs"], "Active")
        successor.add(kind="run_started")
        successor.add(kind="safe_log", execution=None, stream="system",
                      line="Simulated example. No agent, repository change, or delivery operation was executed.")
        answer = {"signals": signals, "output": {"feedback": feedback}}
        successor.add(kind="vector", vector={"kind": "answer", "id": f"{name}-answer", "runId": successor.run_id,
                                             "createdAt": answered, "requestId": request["id"], "answer": answer})
        reference = dict(held["ref"], runId=successor.run_id)
        outcome = {"status": "verifier", "output": answer["output"], "artifacts": [], "signals": signals}
        successor.add(kind="node_completed", completion={"reference": reference, "outcome": outcome})
        successor.settled.append((held["ref"], held["node"], held["indices"], held["inputs"], outcome))
        successor.values.update(signals)
        successor.values["acceptanceFeedback"] = feedback
        self.successor = successor
        return successor

    def carry(self, ref, node, indices, inputs, state):
        self.add(kind="prior_execution", execution={
            "execution": ref["execution"], "node_instance": ref["nodeInstance"],
            "occurrence": {"node": node["name"], "mapIndices": indices},
            "attempt": 1, "input": inputs, "state": state})
        if state != "Active":
            self.settled.append((ref, node, indices, inputs, state["Settled"]["outcome"]))

    def detail(self, cursor, phase, result):
        return {
            "version": 1, "projectionVersion": 1,
            "runId": self.run_id, "title": self.name.replace("-", " ").capitalize(),
            "phase": phase, "cursor": cursor, "terminal": result,
            "createdAt": self.created, "historyAvailable": True, "example": True,
            "source": {"profile": self.profile_name, "simulation": self.description},
            "graph": self.profile["graph"], "runtime": self.profile["runtime"],
            "initialInput": self.initial,
            "history": {"initialCursor": "v2:0", "cursor": cursor, "complete": True,
                        "limitations": ["Simulated history for visual testing. No agents or delivery operations ran."]}
        }

    def finish(self, terminal_name):
        node = self.nodes[terminal_name]
        if node["kind"] == "fail":
            result = {"status": "failed", "reason": node["reason"]}
        else:
            assert node["kind"] == "succeed", terminal_name
            output_values = dict(self.values)
            for binding in node.get("bindings", []):
                current = self.values
                for key in binding.get("value", {}).get("path", []):
                    current = current.get(key) if isinstance(current, dict) else None
                if current is not None and len(binding["target"]) == 1:
                    output_values[binding["target"][0]] = current
            result = {"status": "succeeded", "output": value(node["output"], output_values)}
        self.add(kind="terminal", result=result)
        return {"detail": self.detail(self.events[-1]["cursor"], "finished", result), "events": self.events}


def build(profiles):
    examples = []

    f = Fixture(NAMES[0], profiles[NAMES[0]], {
        "task": "Release the additive customer search API without breaking existing SDKs.",
        "apiVersion": "2026-10", "clients": ["TypeScript SDK 4.x", "Python SDK 2.x"],
        "compatibilityPolicy": "Keep existing fields and pagination semantics; new filters are optional."
    }, "Parallel implementation/documentation plans; rejected pagination review; repair and approval.", 0)
    f.batch(step("assess_compatibility", "The two SDKs rely on stable cursor semantics. Recorded an additive schema plan.", {"implementationPlanPath": "artifacts/api/implementation.md"}),
            step("plan_migration_docs", "Documented optional filters and examples for both SDKs.", {"documentationPlanPath": "artifacts/api/migration.md"}))
    f.batch(step("implement_api_and_docs", "Implemented optional filters and added API/SDK regression tests."))
    f.batch(review("compatibility_review", "Rejected: the empty-page cursor changed from null to an empty string.", "rejected", {"compatibilityFeedback": "Preserve null end cursors for both SDKs."}),
            review("migration_review", "Examples cover both SDKs and existing defaults.", output={"migrationFeedback": "Migration examples are complete."}))
    f.batch(step("prioritize_repairs", "Prioritized the cursor compatibility fix and one regression test.", {"repairPlanPath": "artifacts/api/repair-plan.md"}))
    f.batch(step("implement_api_and_docs", "Restored null end cursors; added regression cases for empty and final pages."))
    f.batch(review("compatibility_review", "Both SDK compatibility suites pass; cursor semantics are preserved.", output={"compatibilityFeedback": "Accepted: compatibility checks pass."}),
            review("migration_review", "Published examples match the revised API contract.", output={"migrationFeedback": "Accepted: examples and schema agree."}))
    examples.append(f.finish("release_package_ready"))

    f = Fixture(NAMES[1], profiles[NAMES[1]], {
        "task": "Prepare a practical one-day engineering conference packet.",
        "city": "Berlin", "date": "2026-11-12", "attendees": 120, "budget": 24000,
        "accessibility": "Step-free access, captioned talks, quiet room, vegetarian and vegan catering."
    }, "Parallel venue and agenda files; budget conflict corrected in a second planning round.", 1)
    for round_number in [1, 2]:
        f.batch(step("venue_logistics", "Wrote venue logistics with a step-free route and quiet-room allocation." if round_number == 1 else "Changed the venue option to remove the after-hours staffing surcharge."),
                step("agenda_design", "Wrote the session schedule, breaks, and captioning requirements."))
        f.batch(step("budget_scenarios", "Prepared itemized costs including a 10% contingency."))
        feedback = "Venue staffing adds EUR 2,600 beyond the budget. Choose the daytime package." if round_number == 1 else "Total EUR 23,100 includes captioning and contingency; all access needs are covered."
        f.batch(review("feasibility_review", feedback, "rejected" if round_number == 1 else "accepted", {"reviewFeedback": feedback}))
    f.batch(step("prepare_planning_packet", "Combined the agenda, venue plan, accessibility checklist, and approved budget.", {"packetPath": "artifacts/conference/planning-packet.md"}))
    examples.append(f.finish("packet_ready"))

    f = Fixture(NAMES[2], profiles[NAMES[2]], {
        "task": "Prepare an onboarding plan for an existing customer expanding to two new teams.",
        "customerGoals": "All products are already provisioned. Focus on team training, ownership, and rollout; no product-specific setup is required.",
        "products": [], "targetDate": "2026-10-05"
    }, "Empty product map; training and rollout proceed concurrently; final plan accepted.", 2)
    f.batch(step("assess_requirements", "Existing provisioning is confirmed. No product-specific plans are needed.",
                 {"questions": "", "requirementsSummary": "Train two new teams using the existing deployment; assign rollout owners."}, {"readiness": "ready"}))
    f.values["productPlanPaths"] = []
    f.batch(step("training_design", "Wrote role-based training sessions for administrators and operators.", {"trainingPlanPath": "artifacts/onboarding/training.md"}),
            step("rollout_design", "Wrote a staged rollout with team owners and a feedback checkpoint.", {"rolloutPlanPath": "artifacts/onboarding/rollout.md"}))
    f.batch(step("compose_onboarding", "Combined the training and rollout files; explicitly recorded that no product setup is pending.", {"onboardingPlanPath": "artifacts/onboarding/plan.md"}))
    f.batch(review("verify_onboarding", "Dates, responsibilities, and success criteria are complete.", output={"reviewFeedback": "Accepted: the plan fits the existing customer scope."}))
    examples.append(f.finish("onboarding_ready"))

    f = Fixture(NAMES[3], profiles[NAMES[3]], {
        "task": "Draft a support reply for review; do not send it.", "product": "Team analytics",
        "customerMessage": "Our invoice includes seats we removed last week. Can you explain the charge?",
        "accountContext": "Fictional account: 18 seats before September 10, 12 afterward; monthly billing in arrears.",
        "supportPolicy": "Explain proration from invoice evidence. Never promise a refund before billing review."
    }, "Billing branch only; first reply overpromises a refund; reviewer requests a safe revision.", 3)
    f.batch(step("classify_case", "This is an invoice proration question.", {"caseSummary": "Customer questions prorated charges after removing six seats."}, {"category": "billing"}))
    for round_number in [1, 2]:
        f.batch(step("billing_reply", "Wrote a draft explaining the billing period and a separate internal escalation note.", {"replyPath": "artifacts/support/reply.md", "escalationNotePath": "artifacts/support/billing-note.md"}))
        feedback = "Remove the unconditional refund promise. Ask billing to confirm the line items." if round_number == 1 else "Reply explains proration, names the next step, and makes no unsupported promises."
        f.batch(review("billing_review", feedback, "rejected" if round_number == 1 else "accepted", {"reviewFeedback": feedback}))
    examples.append(f.finish("billing_reply_ready"))

    f = Fixture(NAMES[4], profiles[NAMES[4]], {
        "task": "Prepare an expand/backfill/contract migration adding an account region without blocking writes.",
        "affectedTables": ["accounts", "invoice_events"], "databaseEngine": "PostgreSQL 16", "allowedDowntimeMinutes": 0
    }, "Parallel compatibility and rollback reviews; missing rollback boundary repaired.", 4)
    f.batch(step("worker", "Wrote migration SQL, a bounded backfill procedure, and an operator checklist."))
    f.batch(review("compatibility_review", "Old and new application versions can run concurrently."),
            review("rollback_review", "Rejected: rollback instructions do not define the safe cutoff before column removal.", "rejected"))
    f.batch(step("revise_migration", "Separated reversible rollout from final cleanup and documented the rollback cutoff."))
    f.batch(review("compatibility_review", "Concurrent old/new writes remain valid throughout the expand phase."),
            review("rollback_review", "Rollback preserves backfilled data and excludes irreversible cleanup until sign-off."))
    examples.append(f.finish("done"))
    return examples + build_more(profiles)


def build_more(profiles):
    examples = []
    claims = [
        {"text": "Inspection leaves available quota unchanged.", "source": "notes/quota-observation.md"},
        {"text": "Tenant isolation prevents shared client IDs from colliding.", "source": "notes/tenant-isolation.md"},
        {"text": "Lower retry volume was observed during the pilot.", "source": "notes/pilot-metrics.csv"},
    ]
    f = Fixture(NAMES[5], profiles[NAMES[5]], {
        "task": "Prepare an evidence-backed editorial packet about tenant-safe quota inspection.",
        "audience": "Engineering managers", "claims": claims,
        "researchBrief": "Distinguish measured pilot observations from guaranteed product behavior."
    }, "Three mapped evidence writers; nested parallel reviews repeat; long evidence transcript remains inspectable.", 5)
    long_excerpt = "Simulated evidence extraction transcript\n\n" + "\n".join(
        f"Observation {i:02}: tenant-scoped quota inspection preserves consumption counters. Pilot sample {i} is observational; no causal performance claim is justified."
        for i in range(1, 81))
    f.batch(*(step("check_claim", f"Checked claim {i + 1} against its supplied source and wrote a separate finding file.",
                    {"findingPath": f"artifacts/editorial/item-{i + 1}/finding.md"},
                    inputs={"claimText": claim["text"], "sourceReference": claim["source"]}, item=i,
                    tool=long_excerpt if i == 1 else None) for i, claim in enumerate(claims)))
    f.values["findingPaths"] = [f"artifacts/editorial/item-{i + 1}/finding.md" for i in range(3)]
    for round_number in [1, 2]:
        f.batch(step("compose_outline", "Wrote an outline and editor packet linking every claim to a finding file.", {"outlinePath": "artifacts/editorial/outline.md", "editorPacketPath": "artifacts/editorial/packet.md"}))
        feedback = "Label the retry-volume change as a pilot observation; do not promise a performance gain." if round_number == 1 else "All claims have source support and the pilot limitation is stated."
        f.batch(review("evidence_review", feedback, "rejected" if round_number == 1 else "accepted", {"evidenceFeedback": feedback}),
                review("clarity_review", "The outline explains the user problem before introducing implementation terms.", output={"clarityFeedback": "Accepted: concise and readable for engineering managers."}))
    examples.append(f.finish("editor_packet_ready"))

    f = Fixture(NAMES[6], profiles[NAMES[6]], {
        "task": "Investigate intermittent CI failures and produce an evidence report.",
        "suites": [{"name": "quota-clock-boundary"}, {"name": "tenant-isolation"}, {"name": "retry-backoff"}],
        "failureLog": "quota-clock-boundary failed twice near a window rollover; retry-backoff stalled awaiting the fake clock.",
        "testCommand": "npm test -- --runInBand"
    }, "Three concurrent mapped investigations; one times out; report nodes remain unreached.", 6)
    f.batch(step("investigate_suite", "Reproduced the exact window-boundary condition and wrote the evidence.", {"findingPath": "artifacts/flaky/item-1/finding.md"}, inputs={"suiteName": "quota-clock-boundary"}, item=0),
            step("investigate_suite", "Tenant isolation passed repeated seeded runs.", {"findingPath": "artifacts/flaky/item-2/finding.md"}, inputs={"suiteName": "tenant-isolation"}, item=1),
            step("investigate_suite", "The fake-clock retry harness did not settle before the authored deadline.", inputs={"suiteName": "retry-backoff"}, item=2, error="timeout"))
    examples.append(f.finish("suite_collection_failed"))

    f = Fixture(NAMES[7], profiles[NAMES[7]], {
        "task": "Prepare a launch kit for the new read-only quota dashboard.", "audience": "Operations teams",
        "productFacts": "Shows remaining quota and reset time. Available on existing paid plans. No automatic capacity reservation.",
        "tone": "Direct, calm, specific", "forbiddenClaims": ["Unlimited quota", "Guaranteed cost savings"]
    }, "Three parallel copy writers, kit assembly, parallel reviews, and one revision round.", 7)
    for round_number in [1, 2]:
        f.batch(step("website_copy", "Wrote a concise page focused on quota visibility.", {"websitePath": "artifacts/launch/website.md"}),
                step("email_copy", "Wrote a short announcement with an existing-plan availability note.", {"emailPath": "artifacts/launch/email.md"}),
                step("social_copy", "Wrote three short launch posts with a link placeholder.", {"socialPath": "artifacts/launch/social.md"}))
        f.batch(step("assemble_kit", "Assembled the channel files and claims checklist.", {"kitPath": "artifacts/launch/kit.md", "checklistPath": "artifacts/launch/claims.md"}))
        feedback = "Remove 'reserve capacity' from the website. Inspection does not reserve quota." if round_number == 1 else "Every claim matches the supplied facts; no capacity reservation is promised."
        f.batch(review("factual_review", feedback, "rejected" if round_number == 1 else "accepted", {"factualFeedback": feedback}),
                review("brand_review", "All three channels use a direct, specific tone.", output={"brandFeedback": "Accepted: consistent and concise."}))
        if round_number == 1:
            f.batch(step("repair_plan", "Wrote a targeted correction for the unsupported website claim.", {"repairPlanPath": "artifacts/launch/repair.md"}))
    examples.append(f.finish("launch_kit_ready"))

    f = Fixture(NAMES[8], profiles[NAMES[8]], {
        "task": "Patch tenant-boundary authorization and include focused regression coverage.", "issueNumber": "42"
    }, "Simulated delivery CI failure, repair, repeated reviews, and merge receipt. No network or delivery ran.", 8)
    f.batch(step("worker", "Added the missing tenant boundary check and regression cases."))
    for round_number in [1, 2]:
        f.batch(review("acceptance", "Tenant-boundary acceptance cases pass.", output={"title": "fix: enforce tenant ownership on quota lookup", "description": "Check account ownership before returning quota metadata. Includes same-ID cross-tenant regression coverage."}),
                review("code", "The guard runs before lookup, and public errors do not reveal another tenant's account."))
        delivery = "ci_failed" if round_number == 1 else "merged"
        f.batch(step("deliver", "Simulated required check failure: the formatting check rejected the new test fixture." if round_number == 1 else "Simulated merge receipt: required checks passed and the exact candidate was merged.",
                     {"headRevision": "1111111111111111111111111111111111111111", "mergeRevision": "" if round_number == 1 else "2222222222222222222222222222222222222222", "mode": "merge", "outcome": delivery, "pullRequestId": "example-pr-42", "repository": "example/simulated-repository", "targetBranch": "main", "version": "v2"}, {"delivery": delivery}))
        if round_number == 1:
            f.batch(step("delivery_repair", "Corrected fixture formatting and reran the focused checks."))
    examples.append(f.finish("done"))

    f = Fixture(NAMES[9], profiles[NAMES[9]], {
        "serviceName": "Tenant quota API", "severityThreshold": 2,
        "incidentNotes": "Fictional incident: 09:02 elevated lookup latency; 09:06 read replicas lagged; 09:14 traffic shifted; 09:21 latency recovered. Two enterprise tenants saw delayed dashboards; no quota was consumed incorrectly."
    }, "Parallel incident analysis, urgent classification, response plan, and verified handoff.", 9)
    f.batch(step("reconstruct_timeline", "Wrote a timestamped incident timeline separating observations from hypotheses.", {"timelinePath": "artifacts/incident/timeline.md"}),
            step("assess_impact", "Wrote impact scope: delayed dashboards for two tenants; no incorrect consumption.", {"impactPath": "artifacts/incident/impact.md"}))
    f.batch(step("classify_urgency", "Enterprise customer impact meets the escalation threshold.", signals={"urgency": "urgent"}))
    f.batch(step("prepare_response_plan", "Assigned the on-call owner, replica-health checks, and a rollback condition.", {"responsePlanPath": "artifacts/incident/response.md"}))
    f.batch(step("draft_handoff", "Wrote a concise handoff linking the timeline, impact, owner, and next checkpoint.", {"handoffPath": "artifacts/incident/handoff.md"}))
    f.batch(review("verify_handoff", "The urgent handoff includes evidence, owners, and the next update time.", output={"reviewFeedback": "Accepted: complete and actionable."}))
    examples.append(f.finish("urgent_handoff_ready"))
    return examples + build_approval(profiles)


def build_approval(profiles):
    """Three runs of one patch: asked, rejected with feedback and asked again, then approved."""
    profile = approval_profile(profiles[NAMES[8]])
    initial = {"task": "Patch tenant-boundary authorization and include focused regression coverage.",
               "issueNumber": "42",
               "approvalQuestion": "Does this patch close the tenant boundary bug? Approve, or reject with what is missing."}
    review_code = lambda text: review("code", text)
    first = Fixture("security-patch-approval", profile, initial,
                    "A person reviews the patch: the run pauses for an answer, and each answer continues it in a new run.",
                    10, profile_name=APPROVAL)
    first.batch(step("worker", "Added the missing tenant boundary check and regression cases."))
    first.batch(ask("approval", initial["approvalQuestion"]),
                review_code("The guard runs before lookup, and public errors do not reveal another tenant's account."))
    second = first.continue_with("security-patch-approval-revised", {"verdict": "rejected"},
                                 "Add a regression test for tenants with no accounts.", 2 * HOUR + 14 * 60000,
                                 "The person rejected the patch with feedback; the repair is asked about again.")
    second.batch(step("review_repair", "Added a regression test for tenants with no accounts."))
    second.values["approvalQuestion"] = "Regression test added for tenants with no accounts. Approve the patch now?"
    second.batch(ask("approval", second.values["approvalQuestion"]),
                 review_code("Empty-tenant case is covered and passes."))
    third = second.continue_with("security-patch-approval-merged", {"verdict": "accepted"},
                                 "Looks good.", 38 * 60000,
                                 "The person approved the revised patch, and it was delivered.")
    third.batch(step("deliver", "Simulated merge receipt: required checks passed and the exact candidate was merged.",
                     {"headRevision": "3333333333333333333333333333333333333333", "mergeRevision": "4444444444444444444444444444444444444444", "mode": "merge", "outcome": "merged", "pullRequestId": "example-pr-42", "repository": "example/simulated-repository", "targetBranch": "main", "version": "v2"},
                     {"delivery": "merged"}))
    return [first.pause(second), second.pause(third), third.finish("done")]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profiles", type=Path)
    args = parser.parse_args()
    if args.profiles:
        profiles = json.loads(args.profiles.read_text())["profiles"]
    else:
        existing = json.loads(OUTPUT.read_text())
        profiles = {item["detail"]["source"]["profile"]: item["detail"] for item in existing}
    examples = build(profiles)
    assert len(examples) == 13
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(examples, ensure_ascii=False, indent=2) + "\n")
    print(f"Wrote {len(examples)} simulated examples, {sum(len(x['events']) for x in examples)} events.")


if __name__ == "__main__":
    main()
