use std::{
    collections::HashMap,
    fmt::{self, Display},
};

use opentelemetry_otlp::tonic_types::metadata::MetadataMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Protocol {
    /// GRPC protocol
    #[serde(rename = "grpc")]
    Grpc,
    /// HTTP protocol with binary protobuf
    #[serde(rename = "http/protobuf")]
    #[default]
    HttpProtobuf,
    /// HTTP protocol with JSON payload
    #[serde(rename = "http/json")]
    HttpJson,
}

impl From<Protocol> for opentelemetry_otlp::Protocol {
    fn from(protocol: Protocol) -> Self {
        match protocol {
            Protocol::Grpc => opentelemetry_otlp::Protocol::Grpc,
            Protocol::HttpProtobuf => opentelemetry_otlp::Protocol::HttpBinary,
            Protocol::HttpJson => opentelemetry_otlp::Protocol::HttpJson,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Endpoint(String);

impl Default for Endpoint {
    fn default() -> Self {
        Endpoint("http://localhost:4318".to_string())
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Endpoint> for String {
    fn from(val: Endpoint) -> Self {
        val.0
    }
}

impl TryFrom<String> for Endpoint {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        // NOTE: Keep behavior permissive to mirror OTEL env usage; validation can be added later.
        Ok(Endpoint(s))
    }
}

/// OTLP Exporter headers
///
/// Canonical representation is `http::HeaderMap` for broad compatibility
/// with both HTTP exporters and tonic metadata.
///
/// Format for string (de)serialization: `key=value,key2=value2` with lowercase keys.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Headers(http::HeaderMap);

impl From<Headers> for http::HeaderMap {
    fn from(h: Headers) -> Self {
        h.0
    }
}

impl From<Headers> for HashMap<String, String> {
    fn from(h: Headers) -> Self {
        let mut map = HashMap::new();
        for (name, value) in h.0.iter() {
            // NOTE: Values are guaranteed valid ASCII; to_str() should succeed.
            if let Ok(v) = value.to_str() {
                map.insert(name.as_str().to_string(), v.to_string());
            }
        }
        map
    }
}

impl From<Headers> for MetadataMap {
    fn from(h: Headers) -> Self {
        MetadataMap::from_headers(h.into())
    }
}

impl Display for Headers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (name, value) in self.0.iter() {
            if !first {
                write!(f, ",")?;
            }
            first = false;
            let v = value.to_str().map_err(|_| fmt::Error)?;
            write!(f, "{}={}", name.as_str(), v)?;
        }
        Ok(())
    }
}

impl From<Headers> for String {
    fn from(h: Headers) -> Self {
        h.to_string()
    }
}

impl TryFrom<String> for Headers {
    type Error = String;
    // NOTE: Parse `key=value,key2=value2` into http headers with lowercase names.
    // Invalid header names/values will be rejected.
    // For details see: https://github.com/open-telemetry/opentelemetry-specification/blob/f2db30de6d2a9dc6a92108548ce40178158149de/specification/protocol/exporter.md
    fn try_from(s: String) -> Result<Self, Self::Error> {
        let mut headers = http::HeaderMap::new();
        for pair in s.split(',').filter(|p| !p.trim().is_empty()) {
            let mut parts = pair.splitn(2, '=');
            let raw_key = parts
                .next()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .ok_or_else(|| format!("invalid header entry: {pair}"))?;

            // Normalize to lowercase per HTTP/2 + gRPC metadata requirements.
            let key_lower = raw_key.to_ascii_lowercase();
            let name = http::header::HeaderName::from_lowercase(key_lower.as_bytes())
                .map_err(|e| format!("invalid header name `{raw_key}`: {e}"))?;

            let raw_value = parts.next().unwrap_or("").trim();
            let value = http::header::HeaderValue::from_str(raw_value)
                .map_err(|e| format!("invalid header value for `{raw_key}`: {e}"))?;

            headers.append(name, value);
        }
        Ok(Self(headers))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;
    use serde_json as json;

    use super::*;

    #[test]
    fn protocol_serde_values() {
        assert_eq!(json::to_string(&Protocol::Grpc).unwrap(), "\"grpc\"");
        assert_eq!(
            json::to_string(&Protocol::HttpProtobuf).unwrap(),
            "\"http/protobuf\""
        );
        assert_eq!(
            json::to_string(&Protocol::HttpJson).unwrap(),
            "\"http/json\""
        );

        assert_eq!(
            json::from_str::<Protocol>("\"grpc\"").unwrap(),
            Protocol::Grpc
        );
        assert_eq!(
            json::from_str::<Protocol>("\"http/protobuf\"").unwrap(),
            Protocol::HttpProtobuf
        );
        assert_eq!(
            json::from_str::<Protocol>("\"http/json\"").unwrap(),
            Protocol::HttpJson
        );
    }

    #[test]
    fn endpoint_default_and_roundtrip() {
        let default = Endpoint::default();
        assert_eq!(default.0, "http://localhost:4318");

        let s = json::to_string(&default).unwrap();
        let back: Endpoint = json::from_str(&s).unwrap();
        assert_eq!(back, default);

        let from_string = Endpoint::try_from(String::from("http://example:4318")).unwrap();
        let into_string: String = String::from(from_string.clone());
        assert_eq!(from_string.0, into_string);
    }

    #[test]
    fn headers_deserialize_trims_parses() {
        let input = " Authorization=Bearer abc , x-trace = 123 , lone ";
        let parsed: Headers = json::from_str(&format!("\"{}\"", input)).unwrap();

        let map: HashMap<String, String> = parsed.into();
        assert_eq!(map.get("authorization").unwrap(), "Bearer abc");
        assert_eq!(map.get("x-trace").unwrap(), "123");
        assert_eq!(map.get("lone").unwrap(), "");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn headers_serde_roundtrip_order_independent() {
        let mut orig = HashMap::new();
        orig.insert("a".to_string(), "1".to_string());
        orig.insert("b".to_string(), "2".to_string());
        let s_in = orig
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(",");
        let headers: Headers = json::from_str(&format!("\"{}\"", s_in)).unwrap();

        let s = json::to_string(&headers).unwrap();
        let de: Headers = json::from_str(&s).unwrap();
        let round_map: HashMap<String, String> = de.into();
        assert_eq!(round_map, orig);
    }

    #[test]
    fn headers_display_single_pair() {
        let h: Headers = json::from_str("\"k=v\"").unwrap();
        assert_eq!(h.to_string(), "k=v");
    }

    #[test]
    fn headers_invalid_key_errors() {
        let input = "=value"; // empty key
        let err = json::from_str::<Headers>(&format!("\"{}\"", input)).unwrap_err();
        assert!(err.is_data());
    }

    proptest! {
        // Property: Header map -> serialize -> deserialize = Header map
        // Constrain keys/values to avoid commas and trimming changes.
        #[test]
        fn headers_serde_roundtrip_prop(
            headers_map in proptest::collection::hash_map(
                // Restrict to valid HTTP header name characters (no colon).
                proptest::string::string_regex("[a-z0-9._-]{1,24}").unwrap(),
                proptest::string::string_regex("[A-Za-z0-9._:=/+-]{0,48}").unwrap(),
                0..16
            )
        ) {
            let s_in = headers_map
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            let h: Headers = json::from_str(&format!("\"{}\"", s_in)).unwrap();
            let s = json::to_string(&h).unwrap();
            let de: Headers = json::from_str(&s).unwrap();
            let round: HashMap<String, String> = de.into();
            prop_assert_eq!(round, headers_map);
        }
    }

    proptest! {
        // Endpoint roundtrip over arbitrary strings
        #[test]
        fn endpoint_serde_roundtrip_prop(s in ".{0,256}") {
            let ep = Endpoint(s.clone());
            let json_s = json::to_string(&ep).unwrap();
            let back: Endpoint = json::from_str(&json_s).unwrap();
            prop_assert_eq!(back.0, s);
        }
    }
}
