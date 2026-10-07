use super::NodeResponseContract;

pub(super) fn runtime_guidance(response: &NodeResponseContract) -> &'static str {
    match response {
        NodeResponseContract::Verifier { .. } => {
            "Runtime-owned verifier guidance:\n\
             Verify the prepared shared checkout. Do not modify reviewed material or implement repairs. \
             Run checks and create artifacts. Do not run setup or dependency-install commands that \
             rewrite the checkout; other nodes may run concurrently. A missing declared dependency \
             is an environment blocker: report the missing dependency and command evidence; do not \
             request unrelated code repairs.\n"
        }
        NodeResponseContract::Worker { .. } => {
            "Runtime-owned workspace setup guidance:\n\
             Before returning, follow repository setup and install manifest/lockfile dependencies in \
             the checkout; do not use an ad hoc unpinned list. Wait for setup to finish and check exit \
             status. Leave ignored dependencies there. Put standalone tools under an executable user \
             path: use `$ZEROSHOT_TOOLS/bin` when provided (shared by all nodes), otherwise \
             `$HOME/.local/bin`. Do not install tools in `/tmp` (possibly `noexec`).\n"
        }
    }
}
