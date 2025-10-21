#![deny(missing_docs)]
// NOTE: `Diagnostic` uses `Option::unwrap` which we've dissallowed in favor,
// of more granular error handling. Using it here through the `Diagnostic` trait is fine though.
#![allow(clippy::disallowed_methods)]
//! # Layered Configuration
//!
//! A layered configuration system that provides type-safe configuration management
//! with comprehensive error reporting.
//!
//! ## Introduction
//!
//! The configuration library enables building robust configuration systems
//! that can merge settings from multiple sources (files, environment variables,
//! command-line arguments) while maintaining type safety and providing detailed
//! error diagnostics. It's built on top of [`figment`], and extends it with
//! enhanced error reporting using [`miette`] and [`documented`] for documentation.
//!
//! ## Quick Start Example
//!
//! ```text
//! use hypha_config::{LayeredConfig, LayeredConfigBuilder};
//! use serde::{Deserialize, Serialize};
//! use documented::{Documented, DocumentedFieldsOpt};
//! use figment::{Figment, providers::{Env, Format, Toml}};
//!
//! #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
//! /// Some network node configuration
//! struct Config {
//!     /// The address to bind to
//!     #[serde(default = "default_bind_address")]
//!     bind_address: String,
//!
//!     /// Maximum number of connections
//!     #[serde(default = "default_max_connections")]
//!     max_connections: usize,
//!
//!     /// SSL certificate file path
//!     cert_pem: String,
//!
//!     /// Enable debug logging
//!     #[serde(default)]
//!     debug: bool,
//!
//!     #[serde(skip)]
//!     figment: Option<Figment>,
//! }
//!
//! fn default_bind_address() -> String {
//!     "0.0.0.0:8080".to_string()
//! }
//!
//! fn default_max_connections() -> usize {
//!     100
//! }
//!
//! impl LayeredConfig for Config {
//!     fn with_figment(mut self, figment: Figment) -> Self {
//!         self.figment = Some(figment);
//!         self
//!     }
//!
//!     fn figment(&self) -> &Option<Figment> {
//!         &self.figment
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Build configuration from multiple sources
//! let config = Config::builder()
//!     .with_provider(Toml::file("config.toml"))
//!     .with_provider(Env::prefixed("CONFIG_"))
//!     .build()?
//!     .validate()?;
//!
//! println!("Binding to: {}", config.bind_address);
//! # Ok(())
//! # }
//! ```
//!
//! ## Error Reporting
//!
//! The library provides detailed error reporting for common configuration issues.
//! Basic validation errors are supported out of the box:
//!
//! **Missing required fields**:
//! ```text
//! Error:   × Failed to build config
//!   ╰─▶ missing field `cert_pem`
//! ```
//!
//! **Type validation errors**:
//! ```text
//! Error:   × Failed to build config
//!   ╰─▶ invalid type: found signed int `1`, expected path string for key "default.cert_pem" in gateway.toml TOML file
//! ```
//!
//! ### Enhanced Error Reporting with Metadata
//!
//! For configuration-related operations that may fail (like file I/O), you can enhance error
//! diagnostics by adding metadata. This provides users with precise source location information to
//! help them fix configuration errors:
//!
//! ```text
//! use hypha_config::{LayeredConfig, LayeredConfigBuilder, ConfigError};
//! use serde::{Deserialize, Serialize};
//! use documented::{Documented, DocumentedFieldsOpt};
//! use figment::{Figment, providers::{Env, Format, Toml}};
//! use std::fs;
//!
//! #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
//! /// Some network node configuration
//! struct Config {
//!     /// The address to bind to
//!     #[serde(default = "default_bind_address")]
//!     bind_address: String,
//!
//!     /// Maximum number of connections
//!     #[serde(default = "default_max_connections")]
//!     max_connections: usize,
//!
//!     /// SSL certificate file path
//!     cert_pem: String,
//!
//!     /// Enable debug logging
//!     #[serde(default)]
//!     debug: bool,
//!
//!     #[serde(skip)]
//!     figment: Option<Figment>,
//! }
//!
//! # fn default_bind_address() -> String {
//! #     "0.0.0.0:8080".to_string()
//! # }
//! #
//! # fn default_max_connections() -> usize {
//! #     100
//! # }
//! #
//! # impl LayeredConfig for Config {
//! #     fn with_figment(mut self, figment: Figment) -> Self {
//! #         self.figment = Some(figment);
//! #         self
//! #     }
//! #
//! #     fn figment(&self) -> &Option<Figment> {
//! #         &self.figment
//! #     }
//! # }
//! #
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Build configuration from multiple sources
//! let config = Config::builder()
//!     .with_provider(Toml::file("config.toml"))
//!     .with_provider(Env::prefixed("CONFIG_"))
//!     .build()?
//!     .validate()?;
//!
//! // Use metadata to enhance file operation errors
//! let metadata = config.find_metadata("cert_pem");
//! let cert_content = fs::read(&config.cert_pem)
//!     .map_err(ConfigError::with_metadata(&metadata))?;
//! # Ok(())
//! # }
//! ```
//!
//! This produces rich diagnostics with source location:
//! ```text
//! Error:   × Configuration error
//!   ├─▶ Failed to load
//!   ╰─▶ No such file or directory (os error 2)
//!     ╭─[/path/to/config.toml:12:1]
//!  11 │
//!  12 │ cert_pem = "cert.pem"
//!     · ──────────────┬──────────────
//!     ·                        ╰── configuration value
//!     ╰────
//! ```
//!
//! ### Automatic Documentation
//!
//! Generate configuration file templates with documentation:
//!
//! ```text
//! # use hypha_config::LayeredConfig;
//! # use serde::{Deserialize, Serialize};
//! # use documented::{Documented, DocumentedFieldsOpt};
//! # use figment::Figment;
//! #
//! # #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
//! # /// Some network node configuration
//! # struct Config {
//! #     /// The address to bind to
//! #     #[serde(default = "default_bind_address")]
//! #     bind_address: String,
//! #
//! #     /// Maximum number of connections
//! #     #[serde(default = "default_max_connections")]
//! #     max_connections: usize,
//! #
//! #     /// SSL certificate file path
//! #     cert_pem: String,
//! #
//! #     /// Enable debug logging
//! #     #[serde(default)]
//! #     debug: bool,
//! #
//! #     #[serde(skip)]
//! #     figment: Option<Figment>,
//! # }
//! #
//! # fn default_bind_address() -> String {
//! #     "0.0.0.0:8080".to_string()
//! # }
//! #
//! # fn default_max_connections() -> usize {
//! #     100
//! # }
//! #
//! # impl LayeredConfig for Config {
//! #     fn with_figment(mut self, figment: Figment) -> Self {
//! #         self.figment = Some(figment);
//! #         self
//! #     }
//! #
//! #     fn figment(&self) -> &Option<Figment> {
//! #         &self.figment
//! #     }
//! # }
//! #
//! # impl Default for Config {
//! #     fn default() -> Self {
//! #         Self {
//! #             bind_address: default_bind_address(),
//! #             max_connections: default_max_connections(),
//! #             cert_pem: String::new(),
//! #             debug: false,
//! #             figment: None,
//! #         }
//! #     }
//! # }
//! #
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Using the same Config from Quick Start
//! let config = Config::default();
//! let config_template = config.to_toml()?;
//! println!("{}", config_template);
//! # Ok(())
//! # }
//! ```
//!
//! Output:
//! ```toml
//! # Config
//! #
//! # Some network node configuration
//! #
//! # Fields:
//! # - bind_address: The address to bind to
//! # - max_connections: Maximum number of connections
//! # - cert_pem: SSL certificate file path
//! # - debug: Enable debug logging
//!
//! bind_address = "0.0.0.0:8080"
//! max_connections = 100
//! cert_pem = ""
//! debug = false
//! ```
//!
//! [`figment`]: https://docs.rs/figment
//! [`miette`]: https://docs.rs/miette
//! [`documented`]: https://docs.rs/documented

