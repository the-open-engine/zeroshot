//! Fixed provider selection over Copilot's native BYOK configuration.
use super::*;
use crate::native_v2_contract::CopilotProvider;
use crate::native_v2_capsule::gateway;

const CREDENTIALS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY",
    "GATEWAY_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
];

pub(crate) fn is_credential(name: &str) -> bool {
    CREDENTIALS.iter().any(|candidate| {
        if cfg!(windows) {
            name.eq_ignore_ascii_case(candidate)
        } else {
            name == *candidate
        }
    })
}

pub(super) fn take_context(environment: &mut BTreeMap<String, String>) -> BTreeMap<String, String> {
    [
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENROUTER_API_KEY",
        "OPENROUTER_BASE_URL",
    ]
    .into_iter()
    .filter_map(|name| {
        environment
            .remove(name)
            .map(|value| (name.to_owned(), value))
    })
    .collect()
}

struct Selection {
    kind: &'static str,
    key: &'static str,
    endpoint_fields: &'static [&'static str],
    default_endpoint: String,
    wire: Option<&'static str>,
}

fn selection(
    provider: CopilotProvider,
    values: &BTreeMap<String, String>,
) -> Result<Selection, NodeRunnerError> {
    Ok(match provider {
        CopilotProvider::OpenAi => Selection {
            kind: "openai",
            key: "OPENAI_API_KEY",
            endpoint_fields: &["OPENAI_BASE_URL", "OPENAI_API_BASE"],
            default_endpoint: "https://api.openai.com/v1".to_owned(),
            wire: Some("responses"),
        },
        CopilotProvider::Anthropic => Selection {
            kind: "anthropic",
            key: "ANTHROPIC_API_KEY",
            endpoint_fields: &["ANTHROPIC_BASE_URL"],
            default_endpoint: "https://api.anthropic.com".to_owned(),
            wire: None,
        },
        CopilotProvider::OpenRouter => Selection {
            kind: "openai",
            key: "OPENROUTER_API_KEY",
            endpoint_fields: &["OPENROUTER_BASE_URL"],
            default_endpoint: "https://openrouter.ai/api/v1".to_owned(),
            wire: Some("completions"),
        },
        CopilotProvider::Gateway => gateway_selection(values)?,
        CopilotProvider::Bedrock => bedrock_selection(values)?,
        CopilotProvider::Github => return Err(failure("Copilot GitHub selection is native")),
    })
}

