//! Rendering and replacement of the delivery-owned section of a forge review.

use crate::source_code_provider::{
    DELIVERY_GENERATED_BODY_END, DELIVERY_GENERATED_BODY_START, SourceDeliveryError,
    SourceReviewRequest,
};

/// The delivery-owned section of a review body: the description and the issue it closes.
pub(super) fn generated_description(request: &SourceReviewRequest) -> String {
    let issue = request
        .source_issue
        .as_ref()
        .map_or(String::new(), |issue| {
            format!("\n\nCloses #{}", issue.number)
        });
    format!(
        "{DELIVERY_GENERATED_BODY_START}\n{}{issue}\n{DELIVERY_GENERATED_BODY_END}",
        request.description
    )
}

/// Replaces the one delivery-owned section of `existing`, or appends it when there is none.
/// `review` names the forge's review in the error, for example `GitLab review`.
pub(super) fn refresh_description(
    existing: &str,
    replacement: &str,
    review: &str,
) -> Result<String, SourceDeliveryError> {
    if !existing.contains(DELIVERY_GENERATED_BODY_START)
        && !existing.contains(DELIVERY_GENERATED_BODY_END)
    {
        return Ok(if existing.is_empty() {
            replacement.to_owned()
        } else {
            format!("{existing}\n\n{replacement}")
        });
    }
    let Some(start) = existing.find(DELIVERY_GENERATED_BODY_START) else {
        return Err(SourceDeliveryError::identity(format!(
            "{review} lacks its generated metadata section"
        )));
    };
    let Some(end) = existing.find(DELIVERY_GENERATED_BODY_END) else {
        return Err(SourceDeliveryError::Rejected);
    };
    if start >= end
        || existing.matches(DELIVERY_GENERATED_BODY_START).count() != 1
        || existing.matches(DELIVERY_GENERATED_BODY_END).count() != 1
    {
        return Err(SourceDeliveryError::Rejected);
    }
    let suffix = end + DELIVERY_GENERATED_BODY_END.len();
    Ok(format!(
        "{}{}{}",
        &existing[..start],
        replacement,
        &existing[suffix..]
    ))
}
