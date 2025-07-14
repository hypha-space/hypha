#![deny(missing_docs)]
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
//! ```no_run
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
//!     .build()?;
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
//! ```no_run
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
//!     .build()?;
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
//! ```no_run
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

mod error;
mod layered_config;

pub use error::ConfigError;
pub use layered_config::{LayeredConfig, LayeredConfigBuilder};
