import React from "react";
import { Box, Text } from "ink";

type CommandInputProps = {
  value: string;
  placeholder?: string;
};

export default function CommandInput({
  value,
  placeholder = "Type /help for commands",
}: CommandInputProps) {
  return (
    <Box>
      <Text color="cyan">{"> "}</Text>
      {value ? <Text>{value}</Text> : <Text color="gray">{placeholder}</Text>}
    </Box>
  );
}
