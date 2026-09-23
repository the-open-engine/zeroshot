use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Number, Value};

use super::rpc::failure;
use crate::native_v2_runner::{NodeRunnerError, ResolvedEnvironment};

const BASE_URL: &str = "COPILOT_PROVIDER_BASE_URL";
const TYPE: &str = "COPILOT_PROVIDER_TYPE";
const API_KEY: &str = "COPILOT_PROVIDER_API_KEY";
const API_KEY_COMMAND: &str = "COPILOT_PROVIDER_API_KEY_COMMAND";
const BEARER_TOKEN: &str = "COPILOT_PROVIDER_BEARER_TOKEN";
const WIRE_API: &str = "COPILOT_PROVIDER_WIRE_API";
const TRANSPORT: &str = "COPILOT_PROVIDER_TRANSPORT";
const AZURE_API_VERSION: &str = "COPILOT_PROVIDER_AZURE_API_VERSION";
const MODEL_ID: &str = "COPILOT_PROVIDER_MODEL_ID";
const WIRE_MODEL: &str = "COPILOT_PROVIDER_WIRE_MODEL";
const MAX_PROMPT_TOKENS: &str = "COPILOT_PROVIDER_MAX_PROMPT_TOKENS";
const MAX_OUTPUT_TOKENS: &str = "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS";
const HEADERS: &str = "COPILOT_PROVIDER_HEADERS";
const PROVIDERS_CONFIG: &str = "COPILOT_PROVIDERS_CONFIG";
const OFFLINE: &str = "COPILOT_OFFLINE";

const LEGACY_NAMES: &[&str] = &[
    BASE_URL,
    TYPE,
    API_KEY,
    API_KEY_COMMAND,
    BEARER_TOKEN,
    WIRE_API,
    TRANSPORT,
    AZURE_API_VERSION,
    MODEL_ID,
    WIRE_MODEL,
    MAX_PROMPT_TOKENS,
    MAX_OUTPUT_TOKENS,
    HEADERS,
];
const CREDENTIAL_NAMES: &[&str] = &[API_KEY, API_KEY_COMMAND, BEARER_TOKEN];
pub(super) const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REGISTRY_ITEMS: usize = 256;
const MAX_REDACTIONS: usize = 2_048;
const MAX_REDACTION_BYTES: usize = 64 * 1024;

pub(super) fn is_configuration(name: &str) -> bool {
    is_configuration_for_platform(name, cfg!(windows))
}

pub(super) fn is_configuration_for_platform(name: &str, windows: bool) -> bool {
    LEGACY_NAMES
        .iter()
        .copied()
        .chain([PROVIDERS_CONFIG, OFFLINE])
        .any(|candidate| {
            if windows {
                name.eq_ignore_ascii_case(candidate)
            } else {
                name == candidate
            }
        })
}

/// Local provider state is captured privately and resolved for each model execution.
#[derive(Clone, Eq, PartialEq)]
pub(super) struct LocalProviderContext {
    legacy: BTreeMap<String, String>,
    registry_path: Option<String>,
    offline: Option<String>,
    default_registry: PathBuf,
}

pub(super) struct LocalProvider {
    parameters: ProviderParameters,
    process_environment: BTreeMap<String, String>,
    redactions: Vec<String>,
    bypasses_github_auth: bool,
    uses_api_key_command: bool,
    session_model: Option<String>,
}

enum ProviderParameters {
    Singular(Value),
    Registry { providers: Value, models: Value },
}

enum RegistryResolution {
    Inactive,
    Active(Option<LocalProvider>),
}

struct ProviderRegistry {
    providers: Vec<Value>,
    models: Vec<Value>,
}

pub(super) fn take_local_configuration(
    environment: &mut BTreeMap<String, String>,
    copilot_home: &Path,
) -> LocalProviderContext {
    let legacy = LEGACY_NAMES
        .iter()
        .filter_map(|name| {
            environment
                .remove(*name)
                .map(|value| ((*name).to_owned(), value))
        })
        .collect();
    LocalProviderContext {
        legacy,
        registry_path: environment.remove(PROVIDERS_CONFIG),
        offline: environment.remove(OFFLINE),
        default_registry: copilot_home.join("providers.json"),
    }
}

