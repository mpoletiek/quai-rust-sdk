use quai_primitives::{Region, Shard, Zone};
use std::{collections::BTreeMap, fmt};
use thiserror::Error;
use url::Url;

/// A validated endpoint. `Debug` deliberately omits paths and query credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    raw: String,
    parsed: Url,
}

impl Endpoint {
    /// Parse HTTP(S)/WS(S). User information and fragments are rejected.
    pub fn parse(input: &str) -> Result<Self, RouteError> {
        // URL parsers accept leading whitespace and embedded ASCII controls.
        // Reject those rather than silently changing the configured endpoint.
        if input.trim() != input
            || input.chars().any(char::is_control)
            || input.bytes().any(|b| b.is_ascii_whitespace())
            || input.contains('\\')
            || !input.contains("://")
        {
            return Err(RouteError::InvalidEndpoint);
        }
        let authority = input
            .split_once("://")
            .ok_or(RouteError::InvalidEndpoint)?
            .1
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        if authority.is_empty() || authority.contains('@') {
            return Err(RouteError::InvalidEndpoint);
        }
        let parsed = Url::parse(input).map_err(|_| RouteError::InvalidEndpoint)?;
        if !matches!(parsed.scheme(), "http" | "https" | "ws" | "wss")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(RouteError::InvalidEndpoint);
        }
        Ok(Self {
            raw: input.to_owned(),
            parsed,
        })
    }

    /// Return the configured URL. This may contain secrets: do not log it.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Return the URL scheme without exposing path/query credentials.
    pub fn scheme(&self) -> &str {
        self.parsed.scheme()
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("scheme", &self.parsed.scheme())
            .field("host", &self.parsed.host_str())
            .field("port", &self.parsed.port())
            .finish_non_exhaustive()
    }
}

/// A validated static shard-to-endpoint table. No silent shard fallback occurs.
#[derive(Clone, Debug)]
pub struct Routing {
    endpoints: BTreeMap<Shard, Endpoint>,
}

/// Invalid routing configuration, without echoing credential-bearing input.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum RouteError {
    /// Endpoint syntax, scheme, user information or fragment is invalid.
    #[error("invalid endpoint; expected HTTP(S) or WS(S) without user information or fragment")]
    InvalidEndpoint,
    /// At least one route is required.
    #[error("at least one shard endpoint is required")]
    Empty,
    /// A route cannot be configured twice.
    #[error("duplicate shard route: {0:?}")]
    Duplicate(Shard),
    /// A configured route is required, even if another shard is available.
    #[error("shard has no configured endpoint: {0:?}")]
    ShardUnavailable(Shard),
    /// A gateway root must not already end in a known shard name.
    #[error("gateway base already ends in a shard name; use direct routing or remove that suffix")]
    AlreadyPathed,
    /// Suffixes must be unambiguous relative paths, not URLs or traversal paths.
    #[error("invalid gateway suffix")]
    InvalidSuffix,
    /// Environment configuration accepts literal lowercase booleans only.
    #[error("use_pathing must be exactly true or false")]
    InvalidBoolean,
}

/// Parse text configuration without JavaScript's nonempty-string truthiness.
pub fn parse_use_pathing(value: &str) -> Result<bool, RouteError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(RouteError::InvalidBoolean),
    }
}

impl Routing {
    /// Use this exact URL for one explicitly selected shard.
    pub fn direct(endpoint: &str, shard: Shard) -> Result<Self, RouteError> {
        Self::explicit([(shard, Endpoint::parse(endpoint)?)])
    }

    /// Compatibility convenience; `false` preserves the supplied endpoint.
    ///
    /// `true` creates one static shard path. It does not discover the network.
    pub fn with_pathing(base: &str, shard: Shard, use_pathing: bool) -> Result<Self, RouteError> {
        if use_pathing {
            Self::gateway(base, [shard])
        } else {
            Self::direct(base, shard)
        }
    }

    /// Configure complete endpoints, including distinct local shard ports.
    pub fn explicit(
        routes: impl IntoIterator<Item = (Shard, Endpoint)>,
    ) -> Result<Self, RouteError> {
        let mut endpoints = BTreeMap::new();
        for (shard, endpoint) in routes {
            if endpoints.insert(shard, endpoint).is_some() {
                return Err(RouteError::Duplicate(shard));
            }
        }
        if endpoints.is_empty() {
            return Err(RouteError::Empty);
        }
        Ok(Self { endpoints })
    }

    /// Append the canonical shard nickname to an immutable gateway base.
    pub fn gateway(
        base: &str,
        shards: impl IntoIterator<Item = Shard>,
    ) -> Result<Self, RouteError> {
        Self::gateway_paths(
            base,
            shards
                .into_iter()
                .map(|s| (s, format!("/{}", s.nickname()))),
        )
    }

