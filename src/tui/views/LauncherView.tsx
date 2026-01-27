import React from "react";
import { Box, Text } from "ink";

export default function LauncherView({
  providerOverride,
}: {
  providerOverride?: string | null;
}) {
  return (
    <Box flexDirection="column">
      <Text color="cyan">Launcher</Text>
      {providerOverride ? (
        <Text color="gray">Provider override: {providerOverride}</Text>
      ) : null}
      <Text color="gray">1 Launcher 2 Monitor 3 Cluster 4 Agent</Text>
      <Text color="gray">Esc back, Ctrl+C exit</Text>
    </Box>
  );
}
