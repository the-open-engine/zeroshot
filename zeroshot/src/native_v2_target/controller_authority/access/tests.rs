use super::*;

#[test]
fn only_authentication_rejections_invalidate_access() {
    let empty = HeaderMap::new();
    assert!(rejects_access(StatusCode::UNAUTHORIZED, &empty));
    assert!(!rejects_access(StatusCode::FORBIDDEN, &empty));

    let mut invalid_token = HeaderMap::new();
    invalid_token.insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static(r#"Bearer realm="controller", error="invalid_token""#),
    );
    assert!(rejects_access(StatusCode::FORBIDDEN, &invalid_token));
    invalid_token.insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer error=invalid_token"),
    );
    assert!(rejects_access(StatusCode::FORBIDDEN, &invalid_token));

    let mut insufficient_scope = HeaderMap::new();
    insufficient_scope.insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static(r#"Bearer error="insufficient_scope""#),
    );
    assert!(!rejects_access(StatusCode::FORBIDDEN, &insufficient_scope));
}
