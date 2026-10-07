use super::*;

impl ProductionCapsuleAllocator {
    pub(in crate::native_v2_hosting) fn with_test_filesystem_and_source(
        mut self,
        source: PathBuf,
        prepare: FilesystemPreparer,
    ) -> Self {
        self.source_override = Some(source);
        self.prepare_filesystem = prepare;
        self
    }

    pub(in crate::native_v2_hosting) fn run_path(&self, run_id: &RunId) -> PathBuf {
        run_directory(&self.config.storage_root, run_id)
    }

    pub(in crate::native_v2_hosting) fn recovery_delivery_run_id(
        &self,
        run_id: &RunId,
    ) -> Option<RunId> {
        read_recovery(&recovery_path(&self.config.storage_root, run_id))?.delivery_run_id
    }

    pub(in crate::native_v2_hosting) fn set_test_filesystem(
        &mut self,
        prepare: FilesystemPreparer,
    ) {
        self.prepare_filesystem = prepare;
    }

    pub(in crate::native_v2_hosting) fn write_test_recovery(
        &self,
        run_id: &RunId,
        state: (bool, Option<RunId>, Option<RunId>),
    ) {
        let (recoverable, resumed_from, successor_run_id) = state;
        write_recovery(
            &recovery_path(&self.config.storage_root, run_id),
            &HostedRecoveryDocument {
                recoverable,
                run_id: Some(run_id.clone()),
                delivery_run_id: Some(run_id.clone()),
                resumed_from,
                successor_run_id,
            },
        )
        .expect("test recovery metadata should be writable");
    }

    pub(in crate::native_v2_hosting) fn reconcile_test_recovery(&self) {
        reconcile_retained_allocations(&self.config.storage_root)
            .expect("test recovery metadata should reconcile");
    }
}
