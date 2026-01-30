const backend = require("../../../lib/tui-backend/services/cluster-launcher");

type ClusterLauncherDeps = {
  getOrchestrator?: () => Promise<any>;
  loadSettings?: typeof import("../../../lib/settings").loadSettings;
  resolveConfigPath?: typeof import("../../../lib/start-cluster").resolveConfigPath;
  loadClusterConfig?: typeof import("../../../lib/start-cluster").loadClusterConfig;
  startClusterFromText?: typeof import("../../../lib/start-cluster").startClusterFromText;
  startClusterFromIssue?: typeof import("../../../lib/start-cluster").startClusterFromIssue;
  detectRunInput?: typeof import("../../../lib/start-cluster").detectRunInput;
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
