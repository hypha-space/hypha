use std::fmt::Display;

use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;
use serde::{Deserialize, Serialize};

/// Resource attributes
///
/// Format: `key=value,key2=value2`. Whitespace around keys and values is trimmed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Attributes(Vec<KeyValue>);

impl Attributes {
    pub fn new(attrs: Vec<KeyValue>) -> Self {
        Attributes(attrs)
    }
}

impl From<Attributes> for Resource {
    fn from(attrs: Attributes) -> Self {
        Resource::builder_empty().with_attributes(attrs.0).build()
    }
}

impl From<Vec<KeyValue>> for Attributes {
    fn from(v: Vec<KeyValue>) -> Self {
        Attributes(v)
    }
}

impl From<Attributes> for Vec<KeyValue> {
    fn from(a: Attributes) -> Self {
        a.0
    }
}

impl Display for Attributes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // NOTE: Display is the canonical on-wire comma-separated format.
        for kv in &self.0 {
            let key = kv.key.as_str();
            let val = kv.value.as_str();
            write!(f, "{}={},", key, val)?;
        }
        Ok(())
    }
}

impl TryFrom<String> for Attributes {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        // NOTE: Following the opentelemetry specification we expected the attributes to be
        // represented in a format matching to the W3C Baggage, i.e.: key1=value1,key2=value2.
        // All values MUST be considered strings and characters outside the baggage-octet range
        // MUST be percent-encoded. Additional metadata is not supported.
        //
        // For details see: https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/resource/sdk.md#specifying-resource-information-via-an-environment-variable
        let mut kvs: Vec<KeyValue> = Vec::new();
        for pair in s.split(',').filter(|p| !p.trim().is_empty()) {
            let mut parts = pair.splitn(2, '=');
            let key = parts
                .next()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .ok_or_else(|| format!("invalid resource attribute entry: {pair}"))?;
            let value = parts.next().unwrap_or("").trim();
            kvs.push(KeyValue::new(key.to_string(), value.to_string()));
        }

        Ok(Self(kvs))
    }
}

impl From<Attributes> for String {
    fn from(a: Attributes) -> Self {
        a.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use opentelemetry::{KeyValue, Value};
    use proptest::prelude::*;
    use serde_json as json;

    use super::Attributes;

    #[test]
    fn deserializes_into_attributes() {
        let input = "service.name=hypha, env=prod, dangling";
        let attrs: Attributes = json::from_str(&format!("\"{}\"", input)).unwrap();

        let mut map = HashMap::new();
        for kv in attrs.0.iter() {
            if let Value::String(s) = &kv.value {
                map.insert(kv.key.as_str().to_string(), s.to_string());
            }
        }
        assert_eq!(map.get("service.name").unwrap(), "hypha");
        assert_eq!(map.get("env").unwrap(), "prod");
        assert_eq!(map.get("dangling").unwrap(), "");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn deserialize_with_extra_whitespaces() {
        let input = " service.name =  hypha, env = prod, dangling ,   ";
        let attrs: Attributes = json::from_str(&format!("\"{}\"", input)).unwrap();

        let mut map = HashMap::new();
        for kv in attrs.0.iter() {
            if let Value::String(s) = &kv.value {
                map.insert(kv.key.as_str().to_string(), s.to_string());
            }
        }
        assert_eq!(map.get("service.name").unwrap(), "hypha");
        assert_eq!(map.get("env").unwrap(), "prod");
        assert_eq!(map.get("dangling").unwrap(), "");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn deserialize_without_whitespace() {
        let input = "service.name=hypha,env=prod,dangling,";
        let attrs: Attributes = json::from_str(&format!("\"{}\"", input)).unwrap();

        let mut map = HashMap::new();
        for kv in attrs.0.iter() {
            if let Value::String(s) = &kv.value {
                map.insert(kv.key.as_str().to_string(), s.to_string());
            }
        }
        assert_eq!(map.get("service.name").unwrap(), "hypha");
        assert_eq!(map.get("env").unwrap(), "prod");
        assert_eq!(map.get("dangling").unwrap(), "");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn handles_invalid_key_errors() {
        let input = "=nope";
        let err = json::from_str::<Attributes>(&format!("\"{}\"", input)).unwrap_err();
        assert!(err.is_data());
    }

    proptest! {
        #[test]
        fn rountrip(
            attrs_map in proptest::collection::hash_map(
                // Keys: 1..20 length, safe charset
                proptest::string::string_regex("[A-Za-z0-9._:-]{1,20}").unwrap(),
                // Values: 0..30 length, allow '=' but no comma/whitespace
                proptest::string::string_regex("[A-Za-z0-9._:=/+-]{0,30}").unwrap(),
                0..16
            )
        ) {
            let mut pairs: Vec<KeyValue> = Vec::with_capacity(attrs_map.len());
            for (k, v) in attrs_map.iter() {
                pairs.push(KeyValue::new(k.clone(), v.clone()));
            }
            let attrs = Attributes(pairs);

            // Round-trip via serde_json
            let s = json::to_string(&attrs).unwrap();
            let de: Attributes = json::from_str(&s).unwrap();

            let mut round_map = HashMap::new();
            for kv in de.0.iter() {
                if let Value::String(s) = &kv.value {
                    round_map.insert(kv.key.as_str().to_string(), s.to_string());
                }
            }
            prop_assert_eq!(round_map, attrs_map);
        }
    }
}