pub(super) fn resolve_local_configuration(
    ambient: &LocalProviderContext,
    declared: &ResolvedEnvironment,
    model: &str,
) -> Result<Option<LocalProvider>, NodeRunnerError> {
    let declared_values = declared
        .iter()
        .filter(|(name, _)| is_configuration(name.as_str()))
        .map(|(name, value)| (name.as_str().to_owned(), value.to_owned()))
        .collect::<BTreeMap<_, _>>();
    let offline = declared_values
        .get(OFFLINE)
        .cloned()
        .or_else(|| ambient.offline.clone());
    let explicit_registry = declared_values
        .get(PROVIDERS_CONFIG)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| {
            ambient
                .registry_path
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .cloned()
        });
    let registry_path = explicit_registry.as_deref().map(PathBuf::from).or_else(|| {
        ambient
            .default_registry
            .is_file()
            .then(|| ambient.default_registry.clone())
    });
    if let Some(path) = registry_path {
        match resolve_registry(
            &path,
            explicit_registry.is_some(),
            model,
            offline.as_deref(),
        )? {
            RegistryResolution::Active(provider) => {
                if provider.is_none() && offline_enabled(offline.as_deref()) {
                    return Err(failure(
                        "Copilot offline mode requires the selected model to use a local provider",
                    ));
                }
                return Ok(provider);
            }
            RegistryResolution::Inactive => {}
        }
    }

    let provider = resolve_legacy(ambient, &declared_values, model, offline.as_deref())?;
    if provider.is_none() && offline_enabled(offline.as_deref()) {
        return Err(failure(
            "Copilot offline mode requires a local model provider",
        ));
    }
    Ok(provider)
}

fn resolve_legacy(
    ambient: &LocalProviderContext,
    declared: &BTreeMap<String, String>,
    model: &str,
    offline: Option<&str>,
) -> Result<Option<LocalProvider>, NodeRunnerError> {
    let values = legacy_values(ambient, declared);
    if nonempty(&values, BASE_URL).is_none() {
        return Ok(None);
    }
    let provider = legacy_provider(&values, model)?;
    let redactions = configuration_redactions(&values).collect();
    Ok(Some(LocalProvider::singular(
        provider, offline, redactions, None,
    )?))
}

fn legacy_values(
    ambient: &LocalProviderContext,
    declared: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut values = ambient.legacy.clone();
    if declared.contains_key(BASE_URL) {
        remove_endpoint_secrets(&mut values);
    }
    if CREDENTIAL_NAMES
        .iter()
        .any(|name| declared.contains_key(*name))
    {
        remove_credentials(&mut values);
    }
    for (name, value) in declared {
        if LEGACY_NAMES.contains(&name.as_str()) {
            values.insert(name.clone(), value.clone());
        }
    }
    values
}

fn legacy_provider(
    values: &BTreeMap<String, String>,
    model: &str,
) -> Result<Map<String, Value>, NodeRunnerError> {
    let mut provider = Map::new();
    insert_text(values, &mut provider, BASE_URL, "baseUrl");
    insert_text(values, &mut provider, TYPE, "type");
    insert_text(values, &mut provider, API_KEY, "apiKey");
    insert_text(values, &mut provider, API_KEY_COMMAND, "apiKeyCommand");
    insert_text(values, &mut provider, BEARER_TOKEN, "bearerToken");
    insert_text(values, &mut provider, WIRE_API, "wireApi");
    insert_text(values, &mut provider, TRANSPORT, "transport");
    provider.insert(
        "modelId".to_owned(),
        Value::String(nonempty(values, MODEL_ID).unwrap_or(model).to_owned()),
    );
    insert_text(values, &mut provider, WIRE_MODEL, "wireModel");
    insert_number(values, &mut provider, MAX_PROMPT_TOKENS, "maxPromptTokens")?;
    insert_number(values, &mut provider, MAX_OUTPUT_TOKENS, "maxOutputTokens")?;
    if let Some(version) = nonempty(values, AZURE_API_VERSION) {
        provider.insert(
            "azure".to_owned(),
            Value::Object(Map::from_iter([(
                "apiVersion".to_owned(),
                Value::String(version.to_owned()),
            )])),
        );
    }
    if let Some(headers) = nonempty(values, HEADERS) {
        provider.insert("headers".to_owned(), Value::Object(parse_headers(headers)?));
    }
    Ok(provider)
}

