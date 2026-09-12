//! Routing compatibility with the pinned published JavaScript artifact.
//!
//! Fixtures use explicit shards, so this test never discovers or contacts nodes.
//! Deliberate safety differences are tested separately below: no unknown-shard
//! fallback, no repeated gateway suffix, and no JavaScript string truthiness.

use quai_primitives::{Shard, Zone};
use quai_rpc::{RouteError, Routing, parse_use_pathing};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixtures {
    schema_version: u32,
    reference: String,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    id: String,
    input: Input,
    expected: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    url: String,
    use_pathing: bool,
    shard: String,
    shard_paths: Option<BTreeMap<String, String>>,
}

#[test]
fn routes_match_all_pinned_javascript_vectors() {
    let fixtures: Fixtures =
        serde_json::from_str(include_str!("../../../compatibility/fixtures/routing.json"))
            .expect("valid checked-in routing fixtures");
    assert_eq!(fixtures.schema_version, 1);
    assert_eq!(fixtures.reference, "quais@1.0.0-alpha.57");
    assert!(!fixtures.vectors.is_empty());

    for vector in fixtures.vectors {
        let shard: Shard = vector.input.shard.parse().expect("known reference shard");
        let routing = match vector.input.shard_paths {
            Some(paths) => {
                assert!(
                    vector.input.use_pathing,
                    "{}: custom-path fixture",
                    vector.id
                );
                let suffix = paths
                    .get(shard.nickname())
                    .expect("configured shard suffix");
                Routing::gateway_paths(&vector.input.url, [(shard, suffix.clone())])
            }
            None => Routing::with_pathing(&vector.input.url, shard, vector.input.use_pathing),
        }
        .unwrap_or_else(|error| panic!("{}: {error}", vector.id));

        assert_eq!(
            routing.endpoint(shard).unwrap().as_str(),
            vector.expected,
            "{}",
            vector.id
        );
    }
}

#[test]
fn unavailable_shards_fail_instead_of_using_javascript_last_connection_fallback() {
    // JS JsonRpcProvider._getConnection uses its last connection when a shard
    // is absent. Rust requires explicit routing to prevent cross-shard reads.
    let selected = Shard::Zone(Zone::Cyprus1);
    let other = Shard::Zone(Zone::Hydra3);
    let routing = Routing::direct("http://127.0.0.1:9002", selected).unwrap();
    assert!(matches!(routing.endpoint(other), Err(RouteError::ShardUnavailable(s)) if s == other));
}

#[test]
fn each_gateway_route_uses_original_base_and_rejects_prepathed_base() {
    // The upstream FetchRequest discovery path can append to an already
    // mutated /prime URL. Rust deliberately builds all routes from one base.
    let zone = Shard::Zone(Zone::Cyprus1);
    let routing = Routing::gateway(
        "https://example.invalid/rpc?key=public-fixture",
        [Shard::Prime, zone],
    )
    .unwrap();
    assert_eq!(
        routing.endpoint(Shard::Prime).unwrap().as_str(),
        "https://example.invalid/rpc/prime?key=public-fixture"
    );
    assert_eq!(
        routing.endpoint(zone).unwrap().as_str(),
        "https://example.invalid/rpc/cyprus1?key=public-fixture"
    );

    for suffix in ["prime", "cyprus", "cyprus1"] {
        let base = format!("https://example.invalid/rpc/{suffix}");
        assert!(matches!(
            Routing::gateway(&base, [zone]),
            Err(RouteError::AlreadyPathed)
        ));
        // A complete user-specified URL remains available through direct mode.
        assert_eq!(
            Routing::direct(&base, zone)
                .unwrap()
                .endpoint(zone)
                .unwrap()
                .as_str(),
            base
        );
    }
}

#[test]
fn environment_false_is_boolean_false_instead_of_javascript_truthy_string() {
    let zone = Shard::Zone(Zone::Cyprus1);
    let endpoint = "http://127.0.0.1:9002/rpc";
    let routing =
        Routing::with_pathing(endpoint, zone, parse_use_pathing("false").unwrap()).unwrap();
    assert_eq!(routing.endpoint(zone).unwrap().as_str(), endpoint);
    // Configuration text is parsed strictly; quoting is not part of the value.
    assert_eq!(
        parse_use_pathing("'false'"),
        Err(RouteError::InvalidBoolean)
    );
}
