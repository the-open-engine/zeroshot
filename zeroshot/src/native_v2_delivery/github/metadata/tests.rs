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
