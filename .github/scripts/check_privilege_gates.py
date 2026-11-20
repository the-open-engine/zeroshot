#!/usr/bin/env python3
"""Enforce that tests gated on effective uid 0 are named root_*.

The privileged CI lanes select tests with the libtest `root_` substring filter
(.github/workflows/ci.yml and .github/workflows/coverage.yml). A test that
returns early unless it runs as root is therefore unreachable in every lane
unless its name starts with root_.

A test that needs root for only part of its body keeps running unprivileged and
uses an `if unsafe { libc::geteuid() } == 0 { ... }` block instead.

Exit codes:
  0  every root-gated test is named root_* and both lanes still select them
  1  at least one root-gated test is not named root_*
  2  the walk found no root-gated tests, so the parser is broken
  3  a workflow lane no longer passes the root_ filter to the lib test binary
"""

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
SOURCE_ROOT = ROOT / "zeroshot" / "src"
WORKFLOWS = (
    ROOT / ".github" / "workflows" / "ci.yml",
    ROOT / ".github" / "workflows" / "coverage.yml",
)

TEST_ATTRIBUTE = re.compile(r"#\[\s*(?:test|tokio::test|rstest)\b[^\]]*\]")
ATTRIBUTE = re.compile(r"#\[[^\]]*\]")
SIGNATURE = re.compile(
    r"(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\("
)
ROOT_CONDITION = re.compile(
    r"geteuid\s*\(\s*\)\s*\}?\s*!=\s*0"
    r"|0\s*!=[^;]*geteuid\s*\(\s*\)"
)
IF_KEYWORD = re.compile(r"\bif\b")
LET_OR_USE = re.compile(r"\b(?:let|use)\b[^;]*;", re.DOTALL)
TEST_THREADS = re.compile(r"--test-threads=1")
ROOT_FILTER = re.compile(r"root_")
RAW_STRING_PREFIX = re.compile(r'(?:b|c)?r(#*)"')


def blank(masked, start, end):
    """Blank a span while preserving newlines so line numbers stay aligned."""
    for index in range(start, min(end, len(masked))):
        if masked[index] != "\n":
            masked[index] = " "


def mask_literals(source):
    """Blank comments and string/char literals so braces only come from code."""
    masked = list(source)
    index = 0
    while index < len(source):
        char = source[index]
        if char == "/" and source.startswith("//", index):
            end = source.find("\n", index)
            if end < 0:
                end = len(source)
            blank(masked, index, end)
            index = end
        elif char == "/" and source.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < len(source) and depth:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            blank(masked, index, end)
            index = end
        elif char == '"':
            end = index + 1
            while end < len(source) and source[end] != '"':
                end += 2 if source[end] == "\\" else 1
            blank(masked, index, end + 1)
            index = end + 1
        elif char in "bc" and source.startswith('"', index + 1):
            end = index + 2
            while end < len(source) and source[end] != '"':
                end += 2 if source[end] == "\\" else 1
            blank(masked, index, end + 1)
            index = end + 1
        elif char == "r" or (char in "bc" and source.startswith("r", index + 1)):
            raw = RAW_STRING_PREFIX.match(source, index)
            if raw is None:
                index += 1
                continue
            terminator = '"' + raw.group(1)
            end = source.find(terminator, raw.end())
            end = len(source) if end < 0 else end + len(terminator)
            blank(masked, index, end)
            index = end
        elif char == "'":
            char_literal = re.match(r"'(?:\\.|[^'\\])'", source[index:])
            if char_literal is None:
                index += 1
                continue
            blank(masked, index, index + char_literal.end())
            index += char_literal.end()
        else:
            index += 1
    return "".join(masked)


def line_of(source, index):
    return source.count("\n", 0, index) + 1


