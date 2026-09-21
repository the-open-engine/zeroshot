use super::NodeResponseContract;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VerifierWorkspace {
    Isolated,
    Shared,
}

pub(super) fn runtime_guidance(
    response: &NodeResponseContract,
    workspace: VerifierWorkspace,
) -> &'static str {
    match (response, workspace) {
        (NodeResponseContract::Verifier { .. }, VerifierWorkspace::Isolated) => {
            "Runtime-owned verifier guidance:\n\
             Verify in this isolated checkout. Do not modify reviewed material or implement repairs. \
             Run checks and create artifacts. Follow repository setup and install manifest/lockfile \
             dependencies in this checkout, then wait for setup to finish and check exit status. A \
             missing declared dependency is not an unavailable check: run setup and retry; reject \
             unless an external blocker remains.\n"
        }
        (NodeResponseContract::Verifier { .. }, VerifierWorkspace::Shared) => {
            "Runtime-owned verifier guidance:\n\
             Verify the prepared shared checkout. Do not modify reviewed material or implement repairs. \
             Run checks and create artifacts. Do not run setup or dependency-install commands that \
             rewrite the checkout; local verifiers may run concurrently. A missing declared dependency \
             is a setup failure, not an unavailable check: reject with evidence unless an external \
             blocker remains.\n"
        }
        (NodeResponseContract::Worker { .. }, _) => {
            "Runtime-owned workspace setup guidance:\n\
             Before returning, follow repository setup and install manifest/lockfile dependencies in \
             the checkout; do not use an ad hoc unpinned list. Wait for setup to finish and check exit \
             status. Leave ignored dependencies there. Put standalone tools under an executable user \
             path such as `$HOME/.local`, not `/tmp` (possibly `noexec`).\n"
        }
    }
}
