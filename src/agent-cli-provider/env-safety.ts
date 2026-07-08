const EXECUTABLE_RESOLUTION_ENV_KEYS = new Set(['path', 'pathext']);
const PROCESS_CONTROL_ENV_KEYS = new Set([
  '_java_options',
  'bash_env',
  'bun_options',
  'bun_preload',
  'classpath',
  'env',
  'java_tool_options',
  'jdk_java_options',
  'ld_library_path',
  'ld_preload',
  'lua_cpath',
  'lua_init',
  'lua_path',
  'node_options',
  'node_path',
  'npm_config_node_options',
  'perl5lib',
  'perl5opt',
  'pythonhome',
  'pythonpath',
  'pythonstartup',
  'rubylib',
  'rubyopt',
  'zshenv',
]);
const PROCESS_CONTROL_ENV_PREFIXES = ['dyld_', 'ld_', 'node_'];

type EnvRecord = Readonly<Record<string, string | undefined>>;

export function isExecutableResolutionEnvKey(key: string): boolean {
  return EXECUTABLE_RESOLUTION_ENV_KEYS.has(key.toLowerCase());
}

export function isProcessControlEnvKey(key: string): boolean {
  const normalized = key.toLowerCase();
  if (PROCESS_CONTROL_ENV_KEYS.has(normalized)) return true;
  return PROCESS_CONTROL_ENV_PREFIXES.some((prefix) => normalized.startsWith(prefix));
}

export function isUnsafeProviderEnvKey(key: string): boolean {
  return isExecutableResolutionEnvKey(key) || isProcessControlEnvKey(key);
}

export function findExecutableResolutionEnvKey(env: EnvRecord): string | null {
  for (const key of Object.keys(env)) {
    if (isExecutableResolutionEnvKey(key)) return key;
  }
  return null;
}

export function findUnsafeProviderEnvKey(env: EnvRecord): string | null {
  for (const key of Object.keys(env)) {
    if (isUnsafeProviderEnvKey(key)) return key;
  }
  return null;
}

function omitEnvKeys(
  env: EnvRecord,
  isUnsafe: (key: string) => boolean
): Readonly<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) {
    if (value === undefined || isUnsafe(key)) continue;
    result[key] = value;
  }
  return result;
}

export function omitExecutableResolutionEnv(env: EnvRecord): Readonly<Record<string, string>> {
  return omitEnvKeys(env, isExecutableResolutionEnvKey);
}

export function omitProcessControlEnv(env: EnvRecord): Readonly<Record<string, string>> {
  return omitEnvKeys(env, isProcessControlEnvKey);
}

export function omitUnsafeProviderEnv(env: EnvRecord): Readonly<Record<string, string>> {
  return omitEnvKeys(env, isUnsafeProviderEnvKey);
}
