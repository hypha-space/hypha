use documented::{Documented, DocumentedFieldsOpt};
use figment::{Figment, Provider, Source};
use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// A trait for configuration types that support layered loading and documentation
///
/// This trait enables configuration structs to be loaded from multiple sources
/// (files, environment variables, command line arguments) while providing enhanced error reporting
/// and documentation generation.
pub trait LayeredConfig:
    Serialize + for<'de> Deserialize<'de> + Documented + DocumentedFieldsOpt
{
    /// Create a new configuration builder
    ///
    /// This is the main entry point for building layered configurations.
    /// The builder allows you to add multiple configuration sources
    /// that will be merged in order.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hypha_config::LayeredConfig;
    /// # use serde::{Deserialize, Serialize};
    /// # use documented::{Documented, DocumentedFieldsOpt};
    /// # use figment::{Figment, providers::{Format, Toml}};
    /// # #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
    /// # /// Example configuration struct
    /// # struct Config {
    /// #     #[serde(skip)]
    /// #     figment: Option<Figment>,
    /// # }
    /// # impl LayeredConfig for Config {
    /// #     fn with_figment(mut self, figment: Figment) -> Self {
    /// #         self.figment = Some(figment);
    /// #         self
    /// #     }
    /// #     fn figment(&self) -> &Option<Figment> {
    /// #         &self.figment
    /// #     }
    /// # }
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = Config::builder()
    ///     .with_provider(Toml::file("config.toml"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    fn builder() -> LayeredConfigBuilder<Self> {
        LayeredConfigBuilder::new()
    }

    /// Associate a figment with this configuration instance
    ///
    /// This method is called internally by the builder to store the figment
    /// that was used to construct this configuration. The figment is needed
    /// for error reporting and metadata lookup.
    ///
    /// NOTE: This method should only be called once during construction
    /// by [`LayeredConfigBuilder::build()`].
    fn with_figment(self, figment: Figment) -> Self;

    /// Get the figment used to build this configuration
    ///
    /// Returns a reference to the figment that was used to construct this
    /// configuration instance. This is used for error reporting and metadata
    /// lookup operations.
    fn figment(&self) -> &Option<Figment>;

    /// Find metadata for a configuration field
    ///
    /// This method looks up metadata for a specific configuration field,
    /// including its value and source location. The metadata is used for
    /// enhanced error reporting when configuration-related operations fail.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hypha_config::LayeredConfig;
    /// # use serde::{Deserialize, Serialize};
    /// # use documented::{Documented, DocumentedFieldsOpt};
    /// # use figment::{Figment, providers::{Format, Toml}};
    /// # #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
    /// # /// Example configuration struct
    /// # struct Config {
    /// #     /// Database connection URL
    /// #     database_url: String,
    /// #     #[serde(skip)]
    /// #     figment: Option<Figment>,
    /// # }
    /// # impl LayeredConfig for Config {
    /// #     fn with_figment(mut self, figment: Figment) -> Self {
    /// #         self.figment = Some(figment);
    /// #         self
    /// #     }
    /// #     fn figment(&self) -> &Option<Figment> {
    /// #         &self.figment
    /// #     }
    /// # }
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = Config::builder()
    ///     .with_provider(Toml::file("config.toml"))
    ///     .build()?;
    ///
    /// if let Some((field, value, source)) = config.find_metadata("database_url") {
    ///     println!("Field '{}' has value '{}' from {:?}", field, value, source);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    fn find_metadata(&self, option: &str) -> Option<(String, String, Source)> {
        if let Some(figment) = self.figment() {
            let option = option.to_owned();

            if let Ok(value) = figment.find_value(&option) {
                if let Some(metadata) = figment.get_metadata(value.tag()) {
                    return Some((
                        option,
                        value.into_string().unwrap_or("".to_string()),
                        metadata
                            .source
                            .clone()
                            .unwrap_or(Source::Custom("unknown".to_string())),
                    ));
                }
            }
        }

        None
    }

    /// Convert the configuration to a documented TOML string
    ///
    /// This method serializes the configuration to TOML format and includes
    /// comprehensive documentation as comments. The output includes:
    /// - Type-level documentation from the struct
    /// - Field-level documentation for each configuration option
    /// - Current values for all fields
    ///
    /// This is particularly useful for generating configuration file templates
    /// or example configurations for users.
    #[allow(clippy::result_large_err)] // Large error types are acceptable for config errors
    fn to_toml(&self) -> Result<String, ConfigError> {
        let body = toml::to_string_pretty(self)?;

        let mut header = format!(
            "\
            # Config\n\
            # \n\
            # {}\n\
            # \n\
            # Fields:\n\
            ",
            Self::DOCS
        );

        // Add field documentation
        for field in Self::FIELD_NAMES {
            if let Ok(docs) = Self::get_field_docs(field) {
                header.push_str(&format!("# - {field}: {docs}\n"));
            }
        }
        header.push('\n');

        Ok(header + &body)
    }
}