fn resolve_registry(
    path: &Path,
    required: bool,
    model: &str,
    offline: Option<&str>,
) -> Result<RegistryResolution, NodeRunnerError> {
    let Some(contents) = read_registry(path, required)? else {
        return Ok(RegistryResolution::Inactive);
    };
    let registry = parse_registry(&contents)?;
    if registry.providers.is_empty() && registry.models.is_empty() {
        return Ok(RegistryResolution::Inactive);
    }
    if registry.providers.len() > MAX_REGISTRY_ITEMS || registry.models.len() > MAX_REGISTRY_ITEMS {
        return Err(failure("Copilot provider registry has too many entries"));
    }
    resolve_active_registry(registry, model, offline)
}

fn read_registry(path: &Path, required: bool) -> Result<Option<Vec<u8>>, NodeRunnerError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) if !required => return Ok(None),
        Err(_) => return Err(failure("Copilot provider registry could not be read")),
    };
    if !metadata.is_file() || metadata.len() > MAX_REGISTRY_BYTES {
        return Err(failure("Copilot provider registry is invalid or oversized"));
    }
    let contents =
        std::fs::read(path).map_err(|_| failure("Copilot provider registry could not be read"))?;
    if contents.len() as u64 > MAX_REGISTRY_BYTES {
        return Err(failure("Copilot provider registry is oversized"));
    }
    Ok(Some(contents))
}

fn parse_registry(contents: &[u8]) -> Result<ProviderRegistry, NodeRunnerError> {
    let root = serde_json::from_slice::<Value>(contents)
        .map_err(|_| failure("Copilot provider registry is invalid"))?;
    let object = root
        .as_object()
        .ok_or_else(|| failure("Copilot provider registry is invalid"))?;
    Ok(ProviderRegistry {
        providers: registry_array(object, "providers")?.to_vec(),
        models: registry_array(object, "models")?.to_vec(),
    })
}

fn resolve_active_registry(
    registry: ProviderRegistry,
    model: &str,
    offline: Option<&str>,
) -> Result<RegistryResolution, NodeRunnerError> {
    let redactions = registry_redactions(&registry.providers, &registry.models)?;
    let Some((provider_name, model_index)) = selected_registry_model(&registry.models, model)
    else {
        return Ok(RegistryResolution::Active(Some(registry_provider(
            registry,
            BTreeMap::new(),
            redactions,
            false,
        ))));
    };
    let provider_index = selected_registry_provider(&registry.providers, &provider_name)?;
    let selected_provider = registry.providers[provider_index]
        .as_object()
        .ok_or_else(|| failure("Copilot provider registry model references an invalid provider"))?;
    let selected_model = registry.models[model_index]
        .as_object()
        .ok_or_else(|| failure("Copilot provider registry model is invalid"))?;
    if has_api_key_command(selected_provider) {
        return command_registry_provider(
            RegistryCommandSelection {
                provider: selected_provider,
                model: selected_model,
                admitted_model: model,
                offline,
            },
            redactions,
        );
    }
    let base_url = selected_provider
        .get("baseUrl")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(RegistryResolution::Active(Some(registry_provider(
        registry,
        offline_environment(offline, base_url.as_deref())?,
        redactions,
        true,
    ))))
}

fn selected_registry_model(models: &[Value], model: &str) -> Option<(String, usize)> {
    models.iter().enumerate().find_map(|(index, entry)| {
        let entry = entry.as_object()?;
        let provider = entry.get("provider")?.as_str()?;
        let id = entry.get("id")?.as_str()?;
        (format!("{provider}/{id}") == model).then(|| (provider.to_owned(), index))
    })
}

fn selected_registry_provider(
    providers: &[Value],
    provider_name: &str,
) -> Result<usize, NodeRunnerError> {
    providers
        .iter()
        .enumerate()
        .find_map(|(index, entry)| {
            let entry = entry.as_object()?;
            (entry.get("name")?.as_str()? == provider_name).then_some(index)
        })
        .ok_or_else(|| failure("Copilot provider registry model references a missing provider"))
}

