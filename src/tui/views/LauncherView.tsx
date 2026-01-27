import React from "react";
import { Box, Text } from "ink";

export default function LauncherView({ provider }: { provider: string | null }) {
  return (
    <Box flexDirection="column">
      <Text color="cyan">Launcher</Text>
      <Text color="gray">Provider: {provider || "auto"}</Text>
      <Text color="gray">
        Commands: /help /monitor /issue &lt;ref&gt; /provider &lt;name&gt; /quit
      </Text>
      <Text color="gray">Esc back, Ctrl+C exit</Text>
    </Box>
  );
}
