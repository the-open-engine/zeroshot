use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use tokio::process::Command;
use crate::source_code_provider::{SourceDeliveryCredential, SourceGitCredential};
use super::GitTransportConfig;

#[cfg(target_os = "linux")]
const NSS_WRAPPER_LIBRARY: &str = "/usr/local/lib/zeroshot/libnss_wrapper.so";

pub(super) struct GitAuthentication {
    configuration: Vec<(String, String)>,
    ssh_command: Option<String>,
    github_token: Option<String>,
    secrets: Vec<String>,
    _private: Option<tempfile::TempDir>,
    #[cfg(target_os = "linux")]
    nss_files: Option<(PathBuf, PathBuf)>,
}

impl GitAuthentication {
    pub(super) fn prepare(
        config: &GitTransportConfig,
        remote: &OsStr,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Self, std::io::Error> {
        let mut authentication = Self {
            configuration: Vec::new(),
            ssh_command: None,
            github_token: None,
            secrets: Vec::new(),
            _private: None,
            #[cfg(target_os = "linux")]
            nss_files: None,
        };
        let token = credential.github_git_token().unwrap_or("");
        authentication.secrets.extend([
            credential.expose().to_owned(),
            token.to_owned(),
            super::super::git_auth::encode_basic_credential(token),
        ]);
        let fallback = github_fallback(remote, credential);
        authentication.github_token = fallback.then(|| token.to_owned());
        let credential = credential.git().or_else(|| {
            fallback.then_some(SourceGitCredential::Https {
                username: "x-access-token",
                password: token,
            })
        });
        match credential {
            Some(SourceGitCredential::Https { username, password }) => {
                authentication.prepare_https(remote, username, password)?;
            }
            Some(SourceGitCredential::Ssh {
                private_key,
                known_hosts,
            }) => {
                require_ssh_remote(remote)?;
                authentication.prepare_ssh(config, private_key, known_hosts)?;
            }
            None => {}
        }
        #[cfg(target_os = "linux")]
        if let Some(identity) = config.identity.filter(|_| is_ssh_remote(remote)) {
            authentication.prepare_contained_identity(config, identity)?;
        }
        authentication.collect_native_secrets(config);
        Ok(authentication)
    }

    fn collect_native_secrets(&mut self, config: &GitTransportConfig) {
        if let Some(environment) = &config.local_environment {
            self.secrets.extend(
                environment
                    .iter()
                    .filter(|(name, _)| super::super::command::native_credential_name(name))
                    .map(|(_, value)| value.clone()),
            );
        }
    }

    fn prepare_https(
        &mut self,
        remote: &OsStr,
        username: &str,
        password: &str,
    ) -> Result<(), std::io::Error> {
        let remote = remote
            .to_str()
            .ok_or_else(|| std::io::Error::other("Git remote is not UTF-8"))?;
        if !remote.starts_with("https://") && !remote.starts_with("http://") {
            return Err(std::io::Error::other(
                "HTTPS credentials cannot authorize an SSH or filesystem remote",
            ));
        }
        let basic = super::super::git_auth::encode_user_password(username, password);
        self.secrets
            .extend([username.to_owned(), password.to_owned(), basic.clone()]);
        self.configuration.extend([
            ("credential.helper".to_owned(), String::new()),
            (format!("http.{remote}.extraheader"), String::new()),
            (
                format!("http.{remote}.extraheader"),
                format!("AUTHORIZATION: basic {basic}"),
            ),
            ("core.askPass".to_owned(), String::new()),
        ]);
        Ok(())
    }

    fn prepare_ssh(
        &mut self,
        config: &GitTransportConfig,
        private_key: &str,
        known_hosts: &str,
    ) -> Result<(), std::io::Error> {
        let directory = private_directory(config)?;
        let key = directory.path().join("identity");
        let hosts = directory.path().join("known_hosts");
        for (path, bytes) in [
            (&key, private_key.as_bytes()),
            (&hosts, known_hosts.as_bytes()),
        ] {
            write_private_file(path, bytes, config.identity)?;
        }
        set_owner(directory.path(), config.identity)?;
        self.ssh_command = Some(format!(
            "ssh -F /dev/null -o BatchMode=yes -o IdentitiesOnly=yes -o IdentityAgent=none \
             -o StrictHostKeyChecking=yes -o UserKnownHostsFile={} -o GlobalKnownHostsFile=/dev/null \
             -i {}",
            shell_path(&hosts)?,
            shell_path(&key)?
        ));
        self.secrets
            .extend([private_key.to_owned(), known_hosts.to_owned()]);
        self.secrets.extend(
            private_key
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
        self._private = Some(directory);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn prepare_contained_identity(
        &mut self,
        config: &GitTransportConfig,
        identity: crate::execution::process::HostedProcessIdentity,
    ) -> Result<(), std::io::Error> {
        check_nss_wrapper(Path::new(NSS_WRAPPER_LIBRARY))?;
        let directory = match &self._private {
            Some(directory) => directory.path().to_owned(),
            None => {
                let directory = private_directory(config)?;
                let path = directory.path().to_owned();
                self._private = Some(directory);
                path
            }
        };
        self.prepare_identity(&directory, identity)?;
        set_owner(&directory, Some(identity))
    }

    #[cfg(target_os = "linux")]
    fn prepare_identity(
        &mut self,
        directory: &Path,
        identity: crate::execution::process::HostedProcessIdentity,
    ) -> Result<(), std::io::Error> {
        for (name, record) in nss_records(identity.uid(), identity.gid()) {
            write_private_file(&directory.join(name), record.as_bytes(), Some(identity))?;
        }
        self.nss_files = Some((directory.join("passwd"), directory.join("group")));
        Ok(())
    }

    pub(super) fn configure(&self, command: &mut Command) {
        if !self.configuration.is_empty() || self.ssh_command.is_some() {
            command.env("GIT_ASKPASS", "").env("SSH_ASKPASS", "");
        }
        if !self.configuration.is_empty() {
            command.env("GIT_CONFIG_COUNT", self.configuration.len().to_string());
            for (index, (key, value)) in self.configuration.iter().enumerate() {
                command
                    .env(format!("GIT_CONFIG_KEY_{index}"), key)
                    .env(format!("GIT_CONFIG_VALUE_{index}"), value);
            }
        }
        if let Some(token) = &self.github_token {
            command.env("GH_TOKEN", token);
        }
        #[cfg(target_os = "linux")]
        if let Some((passwd, group)) = &self.nss_files {
            command
                .env("LD_PRELOAD", NSS_WRAPPER_LIBRARY)
                .env("NSS_WRAPPER_PASSWD", passwd)
                .env("NSS_WRAPPER_GROUP", group);
        }
        if let Some(ssh) = &self.ssh_command {
            command
                .env("GIT_SSH_COMMAND", ssh)
                .env_remove("SSH_AUTH_SOCK")
                .env_remove("SSH_AGENT_PID");
        }
    }

    pub(super) fn secrets(&self) -> &[String] {
        &self.secrets
    }
}

fn private_directory(config: &GitTransportConfig) -> Result<tempfile::TempDir, std::io::Error> {
    let directory = tempfile::Builder::new()
        .prefix("git-auth-")
        .tempdir_in(&config.private_directory)?;
    crate::execution::platform::private_directory(directory.path())?;
    Ok(directory)
}

fn write_private_file(
    path: &Path,
    bytes: &[u8],
    identity: Option<crate::execution::process::HostedProcessIdentity>,
) -> Result<(), std::io::Error> {
    let mut file = crate::execution::platform::private_file(
        path,
        crate::execution::platform::FileAccess::CreateNew,
    )?;
    file.write_all(bytes)?;
    file.sync_all()?;
    set_owner(path, identity)
}

#[cfg(target_os = "linux")]
fn nss_records(uid: u32, gid: u32) -> [(&'static str, String); 2] {
    [
        (
            "passwd",
            format!("zeroshot:x:{uid}:{gid}:Zeroshot Git:/nonexistent:/usr/sbin/nologin\n"),
        ),
        ("group", format!("zeroshot:x:{gid}:\n")),
    ]
}

#[cfg(target_os = "linux")]
fn check_nss_wrapper(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path).map_err(|error| {
        std::io::Error::other(format!(
            "contained SSH requires NSS wrapper {}: {error}",
            path.display(),
        ))
    })?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(std::io::Error::other(
            "contained SSH NSS wrapper must be a root-owned file without group or other write access",
        ));
    }
    Ok(())
}

fn shell_path(path: &Path) -> Result<String, std::io::Error> {
    let value = path
        .to_str()
        .ok_or_else(|| std::io::Error::other("private Git path is not UTF-8"))?;
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn set_owner(
    path: &Path,
    identity: Option<crate::execution::process::HostedProcessIdentity>,
) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    if let Some(identity) = identity {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        if unsafe { libc::chown(name.as_ptr(), identity.uid(), identity.gid()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(not(unix))]
    let _ = (path, identity);
    Ok(())
}

fn github_fallback(remote: &OsStr, credential: SourceDeliveryCredential<'_>) -> bool {
    credential.git().is_none()
        && credential
            .github_git_token()
            .is_some_and(|token| !token.is_empty())
        && remote
            .to_str()
            .is_some_and(|remote| remote.starts_with("https://github.com/"))
}

fn is_ssh_remote(remote: &OsStr) -> bool {
    remote.to_str().is_some_and(|value| {
        !value.starts_with("https://")
            && openengine_cluster_protocol::GitRemoteUrl::new(value).is_ok()
    })
}

fn require_ssh_remote(remote: &OsStr) -> Result<(), std::io::Error> {
    if is_ssh_remote(remote) {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "SSH credentials cannot authorize an HTTPS or filesystem remote",
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "authentication/tests.rs"]
mod tests;