fn has_api_key_command(provider: &Map<String, Value>) -> bool {
    provider
        .get("apiKeyCommand")
        .is_some_and(|value| value.as_str().is_some_and(|value| !value.trim().is_empty()))
}

struct RegistryCommandSelection<'a> {
    provider: &'a Map<String, Value>,
    model: &'a Map<String, Value>,
    admitted_model: &'a str,
    offline: Option<&'a str>,
}

fn command_registry_provider(
    selection: RegistryCommandSelection<'_>,
    redactions: Vec<String>,
) -> Result<RegistryResolution, NodeRunnerError> {
    let singular = singular_registry_provider(selection.provider, selection.model)?;
    let session_model = selection
        .model
        .get("modelId")
        .and_then(Value::as_str)
        .or_else(|| selection.model.get("id").and_then(Value::as_str))
        .unwrap_or(selection.admitted_model)
        .to_owned();
    Ok(RegistryResolution::Active(Some(LocalProvider::singular(
        singular,
        selection.offline,
        redactions,
        Some(session_model),
    )?)))
}

fn registry_provider(
    registry: ProviderRegistry,
    process_environment: BTreeMap<String, String>,
    redactions: Vec<String>,
    bypasses_github_auth: bool,
) -> LocalProvider {
    LocalProvider {
        parameters: ProviderParameters::Registry {
            providers: Value::Array(registry.providers),
            models: Value::Array(registry.models),
        },
        process_environment,
        redactions,
        bypasses_github_auth,
        uses_api_key_command: false,
        session_model: None,
    }
}

fn registry_array<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a [Value], NodeRunnerError> {
    match object.get(name) {
        None => Ok(&[]),
        Some(Value::Array(values)) => Ok(values),
        Some(_) => Err(failure("Copilot provider registry is invalid")),
    }
}

fn singular_registry_provider(
    provider: &Map<String, Value>,
    model: &Map<String, Value>,
) -> Result<Map<String, Value>, NodeRunnerError> {
    let mut singular = provider.clone();
    singular.remove("name");
    let id = model
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| failure("Copilot provider registry model is invalid"))?;
    singular.insert(
        "modelId".to_owned(),
        model
            .get("modelId")
            .cloned()
            .unwrap_or_else(|| Value::String(id.to_owned())),
    );
    singular.insert(
        "wireModel".to_owned(),
        model
            .get("wireModel")
            .cloned()
            .unwrap_or_else(|| Value::String(id.to_owned())),
    );
    for name in [
        "maxPromptTokens",
        "maxContextWindowTokens",
        "maxOutputTokens",
    ] {
        if let Some(value) = model.get(name) {
            singular.insert(name.to_owned(), value.clone());
        }
    }
    if let Some(value) = model.get("capabilities") {
        singular.insert("modelCapabilities".to_owned(), value.clone());
    }
    Ok(singular)
}

impl LocalProvider {
    fn singular(
        provider: Map<String, Value>,
        offline: Option<&str>,
        redactions: Vec<String>,
        session_model: Option<String>,
    ) -> Result<Self, NodeRunnerError> {
        let base_url = provider
            .get("baseUrl")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let uses_api_key_command = provider
            .get("apiKeyCommand")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        Ok(Self {
            parameters: ProviderParameters::Singular(Value::Object(provider)),
            process_environment: offline_environment(offline, base_url.as_deref())?,
            redactions,
            bypasses_github_auth: true,
            uses_api_key_command,
            session_model,
        })
    }

    pub(super) fn configure(&self, params: &mut Value) {
        match &self.parameters {
            ProviderParameters::Singular(provider) => params["provider"] = provider.clone(),
            ProviderParameters::Registry { providers, models } => {
                params["providers"] = providers.clone();
                params["models"] = models.clone();
            }
        }
    }

    pub(super) fn process_environment(&self) -> &BTreeMap<String, String> {
        &self.process_environment
    }

    pub(super) fn bypasses_github_auth(&self) -> bool {
        self.bypasses_github_auth
    }

