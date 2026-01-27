import React from "react";
import { Box, Text } from "ink";

export default function MonitorView({ provider }: { provider: string | null }) {
  return (
    <Box flexDirection="column">
      <Text color="cyan">Monitor</Text>
      <Text color="gray">Provider: {provider || "auto"}</Text>
      <Text color="gray">Stub view</Text>
      <Text color="gray">Type /help for commands</Text>
      <Text color="gray">Esc back, Ctrl+C exit</Text>
    </Box>
  );
}
