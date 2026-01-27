import { loadSettings } from "../../../lib/settings";
import {
  loadClusterConfig,
  resolveConfigPath,
  startClusterFromText,
} from "../../../lib/start-cluster";

const { generateName } = require("../../../src/name-generator");

type ClusterLauncherDeps = {
  getOrchestrator?: () => Promise<any>;
  loadSettings?: typeof loadSettings;
  resolveConfigPath?: typeof resolveConfigPath;
  loadClusterConfig?: typeof loadClusterConfig;
  startClusterFromText?: typeof startClusterFromText;
  generateClusterId?: () => string;
};

let orchestratorPromise: Promise<any> | null = null;

async function getOrchestrator() {
  if (!orchestratorPromise) {
    const Orchestrator = require("../../../src/orchestrator");
    orchestratorPromise = Orchestrator.create({ quiet: true });
  }
  return orchestratorPromise;
}

export function generateClusterId(): string {
  return generateName("cluster");
}

type LaunchClusterFromTextArgs = {
  text: string;
  providerOverride?: string | null;
  clusterId?: string;
  deps?: ClusterLauncherDeps;
};

export async function launchClusterFromText({
  text,
  providerOverride = null,
  clusterId,
  deps = {},
}: LaunchClusterFromTextArgs): Promise<{ clusterId: string }> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const loadSettingsImpl = deps.loadSettings ?? loadSettings;
  const resolveConfigPathImpl = deps.resolveConfigPath ?? resolveConfigPath;
  const loadClusterConfigImpl = deps.loadClusterConfig ?? loadClusterConfig;
  const startClusterFromTextImpl = deps.startClusterFromText ?? startClusterFromText;
  const generateClusterIdImpl = deps.generateClusterId ?? generateClusterId;

  const orchestrator = await getOrchestratorImpl();
  const settings = loadSettingsImpl();
  const configName = settings.defaultConfig || "conductor-bootstrap";
  const configPath = resolveConfigPathImpl(configName);
  const config = loadClusterConfigImpl(
    orchestrator,
    configPath,
    settings,
    providerOverride
  );
  const resolvedClusterId = clusterId || generateClusterIdImpl();

  await startClusterFromTextImpl({
    orchestrator,
    text,
    config,
    settings,
    providerOverride,
    clusterId: resolvedClusterId,
  });

  return { clusterId: resolvedClusterId };
}
