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
  sendAgentGuidance,
  sendClusterGuidance,
} from "./services/guidance-delivery";
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
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [isDispatching, setIsDispatching] = useState(false);
  const [pendingMessages, setPendingMessages] = useState<
    Record<string, PendingAgentMessage[]>
  >({});
  const [clusterGuidanceHistory, setClusterGuidanceHistory] = useState<
    Record<
      string,
      Array<{
        text: string;
        timestamp: number;
        injectedCount: number;
        queuedCount: number;
      }>
    >
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
    : active === "cluster"
    ? "Type guidance or /command"
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
              message: "Select an agent to send guidance.",
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

          try {
            const delivery = await sendAgentGuidance({
              clusterId: activeClusterId,
              agentId: selectedAgentId,
              text: parsed.text,
            });
            const deliveryStatus = {
              status:
                delivery.status === "injected" ? "injected" : "queued",
              reason: delivery.reason,
              method: delivery.method,
              taskId: delivery.taskId ?? null,
            };
            setPendingMessages((prev) => {
              const nextForAgent = (prev[key] ?? []).map((item) =>
                item.id === message.id
                  ? { ...item, deliveryStatus }
                  : item
              );
              return { ...prev, [key]: nextForAgent };
            });
            setStatus({
              tone: delivery.status === "injected" ? "success" : "info",
              message: `Guidance ${deliveryStatus.status} for ${selectedAgentId}.`,
            });
          } catch (error) {
            const errMsg =
              error instanceof Error
                ? error.message
                : "Failed to send guidance.";
            setPendingMessages((prev) => {
              const nextForAgent = (prev[key] ?? []).map((item) =>
                item.id === message.id
                  ? {
                      ...item,
                      deliveryStatus: {
                        status: "error",
                        reason: errMsg,
                        method: null,
                        taskId: null,
                      },
                    }
                  : item
              );
              return { ...prev, [key]: nextForAgent };
            });
            setStatus({ tone: "error", message: errMsg });
          }
          return;
        }
        if (active === "cluster") {
          if (!activeClusterId) {
            setStatus({
              tone: "info",
              message: "Select a cluster to send guidance.",
            });
            return;
          }
          try {
            const result = await sendClusterGuidance({
              clusterId: activeClusterId,
              text: parsed.text,
            });
            const { injected, queued, total } = result.summary;
            setClusterGuidanceHistory((prev) => {
              const history = prev[activeClusterId] ?? [];
              const next = [
                ...history,
                {
                  text: parsed.text,
                  timestamp: Date.now(),
                  injectedCount: injected,
                  queuedCount: queued,
                },
              ];
              return {
                ...prev,
                [activeClusterId]:
                  next.length > 10 ? next.slice(next.length - 10) : next,
              };
            });
            setStatus({
              tone: injected > 0 ? "success" : "info",
              message: `Guidance sent to ${total} agent(s): ${injected} injected, ${queued} queued.`,
            });
          } catch (error) {
            const errMsg =
              error instanceof Error
                ? error.message
                : "Failed to send guidance.";
            setStatus({ tone: "error", message: errMsg });
          }
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

  const guidanceHistoryForCluster = useMemo(() => {
    if (!activeClusterId) {
      return [];
    }
    return clusterGuidanceHistory[activeClusterId] ?? [];
  }, [activeClusterId, clusterGuidanceHistory]);

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
          clusterGuidanceHistory={guidanceHistoryForCluster}
        />
      </Box>
      <StatusBar status={status} provider={provider} />
      <CommandInput value={inputValue} placeholder={commandPlaceholder} />
    </Box>
  );
}