fn bedrock_selection(values: &BTreeMap<String, String>) -> Result<Selection, NodeRunnerError> {
    let region = values
        .get("AWS_REGION")
        .filter(|value| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        .ok_or_else(|| failure("Copilot Bedrock requires a valid AWS_REGION"))?;
    Ok(Selection {
        kind: "openai",
        key: "AWS_BEARER_TOKEN_BEDROCK",
        endpoint_fields: &["AWS_BEDROCK_BASE_URL"],
        default_endpoint: format!("https://bedrock-runtime.{region}.amazonaws.com/openai/v1"),
        wire: Some("responses"),
    })
}

fn gateway_selection(values: &BTreeMap<String, String>) -> Result<Selection, NodeRunnerError> {
    let _ = gateway::connection(values)?;
    let (kind, wire) = match values.get("GATEWAY_API").map(String::as_str) {
        Some("openai-responses") => ("openai", Some("responses")),
        Some("openai-completions") => ("openai", Some("completions")),
        Some("anthropic-messages") => ("anthropic", None),
        _ => return Err(failure("Copilot gateway requires a supported GATEWAY_API")),
    };
    Ok(Selection {
        kind,
        wire,
        key: "GATEWAY_API_KEY",
        endpoint_fields: &["GATEWAY_BASE_URL"],
        default_endpoint: "".to_owned(),
    })
}

pub(crate) fn resolve_explicit_configuration(
    provider: CopilotProvider,
    ambient: Option<&LocalProviderContext>,
    environment: &ResolvedEnvironment,
    model: &str,
) -> Result<LocalProvider, NodeRunnerError> {
    let mut values = environment
        .iter()
        .map(|(key, value)| (key.as_str().to_owned(), value.to_owned()))
        .collect::<BTreeMap<_, _>>();
    let selected = selection(provider, &values)?;
    merge_context(&mut values, ambient, &selected)?;
    let key = declared_key(&values, selected.key)?;
    let mut native = native_configuration(ambient, environment, model)?;
    compatible_parameters(&mut native, provider, &selected, key.is_some())?;
    let changed_endpoint = configure_endpoint(&mut native, provider, &selected, &values)?;
    configure_headers(&mut native, changed_endpoint, &values)?;
    configure_protocol(&mut native, provider, &selected);
    configure_authentication(&mut native, key)?;
    let redactions = registry_redactions(&[Value::Object(native.clone())], &[])?;
    let session_model = native
        .get("modelId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let offline = values
        .get(OFFLINE)
        .map(String::as_str)
        .or_else(|| ambient.and_then(|context| context.offline.as_deref()));
    LocalProvider::singular(native, offline, redactions, session_model)
}

fn configure_headers(
    native: &mut Map<String, Value>,
    changed: bool,
    values: &BTreeMap<String, String>,
) -> Result<(), NodeRunnerError> {
    if changed {
        native.remove("headers");
    }
    if let Some(headers) = values.get(HEADERS) {
        native.insert("headers".to_owned(), Value::Object(parse_headers(headers)?));
    }
    Ok(())
}

fn merge_context(
    values: &mut BTreeMap<String, String>,
    ambient: Option<&LocalProviderContext>,
    selected: &Selection,
) -> Result<(), NodeRunnerError> {
    if !values.contains_key(selected.key)
        && selected
            .endpoint_fields
            .iter()
            .any(|name| values.contains_key(*name))
    {
        return Err(failure(
            "Copilot declared endpoint requires its own declared credential",
        ));
    }
    let Some(ambient) = ambient else {
        return Ok(());
    };
    if let Some(value) = ambient.canonical.get(selected.key) {
        values
            .entry(selected.key.to_owned())
            .or_insert_with(|| value.clone());
    }
    if !selected
        .endpoint_fields
        .iter()
        .any(|name| values.contains_key(*name))
    {
        for name in selected.endpoint_fields {
            if let Some(value) = ambient.canonical.get(*name) {
                values.insert((*name).to_owned(), value.clone());
            }
        }
    }
    Ok(())
}

fn declared_key<'a>(
    values: &'a BTreeMap<String, String>,
    field: &str,
) -> Result<Option<&'a str>, NodeRunnerError> {
    for candidate in CREDENTIALS {
        if values.contains_key(*candidate) && *candidate != field {
            return Err(failure(
                "Copilot credential conflicts with the selected provider",
            ));
        }
    }
    let key = values.get(field).map(String::as_str);
    if key.is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control)) {
        return Err(failure("Copilot declared provider credential is invalid"));
    }
    Ok(key)
}

fn compatible_parameters(
    native: &mut Map<String, Value>,
    provider: CopilotProvider,
    selected: &Selection,
    declared: bool,
) -> Result<(), NodeRunnerError> {
    let kind = native
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("openai");
    let azure_endpoint = native.get("baseUrl").and_then(Value::as_str).is_some();
    let compatible = kind == selected.kind
        || provider == CopilotProvider::OpenAi && kind == "azure" && azure_endpoint;
    if !compatible {
        if !declared {
            return Err(failure(
                "Copilot native provider conflicts with the selected lane",
            ));
        }
        native.clear();
    }
    Ok(())
}

fn configure_endpoint(
    native: &mut Map<String, Value>,
    provider: CopilotProvider,
    selected: &Selection,
    values: &BTreeMap<String, String>,
) -> Result<bool, NodeRunnerError> {
    let declared = values.contains_key(selected.key);
    let endpoint = endpoint(values, selected.endpoint_fields)?;
    let prior = native.get("baseUrl").and_then(Value::as_str);
    let expected = endpoint.as_deref().unwrap_or(&selected.default_endpoint);
    let changed =
        prior.is_none_or(|prior| prior.trim_end_matches('/') != expected.trim_end_matches('/'));
    if !declared
        && changed
        && (endpoint.is_some()
            || matches!(
                provider,
                CopilotProvider::OpenRouter | CopilotProvider::Bedrock
            ))
    {
        return Err(failure(
            "Copilot selected endpoint requires matching declared authentication",
        ));
    }
    let owned = matches!(
        provider,
        CopilotProvider::Gateway | CopilotProvider::OpenRouter | CopilotProvider::Bedrock
    ) && declared;
    let chosen = endpoint
        .as_deref()
        .or_else(|| (!owned).then_some(prior).flatten())
        .unwrap_or(&selected.default_endpoint);
    gateway::validate_base_url(chosen)
        .map_err(|_| failure("Copilot provider endpoint is invalid"))?;
    let changed =
        prior.is_none_or(|prior| prior.trim_end_matches('/') != chosen.trim_end_matches('/'));
    native.insert("baseUrl".to_owned(), Value::String(chosen.to_owned()));
    Ok(changed)
}

