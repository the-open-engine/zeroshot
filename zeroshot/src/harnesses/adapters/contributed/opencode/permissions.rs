//! Preserve native local policy when safe inspection is unavailable.

use crate::execution::process::ProcessSessionCommand;

use super::OpenCodeAdapter;

impl OpenCodeAdapter {
    pub(super) fn apply_permission_default(&self, command: &mut ProcessSessionCommand) {
        // Native debug config starts background npm installs and can leave a dead-owner lock
        // when it exits. Local policy therefore uses the shared unavailable fallback: keep
        // native controls and omit automatic approval. Disposable contained workers use auto.
        if self.runners.is_hosted() {
            command.argv.push("--auto".to_owned());
        }
    }
}
