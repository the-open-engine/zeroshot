use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
fn suspension() -> RunSuspension {
    serde_json::from_value(json!({"reason":"request","node":"approval","createdAt":1000,
        "request":{"id":"request-1","text":"Review?","signals":{"decision":["approved","rejected"]},
        "output":{"kind":"record","fields":{"feedback":{"required":true,"type":{"kind":"string"}}}},"expiresAt":2000}})).assert_value()
}
fn answer(value: serde_json::Value) -> RunResumeFrom {
    serde_json::from_value(value).assert_value()
}
#[test]
fn deadline_precedes_shape_and_is_strict() {
    let valid = answer(
        json!({"kind":"answer","signals":{"decision":"approved"},"output":{"feedback":"ok"}}),
    );
    assert!(matches!(
        resolve(
            &suspension(),
            Some(&valid),
            UnixTimestampMillis::new(1999).assert_value()
        )
        .assert_value(),
        RequestResolution::Answer { .. }
    ));
    for selection in [
        valid,
        answer(json!({"kind":"answer","signals":{},"output":null})),
        RunResumeFrom::Expiry {},
    ] {
        assert_eq!(
            resolve(
                &suspension(),
                Some(&selection),
                UnixTimestampMillis::new(2000).assert_value()
            )
            .assert_value(),
            RequestResolution::Expiry {}
        );
    }
    assert!(
        resolve(
            &suspension(),
            Some(&RunResumeFrom::Expiry {}),
            UnixTimestampMillis::new(1999).assert_value()
        )
        .is_err()
    );
    assert_eq!(
        deadline(UnixTimestampMillis::new(1000).assert_value(), None)
            .assert_value()
            .get(),
        1000 + DEFAULT_DEADLINE_MILLIS
    );
    assert!(deadline(UnixTimestampMillis::MIN, Some(MAX_DEADLINE_MILLIS)).is_ok());
    assert!(deadline(UnixTimestampMillis::MIN, Some(MAX_DEADLINE_MILLIS + 1)).is_err());
}
#[test]
fn invalid_answers_name_the_offending_path() {
    for (signals, output, path) in [
        (json!({}), json!({"feedback":"ok"}), "signals.decision"),
        (
            json!({"decision":"other"}),
            json!({"feedback":"ok"}),
            "signals.decision",
        ),
        (
            json!({"decision":"approved","other":"x"}),
            json!({"feedback":"ok"}),
            "signals.other",
        ),
        (json!({"decision":"approved"}), json!({}), "feedback"),
        (
            json!({"decision":"approved"}),
            json!({"feedback":1}),
            "feedback",
        ),
        (
            json!({"decision":"approved"}),
            json!({"feedback":"ok","extra":true}),
            "extra",
        ),
    ] {
        let from = answer(json!({"kind":"answer","signals":signals,"output":output}));
        let error = resolve(
            &suspension(),
            Some(&from),
            UnixTimestampMillis::new(1999).assert_value(),
        )
        .unwrap_err();
        assert!(error.contains(path), "{error}");
    }
}
