use super::*;

struct ConversationCheckpoint(DriverControl);

#[async_trait]
impl SourceReviewCheckpoint for ConversationCheckpoint {
    async fn retain(&self, state: Value) -> Result<(), SourceDeliveryError> {
        self.0.checkpoint_delivery_state(state).await.map_err(|_| {
            SourceDeliveryError::api(
                None,
                "Trusted delivery could not durably checkpoint conversation authority",
            )
        })
    }
}

impl NativeV2DeliveryAdapter {
    pub(super) async fn conversation_feedback_step(
        &self,
        drive: &mut ReviewDrive<'_, '_>,
    ) -> Result<Option<ReviewStep>, DeliveryStop> {
        let mut feedback = self.read_review_feedback(drive).await?;
        if drive.pull_request_feedback == PullRequestFeedback::Ignore {
            return self.conversation_blocker(drive, &feedback).map(|()| None);
        }
        self.require_authorized_conversations(drive, &feedback)?;
        let checkpoint = ConversationCheckpoint(drive.control.clone());
        let responses = delivery_input(drive.manifest)?.review_responses;
        for response in responses {
            // Replaying a verified response must also inspect a resolved thread: the
            // authority may have been reconstructed after an interrupted confirmation.
            let mut retry = preflight::OperationRetry::default();
            loop {
                ensure_active(drive.control)?;
                match self
                    .authority
                    .resolve_review_conversation(
                        SourceReviewResolutionRequest {
                            review: &drive.review,
                            response: &response,
                            checkpoint: &checkpoint,
                        },
                        drive.credentials.current(),
                    )
                    .await
                {
                    Ok(SourceReviewResolution::Resolved | SourceReviewResolution::Changed) => break,
                    Err(error) => {
                        self.retry_operation(
                            error,
                            drive
                                .credentials
                                .operation_context(drive.control, &mut retry),
                        )
                        .await?
                    }
                }
                // The authority retains the exact pending reply/resolution across errors.
                // Absence from the unresolved list cannot settle an unconfirmed effect.
            }
            feedback = self.read_review_feedback(drive).await?;
            self.require_authorized_conversations(drive, &feedback)?;
        }
        let blockers = feedback.conversations.clone();
        if let Some(step) = self.complete_feedback_repair(drive, feedback).await? {
            return Ok(Some(step));
        }
        self.conversation_blocker(
            drive,
            &SourceReviewFeedback {
                items: Vec::new(),
                conversations: blockers,
            },
        )?;
        Ok(None)
    }

    fn conversation_blocker(
        &self,
        drive: &ReviewDrive<'_, '_>,
        feedback: &SourceReviewFeedback,
    ) -> Result<(), DeliveryStop> {
        let Some(thread) = feedback
            .conversations
            .iter()
            .find(|thread| !thread.authorized)
            .or_else(|| feedback.conversations.first())
        else {
            return Ok(());
        };
        let reason = if thread.authorized {
            "requires a verified response for its current revision; previously forwarded feedback is not resolution"
        } else {
            "contains feedback without repository write authority; automated repair and resolution are not authorized. A repository maintainer must resolve it"
        };
        Err(DeliveryStop::failure(
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
            self.review_context(&format!(
                "Unresolved conversation {} on {}#{} {reason}",
                thread.id, drive.review.repository, drive.review.review_id
            )),
        ))
    }

    fn require_authorized_conversations(
        &self,
        drive: &ReviewDrive<'_, '_>,
        feedback: &SourceReviewFeedback,
    ) -> Result<(), DeliveryStop> {
        if feedback
            .conversations
            .iter()
            .any(|thread| !thread.authorized)
        {
            self.conversation_blocker(drive, feedback)?;
        }
        Ok(())
    }
}