    /// Append explicit suffixes; HTTP and WS tables can be configured separately.
    ///
    /// Suffixes consist of nonempty ASCII path segments using letters, digits,
    /// `-`, `_` and `.`; dot traversal and absolute URL syntax are rejected.
    pub fn gateway_paths(
        base: &str,
        paths: impl IntoIterator<Item = (Shard, String)>,
    ) -> Result<Self, RouteError> {
        let base = Endpoint::parse(base)?;
        let last = base
            .parsed
            .path()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("");
        // Compare decoded ASCII so an escaped nickname cannot acquire a second suffix.
        let last = decode_path_segment(last);
        if last == "prime"
            || [Region::Cyprus, Region::Paxos, Region::Hydra]
                .into_iter()
                .any(|r| Shard::Region(r).nickname() == last)
            || Zone::ALL.into_iter().any(|z| z.nickname() == last)
        {
            return Err(RouteError::AlreadyPathed);
        }
        let mut routes = Vec::new();
        for (shard, suffix) in paths {
            let path = suffix.strip_prefix('/').ok_or(RouteError::InvalidSuffix)?;
            if path.split('/').any(|s| {
                s.is_empty()
                    || s == "."
                    || s == ".."
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            }) {
                return Err(RouteError::InvalidSuffix);
            }
            let mut url = base.parsed.clone();
            url.set_path(&format!(
                "{}{}",
                base.parsed.path().trim_end_matches('/'),
                suffix
            ));
            routes.push((shard, Endpoint::parse(url.as_str())?));
        }
        Self::explicit(routes)
    }

    /// Resolve a shard or return an explicit unavailable-shard error.
    pub fn endpoint(&self, shard: Shard) -> Result<&Endpoint, RouteError> {
        self.endpoints
            .get(&shard)
            .ok_or(RouteError::ShardUnavailable(shard))
    }

    /// Iterate configured shards in a deterministic order.
    pub fn shards(&self) -> impl Iterator<Item = Shard> + '_ {
        self.endpoints.keys().copied()
    }
}

fn decode_path_segment(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            decoded.push((high * 16 + low) as u8);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    const ZONE: Shard = Shard::Zone(Zone::Cyprus1);

    #[test]
    fn false_preserves_direct_endpoints() {
        for url in [
            "http://127.0.0.1:9200",
            "ws://[::1]:8200/ws/cyprus1?token=private",
            "https://node.example:443/rpc/%2F/cyprus1?token=a%2Fb",
        ] {
            assert_eq!(
                Routing::with_pathing(url, ZONE, false)
                    .unwrap()
                    .endpoint(ZONE)
                    .unwrap()
                    .as_str(),
                url
            );
        }
        assert!(!parse_use_pathing("false").unwrap());
        for bad in ["False", "'false'", "0", "", " false"] {
            assert!(parse_use_pathing(bad).is_err());
        }
    }

    #[test]
    fn gateway_uses_immutable_base_and_keeps_query() {
        let r = Routing::gateway(
            "https://node.example/rpc/?token=a%2Fb",
            [Shard::Prime, ZONE],
        )
        .unwrap();
        assert_eq!(
            r.endpoint(Shard::Prime).unwrap().as_str(),
            "https://node.example/rpc/prime?token=a%2Fb"
        );
        assert_eq!(
            r.endpoint(ZONE).unwrap().as_str(),
            "https://node.example/rpc/cyprus1?token=a%2Fb"
        );
        assert!(matches!(
            r.endpoint(Shard::Zone(Zone::Paxos1)),
            Err(RouteError::ShardUnavailable(_))
        ));
    }

    #[test]
    fn explicit_ports_and_custom_ws_paths() {
        let r = Routing::explicit([
            (ZONE, Endpoint::parse("http://localhost:9200").unwrap()),
            (
                Shard::Zone(Zone::Cyprus2),
                Endpoint::parse("http://localhost:9201").unwrap(),
            ),
        ])
        .unwrap();
        assert_eq!(r.endpoint(ZONE).unwrap().as_str(), "http://localhost:9200");
        assert_eq!(
            r.endpoint(Shard::Zone(Zone::Cyprus2)).unwrap().as_str(),
            "http://localhost:9201"
        );
        let ws =
            Routing::gateway_paths("wss://node.example", [(ZONE, "/ws/cyprus1".into())]).unwrap();
        assert_eq!(
            ws.endpoint(ZONE).unwrap().as_str(),
            "wss://node.example/ws/cyprus1"
        );
    }

    #[test]
    fn rejects_ambiguous_invalid_and_duplicate_routes() {
        for bad in [
            "https://n/cyprus1",
            "https://n/prime/",
            "https://n/rpc/%63yprus1",
            "https://n/%70rime",
        ] {
            assert!(matches!(
                Routing::gateway(bad, [ZONE]),
                Err(RouteError::AlreadyPathed)
            ));
        }
        for bad in [
            "/../elsewhere",
            "//evil",
            "/%2e%2e",
            "/x?token=evil",
            "/x#frag",
            "/x\\evil",
            "https://evil",
        ] {
            assert!(Routing::gateway_paths("https://n", [(ZONE, bad.into())]).is_err());
        }
        assert!(matches!(
            Routing::gateway("https://n", [ZONE, ZONE]),
            Err(RouteError::Duplicate(_))
        ));
        assert!(matches!(
            Routing::gateway("https://n", []),
            Err(RouteError::Empty)
        ));
        for bad in [
            "file:///tmp/x",
            "https://u:secret@n",
            "https://n/#fragment",
            " https://n",
            "http:\\n",
            "https://n/\n",
            "https:node.example",
            "https:///node.example",
            "https://@node.example",
            "https://n/path with spaces",
        ] {
            assert!(Endpoint::parse(bad).is_err());
        }
    }

    #[test]
    fn debug_omits_secrets_even_in_path() {
        let endpoint =
            Endpoint::parse("https://node.example/SECRET_PATH?token=SECRET_QUERY").unwrap();
        let debug = format!("{endpoint:?}");
        assert!(!debug.contains("SECRET"));
        assert!(debug.contains("node.example"));
    }
}
