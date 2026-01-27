import React, { useEffect, useMemo, useState } from "react";
import { Box, useApp, useInput } from "ink";
import Router from "./router";
import CommandInput from "./components/CommandInput";
import StatusBar from "./components/StatusBar";
import { dispatchCommand } from "./commands/dispatcher";
import { parseInput } from "./commands/parser";
import { CommandResult } from "./commands/types";
import { submitLauncherText } from "./launcher-actions";
import {
  activeView,
  createViewStack,
  popView,
  pushView,
  ViewId,
} from "./view-stack";

type AppProps = {
  autoExit: boolean;
  exitDelayMs: number;
  providerOverride?: string | null;
  initialView?: ViewId;
};

function pushIfDifferent(stack: ViewId[], view: ViewId): ViewId[] {
  return activeView(stack) === view ? stack : pushView(stack, view);
}

export default function App({
  autoExit,
  exitDelayMs,
  providerOverride,
  initialView = "launcher",
}: AppProps) {
  const { exit } = useApp();
  const [viewStack, setViewStack] = useState<ViewId[]>(() =>
    createViewStack(initialView)
  );
  const [inputValue, setInputValue] = useState("");
  const [status, setStatus] = useState<CommandResult | null>(null);
  const [provider, setProvider] = useState<string | null>(
    providerOverride ?? null
  );
  const [activeClusterId, setActiveClusterId] = useState<string | null>(null);
  const [isDispatching, setIsDispatching] = useState(false);
  const active = useMemo(() => activeView(viewStack), [viewStack]);
  const isInputActive = Boolean(process.stdin.isTTY);

  async function handleSubmit() {
    if (isDispatching) {
      return;
    }

    const parsed = parseInput(inputValue);
    if (parsed.type === "empty") {
      setInputValue("");
      return;
    }

    setIsDispatching(true);
    try {
      if (parsed.type === "text") {
        await submitLauncherText({
          text: parsed.text,
          providerOverride: provider,
          activeView: active,
          setStatus,
          setClusterId: setActiveClusterId,
          navigate: (view) =>
            setViewStack((stack) => pushIfDifferent(stack, view)),
        });
        return;
      }

      const result = await dispatchCommand(parsed, {
        navigate: (view) =>
          setViewStack((stack) => pushIfDifferent(stack, view)),
        setProvider: (next) => setProvider(next),
        exit,
      });
      setStatus(result);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Command failed.";
      setStatus({ tone: "error", message });
    } finally {
      setInputValue("");
      setIsDispatching(false);
    }
  }

  useInput(
    (input, key) => {
      if (key.ctrl && input === "c") {
        exit();
        return;
      }
      if (key.escape) {
        setViewStack((stack) => popView(stack));
        return;
      }
      if (key.return) {
        void handleSubmit();
        return;
      }
      if (key.backspace || key.delete) {
        setInputValue((value) => value.slice(0, -1));
        return;
      }
      if (key.ctrl || key.meta) {
        return;
      }
      if (input) {
        setInputValue((value) => value + input);
      }
    },
    { isActive: isInputActive }
  );

  useEffect(() => {
    if (autoExit) {
      const timer = setTimeout(() => exit(), exitDelayMs);
      return () => clearTimeout(timer);
    }
    return undefined;
  }, [autoExit, exit, exitDelayMs]);

  return (
    <Box flexDirection="column">
      <Box flexGrow={1} flexDirection="column">
        <Router view={active} provider={provider} clusterId={activeClusterId} />
      </Box>
      <StatusBar status={status} provider={provider} />
      <CommandInput value={inputValue} />
    </Box>
  );
}
