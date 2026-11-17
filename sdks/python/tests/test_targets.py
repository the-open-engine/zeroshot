"""Local target management through the native executable."""

import asyncio
from pathlib import Path

import pytest
from conftest import read_invocations

from zeroshot import (
    Client,
    DirectTarget,
    HostedTarget,
    InvalidRequestError,
    NamedTarget,
    ProtocolError,
    TargetRemoveResult,
)


def test_target_inventory_and_removal_are_typed_local_calls(fake_native: Path) -> None:
    async def exercise() -> None:
        async with Client(target=HostedTarget("unreachable")) as client:
            assert await client.list_targets() == (
                NamedTarget("platform", "https://api.app.zeroshot.sh", "hosted"),
                NamedTarget("vm", "http://127.0.0.1:8080", "direct"),
            )
            assert await client.remove_target("vm") == TargetRemoveResult("vm", True)

    asyncio.run(exercise())
    assert [entry["args"] for entry in read_invocations(fake_native)] == [
        ["target", "list", "--json"],
        ["target", "remove", "--json", "--", "vm"],
    ]


@pytest.mark.parametrize(
    "name,message", [("platform", "cannot be removed"), ("missing", "not found")]
)
def test_target_removal_preserves_native_refusals(
    name: str, message: str, fake_native: Path
) -> None:
    async def exercise() -> None:
        async with Client() as client:
            with pytest.raises(InvalidRequestError, match=message):
                await client.remove_target(name)

    asyncio.run(exercise())


def test_target_management_ignores_temporary_direct_routing(
    fake_native: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("ZEROSHOT_CONFIG_DIR", str(tmp_path / "user-config"))

    async def exercise() -> None:
        async with Client(target=DirectTarget("http://127.0.0.1:8080")) as client:
            await client._ready()
            assert client._environment()[0]["ZEROSHOT_CONFIG_DIR"] != str(tmp_path / "user-config")
            assert client._environment(local_registry=True)[0]["ZEROSHOT_CONFIG_DIR"] == str(
                tmp_path / "user-config"
            )
            await client.list_targets()
            await client.remove_target("vm")

    asyncio.run(exercise())
    assert [entry["args"] for entry in read_invocations(fake_native)][-2:] == [
        ["target", "list", "--json"],
        ["target", "remove", "--json", "--", "vm"],
    ]


@pytest.mark.parametrize("value", ["{}", '[{"name":"vm","origin":"https://vm","kind":"other"}]'])
def test_malformed_target_inventory_is_rejected(
    value: str, fake_native: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_TARGET_INVENTORY", value)

    async def exercise() -> None:
        async with Client() as client:
            with pytest.raises(ProtocolError):
                await client.list_targets()

    asyncio.run(exercise())


@pytest.mark.parametrize(
    "value", ["{}", '{"name":"other","removed":true}', '{"name":"vm","removed":1}']
)
def test_malformed_target_removal_is_rejected(
    value: str, fake_native: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_TARGET_REMOVAL", value)

    async def exercise() -> None:
        async with Client() as client:
            with pytest.raises(ProtocolError):
                await client.remove_target("vm")

    asyncio.run(exercise())