use std::sync::Arc;

use documented::{Documented, DocumentedFieldsOpt};
use figment::{Figment, Provider, Source};
use miette::{Diagnostic, NamedSource, SourceSpan};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Comprehensive error type for configuration operations
///
/// This error type provides detailed diagnostics for configuration problems,
/// including source location information and context about where the error
/// occurred in the configuration loading process.
///
/// The error type integrates with the [`miette`] diagnostic framework to
/// provide rich error messages with source code context.
#[derive(Error, Debug, Diagnostic)]
pub enum ConfigError {
    /// Configuration error with metadata about source location
    ///
    /// This variant provides rich diagnostic information including:
    /// - The configuration option that caused the error
    /// - The problematic value
    /// - Source location (file, line, column) if available
    /// - The underlying error cause
    #[error("Configuration error")]
    #[diagnostic(help("Check the configuration"))]
    WithMetadata {
        /// The configuration option name (e.g., "bind_address")
        option: String,
        /// The problematic value from the configuration
        value: String,
        /// Source location span for highlighting the error
        #[label("configuration value")]
        span: Option<SourceSpan>,
        /// The source code context for diagnostics
        #[source_code]
        source: Arc<NamedSource<String>>,
        /// The underlying error that occurred
        #[source]
        inner: Box<ConfigError>,
    },
    /// I/O error when loading configuration files
    ///
    /// This occurs when configuration files cannot be read due to
    /// filesystem issues, permissions, or missing files.
    #[error("Failed to load")]
    Load(#[from] std::io::Error),
    /// Error from the figment configuration library
    ///
    /// This includes parsing errors, type conversion failures,
    /// and other configuration processing issues.
    #[error("Failed to build config")]
    Figment(#[from] Box<figment::Error>),
    /// Error serializing configuration to TOML format
    ///
    /// This occurs when calling [`LayeredConfig::to_toml()`] and
    /// the configuration cannot be serialized.
    #[error("Failed to serialize configuration")]
    Toml(#[from] Box<toml::ser::Error>),
    /// Semantic validation error produced by configuration checks.
    #[error("Invalid configuration: {0}")]
    #[diagnostic(help("Update the configuration value to satisfy validation rules"))]
    Invalid(String),
}

impl From<figment::Error> for ConfigError {
    fn from(error: figment::Error) -> Self {
        ConfigError::Figment(Box::new(error))
    }
}

impl From<toml::ser::Error> for ConfigError {
    fn from(error: toml::ser::Error) -> Self {
        ConfigError::Toml(Box::new(error))
    }
}

impl ConfigError {
    /// Create an error wrapper function that enhances errors with configuration metadata
    ///
    /// This method returns a closure that can be used with `map_err` to transform
    /// basic errors into rich configuration errors with source location information.
    /// When metadata is available, it creates a `WithMetadata` variant that includes
    /// the configuration option name, value, (and source location).
    pub fn with_metadata<E>(
        metadata: &Option<(String, String, Source)>,
    ) -> impl Fn(E) -> ConfigError
    where
        E: Into<ConfigError>,
    {
        move |error: E| {
            let error = error.into();

            if let Some((option, value, source)) = metadata {
                let (named_source, span) = match &source {
                    Source::File(path) => {
                        let desc = path.display().to_string();
                        match std::fs::read_to_string(path) {
                            Ok(content) => {
                                let span = find_config_span(&content, option, value);
                                (NamedSource::new(desc, content), span)
                            }
                            Err(_) => (NamedSource::new(desc, String::new()), None),
                        }
                    }
                    Source::Code(loc) => (
                        NamedSource::new(format!("code at {loc}"), String::new()),
                        None,
                    ),
                    Source::Custom(s) => (NamedSource::new(s.clone(), String::new()), None),
                    _ => (NamedSource::new("unknown source", String::new()), None),
                };

                return ConfigError::WithMetadata {
                    option: option.clone(),
                    value: value.clone(),
                    source: Arc::new(named_source),
                    span,
                    inner: Box::new(error),
                };
            }

            error
        }
    }
}

/// Wrapper that pairs a config with its Figment source for metadata.
pub struct ConfigWithMetadata<T> {
    /// The extracted configuration.
    pub config: T,
    figment: Figment,
}

impl<T> std::ops::Deref for ConfigWithMetadata<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl<T> ConfigWithMetadata<T> {
    /// Find metadata for a configuration field (name, value, source).
    pub fn find_metadata(&self, option: &str) -> Option<(String, String, Source)> {
        let option = option.to_owned();

        if let Ok(value) = self.figment.find_value(&option)
            && let Some(metadata) = self.figment.get_metadata(value.tag())
        {
            return Some((
                option,
                value.into_string().unwrap_or("".to_string()),
                metadata
                    .source
                    .clone()
                    .unwrap_or(Source::Custom("unknown".to_string())),
            ));
        }

        None
    }

    /// Validate the configuration.
    pub fn validate(self) -> Result<Self, ConfigError>
    where
        T: ValidatableConfig,
    {
        T::validate(&self)?;

        Ok(self)
    }
}

/// Optional validation hook for configuration structs.
///
/// Types implementing this trait can perform semantic validation using the full
/// [`ConfigWithMetadata`] wrapper, allowing them to attach rich diagnostics via
/// [`ConfigError::with_metadata`].
pub trait ValidatableConfig {
    /// Validate the configuration, returning an error when a constraint is violated.
    fn validate(_cfg: &ConfigWithMetadata<Self>) -> Result<(), ConfigError>
    where
        Self: Sized,
    {
        Ok(())
    }
}

/// Provide TLS-related config (PEM paths) for a configuration type.
pub trait TLSConfig {
    /// Path to the certificate PEM file.
    fn cert_pem_path(&self) -> &std::path::Path;
    /// Path to the private key PEM file.
    fn key_pem_path(&self) -> &std::path::Path;
    /// Path to the trust bundle PEM file.
    fn trust_pem_path(&self) -> &std::path::Path;
    /// Optional path to the certificate revocation list PEM file.
    fn crls_pem_path(&self) -> Option<&std::path::Path>;
}

/// TLS-related loading helpers for `ConfigWithMetadata<T>`.
pub trait ConfigWithMetadataTLSExt {
    /// Load certificate chain from the configured certificate PEM file.
    fn load_cert_chain(&self) -> Result<Vec<hypha_network::CertificateDer<'static>>, ConfigError>;
    /// Load private key from the configured key PEM file.
    fn load_key(&self) -> Result<hypha_network::PrivateKeyDer<'static>, ConfigError>;
    /// Load CA certificates from the configured trust bundle PEM file.
    fn load_trust_chain(&self) -> Result<Vec<hypha_network::CertificateDer<'static>>, ConfigError>;
    /// Load certificate revocation lists from the optional CRLs PEM file.
    fn load_crls(
        &self,
    ) -> Result<Vec<hypha_network::CertificateRevocationListDer<'static>>, ConfigError>;
}

impl<T> ConfigWithMetadataTLSExt for ConfigWithMetadata<T>
where
    T: TLSConfig,
{
    fn load_cert_chain(&self) -> Result<Vec<hypha_network::CertificateDer<'static>>, ConfigError> {
        use std::fs;
        let metadata = self.find_metadata("cert_pem");

        hypha_network::cert::load_certs_from_pem(
            &fs::read(self.config.cert_pem_path())
                .map_err(ConfigError::with_metadata(&metadata))?,
        )
        .map_err(ConfigError::with_metadata(&metadata))
    }

    fn load_key(&self) -> Result<hypha_network::PrivateKeyDer<'static>, ConfigError> {
        use std::fs;
        let metadata = self.find_metadata("key_pem");

        hypha_network::cert::load_private_key_from_pem(
            &fs::read(self.config.key_pem_path()).map_err(ConfigError::with_metadata(&metadata))?,
        )
        .map_err(ConfigError::with_metadata(&metadata))
    }

    fn load_trust_chain(&self) -> Result<Vec<hypha_network::CertificateDer<'static>>, ConfigError> {
        use std::fs;
        let metadata = self.find_metadata("trust_pem");

        hypha_network::cert::load_certs_from_pem(
            &fs::read(self.config.trust_pem_path())
                .map_err(ConfigError::with_metadata(&metadata))?,
        )
        .map_err(ConfigError::with_metadata(&metadata))
    }

