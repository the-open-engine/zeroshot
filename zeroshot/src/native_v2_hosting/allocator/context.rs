//! Ephemeral, supervisor-owned context checkouts stay outside workspace checkpoints and delivery.
use super::*;
use crate::native_v2_runner::AgentContextProvider;

pub(super) fn directory(run_root: &Path) -> PathBuf {
    run_root.join("context")
}

pub(super) fn process_environment(run_root: &Path) -> [(String, String); 2] {
    let root = directory(run_root);
    [
        (
            "ZEROSHOT_CONTEXT".to_owned(),
            root.to_string_lossy().into_owned(),
        ),
        (
            "GIT_CONFIG_GLOBAL".to_owned(),
            run_root
                .join("runtime/context-gitconfig")
                .to_string_lossy()
                .into_owned(),
        ),
    ]
}

impl ProductionCapsuleAllocator {
    pub(super) async fn install_context_sources(
        &self,
        request: &CapsuleBuildRequest<'_>,
        filesystem: &CapsuleFilesystem,
        pool: HostedProcessPool,
    ) -> Result<(), CapsuleAllocationUnavailable> {
        openengine_cluster_protocol::validate_context_sources(
            request.admitted.environment.as_ref(),
            &request.admitted.context_sources,
            request.admitted.source.as_ref(),
        )
        .map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?;
        if request.admitted.context_sources.is_empty() {
            return Ok(());
        }
        let run_root = filesystem
            .workspace
            .parent()
            .ok_or(CapsuleAllocationUnavailable::ContextCheckout)?;
        let root = directory(run_root);
        // Allocation owns this directory, including an interrupted preparation's partial clones.
        if root.exists() {
            std::fs::remove_dir_all(&root)
                .map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?;
        }
        let writer = pool
            .identity(HostedProcessScope::Writer)
            .map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?;
        create_context_root(&root, writer.uid())
            .map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?;
        for pin in &request.admitted.context_sources {
            let checkout = root.join(pin.repository.as_str());
            let result = async {
                std::fs::create_dir_all(&checkout)?;
                let mut parent = checkout.as_path();
                while parent != root {
                    traversable(parent)?;
                    parent = parent
                        .parent()
                        .ok_or_else(|| std::io::Error::other("context path escaped its root"))?;
                }
                #[cfg(unix)]
                std::os::unix::fs::chown(&checkout, Some(writer.uid()), Some(writer.gid()))?;
                let source = production_source(pin);
                super::super::repository::install_context_repository(RepositoryInstall {
                    git_program: &self.config.git_program,
                    source: &source,
                    resolved: pin,
                    checkout_references: &[],
                    workspace: &checkout,
                    process_pool: pool,
                    github_token: request.github_token,
                    git_credentials: request.git_credentials,
                    private_directory: &filesystem.runtime_home,
                })
                .await?;
                Ok::<_, super::super::repository::RepositoryInstallError>(())
            }
            .await;
            if let Err(error) = result {
                error.record_context_diagnostic(
                    request.run_id,
                    pin.repository.as_str(),
                    &self.operator_diagnostics,
                );
                return Err(CapsuleAllocationUnavailable::ContextCheckout);
            }
        }
        // Git refuses repositories owned by another UID unless a protected config trusts their
        // exact paths. This file contains no auth and is shared by hooks and every harness.
        let mut config = String::from("[safe]\n");
        for pin in &request.admitted.context_sources {
            let path = root
                .join(pin.repository.as_str())
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\t', "\\t");
            config.push_str(&format!("\tdirectory = \"{path}\"\n"));
        }
        let trust = filesystem.runtime_home.join("context-gitconfig");
        std::fs::write(&trust, config)
            .map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?;
        seal(&trust).map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?;
        // Keep the root's UID-specific ACL intact while sealing every checkout below it.
        for entry in
            std::fs::read_dir(&root).map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?
        {
            seal(
                &entry
                    .map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?
                    .path(),
            )
            .map_err(|_| CapsuleAllocationUnavailable::ContextCheckout)?;
        }
        Ok(())
    }
}

fn traversable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn create_context_root(path: &Path, writer_uid: u32) -> std::io::Result<()> {
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder.create(path)?;
    restrict_context_root(path, writer_uid)
}

#[cfg(target_os = "linux")]
fn restrict_context_root(path: &Path, writer_uid: u32) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    // Linux POSIX access ACL: supervisor rwx, this run's UID r-x, group/other no access.
    // Run pools share their primary GID, so group permissions cannot isolate private histories.
    let mut acl = 2_u32.to_le_bytes().to_vec(); // POSIX_ACL_XATTR_VERSION
    for (tag, permissions, id) in [
        (0x01_u16, 7_u16, u32::MAX), // ACL_USER_OBJ
        (0x02, 5, writer_uid),       // ACL_USER
        (0x04, 0, u32::MAX),         // ACL_GROUP_OBJ
        (0x10, 5, u32::MAX),         // ACL_MASK
        (0x20, 0, u32::MAX),         // ACL_OTHER
    ] {
        acl.extend_from_slice(&tag.to_le_bytes());
        acl.extend_from_slice(&permissions.to_le_bytes());
        acl.extend_from_slice(&id.to_le_bytes());
    }
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: the path and attribute are NUL-terminated; acl is live for the supplied byte length.
    if unsafe {
        libc::setxattr(
            path.as_ptr(),
            c"system.posix_acl_access".as_ptr(),
            acl.as_ptr().cast(),
            acl.len(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn restrict_context_root(_path: &Path, _writer_uid: u32) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "context checkouts require Linux UID isolation",
    ))
}

fn seal(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            seal(&entry?.path())?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Only the supervisor owns these files. Agent/hook identities cannot chmod them back.
        std::os::unix::fs::chown(
            path,
            Some(unsafe { libc::geteuid() }),
            Some(unsafe { libc::getegid() }),
        )?;
        let mode = if metadata.is_dir() || metadata.permissions().mode() & 0o111 != 0 {
            0o755
        } else {
            0o644
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

struct ContextGuidance {
    inherited: Option<Arc<dyn AgentContextProvider>>,
    guidance: String,
}

impl AgentContextProvider for ContextGuidance {
    fn context(&self) -> String {
        let inherited = self
            .inherited
            .as_ref()
            .map(|provider| provider.context())
            .unwrap_or_default();
        format!("{inherited}\n{}", self.guidance)
    }
}

pub(super) fn agent_context(
    admitted: &AdmittedRun,
    run_root: &Path,
    inherited: Option<Arc<dyn AgentContextProvider>>,
) -> Option<Arc<dyn AgentContextProvider>> {
    if admitted.context_sources.is_empty() {
        return inherited;
    }
    let mut guidance = String::from(
        "Read-only context repositories are available at $ZEROSHOT_CONTEXT. They cannot be modified or pushed. For experiments, copy a repository into $TMPDIR or another location outside the workspace. The workspace is delivered; never copy context repositories into it.\n",
    );
    for source in &admitted.context_sources {
        use std::fmt::Write;
        let _ = writeln!(
            guidance,
            "- {} at {} (pinned commit {})",
            source.repository,
            directory(run_root)
                .join(source.repository.as_str())
                .display(),
            source.revision
        );
    }
    Some(Arc::new(ContextGuidance {
        inherited,
        guidance,
    }))
}
