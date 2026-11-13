use super::NodeResponseContract;

pub(super) fn runtime_guidance(response: &NodeResponseContract) -> &'static str {
    match response {
        NodeResponseContract::Verifier { .. } => {
            "Runtime-owned verifier guidance:\n\
             Verify the prepared shared checkout. Do not modify reviewed material or implement repairs. \
             Run checks and create artifacts. Do not run setup or dependency-install commands that \
             rewrite the checkout or shared dependencies; other nodes may run concurrently. Return \
             installable setup gaps or an incomplete candidate build as actionable rejected feedback \
             for the worker, with missing prerequisites and command evidence; do not request unrelated \
             code repairs. Reserve blocked for a demonstrated external limitation.\n"
        }
        NodeResponseContract::Worker { .. } => {
            "Runtime-owned workspace setup guidance:\n\
             Before returning, follow repository setup and install manifest/lockfile dependencies in \
             the checkout; do not use an ad hoc unpinned list. Wait for setup to finish and check exit \
             status. Leave ignored dependencies there. Homes are session-local. Prepare required \
             dependencies and candidate binaries in shared paths, and report the paths/settings fresh \
             reviewers need. Put standalone tools under an executable user \
             path: use `$ZEROSHOT_TOOLS/bin` when provided (shared by all nodes), otherwise \
             `$HOME/.local/bin`. Do not install tools in `/tmp` (possibly `noexec`).\n"
        }
    }
}
