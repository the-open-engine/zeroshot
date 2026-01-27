import React from "react";
import { Box, Text } from "ink";

export default function AgentView({ provider }: { provider: string | null }) {
  return (
    <Box flexDirection="column">
      <Text color="cyan">Agent</Text>
      <Text color="gray">Provider: {provider || "auto"}</Text>
      <Text color="gray">Stub view</Text>
      <Text color="gray">Type /help for commands</Text>
      <Text color="gray">Esc back, Ctrl+C exit</Text>
    </Box>
  );
}