def matching_brace(masked, open_index):
    depth = 0
    for index in range(open_index, len(masked)):
        if masked[index] == "{":
            depth += 1
        elif masked[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    return -1


def find_signature(masked, start):
    """Skip attributes and whitespace, then return (name, params_open)."""
    index = start
    while True:
        while index < len(masked) and masked[index].isspace():
            index += 1
        attribute = ATTRIBUTE.match(masked, index)
        if attribute is None:
            break
        index = attribute.end()
    signature = SIGNATURE.match(masked, index)
    if signature is None:
        return None
    return signature.group(1), signature.end() - 1


def find_body(masked, start):
    """Return (name, body_open) for the function following a test attribute."""
    found = find_signature(masked, start)
    if found is None:
        return None
    name, params_open = found
    depth = 0
    index = params_open
    while index < len(masked):
        if masked[index] == "(":
            depth += 1
        elif masked[index] == ")":
            depth -= 1
            if depth == 0:
                break
        index += 1
    index += 1
    while index < len(masked):
        if masked[index] == ";":
            return None
        if masked[index] == "{":
            return name, index
        index += 1
    return None


def top_level_return(block):
    depth = 0
    index = 0
    while index < len(block):
        char = block[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        elif depth == 0 and block.startswith("return", index):
            before = block[:index].rstrip()
            after = block[index + len("return") : index + len("return") + 1]
            word_end = not (after.isalnum() or after == "_")
            if word_end and (not before or before[-1] in ";{}"):
                return True
        index += 1
    return False


def find_gate(masked, body_open, body_close):
    """Return the offset of a leading hard root gate, or None."""
    body = masked[body_open + 1 : body_close]
    for match in ROOT_CONDITION.finditer(body):
        keywords = list(IF_KEYWORD.finditer(body[: match.start()]))
        if not keywords:
            continue
        if_at = keywords[-1].start()
        if ";" in body[if_at : match.start()]:
            continue
        brace = body.find("{", match.end())
        if brace < 0 or ";" in body[if_at:brace]:
            continue
        prefix = ATTRIBUTE.sub("", body[:if_at])
        prefix = LET_OR_USE.sub("", prefix)
        if prefix.strip():
            continue
        close = matching_brace(masked, body_open + 1 + brace)
        if close < 0 or close > body_close:
            continue
        if not top_level_return(masked[body_open + 1 + brace + 1 : close]):
            continue
        return body_open + 1 + if_at
    return None


def gated_tests(path):
    text = path.read_text(encoding="utf-8")
    masked = mask_literals(text)
    found = []
    seen = set()
    for attribute in TEST_ATTRIBUTE.finditer(masked):
        body = find_body(masked, attribute.end())
        if body is None:
            continue
        name, body_open = body
        if body_open in seen:
            continue
        seen.add(body_open)
        body_close = matching_brace(masked, body_open)
        if body_close < 0:
            continue
        gate = find_gate(masked, body_open, body_close)
        if gate is not None:
            found.append((name, line_of(masked, gate)))
    return found


def lanes_select_gates():
    """Return workflow paths that no longer pass the root_ filter."""
    broken = []
    for workflow in WORKFLOWS:
        try:
            text = workflow.read_text(encoding="utf-8")
        except OSError:
            broken.append(workflow)
            continue
        selected = False
        for invocation in TEST_THREADS.finditer(text):
            window = text[max(0, invocation.start() - 240) : invocation.end() + 240]
            if ROOT_FILTER.search(window):
                selected = True
                break
        if not selected:
            broken.append(workflow)
    return broken


def main():
    gated = []
    for path in sorted(SOURCE_ROOT.rglob("*.rs")):
        for name, line in gated_tests(path):
            gated.append((path.relative_to(ROOT), name, line))
    if not gated:
        print(
            "check_privilege_gates: found no root-gated tests under zeroshot/src; "
            "the test parser is broken",
            file=sys.stderr,
        )
        return 2
    broken = lanes_select_gates()
    if broken:
        for workflow in broken:
            relative = (
                workflow.relative_to(ROOT) if workflow.is_relative_to(ROOT) else workflow
            )
            print(
                f"check_privilege_gates: {relative} no longer runs the lib test binary "
                "with the root_ filter, so root_* tests are unreachable",
                file=sys.stderr,
            )
        return 3
    violations = [entry for entry in gated if not entry[1].startswith("root_")]
    for path, name, line in sorted(violations, key=lambda entry: (str(entry[0]), entry[2])):
        print(f"{path}:{line} {name}: root-gated test must be named root_*")
    print(f"checked {len(gated)} privileged tests, {len(violations)} violations")
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
