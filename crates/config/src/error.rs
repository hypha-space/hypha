#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use figment::Source;
use miette::{Diagnostic, NamedSource, SourceSpan};
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

    use figment::Source;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::error::find_config_span;

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
