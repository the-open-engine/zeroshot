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
  agentId?: string | null;
  onOpenCluster?: (clusterId: string) => void;
  onSelectAgent?: (agentId: string | null) => void;
  onOpenAgent?: (agentId: string) => void;
  isCommandInputEmpty?: boolean;
};

export default function Router({
  view,
  provider,
  clusterId,
  agentId,
  onOpenCluster,
  onSelectAgent,
  onOpenAgent,
  isCommandInputEmpty,
}: RouterProps) {
  switch (view) {
    case "launcher":
      return <LauncherView provider={provider} />;
    case "monitor":
      return (
        <MonitorView
          provider={provider}
          onOpenCluster={onOpenCluster}
          isCommandInputEmpty={isCommandInputEmpty ?? false}
        />
      );
    case "cluster":
      return (
        <ClusterView
          provider={provider}
          clusterId={clusterId}
          selectedAgentId={agentId}
          onSelectAgent={onSelectAgent}
          onOpenAgent={onOpenAgent}
          isCommandInputEmpty={isCommandInputEmpty}
        />
      );
    case "agent":
      return (
        <AgentView provider={provider} clusterId={clusterId} agentId={agentId} />
      );
    default:
      return <LauncherView provider={provider} />;
  }
}
