use super::NodeResponseContract;

pub(super) fn runtime_guidance(response: &NodeResponseContract) -> &'static str {
    match response {
        NodeResponseContract::Verifier { .. } => {
            "Runtime-owned verifier guidance:\n\
             Verify the prepared checkout. Do not modify reviewed material or implement repairs. \
             Run checks and create artifacts. Do not run setup or dependency-install commands that \
             rewrite the checkout; local verifiers may run concurrently. A missing declared dependency \
             is a setup failure, not an unavailable check: reject with evidence unless an external \
             blocker remains.\n"
        }
        NodeResponseContract::Worker { .. } => {
            "Runtime-owned workspace setup guidance:\n\
             Before returning, follow repository setup and install manifest/lockfile dependencies in \
             the checkout; do not use an ad hoc unpinned list. Wait for setup to finish and check exit \
             status. Leave ignored dependencies there. Put standalone tools under an executable user \
             path such as `$HOME/.local`, not `/tmp` (possibly `noexec`).\n"
        }
    }
}
