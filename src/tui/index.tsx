import React, { useEffect } from "react";
import { Box, Text, render, useApp, useInput } from "ink";

type StartOptions = {
  autoExit?: boolean;
  exitDelayMs?: number;
  providerOverride?: string | null;
};

const DEFAULT_EXIT_DELAY_MS = 250;

function App({
  autoExit,
  exitDelayMs,
  providerOverride,
}: {
  autoExit: boolean;
  exitDelayMs: number;
  providerOverride?: string | null;
}) {
  const { exit } = useApp();
  const isInteractive = !autoExit && Boolean(process.stdin.isTTY);

  useInput(
    () => {
      if (isInteractive) {
        exit();
      }
    },
    { isActive: isInteractive }
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
      <Text>zeroshot watch</Text>
      <Text color="green">Hello Ink TUI</Text>
      {providerOverride ? (
        <Text color="cyan">Provider override: {providerOverride}</Text>
      ) : null}
      <Text color="gray">
        {autoExit ? "Exiting..." : isInteractive ? "Press any key to exit." : "Non-interactive input."}
      </Text>
    </Box>
  );
}

export function start(options: StartOptions = {}): void {
  const { autoExit = true, exitDelayMs = DEFAULT_EXIT_DELAY_MS, providerOverride = null } = options;
  render(<App autoExit={autoExit} exitDelayMs={exitDelayMs} providerOverride={providerOverride} />);
}

export default { start };
