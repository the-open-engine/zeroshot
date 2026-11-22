//! Retries belong to these read-only policy operations, independently of CI/queue polling.
use super::*;
use tokio::time::{Instant, sleep, timeout_at};

impl GhCliDeliveryAuthority {
    pub(in crate::source_code_provider::adapters::core::github) async fn policy_api(
        &self,
        arguments: &[String],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        self.ensure_instance(credential).await?;
        let deadline = Instant::now() + self.config.api_deadline;
        let mut attempt = 1;
        let mut previous = None;
        loop {
            let result = timeout_at(deadline, async {
                if attempt > 1 {
                    sleep(Duration::from_secs(attempt - 1)).await;
                }
                self.request_api_output(arguments, credential, false).await
            })
            .await;
            match result {
                Ok(Ok(output)) => return self.decode_api_output(output, credential),
                Ok(Err(failure)) if !failure.policy_read_transient => return Err(failure.error),
                Ok(Err(failure)) if attempt == 3 => {
                    return Err(self.policy_read_exhausted(
                        failure.error,
                        "after 3 attempts",
                        credential,
                    ));
                }
                Ok(Err(failure)) => previous = Some(failure.error),
                Err(_) => {
                    return Err(match previous {
                        Some(error) => {
                            self.policy_read_exhausted(error, "at the API deadline", credential)
                        }
                        None => redacted_api_error(
                            None,
                            self.network
                                .redact("GitHub policy read timed out at the API deadline"),
                            credential,
                        )
                        .temporary(),
                    });
                }
            }
            attempt += 1;
        }
    }
    pub(in crate::source_code_provider::adapters::core::github) fn policy_read_exhausted(
        &self,
        error: SourceDeliveryError,
        reason: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> SourceDeliveryError {
        // The recognized response has no HTTP status. Do not mark exhaustion temporary:
        // the default outer delivery poll intentionally has no attempt limit.
        redacted_api_error(
            error.api_status(),
            self.network.redact(&format!(
                "GitHub policy read retry allowance exhausted {reason}.\n{error}"
            )),
            credential,
        )
        .exhausted()
    }
}

pub(super) fn transient(output: &ApiOutput) -> bool {
    if output.stdout.len() > MAX_API_OUTPUT_BYTES || output.stderr.len() > MAX_API_ERROR_BYTES {
        return false;
    }
    let Ok(Value::Array(pages)) = serde_json::from_slice(&output.stdout) else {
        return false;
    };
    let Some((last, earlier)) = pages.split_last() else {
        return false;
    };
    // gh stops pagination at an error. Never combine an earlier attempt's successful
    // pages with a retry, or admit data plus errors as a complete policy snapshot.
    if !earlier.iter().all(|page| {
        page.as_object()
            .is_some_and(|page| page.len() == 1 && page.get("data").is_some_and(Value::is_object))
    }) {
        return false;
    }
    let Some(errors) = last
        .as_object()
        .filter(|page| page.len() == 1)
        .and_then(|page| page.get("errors"))
        .and_then(Value::as_array)
        .filter(|errors| !errors.is_empty())
    else {
        return false;
    };
    let Ok(stderr) = std::str::from_utf8(&output.stderr) else {
        return false;
    };
    let mut lines = stderr.lines();
    errors.iter().all(|error| {
        error
            .as_object()
            .filter(|error| error.len() == 1)
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .is_some_and(|message| {
                execution_error(message)
                    && lines.next().and_then(|line| line.strip_prefix("gh: ")) == Some(message)
            })
    }) && lines.next().is_none()
}

fn execution_error(message: &str) -> bool {
    let Some(detail) = message
        .strip_prefix("Something went wrong while executing your query on ")
        .and_then(|text| text.strip_suffix("` when reporting this issue."))
    else {
        return false;
    };
    let Some((timestamp, request_id)) = detail.split_once(". Please include `") else {
        return false;
    };
    [timestamp, request_id].into_iter().all(|value| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b'`')
    })
}
