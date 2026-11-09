"""Public observation projections preserve verifier and structural evidence."""

import pytest

from zeroshot import ProtocolError
from zeroshot._projection import _failure_context, _log_event, _status


def test_verifier_results_loop_counts_and_nested_paths():
    context = _failure_context(
        {
            "verifiers": [
                {
                    "message": "readiness=revise: missing test",
                    "execution": {
                        "execution": "stable-id",
                        "node": "assessor",
                        "mapIndices": [1, 2],
                    },
                    "cursor": "v2:7",
                }
            ],
            "exhaustedLoops": [{"node": "refinement", "completedIterations": 3}],
        }
    )
    assert context is not None
    assert context.primary is None
    assert context.verifiers[0].execution.map_indices == (1, 2)
    assert context.verifiers[0].cursor == "v2:7"
    assert context.exhausted_loops[0].completed_iterations == 3
    assert context.exhausted_loops[0].map_indices == ()
    log = _log_event(
        {
            "runId": "run",
            "cursor": "v2:7",
            "timestamp": 1,
            "execution": "stable-id",
            "mapIndices": [1, 2],
            "record": {"level": "debug", "target": "node", "message": "readiness=revise"},
        }
    )
    assert log.map_indices == (1, 2)
    status = _status(
        {
            "runId": "run",
            "title": "Map",
            "size": "host",
            "atCursor": "v2:6",
            "status": {
                "phase": "running",
                "activeExecutions": [
                    {"execution": "stable-id", "node": "assessor", "mapIndices": [1, 2]}
                ],
            },
        }
    )
    assert status.active_executions[0].map_indices == log.map_indices


@pytest.mark.parametrize("indices", [[-1], [True], [1.5], ["1"], "1"])
def test_malformed_indices_are_rejected(indices):
    with pytest.raises(ProtocolError):
        _failure_context(
            {
                "verifiers": [
                    {
                        "message": "detail",
                        "execution": {
                            "execution": "id",
                            "node": "assessor",
                            "mapIndices": indices,
                        },
                    }
                ]
            }
        )
