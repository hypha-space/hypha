#![deny(missing_docs)]
//! Command-line interface utilities for configuration management.
//!
//! This module provides utility functions for handling configuration overrides
//! via command-line arguments, supporting the `--set key=value` functionality
//! across all Hypha binaries.

use figment::providers::{Format, Toml};
use miette::Result;
use serde::{Deserialize, Serialize};

use crate::LayeredConfigBuilder;

/// Parse a single key-value pair for configuration overrides.
///
/// Parses command-line arguments in the format `KEY=value` into separate
/// key and value strings for use with configuration override systems.
///
/// # Arguments
///
/// * `s` - A string slice in the format "key=value"
///
/// # Returns
///
/// Returns `Ok((key, value))` on successful parsing, or an error message
/// describing what went wrong.
///
/// # Examples
///
/// ```rust
/// use hypha_config::cli::parse_key_val;
///
/// let (key, value) = parse_key_val("listen_port=8080").unwrap();
/// assert_eq!(key, "listen_port");
/// assert_eq!(value, "8080");
///
/// let (key, value) = parse_key_val("debug=true").unwrap();
/// assert_eq!(key, "debug");
/// assert_eq!(value, "true");
///
/// // Handles nested configuration via dot notation
/// let (key, value) = parse_key_val("listen_addresses.0=/ip4/127.0.0.1/tcp/8080").unwrap();
/// assert_eq!(key, "listen_addresses.0");
/// assert_eq!(value, "/ip4/127.0.0.1/tcp/8080");
///
/// // Returns error for invalid format
/// assert!(parse_key_val("invalid_format").is_err());
/// ```
pub fn parse_key_val(s: &str) -> Result<(String, String)> {
    let pos = s
        .find('=')
        .ok_or_else(|| miette::miette!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

/// Convert key-value pairs to TOML format for configuration overrides.
///
/// Takes a collection of key-value pairs and converts them into a TOML
/// string that can be used as a configuration provider. Performs basic
/// type detection to avoid unnecessarily quoting numeric and boolean values.
/// Supports nested structure notation (e.g., "scheduler.model.repository") and
/// array index notation (e.g., "scheduler.model.filenames.0").
///
/// # Arguments
///
/// * `overrides` - A slice of `(key, value)` tuples to convert
///
/// # Returns
///
/// Returns a TOML-formatted string ready for use with figment providers.
///
/// # Examples
///
/// ```rust
/// use hypha_config::cli::create_overrides_toml;
///
/// let overrides = vec![
///     ("debug".to_string(), "true".to_string()),
///     ("port".to_string(), "8080".to_string()),
///     ("name".to_string(), "my-service".to_string()),
///     ("scheduler.job.model.repository".to_string(), "hypha-space/lenet".to_string()),
///     ("scheduler.job.model.filenames.0".to_string(), "config.json".to_string()),
///     ("scheduler.job.model.filenames.1".to_string(), "model.safetensors".to_string()),
/// ];
///
/// let toml = create_overrides_toml(&overrides);
/// assert!(toml.contains("debug = true"));    // Boolean without quotes
/// assert!(toml.contains("port = 8080"));     // Number without quotes  
/// assert!(toml.contains("name = \"my-service\"")); // String with quotes
/// assert!(toml.contains("[scheduler.job.model]"));
/// assert!(toml.contains("repository = \"hypha-space/lenet\""));
/// assert!(toml.contains("filenames = [\"config.json\", \"model.safetensors\"]"));
/// ```
pub fn create_overrides_toml(overrides: &[(String, String)]) -> String {
    use toml_edit::DocumentMut;

    // Parse all override keys to build a nested structure
    let mut doc = DocumentMut::new();

    for (key, val) in overrides {
        // NOTE: We ignore errors for individual overrides to keep the function infallible.
        let _ = set_nested_value(&mut doc, key, val);
    }

    // Convert document to string
    doc.to_string()
}

/// Helper function to set a nested value in a TOML document
fn set_nested_value(doc: &mut toml_edit::DocumentMut, key: &str, val: &str) -> Result<()> {
    use toml_edit::Value;

    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return Err(crate::ConfigError::Invalid(format!("Empty key: {}", key)).into());
    }

    // Format the value with proper type detection
    let formatted_value = if val.parse::<bool>().is_ok() {
        Value::Boolean(toml_edit::Formatted::new(val.parse::<bool>().unwrap()))
    } else if let Ok(int_val) = val.parse::<i64>() {
        Value::Integer(toml_edit::Formatted::new(int_val))
    } else if let Ok(float_val) = val.parse::<f64>() {
        Value::Float(toml_edit::Formatted::new(float_val))
    } else {
        Value::String(toml_edit::Formatted::new(val.to_string()))
    };

    let root = doc.as_table_mut();
    set_value_at_path(root, &parts, formatted_value)
}

/// Recursively set a value at the given path in a TOML table
fn set_value_at_path(
    table: &mut toml_edit::Table,
    path: &[&str],
    val: toml_edit::Value,
) -> Result<()> {
    use toml_edit::Item;

    if path.is_empty() {
        return Err(crate::ConfigError::Invalid("Empty path".to_string()).into());
    }

    if path.len() == 1 {
        let key = path[0];

        // Check if this is an array index
        if let Ok(index) = key.parse::<usize>() {
            return Err(crate::ConfigError::Invalid(format!(
                "Array index {} found without parent array",
                index
            ))
            .into());
        }

        table[key] = Item::Value(val);
        return Ok(());
    }

    let key = path[0];
    let rest = &path[1..];

    // Check if the next part is an array index
    if !rest.is_empty()
        && let Ok(index) = rest[0].parse::<usize>()
    {
        // This is an array operation
        ensure_array_at_key(table, key)?;

        if let Some(Item::Value(val_ref)) = table.get_mut(key)
            && let Some(array) = val_ref.as_array_mut()
        {
            // Ensure array is large enough
            while array.len() <= index {
                array.push(toml_edit::Value::String(toml_edit::Formatted::new(
                    "".to_string(),
                )));
            }

            if rest.len() == 1 {
                // Set the array element directly
                array.replace(index, val);
            } else {
                // Need to set a nested value in the array element
                return Err(crate::ConfigError::Invalid(
                    "Nested values in array elements not supported".to_string(),
                )
                .into());
            }
        }
        return Ok(());
    }

    // This is a regular nested table operation
    ensure_table_at_key(table, key)?;

    if let Some(Item::Table(subtable)) = table.get_mut(key) {
        set_value_at_path(subtable, rest, val)
    } else {
        Err(crate::ConfigError::Invalid(format!("Failed to create table at key: {}", key)).into())
    }
}

/// Ensure there's a table at the given key, creating one if necessary
fn ensure_table_at_key(table: &mut toml_edit::Table, key: &str) -> Result<()> {
    use toml_edit::{Item, Table};

    if !table.contains_key(key) {
        table[key] = Item::Table(Table::new());
    } else if !table[key].is_table() {
        return Err(crate::ConfigError::Invalid(format!(
            "Key '{}' exists but is not a table",
            key
        ))
        .into());
    }
    Ok(())
}

/// Ensure there's an array at the given key, creating one if necessary
fn ensure_array_at_key(table: &mut toml_edit::Table, key: &str) -> Result<()> {
    use toml_edit::{Array, Item, Value};

    if !table.contains_key(key) {
        table[key] = Item::Value(Value::Array(Array::new()));
    // NOTE: Used is_some_and() instead of map_or(false, ...) per Clippy suggestion (unnecessary_map_or)
    } else if !table[key].as_value().is_some_and(|v| v.is_array()) {
        return Err(crate::ConfigError::Invalid(format!(
            "Key '{}' exists but is not an array",
            key
        ))
        .into());
    }
    Ok(())
}

/// Apply configuration overrides to a builder.
///
/// Takes a `LayeredConfigBuilder` and applies configuration overrides
/// by converting them to TOML format and adding them as a provider.
/// This allows command-line arguments to override configuration file values.
///
/// # Arguments
///
/// * `builder` - The configuration builder to modify
/// * `overrides` - A slice of `(key, value)` pairs to apply as overrides
///
/// # Returns
///
/// Returns the modified builder with the overrides applied as the highest
/// priority configuration source.
///
/// # Examples
///
/// ```rust
/// use hypha_config::{builder, cli::apply_config_overrides};
/// use serde::{Deserialize, Serialize};
/// use documented::{Documented, DocumentedFieldsOpt};
/// use figment::Figment;
///
/// #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt, Default)]
/// /// Configuration for example
/// struct Config {
///     /// Debug flag
///     #[serde(default)]
///     debug: bool,
///     /// Port number
///     #[serde(default)]
///     port: u16,
///     /// Figment metadata
///     #[serde(skip)]
///     figment: Option<Figment>,
/// }
///
/// let overrides = vec![
///     ("debug".to_string(), "true".to_string()),
///     ("port".to_string(), "9090".to_string()),
/// ];
///
/// let builder = builder::<Config>();
/// let config_builder = apply_config_overrides(builder, &overrides).unwrap();
/// let config = config_builder.build().unwrap();
///
/// assert_eq!(config.debug, true);
/// assert_eq!(config.port, 9090);
/// ```
pub fn apply_config_overrides<T>(
    mut builder: LayeredConfigBuilder<T>,
    overrides: &[(String, String)],
) -> Result<LayeredConfigBuilder<T>>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    if !overrides.is_empty() {
        let overrides_toml = create_overrides_toml(overrides);
        builder = builder.with_provider(Toml::string(&overrides_toml));
    }
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_val_handles_simple_pairs() {
        let (key, value) = parse_key_val("debug=true").unwrap();
        assert_eq!(key, "debug");
        assert_eq!(value, "true");

        let (key, value) = parse_key_val("port=8080").unwrap();
        assert_eq!(key, "port");
        assert_eq!(value, "8080");
    }

    #[test]
    fn parse_key_val_handles_complex_values() {
        let (key, value) = parse_key_val("path=/some/complex/path").unwrap();
        assert_eq!(key, "path");
        assert_eq!(value, "/some/complex/path");

        let (key, value) = parse_key_val("addr=/ip4/127.0.0.1/tcp/8080").unwrap();
        assert_eq!(key, "addr");
        assert_eq!(value, "/ip4/127.0.0.1/tcp/8080");
    }

    #[test]
    fn parse_key_val_handles_nested_keys() {
        let (key, value) = parse_key_val("listen_addresses.0=/ip4/0.0.0.0/tcp/8080").unwrap();
        assert_eq!(key, "listen_addresses.0");
        assert_eq!(value, "/ip4/0.0.0.0/tcp/8080");
    }

    #[test]
    fn parse_key_val_handles_empty_value() {
        let (key, value) = parse_key_val("empty=").unwrap();
        assert_eq!(key, "empty");
        assert_eq!(value, "");
    }

    #[test]
    fn parse_key_val_handles_multiple_equals() {
        let (key, value) = parse_key_val("equation=x=y+1").unwrap();
        assert_eq!(key, "equation");
        assert_eq!(value, "x=y+1");
    }

    #[test]
    fn parse_key_val_rejects_invalid_format() {
        assert!(parse_key_val("no_equals_sign").is_err());
        assert!(parse_key_val("").is_err());
    }

    #[test]
    fn create_overrides_toml_handles_different_types() {
        let overrides = vec![
            ("debug".to_string(), "true".to_string()),
            ("port".to_string(), "8080".to_string()),
            ("rate".to_string(), "1.5".to_string()),
            ("name".to_string(), "test-service".to_string()),
        ];

        let toml = create_overrides_toml(&overrides);

        assert!(toml.contains("debug = true"));
        assert!(toml.contains("port = 8080"));
        assert!(toml.contains("rate = 1.5"));
        assert!(toml.contains("name = \"test-service\""));
    }

    #[test]
    fn create_overrides_toml_escapes_quotes_in_strings() {
        let overrides = vec![("message".to_string(), "Hello \"World\"".to_string())];

        let toml = create_overrides_toml(&overrides);
        // In toml_edit v0.23.7, it uses single quotes to avoid escaping
        assert!(toml.contains("message = 'Hello \"World\"'"));
    }

    #[test]
    fn create_overrides_toml_handles_empty_input() {
        let overrides = vec![];
        let toml = create_overrides_toml(&overrides);
        assert_eq!(toml, "");
    }

    #[test]
    fn apply_config_overrides_with_empty_overrides() {
        use documented::{Documented, DocumentedFieldsOpt};
        use figment::Figment;
        use serde::{Deserialize, Serialize};

        use crate::builder;

        #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt, Default)]
        /// Test configuration for CLI tests
        #[allow(dead_code)]
        struct TestConfig {
            /// Debug flag for testing
            #[serde(default)]
            debug: bool,
            /// Figment metadata
            #[serde(skip)]
            figment: Option<Figment>,
        }

        let builder = builder::<TestConfig>();
        let result = apply_config_overrides(builder, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_config_overrides_applies_values() {
        use documented::{Documented, DocumentedFieldsOpt};
        use figment::Figment;
        use serde::{Deserialize, Serialize};

        use crate::builder;

        #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt, Default)]
        /// Test configuration for CLI apply tests
        #[allow(dead_code)]
        struct TestConfig {
            /// Debug flag for testing
            #[serde(default)]
            debug: bool,
            /// Port number for testing
            #[serde(default)]
            port: u16,
            /// Figment metadata
            #[serde(skip)]
            figment: Option<Figment>,
        }

        let overrides = vec![
            ("debug".to_string(), "true".to_string()),
            ("port".to_string(), "9090".to_string()),
        ];

        let builder = builder::<TestConfig>();
        let config_builder = apply_config_overrides(builder, &overrides).unwrap();
        let config = config_builder.build().unwrap();

        assert_eq!(config.debug, true);
        assert_eq!(config.port, 9090);
    }
}
