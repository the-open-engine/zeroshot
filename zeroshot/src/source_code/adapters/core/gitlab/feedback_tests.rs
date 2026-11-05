use super::*;

fn discussions() -> Value {
    let notes = (31..=35)
        .map(|author| {
            let mut note = note(author, "Please check this case.");
            note["author"]["id"] = json!(author);
            note["position"] = json!({"new_path":"src/main.rs","new_line":3});
            note
        })
        .collect::<Vec<_>>();
    json!([
        {"id":"root","notes":notes},
        {"id":"replies","notes":notes},
    ])
}

fn member_calls(fixture: &Fixture, id: u64) -> usize {
    fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(method, path, _)| {
            method == "GET"
                && path == &format!("/api/v4/projects/group%2Fsub%2Fproject/members/all/{id}")
        })
        .count()
}

#[tokio::test]
async fn only_active_developers_including_inherited_members_can_supply_feedback() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.response("GET", "/discussions", (StatusCode::OK, discussions()));
    // Effective inherited membership is available from members/all, not direct members.
    for (id, level, state) in [
        (31, 30, "active"),
        (32, 20, "active"),
        (34, 40, "blocked"),
        (35, 40, "active"),
    ] {
        fixture.response(
            "GET",
            &format!("/members/all/{id}"),
            (
                StatusCode::OK,
                json!({
                    "id":id,"state":state,"access_level":level,
                    "created_by":{"id":99},"membership_state":"active"
                }),
            ),
        );
    }
    fixture.response(
        "GET",
        "/members/all/33",
        (StatusCode::NOT_FOUND, Value::Null),
    );
    let first = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 4);
    assert!(
        first
            .items
            .iter()
            .all(|item| matches!(item.author.as_str(), "31" | "35"))
    );
    for id in 31..=35 {
        assert_eq!(member_calls(&fixture, id), 1);
    }

    fixture.response(
        "GET",
        "/members/all/31",
        (
            StatusCode::OK,
            json!({"id":31,"state":"active","membership_state":"active","access_level":20}),
        ),
    );
    let second = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 2);
    assert!(second.items.iter().all(|item| item.author == "35"));
    for id in 31..=35 {
        assert_eq!(member_calls(&fixture, id), 2);
    }
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(method, _, _)| method == "GET")
    );
}

#[tokio::test]
async fn incomplete_or_mismatched_membership_fails_the_entire_inspection() {
    for enterprise in [true, false] {
        check_incomplete_membership(enterprise).await;
    }
}

async fn check_incomplete_membership(enterprise: bool) {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.response(
        "GET",
        "/version",
        (
            StatusCode::OK,
            json!({"version":"19.5.1","enterprise":enterprise}),
        ),
    );
    let mut trusted = note(1, "Trusted feedback must not mask an incomplete lookup.");
    trusted["author"]["id"] = json!(35);
    fixture.response(
        "GET",
        "/members/all/35",
        (
            StatusCode::OK,
            json!({"id":35,"state":"active","membership_state":"active","access_level":30}),
        ),
    );
    fixture.response(
        "GET",
        "/discussions",
        (
            StatusCode::OK,
            json!([{"id":"discussion","notes":[trusted,note(2,"Unknown permission")]}]),
        ),
    );
    for value in [
        json!({"id":31,"state":"active","membership_state":"active"}),
        json!({"id":31,"membership_state":"active","access_level":30}),
        json!({"state":"active","membership_state":"active","access_level":30}),
        json!({"id":31,"state":"active","membership_state":"active","access_level":"30"}),
        json!({"id":31,"state":"active","membership_state":"active","access_level":null}),
        json!({"id":31,"state":"unknown","membership_state":"active","access_level":30}),
        json!({"id":99,"state":"active","membership_state":"active","access_level":30}),
        json!({"id":31,"state":"active","membership_state":"unknown","access_level":30}),
        json!({"id":31,"state":"active","membership_state":"unreported","access_level":30}),
        json!({"id":31,"state":"active","membership_state":null,"access_level":30}),
        json!({"id":31,"state":"active","membership_state":true,"access_level":30}),
        json!({"id":31,"state":"active","membership_state":1,"access_level":30}),
    ] {
        fixture.response("GET", "/members/all/31", (StatusCode::OK, value.clone()));
        assert!(
            fixture
                .authority
                .inspect_review_feedback(&receipt(), credential())
                .await
                .is_err(),
            "accepted {value}"
        );
    }
    for (status, retryable) in [
        (StatusCode::UNAUTHORIZED, false),
        (StatusCode::FORBIDDEN, false),
        (StatusCode::TOO_MANY_REQUESTS, true),
        (StatusCode::SERVICE_UNAVAILABLE, true),
    ] {
        fixture.response("GET", "/members/all/31", (status, Value::Null));
        let error = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential())
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), Some(status.as_u16()));
        assert_eq!(error.retryable_operation(), retryable);
    }
    fixture.state.lock().unwrap().disconnect_membership = true;
    let error = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap_err();
    assert_eq!(error.api_status(), None);
    assert!(error.retryable_operation());
}

