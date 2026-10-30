use openengine_cluster_protocol::{
    RUN_PROFILES_KIND, TargetDiscoveryExtensions, TargetRunProfileRoutes,
    TargetRunProfilesDiscovery,
};
use reqwest::Url;

use super::*;

fn routes(list: &str) -> TargetRunProfileRoutes {
    TargetRunProfileRoutes {
        list: list.to_owned(),
        show: "/show".to_owned(),
        set: "/set".to_owned(),
        delete: "/delete".to_owned(),
        default: "/default".to_owned(),
        run: "/run".to_owned(),
    }
}

fn extensions(kind: &str, base_url: &str, list: &str) -> TargetDiscoveryExtensions {
    TargetDiscoveryExtensions {
        run_profiles: Some(TargetRunProfilesDiscovery {
            kind: kind.to_owned(),
            base_url: base_url.to_owned(),
            route_templates: routes(list),
        }),
        ..TargetDiscoveryExtensions::default()
    }
}

#[test]
fn descriptor_builds_every_literal_route_under_the_discovered_base() {
    let origin = Url::parse("https://target.example").unwrap();
    let descriptor = build_profiles_descriptor(
        &origin,
        &extensions(
            RUN_PROFILES_KIND,
            "https://target.example/api/profiles",
            "/list",
        ),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        descriptor.list.as_str(),
        "https://target.example/api/profiles/list"
    );
    assert_eq!(
        descriptor.show.as_str(),
        "https://target.example/api/profiles/show"
    );
    assert_eq!(
        descriptor.set.as_str(),
        "https://target.example/api/profiles/set"
    );
    assert_eq!(
        descriptor.delete.as_str(),
        "https://target.example/api/profiles/delete"
    );
    assert_eq!(
        descriptor.default.as_str(),
        "https://target.example/api/profiles/default"
    );
    assert_eq!(
        descriptor.run.as_str(),
        "https://target.example/api/profiles/run"
    );
}

#[test]
fn absent_discovery_advertises_no_profile_routes() {
    let origin = Url::parse("https://target.example").unwrap();
    assert!(matches!(
        build_profiles_descriptor(&origin, &TargetDiscoveryExtensions::default()),
        Ok(None)
    ));
}

#[test]
fn incompatible_kind_and_invalid_routes_are_refused_without_guessing() {
    let origin = Url::parse("https://target.example").unwrap();
    let Err(kind_error) = build_profiles_descriptor(
        &origin,
        &extensions(
            "other.profiles/v1",
            "https://target.example/profiles",
            "/list",
        ),
    ) else {
        panic!("incompatible profile discovery must be refused");
    };
    assert_eq!(
        kind_error.to_string(),
        "run-profile discovery is incompatible"
    );

    for list in ["//list", "/../escape", "/lists?admin=true"] {
        let Err(error) = build_profiles_descriptor(
            &origin,
            &extensions(RUN_PROFILES_KIND, "https://target.example/profiles", list),
        ) else {
            panic!("invalid run-profile route must be refused: {list}");
        };
        assert_eq!(error.to_string(), "run-profile route template is invalid");
    }

    for base_url in ["/profiles", "https://elsewhere.example/profiles"] {
        let Err(error) =
            build_profiles_descriptor(&origin, &extensions(RUN_PROFILES_KIND, base_url, "/list"))
        else {
            panic!("invalid run-profile base must be refused: {base_url}");
        };
        assert!(
            error.to_string().starts_with("target route"),
            "unexpected diagnostic for {base_url}: {error}"
        );
    }
}
