use std::{fs, path::PathBuf, process::Command};

use tempfile::TempDir;

/// Integration tests for the CLI init command's --name parameter functionality
///
/// These tests verify that the hypha-scheduler binary correctly:
/// 1. Parses the --name parameter
/// 2. Creates configurations with the correct certificate filenames
/// 3. Generates appropriate output filenames when --output is not specified
/// 4. Writes valid TOML configuration files
#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Get the path to the hypha-scheduler binary
    fn get_binary_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop(); // Go up from crates/scheduler to crates
        path.pop(); // Go up from crates to root
        path.push("target");
        path.push("debug");
        path.push("hypha-scheduler");
        path
    }

    #[test]
    fn init_with_name_parameter_creates_config_with_correct_certificate_names() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("test-scheduler")
            .arg("--output")
            .arg(temp_path.join("test-config.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("test-config.toml"))
            .expect("Failed to read config file");

        // Verify that the certificate file names use the specified name prefix
        assert!(
            config_content.contains("cert_pem = \"test-scheduler-cert.pem\""),
            "Config should contain test-scheduler-cert.pem, got: {}",
            config_content
        );
        assert!(
            config_content.contains("key_pem = \"test-scheduler-key.pem\""),
            "Config should contain test-scheduler-key.pem, got: {}",
            config_content
        );
        assert!(
            config_content.contains("trust_pem = \"test-scheduler-trust.pem\""),
            "Config should contain test-scheduler-trust.pem, got: {}",
            config_content
        );
    }

    #[test]
    fn init_with_name_and_no_output_uses_default_filename_pattern() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("my-scheduler")
            .current_dir(temp_path)
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Verify that the default output filename was used
        let expected_filename = temp_path.join("my-scheduler-config.toml");
        assert!(
            expected_filename.exists(),
            "Expected config file {} should exist",
            expected_filename.display()
        );

        let config_content =
            fs::read_to_string(&expected_filename).expect("Failed to read config file");

        // Verify content matches the name
        assert!(config_content.contains("cert_pem = \"my-scheduler-cert.pem\""));
        assert!(config_content.contains("key_pem = \"my-scheduler-key.pem\""));
        assert!(config_content.contains("trust_pem = \"my-scheduler-trust.pem\""));
    }

    #[test]
    fn init_with_default_name_uses_scheduler_prefix() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--output")
            .arg(temp_path.join("default-config.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("default-config.toml"))
            .expect("Failed to read config file");

        // When no --name is provided, it should use the default value "scheduler"
        assert!(config_content.contains("cert_pem = \"scheduler-cert.pem\""));
        assert!(config_content.contains("key_pem = \"scheduler-key.pem\""));
        assert!(config_content.contains("trust_pem = \"scheduler-trust.pem\""));
    }

    #[test]
    fn init_creates_valid_toml_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("validation-test")
            .arg("--output")
            .arg(temp_path.join("validation.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("validation.toml"))
            .expect("Failed to read config file");

        // Verify the file is valid TOML
        let parsed_toml: toml::Table = config_content
            .parse()
            .expect("Generated config should be valid TOML");

        // Verify essential fields exist
        assert!(parsed_toml.contains_key("cert_pem"));
        assert!(parsed_toml.contains_key("key_pem"));
        assert!(parsed_toml.contains_key("trust_pem"));
        assert!(parsed_toml.contains_key("gateway_addresses"));
        assert!(parsed_toml.contains_key("listen_addresses"));

        // Verify the certificate fields have the expected values
        assert_eq!(
            parsed_toml["cert_pem"].as_str().unwrap(),
            "validation-test-cert.pem"
        );
        assert_eq!(
            parsed_toml["key_pem"].as_str().unwrap(),
            "validation-test-key.pem"
        );
        assert_eq!(
            parsed_toml["trust_pem"].as_str().unwrap(),
            "validation-test-trust.pem"
        );
    }
}
