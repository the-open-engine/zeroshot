import * as backend from "../../../lib/tui-backend/services/cluster-launcher.js";

type ClusterLauncherDeps = {
  getOrchestrator?: () => Promise<any>;
  loadSettings?: typeof import("../../../lib/settings.js").loadSettings;
  resolveConfigPath?: typeof import("../../../lib/start-cluster.js").resolveConfigPath;
  loadClusterConfig?: typeof import("../../../lib/start-cluster.js").loadClusterConfig;
  startClusterFromText?: typeof import("../../../lib/start-cluster.js").startClusterFromText;
  startClusterFromIssue?: typeof import("../../../lib/start-cluster.js").startClusterFromIssue;
  detectRunInput?: typeof import("../../../lib/start-cluster.js").detectRunInput;
  generateClusterId?: () => string;
};

type LaunchClusterFromTextArgs = {
  text: string;
  providerOverride?: string | null;
  clusterId?: string;
  deps?: ClusterLauncherDeps;
};

type LaunchClusterFromIssueArgs = {
  ref: string;
  providerOverride?: string | null;
  clusterId?: string;
  deps?: ClusterLauncherDeps;
};

export const generateClusterId: () => string = backend.generateClusterId;

export const launchClusterFromText: (
  args: LaunchClusterFromTextArgs
) => Promise<{ clusterId: string }> = backend.launchClusterFromText;

export const launchClusterFromIssue: (
  args: LaunchClusterFromIssueArgs
) => Promise<{ clusterId: string }> = backend.launchClusterFromIssue;