    pub(super) fn uses_api_key_command(&self) -> bool {
        self.uses_api_key_command
    }

    pub(super) fn session_model<'a>(&'a self, admitted: &'a str) -> &'a str {
        self.session_model.as_deref().unwrap_or(admitted)
    }

    pub(super) fn redactions(&self) -> impl Iterator<Item = String> + '_ {
        self.redactions.iter().cloned()
    }
}

pub(super) fn remove_local_configuration(environment: &mut BTreeMap<String, String>) {
    for name in LEGACY_NAMES
        .iter()
        .copied()
        .chain([PROVIDERS_CONFIG, OFFLINE])
    {
        environment.remove(name);
    }
}

pub(super) fn configuration_redactions(
    values: &BTreeMap<String, String>,
) -> impl Iterator<Item = String> + '_ {
    [BASE_URL, API_KEY, API_KEY_COMMAND, BEARER_TOKEN, HEADERS]
        .into_iter()
        .filter_map(|name| values.get(name))
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn registry_redactions(
    providers: &[Value],
    models: &[Value],
) -> Result<Vec<String>, NodeRunnerError> {
    let mut redactions = Vec::new();
    let mut pending = providers.iter().chain(models).collect::<Vec<_>>();
    while let Some(value) = pending.pop() {
        if let Some(value) = value.as_str().filter(|value| !value.trim().is_empty()) {
            record_redaction(value, &mut redactions)?;
            continue;
        }
        if let Some(values) = value.as_array() {
            pending.extend(values);
            continue;
        }
        if let Some(values) = value.as_object() {
            pending.extend(values.values());
        }
    }
    Ok(redactions)
}

fn record_redaction(value: &str, redactions: &mut Vec<String>) -> Result<(), NodeRunnerError> {
    if value.len() > MAX_REDACTION_BYTES || redactions.len() >= MAX_REDACTIONS {
        return Err(failure("Copilot provider registry is too complex"));
    }
    redactions.push(value.to_owned());
    Ok(())
}

fn offline_environment(
    offline: Option<&str>,
    base_url: Option<&str>,
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    if !offline_enabled(offline) {
        return Ok(BTreeMap::new());
    }
    let base_url = base_url
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| failure("Copilot offline provider has no base URL"))?;
    Ok(BTreeMap::from([
        (OFFLINE.to_owned(), offline.unwrap_or("true").to_owned()),
        (BASE_URL.to_owned(), base_url.to_owned()),
    ]))
}

fn offline_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "y"
        )
    })
}

fn remove_endpoint_secrets(values: &mut BTreeMap<String, String>) {
    remove_credentials(values);
    values.remove(HEADERS);
}

fn remove_credentials(values: &mut BTreeMap<String, String>) {
    for name in CREDENTIAL_NAMES {
        values.remove(*name);
    }
}

fn nonempty<'a>(values: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    values
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn insert_text(
    values: &BTreeMap<String, String>,
    provider: &mut Map<String, Value>,
    name: &str,
    field: &str,
) {
    if let Some(value) = nonempty(values, name) {
        provider.insert(field.to_owned(), Value::String(value.to_owned()));
    }
}

fn insert_number(
    values: &BTreeMap<String, String>,
    provider: &mut Map<String, Value>,
    name: &str,
    field: &str,
) -> Result<(), NodeRunnerError> {
    if let Some(value) = nonempty(values, name) {
        let number = value
            .parse::<u64>()
            .map_err(|_| failure(format!("Copilot {name} is not a valid token limit")))?;
        provider.insert(field.to_owned(), Value::Number(Number::from(number)));
    }
    Ok(())
}

fn parse_headers(text: &str) -> Result<Map<String, Value>, NodeRunnerError> {
    text.replace("\\n", "\n")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .try_fold(Map::new(), |mut headers, line| {
            let (name, value) = line
                .split_once(':')
                .filter(|(name, _)| !name.trim().is_empty())
                .ok_or_else(|| failure("Copilot provider headers are invalid"))?;
            headers.insert(
                name.trim().to_owned(),
                Value::String(value.trim().to_owned()),
            );
            Ok(headers)
        })
}

#[cfg(test)]
#[path = "tests/provider.rs"]
mod tests;
