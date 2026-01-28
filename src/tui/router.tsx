import React from "react";
import { ViewId } from "./view-stack";
import LauncherView from "./views/LauncherView";
import MonitorView from "./views/MonitorView";
import ClusterView from "./views/ClusterView";
import AgentView from "./views/AgentView";
import { PendingAgentMessage } from "./services/agent-messages";

type RouterProps = {
  view: ViewId;
  provider: string | null;
  clusterId?: string | null;
  agentId?: string | null;
  agentPendingMessages?: PendingAgentMessage[];
  agentMessageDraft?: string;
  agentInputMode?: "command" | "message";
  onOpenCluster?: (clusterId: string) => void;
  onSelectAgent?: (agentId: string | null) => void;
  onOpenAgent?: (agentId: string) => void;
  isCommandInputEmpty?: boolean;
  clusterGuidanceHistory?: Array<{
    text: string;
    timestamp: number;
    injectedCount: number;
    queuedCount: number;
  }>;
};

export default function Router({
  view,
  provider,
  clusterId,
  agentId,
  agentPendingMessages,
  agentMessageDraft,
  agentInputMode,
  onOpenCluster,
  onSelectAgent,
  onOpenAgent,
  isCommandInputEmpty,
  clusterGuidanceHistory,
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
          guidanceHistory={clusterGuidanceHistory}
        />
      );
    case "agent":
      return (
        <AgentView
          provider={provider}
          clusterId={clusterId}
          agentId={agentId}
          pendingMessages={agentPendingMessages}
          messageDraft={agentMessageDraft}
          inputMode={agentInputMode}
        />
      );
    default:
      return <LauncherView provider={provider} />;
  }
}
