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
  PendingAgentMessage,
  agentMessageKey,
  createPendingAgentMessage,
} from "./services/agent-messages";
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

export type AppInputHandlers = {
  exit: () => void;
  popView: () => void;
  submit: () => void;
  deleteChar: () => void;
  appendText: (text: string) => void;
};

export type AppInputKey = {
  ctrl?: boolean;
  meta?: boolean;
  escape?: boolean;
  return?: boolean;
  backspace?: boolean;
  delete?: boolean;
};

export function handleAppInput(
  input: string,
  key: AppInputKey,
  handlers: AppInputHandlers
): void {
  if (key.ctrl && input === "c") {
    handlers.exit();
    return;
  }
  if (key.escape) {
    handlers.popView();
    return;
  }
  if (key.return) {
    handlers.submit();
    return;
  }
  if (key.backspace || key.delete) {
    handlers.deleteChar();
    return;
  }
  if (key.ctrl || key.meta) {
    return;
  }
  if (input) {
    handlers.appendText(input);
  }
}

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
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [isDispatching, setIsDispatching] = useState(false);
  const [pendingMessages, setPendingMessages] = useState<
    Record<string, PendingAgentMessage[]>
  >({});
  const active = useMemo(() => activeView(viewStack), [viewStack]);
  const isInputActive = Boolean(process.stdin.isTTY);
  const isCommandInputEmpty = inputValue.length === 0;
  const trimmedInput = inputValue.trim();
  const isAgentView = active === "agent";
  const isAgentCommand = isAgentView && trimmedInput.startsWith("/");
  const agentInputMode = isAgentView
    ? isAgentCommand
      ? "command"
      : "message"
    : "command";
  const commandPlaceholder = isAgentView
    ? "Type a message or /command"
    : "Type /help for commands";

  const setClusterId = (clusterId: string | null) => {
    setActiveClusterId(clusterId);
    setSelectedAgentId(null);
  };

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
        if (active === "agent") {
          if (!activeClusterId || !selectedAgentId) {
            setStatus({
              tone: "info",
              message: "Select an agent to queue messages.",
            });
            return;
          }
          const message = createPendingAgentMessage({
            clusterId: activeClusterId,
            agentId: selectedAgentId,
            text: parsed.text,
          });
          const key = agentMessageKey(activeClusterId, selectedAgentId);
          setPendingMessages((prev) => {
            const nextForAgent = prev[key]
              ? [...prev[key], message]
              : [message];
            return { ...prev, [key]: nextForAgent };
          });
          setStatus({
            tone: "success",
            message: `Queued message for ${selectedAgentId}.`,
          });
          return;
        }
        await submitLauncherText({
          text: parsed.text,
          providerOverride: provider,
          activeView: active,
          setStatus,
          setClusterId,
          navigate: (view) =>
            setViewStack((stack) => pushIfDifferent(stack, view)),
        });
        return;
      }

      const result = await dispatchCommand(parsed, {
        navigate: (view) =>
          setViewStack((stack) => pushIfDifferent(stack, view)),
        setProvider: (next) => setProvider(next),
        setClusterId,
        provider,
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
      handleAppInput(input, key, {
        exit,
        popView: () => setViewStack((stack) => popView(stack)),
        submit: () => void handleSubmit(),
        deleteChar: () => setInputValue((value) => value.slice(0, -1)),
        appendText: (text) => setInputValue((value) => value + text),
      });
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

  function handleOpenCluster(clusterId: string) {
    setClusterId(clusterId);
    setViewStack((stack) => pushIfDifferent(stack, "cluster"));
  }

  function handleOpenAgent(agentId: string) {
    setSelectedAgentId(agentId);
    setViewStack((stack) => pushIfDifferent(stack, "agent"));
  }

  const pendingForActiveAgent = useMemo(() => {
    if (!activeClusterId || !selectedAgentId) {
      return [];
    }
    const key = agentMessageKey(activeClusterId, selectedAgentId);
    return pendingMessages[key] ?? [];
  }, [activeClusterId, pendingMessages, selectedAgentId]);

  return (
    <Box flexDirection="column">
      <Box flexGrow={1} flexDirection="column">
        <Router
          view={active}
          provider={provider}
          clusterId={activeClusterId}
          agentId={selectedAgentId}
          agentPendingMessages={pendingForActiveAgent}
          agentMessageDraft={inputValue}
          agentInputMode={agentInputMode}
          onOpenCluster={handleOpenCluster}
          onSelectAgent={setSelectedAgentId}
          onOpenAgent={handleOpenAgent}
          isCommandInputEmpty={isCommandInputEmpty}
        />
      </Box>
      <StatusBar status={status} provider={provider} />
      <CommandInput value={inputValue} placeholder={commandPlaceholder} />
    </Box>
  );
}