fn configure_protocol(
    native: &mut Map<String, Value>,
    provider: CopilotProvider,
    selected: &Selection,
) {
    native
        .entry("type")
        .or_insert_with(|| Value::String(selected.kind.to_owned()));
    if provider == CopilotProvider::Gateway || !native.contains_key("wireApi") {
        if let Some(wire) = selected.wire {
            native.insert("wireApi".to_owned(), Value::String(wire.to_owned()));
        } else {
            native.remove("wireApi");
        }
    }
}

fn configure_authentication(
    native: &mut Map<String, Value>,
    key: Option<&str>,
) -> Result<(), NodeRunnerError> {
    if let Some(key) = key {
        remove_authentication(native);
        native.insert("apiKey".to_owned(), Value::String(key.to_owned()));
    }
    if !["apiKey", "apiKeyCommand", "bearerToken"]
        .iter()
        .any(|name| {
            native
                .get(*name)
                .and_then(Value::as_str)
                .is_some_and(|v| !v.trim().is_empty())
        })
    {
        return Err(failure(
            "Copilot selected provider requires native or declared authentication",
        ));
    }
    Ok(())
}

fn native_configuration(
    ambient: Option<&LocalProviderContext>,
    environment: &ResolvedEnvironment,
    model: &str,
) -> Result<Map<String, Value>, NodeRunnerError> {
    let Some(ambient) = ambient else {
        return Ok(Map::new());
    };
    let Some(native) = resolve_local_configuration(ambient, environment, model)? else {
        return Ok(Map::new());
    };
    match native.parameters {
        ProviderParameters::Singular(Value::Object(parameters)) => Ok(parameters),
        ProviderParameters::Registry {
            providers: Value::Array(providers),
            models: Value::Array(models),
        } => registry_parameters(&providers, &models, model),
        _ => Err(failure("Copilot native provider configuration is invalid")),
    }
}

fn registry_parameters(
    providers: &[Value],
    models: &[Value],
    model: &str,
) -> Result<Map<String, Value>, NodeRunnerError> {
    let Some((name, index)) = selected_registry_model(models, model) else {
        return Ok(Map::new());
    };
    let index_provider = selected_registry_provider(providers, &name)?;
    singular_registry_provider(
        providers[index_provider]
            .as_object()
            .ok_or_else(|| failure("Copilot provider is invalid"))?,
        models[index]
            .as_object()
            .ok_or_else(|| failure("Copilot model is invalid"))?,
    )
}

fn endpoint(
    values: &BTreeMap<String, String>,
    fields: &[&str],
) -> Result<Option<String>, NodeRunnerError> {
    let mut result: Option<String> = None;
    for field in fields {
        if let Some(value) = values.get(*field) {
            gateway::validate_base_url(value)
                .map_err(|_| failure("Copilot provider endpoint is invalid"))?;
            if result.as_ref().is_some_and(|prior| prior != value) {
                return Err(failure("Copilot provider endpoints disagree"));
            }
            result = Some(value.clone());
        }
    }
    Ok(result)
}

fn remove_authentication(parameters: &mut Map<String, Value>) {
    for field in [
        "apiKey",
        "apiKeyCommand",
        "bearerToken",
        "bearerTokenProvider",
    ] {
        parameters.remove(field);
    }
    if let Some(Value::Object(headers)) = parameters.get_mut("headers") {
        headers.retain(|name, _| !authentication_header(name));
    }
}

fn authentication_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "proxy-authorization" | "x-api-key" | "api-key"
    )
}
