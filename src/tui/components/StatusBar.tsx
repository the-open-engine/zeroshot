import React from "react";
import { Box, Text } from "ink";
import { CommandResult } from "../commands/types";

type StatusBarProps = {
  status: CommandResult | null;
  provider: string | null;
};

function toneColor(tone?: CommandResult["tone"]): string {
  if (tone === "error") return "red";
  if (tone === "success") return "green";
  if (tone === "info") return "yellow";
  return "gray";
}

export default function StatusBar({ status, provider }: StatusBarProps) {
  const providerLabel = provider ? provider : "auto";
  const message = status ? status.message : "Ready";
  const color = toneColor(status?.tone);

  return (
    <Box justifyContent="space-between">
      <Text color="gray">Provider: {providerLabel}</Text>
      <Text color={color}>{message}</Text>
    </Box>
  );
}
