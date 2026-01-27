import React from "react";
import { ViewId } from "./view-stack";
import LauncherView from "./views/LauncherView";
import MonitorView from "./views/MonitorView";
import ClusterView from "./views/ClusterView";
import AgentView from "./views/AgentView";

type RouterProps = {
  view: ViewId;
  provider: string | null;
  clusterId?: string | null;
};

export default function Router({ view, provider, clusterId }: RouterProps) {
  switch (view) {
    case "launcher":
      return <LauncherView provider={provider} />;
    case "monitor":
      return <MonitorView provider={provider} />;
    case "cluster":
      return <ClusterView provider={provider} clusterId={clusterId} />;
    case "agent":
      return <AgentView provider={provider} />;
    default:
      return <LauncherView provider={provider} />;
  }
}
