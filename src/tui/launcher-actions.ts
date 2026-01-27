import { CommandResult } from "./commands/types";
import { generateClusterId, launchClusterFromText } from "./services/cluster-launcher";
import { ViewId } from "./view-stack";

type LauncherActionDeps = {
  generateClusterId?: () => string;
  launchClusterFromText?: typeof launchClusterFromText;
};

type SubmitLauncherTextArgs = {
  text: string;
  providerOverride?: string | null;
  activeView: ViewId;
  setStatus: (status: CommandResult) => void;
  setClusterId: (clusterId: string | null) => void;
  navigate: (view: ViewId) => void;
  deps?: LauncherActionDeps;
};

export async function submitLauncherText({
  text,
  providerOverride = null,
  activeView,
  setStatus,
  setClusterId,
  navigate,
  deps = {},
}: SubmitLauncherTextArgs): Promise<void> {
  if (activeView !== "launcher") {
    setStatus({
      tone: "info",
      message: "Text launch only works from the Launcher view.",
    });
    return;
  }

  if (text.trim().startsWith("/")) {
    setStatus({
      tone: "info",
      message: "Use slash commands for /help, /monitor, and /provider.",
    });
    return;
  }

  const generateClusterIdImpl = deps.generateClusterId ?? generateClusterId;
  const launchClusterFromTextImpl =
    deps.launchClusterFromText ?? launchClusterFromText;

  const clusterId = generateClusterIdImpl();
  setClusterId(clusterId);
  setStatus({
    tone: "info",
    message: `Starting cluster ${clusterId}...`,
  });

  try {
    await launchClusterFromTextImpl({
      text,
      providerOverride,
      clusterId,
    });
    navigate("cluster");
    setStatus({
      tone: "success",
      message: `Cluster ${clusterId} started.`,
    });
  } catch (error) {
    const message =
      error instanceof Error ? error.message : "Cluster start failed.";
    setStatus({ tone: "error", message });
    setClusterId(null);
  }
}
