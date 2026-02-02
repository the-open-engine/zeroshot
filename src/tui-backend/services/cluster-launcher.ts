import { loadSettings } from '../../../lib/settings';
import {
  detectRunInput,
  loadClusterConfig,
  resolveConfigPath,
  startClusterFromIssue,
  startClusterFromText,
} from '../../../lib/start-cluster';

const { generateName } = require('../../../src/name-generator');

type ClusterLauncherDeps = {
  getOrchestrator?: () => Promise<any>;
  loadSettings?: typeof loadSettings;
  resolveConfigPath?: typeof resolveConfigPath;
  loadClusterConfig?: typeof loadClusterConfig;
  startClusterFromText?: typeof startClusterFromText;
  startClusterFromIssue?: typeof startClusterFromIssue;
  detectRunInput?: typeof detectRunInput;
  generateClusterId?: () => string;
};

let orchestratorPromise: Promise<any> | null = null;

export class InvalidIssueReferenceError extends Error {
  constructor(ref: string) {
    super(`Invalid issue reference: ${ref}`);
    this.name = 'InvalidIssueReferenceError';
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

async function getOrchestrator() {
  if (!orchestratorPromise) {
    const Orchestrator = require('../../../src/orchestrator');
    orchestratorPromise = Orchestrator.create({ quiet: true });
  }
  return orchestratorPromise;
}

export function generateClusterId(): string {
  return generateName('cluster');
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
  const configName = settings.defaultConfig || 'conductor-bootstrap';
  const configPath = resolveConfigPathImpl(configName);
  const config = loadClusterConfigImpl(orchestrator, configPath, settings, providerOverride);
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

type LaunchClusterFromIssueArgs = {
  ref: string;
  providerOverride?: string | null;
  clusterId?: string;
  deps?: ClusterLauncherDeps;
};

export async function launchClusterFromIssue({
  ref,
  providerOverride = null,
  clusterId,
  deps = {},
}: LaunchClusterFromIssueArgs): Promise<{ clusterId: string }> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const loadSettingsImpl = deps.loadSettings ?? loadSettings;
  const resolveConfigPathImpl = deps.resolveConfigPath ?? resolveConfigPath;
  const loadClusterConfigImpl = deps.loadClusterConfig ?? loadClusterConfig;
  const startClusterFromIssueImpl = deps.startClusterFromIssue ?? startClusterFromIssue;
  const detectRunInputImpl = deps.detectRunInput ?? detectRunInput;
  const generateClusterIdImpl = deps.generateClusterId ?? generateClusterId;

  const parsed = detectRunInputImpl(ref);
  if (!parsed || typeof parsed !== 'object' || !('issue' in parsed)) {
    throw new InvalidIssueReferenceError(ref);
  }

  const orchestrator = await getOrchestratorImpl();
  const settings = loadSettingsImpl();
  const configName = settings.defaultConfig || 'conductor-bootstrap';
  const configPath = resolveConfigPathImpl(configName);
  const config = loadClusterConfigImpl(orchestrator, configPath, settings, providerOverride);
  const resolvedClusterId = clusterId || generateClusterIdImpl();

  await startClusterFromIssueImpl({
    orchestrator,
    issue: parsed.issue,
    config,
    settings,
    providerOverride,
    clusterId: resolvedClusterId,
  });

  return { clusterId: resolvedClusterId };
}
