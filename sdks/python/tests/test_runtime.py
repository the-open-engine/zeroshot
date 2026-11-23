from zeroshot import UniformRuntime


def test_uniform_runtime_forwards_max_concurrency_without_inventing_a_default() -> None:
    runtime = UniformRuntime(harness="codex", provider="openai", model="opaque")
    assert "maxConcurrency" not in runtime.to_dict()
    limited = UniformRuntime(harness="codex", provider="openai", model="opaque", max_concurrency=1)
    assert limited.to_dict()["maxConcurrency"] == 1


def test_local_controller_policy_defaults_and_rejects_unknown_modes() -> None:
    import pytest

    from zeroshot import LocalTarget

    assert LocalTarget().controller == "detached"
    with pytest.raises(ValueError, match="controller must be"):
        LocalTarget(controller="automatic")  # type: ignore[arg-type]