#[tokio::test]
async fn community_edition_membership_accepts_writers_and_ignores_read_only_authors() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.response(
        "GET",
        "/version",
        (
            StatusCode::OK,
            json!({"version":"19.5.1","enterprise":false}),
        ),
    );
    fixture.response("GET", "/discussions", (StatusCode::OK, discussions()));
    for (id, level, state) in [
        (31, 30, "active"),
        (32, 20, "active"),
        (34, 40, "blocked"),
        (35, 40, "active"),
    ] {
        // The CE serializer omits membership_state, including for inherited members.
        fixture.response(
            "GET",
            &format!("/members/all/{id}"),
            (
                StatusCode::OK,
                json!({"id":id,"state":state,"access_level":level}),
            ),
        );
    }
    fixture.response(
        "GET",
        "/members/all/33",
        (StatusCode::NOT_FOUND, Value::Null),
    );
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 4);
    assert!(
        feedback
            .items
            .iter()
            .all(|item| matches!(item.author.as_str(), "31" | "35"))
    );
    for id in 31..=35 {
        assert_eq!(member_calls(&fixture, id), 1);
    }
    fixture.response(
        "GET",
        "/members/all/31",
        (
            StatusCode::OK,
            json!({"id":31,"state":"active","access_level":20}),
        ),
    );
    let revoked = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(revoked.items.len(), 2);
    assert!(revoked.items.iter().all(|item| item.author == "35"));
    for id in 31..=35 {
        assert_eq!(member_calls(&fixture, id), 2);
    }
}

#[tokio::test]
async fn unreported_membership_requires_authoritative_community_edition_metadata() {
    for metadata in [
        json!({"version":"19.5.1-ee","enterprise":true}),
        json!({"version":"19.5.1"}),
        json!({"version":"19.5.1","enterprise":null}),
        json!({"version":"19.5.1","enterprise":"false"}),
        json!({"version":"19.5.1","enterprise":0}),
        json!({"enterprise":false}),
    ] {
        let fixture = Fixture::new().await;
        fixture.open_review();
        fixture.response("GET", "/version", (StatusCode::OK, metadata.clone()));
        fixture.response(
            "GET",
            "/members/all/31",
            (
                StatusCode::OK,
                json!({"id":31,"state":"active","access_level":30}),
            ),
        );
        let mut trusted = note(1, "Trusted feedback must not mask missing evidence.");
        trusted["author"]["id"] = json!(35);
        fixture.response(
            "GET",
            "/members/all/35",
            (
                StatusCode::OK,
                json!({"id":35,"state":"active","membership_state":"active","access_level":30}),
            ),
        );
        fixture.response(
            "GET",
            "/discussions",
            (
                StatusCode::OK,
                json!([{"id":"discussion","notes":[trusted,note(2,"Unknown membership")]}]),
            ),
        );
        assert!(
            fixture
                .authority
                .inspect_review_feedback(&receipt(), credential())
                .await
                .is_err(),
            "accepted unreported membership with {metadata}"
        );
    }
}

#[tokio::test]
async fn active_accounts_require_current_active_membership_for_every_note() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    // Two actionable notes from the same Developer, including a positioned discussion reply.
    let mut reply = note(2, "Please check this line.");
    reply["position"] = json!({"new_path":"src/main.rs","new_line":3});
    fixture.response(
        "GET",
        "/discussions",
        (
            StatusCode::OK,
            json!([
                {"id":"discussion","notes":[note(1, "Please check this case."), reply]}
            ]),
        ),
    );
    fixture.response(
        "GET",
        "/members/all/31",
        (
            StatusCode::OK,
            json!({
                "id":31,"state":"active","membership_state":"active","access_level":30
            }),
        ),
    );
    let active = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(active.items.len(), 2);
    assert_eq!(member_calls(&fixture, 31), 1);

    // Suspension changes membership alone: the account and its role remain active/Developer.
    fixture.response(
        "GET",
        "/members/all/31",
        (
            StatusCode::OK,
            json!({
                "id":31,"state":"active","membership_state":"awaiting","access_level":30
            }),
        ),
    );
    let awaiting = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert!(awaiting.items.is_empty());
    assert_eq!(member_calls(&fixture, 31), 2);

    let mut edited = note(1, "Edited feedback while awaiting membership.");
    edited["updated_at"] = json!("2026-11-05T12:00:00Z");
    fixture.response(
        "GET",
        "/discussions",
        (
            StatusCode::OK,
            json!([
                {"id":"discussion","notes":[edited]}
            ]),
        ),
    );
    assert!(
        fixture
            .authority
            .inspect_review_feedback(&receipt(), credential())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(member_calls(&fixture, 31), 3);

    fixture.response(
        "GET",
        "/members/all/31",
        (
            StatusCode::OK,
            json!({
                "id":31,"state":"active","membership_state":"active","access_level":30
            }),
        ),
    );
    let restored = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(restored.items.len(), 1);
    assert_eq!(restored.items[0].key, active.items[0].key);
    assert_ne!(restored.items[0].version, active.items[0].version);
    assert_eq!(member_calls(&fixture, 31), 4);
}

#[tokio::test]
async fn denied_authors_still_require_the_exact_project_and_merge_request_scope() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.response(
        "GET",
        "/members/all/31",
        (StatusCode::NOT_FOUND, Value::Null),
    );
    for (field, value) in [
        ("project_id", 22),
        ("noteable_id", 999),
        ("noteable_iid", 18),
    ] {
        let mut current = note(1, "Wrong scope");
        current[field] = json!(value);
        fixture.state.lock().unwrap().discussion_note = current;
        assert!(
            fixture
                .authority
                .inspect_review_feedback(&receipt(), credential())
                .await
                .is_err()
        );
    }
    assert_eq!(member_calls(&fixture, 31), 0);
}
