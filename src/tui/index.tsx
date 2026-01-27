import React from "react";
import { render } from "ink";
import App from "./app";
import { ViewId } from "./view-stack";

type StartOptions = {
  autoExit?: boolean;
  exitDelayMs?: number;
  providerOverride?: string | null;
  initialView?: ViewId;
};

const DEFAULT_EXIT_DELAY_MS = 250;

export function start(options: StartOptions = {}): void {
  const {
    autoExit = true,
    exitDelayMs = DEFAULT_EXIT_DELAY_MS,
    providerOverride = null,
    initialView = "launcher",
  } = options;

  render(
    <App
      autoExit={autoExit}
      exitDelayMs={exitDelayMs}
      providerOverride={providerOverride}
      initialView={initialView}
    />
  );
}

export default { start };
