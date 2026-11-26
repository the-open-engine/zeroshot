use super::*;

#[tokio::test]
async fn malformed_link_entries_cannot_end_a_rest_observation() {
    for link in [
        "",
        "</repos/acme/project/pulls?page=2>",
        "</repos/acme/project/pulls?page=2>; broken",
        "</repos/acme/project/pulls?page=2>; rel=\"nex",
        "</repos/acme/project/pulls?page=2>; rel=\"next\"; rel=\"prev\"",
        "</repos/acme/project/pulls?page=2>; rel=\"last\", broken",
        "</repos/acme/project/pulls?page=2>; rel=\"last\",",
    ] {
        let fixture =
            ApiFixture::serve(move |_, _| Json(json!([])).into_response().header("link", link))
                .await;
        let error = fixture
            .authority()
            .rest_pages(REST_PATH, credential(), false)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("pagination is incomplete"),
            "{link}"
        );
        assert_eq!(fixture.calls(), 1, "{link}");
    }
}

#[tokio::test]
async fn rest_pagination_reconstructs_the_request_from_github_page_links() {
    let fixture = ApiFixture::serve(|index, request| {
        assert_eq!(request.uri.path(), "/repos/acme/project/pulls");
        assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
        match index {
            0 => {
                assert_eq!(request.uri.query(), Some("per_page=100"));
                Json(json!([{"number":1}])).into_response().header(
                    "link",
                    concat!(
                        "</repositories/73/pulls?page=2&state=closed>; rel=\"next\",",
                        " </repositories/73/pulls?page=2>; rel=\"last\""
                    ),
                )
            }
            1 => {
                assert_eq!(request.uri.query(), Some("per_page=100&page=2"));
                Json(json!([{"number":2}]))
                    .into_response()
                    .header("link", "</repositories/73/pulls?page=1>; rel=\"first\"")
                    .header("link", "</repositories/73/pulls?page=1>; rel=prev")
            }
            _ => panic!("unexpected paginated request"),
        }
    })
    .await;
    let pages = fixture
        .authority()
        .rest_pages(REST_PATH, credential(), false)
        .await
        .unwrap();
    assert_eq!(pages, json!([[{"number":1}], [{"number":2}]]));
    assert_eq!(fixture.calls(), 2);
}

#[tokio::test]
async fn rest_pagination_rejects_nonadvancing_foreign_and_repeated_next_links() {
    for links in [
        vec!["</repositories/73/pulls?page=3>; rel=\"next\""],
        vec!["<https://foreign.invalid/pulls?page=2>; rel=\"next\""],
        vec!["</repositories/73/pulls?page=2&page=3>; rel=\"next\""],
        vec![
            "</repositories/73/pulls?page=2>; rel=\"next\"",
            "</repositories/73/pulls?page=2>; rel=\"next\"",
        ],
    ] {
        let fixture = ApiFixture::serve(move |_, _| {
            links
                .iter()
                .fold(Json(json!([])).into_response(), |reply, link| {
                    reply.header("link", link)
                })
        })
        .await;
        let error = fixture
            .authority()
            .rest_pages(REST_PATH, credential(), false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("pagination is incomplete"));
        assert_eq!(fixture.calls(), 1);
    }
}
