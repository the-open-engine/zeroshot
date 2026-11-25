//! A public subscription drains one part at a time. Its cursor identifies the
//! origin ledger, so reconnecting at the pause cannot replay the answer.
use super::*;
use super::lineage::{part_cursor, LineageError, RunLineage, StreamSelection};
use openengine_cluster_protocol::WorkspaceRecovery;

#[derive(Clone)]
pub(super) struct RunFollow {
    pub(super) root: RunId,
    observer: NativeV2Observability,
    through_parts: bool,
    pub(super) qualify: bool,
    wait_at_suspension: bool,
}

fn lineage_error(error: LineageError<NativeV2ObservationError>) -> NativeV2ObservationError {
    match error {
        LineageError::Read(error) => error,
        LineageError::Cycle => NativeV2ObservationError::InvalidState,
    }
}

impl NativeV2Observability {
    pub(crate) async fn recovery_for_part(
        &self,
        part: RunId,
    ) -> Result<Option<WorkspaceRecovery>, NativeV2ObservationError> {
        let Some(host) = &self.recovery else {
            return Ok(None);
        };
        if self.runtime.failure(&part).is_none() && self.ledger.get(&part).await?.is_none() {
            return Ok(None);
        }
        Ok(Some(host.committed_recovery(&part).await))
    }

    async fn stream_start(
        &self,
        id: &RunId,
        selected: StreamSelection<'_>,
        cursor: Option<Cursor>,
    ) -> Result<(RunId, Option<Cursor>, Option<RunFollow>), NativeV2ObservationError> {
        if self.recovery.is_none() {
            return Ok((id.clone(), cursor, None));
        }
        let chain = RunLineage::resolve(id, |part| self.recovery_for_part(part))
            .await
            .map_err(lineage_error)?;
        let (part, local) = chain.stream_position(id, selected, cursor)?;
        let follow = RunFollow {
            root: chain.root().clone(),
            observer: self.clone(),
            qualify: !matches!(selected, StreamSelection::Part(_)),
            through_parts: matches!(selected, StreamSelection::All),
            wait_at_suspension: matches!(selected, StreamSelection::All),
        };
        Ok((part, local, Some(follow)))
    }

    pub async fn watch(
        &self,
        mut params: RunWatchParams,
    ) -> Result<(RunWatchResult, RunWatchSubscription), NativeV2ObservationError> {
        let (part, cursor, follow) = self
            .stream_start(
                &params.run_id,
                StreamSelection::new(params.part_id.as_ref(), false),
                params.from_cursor.take(),
            )
            .await?;
        params.run_id = part.clone();
        params.from_cursor = cursor;
        let previous_usage = if follow
            .as_ref()
            .is_some_and(|scope| scope.through_parts && scope.root != part)
        {
            self.usage_before(&part).await?
        } else {
            None
        };
        let (mut receipt, mut stream) = self.watch_part(params, previous_usage).await?;
        if let Some(scope) = &follow {
            receipt.run_id = scope.root.clone();
            if scope.qualify {
                receipt.at_cursor = part_cursor(&part, &receipt.at_cursor);
            }
        }
        stream.follow = follow;
        Ok((receipt, stream))
    }

    async fn usage_before(
        &self,
        part: &RunId,
    ) -> Result<Option<openengine_cluster_protocol::TokenUsage>, NativeV2ObservationError> {
        let mut previous_usage = None;
        let family = RunLineage::resolve(part, |id| self.recovery_for_part(id))
            .await
            .map_err(lineage_error)?;
        for id in family.parts().iter().take_while(|id| *id != part) {
            let previous = self
                .ledger
                .get(id)
                .await?
                .ok_or(NativeV2ObservationError::RunNotFound)?;
            crate::v2_run_ledger::merge_usage(
                &mut previous_usage,
                previous.snapshot.token_usage.as_ref(),
            )?;
        }
        Ok(previous_usage)
    }

    pub async fn logs(
        &self,
        mut params: RunLogsParams,
    ) -> Result<(RunLogsResult, RunLogsSubscription), NativeV2ObservationError> {
        let (part, cursor, mut follow) = self
            .stream_start(
                &params.run_id,
                StreamSelection::new(params.part_id.as_ref(), params.execution.is_some()),
                params.from_cursor.take(),
            )
            .await?;
        params.run_id = part.clone();
        params.from_cursor = cursor;
        let (mut receipt, mut stream) = self.logs_part(params).await?;
        if let Some(scope) = &mut follow {
            scope.wait_at_suspension = false;
        }
        if let Some(scope) = &follow {
            receipt.run_id = scope.root.clone();
            if scope.qualify {
                receipt.at_cursor = part_cursor(&part, &receipt.at_cursor);
            }
        }
        stream.follow = follow;
        Ok((receipt, stream))
    }
}

impl RunFollow {
    /// A current suspension may be continued days later. Terminal failure ends
    /// a live watch, but an already accepted retry is part of retained replay.
    pub(super) async fn advance(
        &self,
        part: &RunId,
    ) -> Result<Option<RunId>, NativeV2ObservationError> {
        if !self.through_parts {
            return Ok(None);
        }
        let Some(host) = &self.observer.recovery else {
            return Ok(None);
        };
        let Some(next) = host.committed_recovery(part).await.successor_run_id else {
            return Ok(None);
        };
        if next == *part {
            return Err(NativeV2ObservationError::InvalidState);
        }
        let other = self.observer.recovery_for_part(next.clone()).await?;
        Ok(other
            .is_some_and(|record| record.resumed_from.as_ref() == Some(part))
            .then_some(next))
    }
    pub(super) fn follows_suspensions(&self) -> bool {
        self.wait_at_suspension
    }
}

impl RunWatchSubscription {
    pub(super) async fn enter_part(&mut self, part: RunId) -> Result<(), NativeV2ObservationError> {
        let projection = self
            .ledger
            .get(&part)
            .await?
            .ok_or(NativeV2ObservationError::RunNotFound)?
            .snapshot
            .replay_seed();
        crate::v2_run_ledger::merge_usage(
            &mut self.previous_usage,
            self.projection.token_usage.as_ref(),
        )?;
        self.after = 0;
        self.scanned_through = initial_cursor();
        self.run_id = part;
        self.projection = projection;
        Ok(())
    }
}

impl RunLogsSubscription {
    pub(super) async fn enter_part(&mut self, part: RunId) -> Result<(), NativeV2ObservationError> {
        self.scanned_through = initial_cursor();
        self.run_id = part;
        Ok(())
    }
}
