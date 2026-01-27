import React, { useEffect, useMemo, useState } from "react";
import { Box, useApp, useInput } from "ink";
import Router from "./router";
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

const VIEW_SHORTCUTS: Record<string, ViewId> = {
  "1": "launcher",
  "2": "monitor",
  "3": "cluster",
  "4": "agent",
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
  const active = useMemo(() => activeView(viewStack), [viewStack]);
  const isInputActive = Boolean(process.stdin.isTTY);

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
      const shortcut = VIEW_SHORTCUTS[input];
      if (shortcut) {
        setViewStack((stack) => pushIfDifferent(stack, shortcut));
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
      <Router view={active} providerOverride={providerOverride} />
    </Box>
  );
}
