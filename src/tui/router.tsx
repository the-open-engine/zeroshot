import React from "react";
import { ViewId } from "./view-stack";
import LauncherView from "./views/LauncherView";
import MonitorView from "./views/MonitorView";
import ClusterView from "./views/ClusterView";
import AgentView from "./views/AgentView";

type RouterProps = {
  view: ViewId;
  providerOverride?: string | null;
};

export default function Router({ view, providerOverride }: RouterProps) {
  switch (view) {
    case "launcher":
      return <LauncherView providerOverride={providerOverride} />;
    case "monitor":
      return <MonitorView />;
    case "cluster":
      return <ClusterView />;
    case "agent":
      return <AgentView />;
    default:
      return <LauncherView providerOverride={providerOverride} />;
  }
}