/// Builder for creating layered configurations from multiple sources
///
/// This builder allows you to construct configurations by merging multiple
/// providers in order. Later providers override earlier ones, creating a
/// layered configuration system.
pub struct LayeredConfigBuilder<C>
where
    C: LayeredConfig,
{
    figment: Figment,
    _phantom: std::marker::PhantomData<C>,
}

impl<C> LayeredConfigBuilder<C>
where
    C: LayeredConfig,
{
    /// Create a new builder with an empty figment
    ///
    /// This creates a builder with no configuration sources. You'll typically
    /// want to add providers using [`with_provider`] before building.
    ///
    /// [`with_provider`]: LayeredConfigBuilder::with_provider
    pub fn new() -> Self {
        Self {
            figment: Figment::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Add a configuration provider to the builder
    ///
    /// Providers are merged in the order they're added, with later providers
    /// overriding earlier ones. This allows you to create a layered configuration
    /// system where more specific sources override general ones.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use hypha_config::LayeredConfig;
    /// use figment::providers::{Env, Format, Toml};
    /// # use serde::{Deserialize, Serialize};
    /// # use documented::{Documented, DocumentedFieldsOpt};
    /// # use figment::Figment;
    /// # #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
    /// # /// Example configuration struct
    /// # struct Config {
    /// #     #[serde(skip)]
    /// #     figment: Option<Figment>,
    /// # }
    /// # impl LayeredConfig for Config {
    /// #     fn with_figment(mut self, figment: Figment) -> Self {
    /// #         self.figment = Some(figment);
    /// #         self
    /// #     }
    /// #     fn figment(&self) -> &Option<Figment> {
    /// #         &self.figment
    /// #     }
    /// # }
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = Config::builder()
    ///     .with_provider(Toml::file("config.toml"))
    ///     .with_provider(Env::prefixed("APP_"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_provider<P: Provider>(mut self, provider: P) -> Self {
        self.figment = self.figment.merge(provider);
        self
    }

    /// Build the final configuration from all providers
    ///
    /// This method extracts the configuration from the merged figment and
    /// associates the figment with the configuration instance for error reporting.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use hypha_config::LayeredConfig;
    /// use figment::providers::{Format, Toml};
    /// # use serde::{Deserialize, Serialize};
    /// # use documented::{Documented, DocumentedFieldsOpt};
    /// # use figment::Figment;
    /// # #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
    /// # /// Example configuration struct
    /// # struct Config {
    /// #     #[serde(skip)]
    /// #     figment: Option<Figment>,
    /// # }
    /// # impl LayeredConfig for Config {
    /// #     fn with_figment(mut self, figment: Figment) -> Self {
    /// #         self.figment = Some(figment);
    /// #         self
    /// #     }
    /// #     fn figment(&self) -> &Option<Figment> {
    /// #         &self.figment
    /// #     }
    /// # }
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = Config::builder()
    ///     .with_provider(Toml::file("config.toml"))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    #[allow(clippy::result_large_err)] // Large error types are acceptable for config errors
    pub fn build(self) -> Result<C, ConfigError> {
        let config: C = self.figment.extract()?;

        Ok(config.with_figment(self.figment))
    }
}

impl<C> Default for LayeredConfigBuilder<C>
where
    C: LayeredConfig,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use documented::{Documented, DocumentedFieldsOpt};
    use figment::providers::{Format, Toml};
    use serde::{Deserialize, Serialize};
    use tempfile::NamedTempFile;

    use super::*;

    /// Test configuration for unit tests
    #[derive(Debug, Serialize, Deserialize, Documented, DocumentedFieldsOpt)]
    struct TestConfig {
        /// First test field
        #[serde(default)]
        field1: String,
        /// Second test field
        #[serde(default)]
        field2: i32,

        #[serde(skip)]
        figment: Option<Figment>,
    }

    impl PartialEq for TestConfig {
        fn eq(&self, other: &Self) -> bool {
            self.field1 == other.field1 && self.field2 == other.field2
        }
    }

    impl LayeredConfig for TestConfig {
        fn with_figment(mut self, figment: Figment) -> Self {
            self.figment = Some(figment);
            self
        }

        fn figment(&self) -> &Option<Figment> {
            &self.figment
        }
    }

    impl Default for TestConfig {
        fn default() -> Self {
            Self {
                field1: "default".to_string(),
                field2: 42,
                figment: None,
            }
        }
    }

    // LayeredConfig trait tests
    #[test]
    fn find_metadata_returns_none_when_no_figment() {
        let mut config = TestConfig::default();
        config.figment = None;

        let metadata = config.find_metadata("field1");
        assert!(metadata.is_none());
    }

    #[test]
    fn find_metadata_returns_field_details_when_figment_exists() {
        let toml_data = r#"
            field1 = "metadata_test"
            field2 = 777
        "#;

        let config = TestConfig::builder()
            .with_provider(Toml::string(toml_data))
            .build()
            .unwrap();

        let metadata = config.find_metadata("field1");
        assert!(metadata.is_some());

        let (option, value, _source) = metadata.unwrap();
        assert_eq!(option, "field1");
        assert_eq!(value, "metadata_test");
    }

    #[test]
    fn find_metadata_returns_none_for_missing_field() {
        let config = TestConfig::builder().build().unwrap();

        let metadata = config.find_metadata("nonexistent_field");
        assert!(metadata.is_none());
    }

    #[test]
    fn to_toml_serializes_config_with_header() {
        let config = TestConfig {
            field1: "test_value".to_string(),
            field2: 123,
            figment: None,
        };

        let toml_output = config.to_toml().unwrap();

        assert!(toml_output.contains("# Config"));
        assert!(toml_output.contains("# Fields:"));
        assert!(toml_output.contains("field1 = \"test_value\""));
        assert!(toml_output.contains("field2 = 123"));
    }

    #[test]
    fn to_toml_includes_field_documentation() {
        let config = TestConfig::default();
        let toml_output = config.to_toml().unwrap();

        assert!(toml_output.contains("Test configuration for unit tests"));
        assert!(toml_output.contains("First test field"));
        assert!(toml_output.contains("Second test field"));
    }

    #[test]
    fn figment_integration_preserves_extracted_values() {
        let toml_data = r#"
            field1 = "integration_test"
            field2 = 456
        "#;

        let figment = Figment::new().merge(Toml::string(toml_data));
        let config: TestConfig = figment.extract().unwrap();
        let config_with_figment = config.with_figment(figment);

        assert_eq!(config_with_figment.field1, "integration_test");
        assert_eq!(config_with_figment.field2, 456);
        assert!(config_with_figment.figment.is_some());
    }

    #[test]
    fn empty_config_file_produces_valid_config_with_defaults() {
        let temp_file = NamedTempFile::new().expect("Should create temp file");
        std::fs::write(&temp_file, "").expect("Should write empty content");

        let config = TestConfig::builder()
            .with_provider(Toml::file(temp_file.path()))
            .build()
            .expect("Should build config from empty file");

        assert_eq!(config.field1, "");
        assert_eq!(config.field2, 0);
    }

    // LayeredConfigBuilder tests
    #[test]
    fn new_builder_creates_empty_figment() {
        let builder = LayeredConfigBuilder::<TestConfig>::new();
        assert_eq!(builder.figment.metadata().count(), 0);
    }

    #[test]
    fn default_builder_creates_empty_figment() {
        let builder = LayeredConfigBuilder::<TestConfig>::default();
        assert_eq!(builder.figment.metadata().count(), 0);
    }

    #[test]
    fn builder_with_toml_provider_loads_configuration() {
        const TOML_DATA: &str = r#"
            field1 = "from_toml"
            field2 = 999
        "#;

        let config = TestConfig::builder()
            .with_provider(Toml::string(TOML_DATA))
            .build()
            .expect("Should build config with TOML provider");

        assert_eq!(config.field1, "from_toml");
        assert_eq!(config.field2, 999);
        assert!(
            config.figment.is_some(),
            "Config should have associated figment"
        );
    }

    #[test]
    fn empty_builder_produces_config_with_serde_defaults() {
        let config = TestConfig::builder()
            .build()
            .expect("Should build config with empty builder");

        // Empty figment extracts to serde defaults (empty string, 0)
        assert_eq!(config.field1, "", "field1 should default to empty string");
        assert_eq!(config.field2, 0, "field2 should default to zero");
        assert!(
            config.figment.is_some(),
            "Config should have associated figment"
        );
    }

    #[test]
    fn malformed_toml_with_invalid_syntax_produces_figment_error() {
        const INVALID_TOML: &str = r#"
            field1 = "unclosed_string
            field2 = 123
        "#;

        let result = TestConfig::builder()
            .with_provider(Toml::string(INVALID_TOML))
            .build();

        assert!(
            matches!(result, Err(ConfigError::Figment(_))),
            "Expected Figment error for invalid syntax"
        );
    }

    #[test]
    fn malformed_toml_with_duplicate_keys_produces_figment_error() {
        const DUPLICATE_KEYS_TOML: &str = r#"
            field1 = "first_value"
            field1 = "second_value"
            field2 = 123
        "#;

        let result = TestConfig::builder()
            .with_provider(Toml::string(DUPLICATE_KEYS_TOML))
            .build();

        assert!(
            matches!(result, Err(ConfigError::Figment(_))),
            "Expected Figment error for invalid syntax"
        );
    }

    #[test]
    fn malformed_toml_with_invalid_unicode_produces_figment_error() {
        const INVALID_UNICODE_TOML: &str = "field1 = \"\\u{D800}\"";
        let result = TestConfig::builder()
            .with_provider(Toml::string(INVALID_UNICODE_TOML))
            .build();

        assert!(
            matches!(result, Err(ConfigError::Figment(_))),
            "Expected Figment error for invalid syntax"
        );
    }

    #[test]
    fn malformed_toml_with_invalid_number_format_produces_figment_error() {
        const INVALID_NUMBER_TOML: &str = r#"
            field1 = "valid_string"
            field2 = 123.456.789
        "#;

        let result = TestConfig::builder()
            .with_provider(Toml::string(INVALID_NUMBER_TOML))
            .build();

        assert!(
            matches!(result, Err(ConfigError::Figment(_))),
            "Expected Figment error for invalid syntax"
        );
    }

    #[test]
    fn malformed_toml_with_unmatched_brackets_produces_figment_error() {
        const UNMATCHED_BRACKETS_TOML: &str = r#"
            field1 = "valid_string"
            [incomplete_table
            field2 = 123
        "#;

        let result = TestConfig::builder()
            .with_provider(Toml::string(UNMATCHED_BRACKETS_TOML))
            .build();

        assert!(
            matches!(result, Err(ConfigError::Figment(_))),
            "Expected Figment error for invalid syntax"
        );
    }

    #[test]
    fn toml_with_wrong_type_for_field_produces_figment_error() {
        const WRONG_TYPE_TOML: &str = r#"
            field1 = "valid_string"
            field2 = "not_a_number"
        "#;

        let result = TestConfig::builder()
            .with_provider(Toml::string(WRONG_TYPE_TOML))
            .build();

        assert!(
            matches!(result, Err(ConfigError::Figment(_))),
            "Expected Figment error for invalid syntax"
        );
    }

    #[test]
    fn multiple_providers_with_precedence_override_correctly() {
        const BASE_TOML: &str = r#"
            field1 = "base_value"
            field2 = 100
        "#;
        const OVERRIDE_TOML: &str = r#"
            field1 = "override_value"
        "#;

        let config = TestConfig::builder()
            .with_provider(Toml::string(BASE_TOML))
            .with_provider(Toml::string(OVERRIDE_TOML))
            .build()
            .expect("Should build config with multiple providers");

        assert_eq!(
            config.field1, "override_value",
            "field1 should be overridden by second provider"
        );
        assert_eq!(
            config.field2, 100,
            "field2 should remain from base provider"
        );
    }
}
