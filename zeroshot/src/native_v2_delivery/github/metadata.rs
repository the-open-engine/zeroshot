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

fn generated_body_range(body: &str) -> Result<Option<Range<usize>>, GitHubAuthorityError> {
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
mod tests {
    use super::*;

    #[test]
    fn creates_and_replaces_generated_section() {
        let original = generated_body("Original description.").unwrap();
        assert_eq!(
            refresh_generated_body(Some(&original), "Updated description.").unwrap(),
            generated_body("Updated description.").unwrap()
        );
        assert_eq!(
            refresh_generated_body(None, "New description.").unwrap(),
            generated_body("New description.").unwrap()
        );
    }

    #[test]
    fn preserves_human_text_outside_generated_section() {
        let current = format!(
            "Human preface.\n\n{}\n\nHuman notes.",
            generated_body("Old description.").unwrap()
        );
        assert_eq!(
            refresh_generated_body(Some(&current), "New description.").unwrap(),
            format!(
                "Human preface.\n\n{}\n\nHuman notes.",
                generated_body("New description.").unwrap()
            )
        );
    }

    #[test]
    fn appends_section_to_unmanaged_body() {
        assert_eq!(
            refresh_generated_body(Some("Human-authored body."), "Generated description.").unwrap(),
            format!(
                "Human-authored body.\n\n{}",
                generated_body("Generated description.").unwrap()
            )
        );
    }

    #[test]
    fn rejects_missing_duplicate_or_misordered_markers() {
        for body in [
            GENERATED_BODY_START.to_owned(),
            GENERATED_BODY_END.to_owned(),
            format!("{GENERATED_BODY_START}\n{GENERATED_BODY_START}\n{GENERATED_BODY_END}"),
            format!("{GENERATED_BODY_START}\n{GENERATED_BODY_END}\n{GENERATED_BODY_END}"),
            format!("{GENERATED_BODY_END}\n{GENERATED_BODY_START}"),
        ] {
            assert!(refresh_generated_body(Some(&body), "Description.").is_err());
        }
    }

    #[test]
    fn rejects_reserved_markers_in_generated_descriptions() {
        for description in [
            format!("Text before {GENERATED_BODY_START}"),
            format!("{GENERATED_BODY_END} text after"),
        ] {
            assert!(generated_body(&description).is_err());
            assert!(refresh_generated_body(None, &description).is_err());
        }
    }
}
