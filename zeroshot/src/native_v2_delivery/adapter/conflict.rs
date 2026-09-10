use super::*;

impl NativeV2DeliveryAdapter {
    pub(super) async fn complete_conflict(
        &self,
        drive: &mut ReviewDrive<'_>,
        authority_diagnostic: &str,
    ) -> Result<ReviewStep, DeliveryStop> {
        emit(drive.control, "delivery: materializing merge conflict").await?;
        let request = GitHubConflictRequest {
            workspace: self.config.workspace.clone(),
            review: drive.review.clone(),
        };
        let outcome = self
            .materialize_conflict(&request, &mut drive.credentials, drive.control)
            .await?;
        let GitHubConflictOutcome::Materialized(materialization) = outcome else {
            emit(
                drive.control,
                "delivery: conflict observation changed; rechecking GitHub",
            )
            .await?;
            return Ok(ReviewStep::Continue);
        };
        let diagnostic = conflict_diagnostic(authority_diagnostic, &materialization)
            .ok_or_else(|| DeliveryStop::Outcome(WorkerOutcome::malformed()))?;
        review_completion(drive, DELIVERY_CONFLICT_LABEL, &diagnostic, None).await
    }

    async fn materialize_conflict(
        &self,
        request: &GitHubConflictRequest,
        credentials: &mut DeliveryCredentials<'_>,
        control: &DriverControl,
    ) -> Result<GitHubConflictOutcome, DeliveryStop> {
        match self
            .authority
            .materialize_merge_conflict(request, credentials.current())
            .await
        {
            Ok(materialization) => Ok(materialization),
            Err(error) if error.authentication_failed() && credentials.can_refresh() => {
                emit(control, "delivery: refreshing GitHub credential").await?;
                credentials.refresh().await?;
                self.authority
                    .materialize_merge_conflict(request, credentials.current())
                    .await
                    .map_err(|_| crash_outcome())
            }
            Err(_) => Err(crash_outcome()),
        }
    }
}