    fn load_crls(
        &self,
    ) -> Result<Vec<hypha_network::CertificateRevocationListDer<'static>>, ConfigError> {
        use std::fs;
        let metadata = self.find_metadata("crls_pem");

        if let Some(crl_file) = self.config.crls_pem_path() {
            return hypha_network::cert::load_crls_from_pem(
                &fs::read(crl_file).map_err(ConfigError::with_metadata(&metadata))?,
            )
            .map_err(ConfigError::with_metadata(&metadata));
        }

        Ok(vec![])
    }
}

/// Convert a configuration to documented TOML text.
#[allow(clippy::result_large_err)]
pub fn to_toml<T>(config: &T) -> Result<String, ConfigError>
where
    T: Serialize + Documented + DocumentedFieldsOpt,
{
    let body = toml::to_string_pretty(config)?;

    let mut header = format!("# Config\n# \n# {}\n# \n# Fields:\n", T::DOCS);
    for field in T::FIELD_NAMES {
        if let Ok(docs) = T::get_field_docs(field) {
            header.push_str(&format!("# - {field}: {docs}\n"));
        }
    }
    header.push('\n');
    Ok(header + &body)
}

/// Builder for creating layered configurations from multiple sources.
pub struct LayeredConfigBuilder<T> {
    pub(crate) figment: Figment,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> LayeredConfigBuilder<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    /// Create a new builder with an empty figment.
    pub fn new() -> Self {
        Self {
            figment: Figment::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Add a configuration provider to the builder.
    pub fn with_provider<P: Provider>(mut self, provider: P) -> Self {
        self.figment = self.figment.merge(provider);
        self
    }

    /// Build the final configuration and wrap with metadata source.
    // #[allow(clippy::result_large_err)]
    pub fn build(self) -> Result<ConfigWithMetadata<T>, ConfigError> {
        let config: T = self.figment.extract()?;

        Ok(ConfigWithMetadata {
            config,
            figment: self.figment,
        })
    }
}

impl<T> Default for LayeredConfigBuilder<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience free function to create a builder for `T`.
pub fn builder<T>() -> LayeredConfigBuilder<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    LayeredConfigBuilder::new()
}

/// Find the source span for a configuration field and value in text content
///
/// This internal function searches through configuration file content to locate
/// the specific line containing both the field name and value, returning
/// a source span that can be used for error highlighting in diagnostic messages.
///
/// NOTE: The implementation assumes that the field and value are in the same line, which will
/// probably hold true in most cases but not all. It's deliberately user a super simple mathc and
/// does not make any assumptions on how the value is assigned or formatted, such that it should
/// work for different file formats like TOML, YAML, or JSON.
pub(crate) fn find_config_span(content: &str, field: &str, value: &str) -> Option<SourceSpan> {
    let mut byte_offset = 0;

    for line in content.lines() {
        if line.contains(field) && line.contains(value) {
            let line_end = byte_offset + line.len();
            return Some(SourceSpan::from(byte_offset..line_end));
        }
        byte_offset += line.len() + 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use std::io;

    use figment::{
        Source,
        providers::{Format, Toml},
    };
    use tempfile::NamedTempFile;

    use super::*;

    /// Test configuration for unit tests
    #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt, Default)]
    struct TestConfig {
        /// First test field
        #[serde(default)]
        field1: String,
        /// Second test field
        #[serde(default)]
        field2: i32,
    }

    impl ValidatableConfig for TestConfig {}

    #[test]
    fn builder_with_toml_provider_loads_configuration() {
        const TOML_DATA: &str = r#"
            field1 = "from_toml"
            field2 = 999
        "#;

        let cfg = builder::<TestConfig>()
            .with_provider(Toml::string(TOML_DATA))
            .build()
            .expect("Should build config with TOML provider");

        assert_eq!(cfg.field1, "from_toml");
        assert_eq!(cfg.field2, 999);
    }

    #[test]
    fn empty_builder_produces_config_with_serde_defaults() {
        let cfg = builder::<TestConfig>()
            .build()
            .expect("Should build config with empty builder");

        assert_eq!(cfg.field1, "");
        assert_eq!(cfg.field2, 0);
    }

    #[test]
    fn malformed_toml_with_invalid_syntax_produces_figment_error() {
        const INVALID_TOML: &str = r#"
            field1 = "unclosed_string
            field2 = 123
        "#;

        let result = builder::<TestConfig>()
            .with_provider(Toml::string(INVALID_TOML))
            .build();

        assert!(matches!(result, Err(ConfigError::Figment(_))));
    }

    #[test]
    fn find_metadata_returns_field_details_when_figment_exists() {
        let toml_data = r#"
            field1 = "metadata_test"
            field2 = 777
        "#;

        let cfg = builder::<TestConfig>()
            .with_provider(Toml::string(toml_data))
            .build()
            .unwrap();

        let metadata = cfg.find_metadata("field1");
        assert!(metadata.is_some());

        let (option, value, _source) = metadata.unwrap();
        assert_eq!(option, "field1");
        assert_eq!(value, "metadata_test");
    }

    #[test]
    fn to_toml_includes_field_documentation() {
        let toml_output = to_toml(&TestConfig::default()).unwrap();

        assert!(toml_output.contains("# Config"));
        assert!(toml_output.contains("# Fields:"));
        assert!(toml_output.contains("field1 = \"\""));
    }

    #[test]
    fn find_config_span_locates_field_value_in_content() {
        let content = r#"
        field1 = "value1"
        field2 = 123
        "#;

        let span = find_config_span(content, "field1", "value1");
        assert!(span.is_some());

        let span = find_config_span(content, "field2", "123");
        assert!(span.is_some());

        let span = find_config_span(content, "nonexistent", "value");
        assert!(span.is_none());
    }

    #[test]
    fn io_error_converts_to_config_load_error() {
        let io_error = io::Error::new(io::ErrorKind::NotFound, "File not found");
        let config_error = ConfigError::from(io_error);

        assert!(
            matches!(config_error, ConfigError::Load(_)),
            "Expected ConfigError::Load variant"
        );
    }

    #[test]
    fn error_with_no_metadata_preserves_original_error() {
        let io_error = io::Error::new(io::ErrorKind::PermissionDenied, "Access denied");
        let config_error = ConfigError::with_metadata(&None)(io_error);

        if let ConfigError::Load(inner) = &config_error {
            assert_eq!(inner.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(inner.to_string(), "Access denied");
        } else {
            panic!("Expected ConfigError::Load variant");
        }
    }

    #[test]
    fn error_with_file_metadata_creates_rich_diagnostic() {
        let temp_file = NamedTempFile::new().expect("Should create temp file");
        let temp_path = temp_file
            .path()
            .to_str()
            .expect("Path should be valid UTF-8");

        std::fs::write(&temp_file, r#"field1 = "test_value""#).unwrap();

        let io_error = io::Error::new(io::ErrorKind::InvalidData, "Invalid data");
        let metadata = Some((
            "field1".to_string(),
            "test_value".to_string(),
            Source::File(temp_file.path().to_path_buf()),
        ));
        let config_error = ConfigError::with_metadata(&metadata)(io_error);

        if let ConfigError::WithMetadata {
            option,
            value,
            source,
            span,
            ..
        } = &config_error
        {
            assert_eq!(option, "field1");
            assert_eq!(value, "test_value");

            assert!(
                source.name().contains(temp_path),
                "Source should reference temp file path"
            );
            assert!(span.is_some(), "Should have a span");
        } else {
            panic!("Expected ConfigError::WithMetadata variant")
        }
    }

    #[test]
    fn error_with_custom_metadata_preserves_custom_source() {
        let io_error = io::Error::new(io::ErrorKind::InvalidInput, "Invalid input");
        let metadata = Some((
            "custom_field".to_string(),
            "custom_value".to_string(),
            Source::Custom("custom_source".to_string()),
        ));
        let config_error = ConfigError::with_metadata(&metadata)(io_error);

        if let ConfigError::WithMetadata {
            option,
            value,
            source,
            span,
            ..
        } = &config_error
        {
            assert_eq!(option, "custom_field");
            assert_eq!(value, "custom_value");
            assert_eq!(config_error.to_string(), "Configuration error");
            assert_eq!(source.name(), "custom_source");
            assert!(span.is_none(), "Custom source should have no span");
        } else {
            panic!("Expected ConfigError::WithMetadata variant")
        }
    }

    #[test]
    fn config_error_with_metadata_handles_missing_source_file() {
        let nonexistent_path = std::path::PathBuf::from("/nonexistent/file.toml");
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "File not found");
        let metadata = Some((
            "field1".to_string(),
            "test_value".to_string(),
            Source::File(nonexistent_path.clone()),
        ));

        let error_fn = ConfigError::with_metadata(&metadata);
        let config_error = error_fn(io_error);

        if let ConfigError::WithMetadata {
            option,
            value,
            source,
            span,
            ..
        } = config_error
        {
            assert_eq!(option, "field1");
            assert_eq!(value, "test_value");
            let nonexistent_str = nonexistent_path
                .to_str()
                .expect("Path should be valid UTF-8");
            assert!(
                source.name().contains(nonexistent_str),
                "Source should reference nonexistent path"
            );
            assert!(
                span.is_none(),
                "Should have no span when file can't be read"
            );
        } else {
            panic!("Expected ConfigError::WithMetadata variant");
        }
    }

    #[test]
    fn find_config_span_handles_special_characters() {
        let content = r#"
field1 = "value with spaces and símbolos"
field2 = 123
special_field = "!@#$%^&*()"
"#;

        let span = find_config_span(content, "special_field", "!@#$%^&*()");
        assert!(span.is_some());

        let span = find_config_span(content, "field1", "value with spaces and símbolos");
        assert!(span.is_some());
    }

    #[test]
    fn find_config_span_handles_empty_content() {
        let empty_content = "";
        let span = find_config_span(empty_content, "field1", "value");
        assert!(span.is_none());
    }

    #[test]
    fn find_config_span_handles_whitespace_only_content() {
        let whitespace_content = "   \n\t  \n  ";
        let span = find_config_span(whitespace_content, "field1", "value");
        assert!(span.is_none());
    }
}
