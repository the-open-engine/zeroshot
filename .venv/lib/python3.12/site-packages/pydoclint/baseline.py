from __future__ import annotations

from itertools import groupby
from pathlib import Path

from pydoclint.utils.violation import Violation

SEPARATOR = '--------------------\n'  # 20 dashes
LEN_INDENT = 4
ONE_SPACE = ' '
INDENT = ONE_SPACE * LEN_INDENT


def generateBaseline(
        violationsAllFiles: dict[str, list[Violation]] | dict[str, list[str]],
        path: Path,
) -> None:
    """Generate baseline file based of passed violations."""
    with path.open('w', encoding='utf-8') as baseline:
        for file, violations in violationsAllFiles.items():
            if violations:
                baseline.write(f'{file}\n')
                for violation in violations:
                    baseline.write(f'{INDENT}{str(violation).strip()}\n')

                baseline.write(f'{SEPARATOR}')


def parseBaseline(path: Path) -> dict[str, list[str]]:
    """Parse baseline file."""
    with path.open('r', encoding='utf-8') as baseline:
        parsed: dict[str, list[str]] = {}
        splittedFiles = [
            list(group)
            for key, group in groupby(
                baseline.readlines(), lambda x: x == SEPARATOR
            )
            if not key
        ]
        for file in splittedFiles:
            parsed[file[0].strip()] = [func.strip() for func in file[1:]]

        return parsed


def reEvaluateBaseline(
        baseline: dict[str, list[str]],
        actualViolationsInAllFiles: dict[str, list[Violation]],
) -> tuple[bool, dict[str, list[str]], dict[str, list[Violation]]]:
    """
    Re-evaluate baseline violations, dropping those that are already fixed
    by the users, and calculating those that still need to be fixed.

    Parameters
    ----------
    baseline : dict[str, list[str]]
        The baseline violations, parsed from the baseline file
    actualViolationsInAllFiles : dict[str, list[Violation]]
        The actual violations that pydoclint finds, which may contain
        baseline violations. The keys of the dictionary are the file names
        in the repo that pydoclint looks at

    Returns
    -------
    baselineRegenerationNeeded : bool
        Whether the baseline file should be regenerated
    unfixedBaselineViolationsInAllFiles : dict[str, list[str]]
        The unfixed baseline violations in all the Python files of the repo
        that pydoclint looks at. The keys are file names, and the values
        (``list[str]``) are lists of violation messages (``str``) in
        each file
    remainingViolationsInAllFiles : dict[str, list[Violation]]
        The remaining violations that users still need to fix. The keys are
        file names, and the values (``list[Violation]``) are lists of
        violations (``Violation``) in each file
    """
    baselineRegenerationNeeded: bool = False

    unfixedBaselineViolationsInAllFiles: dict[str, list[str]] = {}
    remainingViolationsInAllFiles: dict[str, list[Violation]] = {}

    for file, actualViolations in actualViolationsInAllFiles.items():
        baselineViolations: list[str] = baseline.get(file, [])

        unfixedBaselineViolations: list[str]
        remainingViolations: list[Violation]

        (
            unfixedBaselineViolations,
            remainingViolations,
        ) = calcUnfixedBaselineViolationsAndRemainingViolations(
            baselineViolations=baselineViolations,
            actualViolations=actualViolations,
        )

        if unfixedBaselineViolations != baselineViolations:
            baselineRegenerationNeeded = True

        unfixedBaselineViolationsInAllFiles[file] = unfixedBaselineViolations
        remainingViolationsInAllFiles[file] = remainingViolations

    return (
        baselineRegenerationNeeded,
        unfixedBaselineViolationsInAllFiles,
        remainingViolationsInAllFiles,
    )


def calcUnfixedBaselineViolationsAndRemainingViolations(
        baselineViolations: list[str],
        actualViolations: list[Violation],
) -> tuple[list[str], list[Violation]]:
    """
    Based on the baseline violations and the actual violations, calculate
    which baseline violations have not been fixed, and which violations are
    new (not part of the baseline) and need to be fixed.
    """
    unfixedBaselineViolations: list[str] = []
    remainingViolations: list[Violation] = []
    for viol in actualViolations:
        if str(viol) in baselineViolations:
            unfixedBaselineViolations.append(str(viol))
        else:
            remainingViolations.append(viol)

    return unfixedBaselineViolations, remainingViolations
