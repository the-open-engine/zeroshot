//! Assertion helpers for verifying advertised graph profiles against a caller-supplied
//! expectation. This module holds no backend-to-profile registry: callers supply the
//! expected set, so a scripted vector here makes no claim about any production backend.

use openengine_cluster_protocol::{GraphProfile, ServerCapabilities};

pub fn assert_advertised_profiles(capabilities: &ServerCapabilities, expected: &[GraphProfile]) {
    assert_eq!(
        capabilities.graph_profiles.values(),
        expected,
        "advertised graph profiles did not match expected vector"
    );
}
