use std::ops::Range;

use super::GitHubAuthorityError;

const GENERATED_BODY_START: &str = "<!-- zeroshot-delivery:generated:v1:start -->";
const GENERATED_BODY_END: &str = "<!-- zeroshot-delivery:generated:v1:end -->";

pub(in crate::native_v2_delivery) fn valid_generated_description(description: &str) -> bool {
    !description.contains(GENERATED_BODY_START) && !description.contains(GENERATED_BODY_END)
}

pub(super) fn generated_body(description: &str) -> Result<String, GitHubAuthorityError> {
    valid_generated_description(description)
        .then(|| format!("{GENERATED_BODY_START}\n{description}\n{GENERATED_BODY_END}"))
        .ok_or(GitHubAuthorityError::Rejected)
}

pub(super) fn refresh_generated_body(
    current: Option<&str>,
    description: &str,
) -> Result<String, GitHubAuthorityError> {
    let generated = generated_body(description)?;
    let Some(current) = current.filter(|body| !body.is_empty()) else {
        return Ok(generated);
    };
    let Some(range) = generated_body_range(current)? else {
        return Ok(format!("{current}\n\n{generated}"));
    };
    let mut refreshed = String::with_capacity(current.len() - range.len() + generated.len());
    refreshed.push_str(&current[..range.start]);
    refreshed.push_str(&generated);
    refreshed.push_str(&current[range.end..]);
    Ok(refreshed)
}

pub(super) fn generated_body_range(
    body: &str,
) -> Result<Option<Range<usize>>, GitHubAuthorityError> {
    let starts = body.match_indices(GENERATED_BODY_START).collect::<Vec<_>>();
    let ends = body.match_indices(GENERATED_BODY_END).collect::<Vec<_>>();
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([(start, _)], [(end, _)]) if start < end => {
            Ok(Some(*start..end + GENERATED_BODY_END.len()))
        }
        _ => Err(GitHubAuthorityError::Rejected),
    }
}

#[cfg(test)]
#[path = "metadata/tests.rs"]
mod tests;
