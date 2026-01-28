export type DeliveryStatus = {
  status: string;
  reason: string | null;
  method: string | null;
  taskId?: string | null;
};

export type PendingAgentMessage = {
  id: string;
  clusterId: string;
  agentId: string;
  text: string;
  createdAt: number;
  status: "pending";
  deliveryStatus?: DeliveryStatus;
};

type CreatePendingAgentMessageArgs = {
  clusterId: string;
  agentId: string;
  text: string;
  now?: number;
  id?: string;
};

export function agentMessageKey(clusterId: string, agentId: string): string {
  return `${clusterId}:${agentId}`;
}

export function createPendingAgentMessage({
  clusterId,
  agentId,
  text,
  now = Date.now(),
  id,
}: CreatePendingAgentMessageArgs): PendingAgentMessage {
  const createdAt = now;
  const messageId =
    id ?? `${createdAt}-${Math.random().toString(16).slice(2)}`;
  return {
    id: messageId,
    clusterId,
    agentId,
    text,
    createdAt,
    status: "pending",
  };
}
