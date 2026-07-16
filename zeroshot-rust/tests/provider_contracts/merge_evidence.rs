use super::*;

fn mismatched_receipt(
    request: &SourceOperationRequest,
    receipt_base: SourceRevisionId,
    receipt_head: SourceRevisionId,
) -> SourceOperationReceipt {
    SourceOperationReceipt::Merge(
        SourceMergeReceipt::new(
            request.repository().clone(),
            (
                request.operation_id().clone(),
                request.fingerprint().clone(),
            ),
            (receipt_base, receipt_head),
            (SourceRevisionId::new("integrated-sha").unwrap(), Vec::new()),
        )
        .unwrap(),
    )
}

async fn assert_mismatch_rejected(
    reference: &SourceProviderRef,
    request: &SourceOperationRequest,
    field: &str,
    mismatched: SourceOperationReceipt,
) {
    let inspected = Arc::new(FakeSourceProvider::new(
        source_descriptor(reference.clone(), [SourceCapability::Merge], []),
        SourceOperationInspection::Applied(Box::new(mismatched.clone())),
    ));
    let mut inspection_registry = SourceCodeProviderRegistry::new();
    inspection_registry.register(inspected.clone()).unwrap();
    assert!(
        matches!(
            inspection_registry.operate(request).await,
            Err(SourceCallError::InvalidEvidence { .. })
        ),
        "applied inspection with mismatched {field} was accepted"
    );
    assert_eq!(inspected.operation_calls.load(Ordering::SeqCst), 0);

    let invoked = Arc::new(FakeSourceProvider::new(
        source_descriptor(reference.clone(), [SourceCapability::Merge], []),
        SourceOperationInspection::Unobserved,
    ));
    invoked.set_operation_result(mismatched);
    let mut invocation_registry = SourceCodeProviderRegistry::new();
    invocation_registry.register(invoked.clone()).unwrap();
    assert!(
        matches!(
            invocation_registry.operate(request).await,
            Err(SourceCallError::InvalidEvidence { .. })
        ),
        "invocation result with mismatched {field} was accepted"
    );
    assert_eq!(invoked.operation_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn merge_evidence_must_match_requested_base_and_head() {
    let reference = source_ref("source.github", 1);
    let request = source_operation(canonical_repository(reference.clone()));
    let SourceOperation::Merge {
        expected_base,
        expected_head,
    } = request.operation()
    else {
        panic!("test request must be a merge")
    };

    for (field, receipt_base, receipt_head) in [
        (
            "base",
            SourceRevisionId::new("different-base").unwrap(),
            expected_head.clone(),
        ),
        (
            "head",
            expected_base.clone(),
            SourceRevisionId::new("different-head").unwrap(),
        ),
    ] {
        assert_mismatch_rejected(
            &reference,
            &request,
            field,
            mismatched_receipt(&request, receipt_base, receipt_head),
        )
        .await;
    }
}
