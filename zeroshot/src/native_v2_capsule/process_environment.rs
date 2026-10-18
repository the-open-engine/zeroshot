//! Non-secret launch context shared by CLI harness adapters.
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) const MINIMAL_ENVIRONMENT_NAMES: [&str; 7] = [
    "HOME",
    "LANG",
    "LC_ALL",
    "PATH",
    "TERM",
    "TMPDIR",
    "ZEROSHOT_TOOLS",
];

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProcessEnvironmentError {
    #[error("base process environment contains non-minimal name {0}")]
    NonMinimalEnvironment(String),
    #[error("base process environment contains an invalid value")]
    InvalidEnvironment,
}

/// Explicit non-secret environment needed to launch the CLI and its tools.
///
/// The controller supplies this value. It is deliberately not populated from the ambient process,
/// and only a small allowlist can cross this boundary.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct HarnessProcessEnvironment(BTreeMap<String, String>);

impl HarnessProcessEnvironment {
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, ProcessEnvironmentError> {
        for (name, value) in &values {
            if !MINIMAL_ENVIRONMENT_NAMES.contains(&name.as_str()) {
                return Err(ProcessEnvironmentError::NonMinimalEnvironment(name.clone()));
            }
            if value.contains('\0') {
                return Err(ProcessEnvironmentError::InvalidEnvironment);
            }
        }
        Ok(Self(values))
    }

    pub(crate) fn clone_values(&self) -> BTreeMap<String, String> {
        self.0.clone()
    }

    pub(crate) fn for_capsule(
        &self,
        runtime_home: &Path,
        default_path: &str,
    ) -> Result<Self, ProcessEnvironmentError> {
        let runtime_home = runtime_home
            .to_str()
            .filter(|value| !value.is_empty())
            .ok_or(ProcessEnvironmentError::InvalidEnvironment)?;
        let mut values = self.clone_values();
        values.insert("HOME".to_owned(), runtime_home.to_owned());
        values
            .entry("PATH".to_owned())
            .or_insert_with(|| default_path.to_owned());
        Self::new(values)
    }
}

impl std::fmt::Debug for HarnessProcessEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("HarnessProcessEnvironment")
            .field(&self.0.keys().collect::<Vec<_>>())
            .finish()
    }
}
