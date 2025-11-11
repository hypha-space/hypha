use std::{fs, path::PathBuf, process::Command};

use tempfile::TempDir;

/// Integration tests for the CLI init command's --name parameter functionality
///
/// These tests verify that the hypha-gateway binary correctly:
/// 1. Parses the --name parameter
/// 2. Creates configurations with the correct certificate filenames
/// 3. Generates appropriate output filenames when --output is not specified
/// 4. Writes valid TOML configuration files
#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Get the path to the hypha-gateway binary
    fn get_binary_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop(); // Go up from crates/gateway to crates
        path.pop(); // Go up from crates to root
        path.push("target");
        path.push("debug");
        path.push("hypha-gateway");
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
            .arg("test-gateway")
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
            config_content.contains("cert_pem = \"test-gateway-cert.pem\""),
            "Config should contain test-gateway-cert.pem, got: {}",
            config_content
        );
        assert!(
            config_content.contains("key_pem = \"test-gateway-key.pem\""),
            "Config should contain test-gateway-key.pem, got: {}",
            config_content
        );
        assert!(
            config_content.contains("trust_pem = \"test-gateway-trust.pem\""),
            "Config should contain test-gateway-trust.pem, got: {}",
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
            .arg("my-gateway")
            .current_dir(temp_path)
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Verify that the default output filename was used
        let expected_filename = temp_path.join("my-gateway-config.toml");
        assert!(
            expected_filename.exists(),
            "Expected config file {} should exist",
            expected_filename.display()
        );

        let config_content =
            fs::read_to_string(&expected_filename).expect("Failed to read config file");

        // Verify content matches the name
        assert!(config_content.contains("cert_pem = \"my-gateway-cert.pem\""));
        assert!(config_content.contains("key_pem = \"my-gateway-key.pem\""));
        assert!(config_content.contains("trust_pem = \"my-gateway-trust.pem\""));
    }

    #[test]
    fn init_with_default_name_uses_gateway_prefix() {
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

        // When no --name is provided, it should use the default value "gateway"
        assert!(config_content.contains("cert_pem = \"gateway-cert.pem\""));
        assert!(config_content.contains("key_pem = \"gateway-key.pem\""));
        assert!(config_content.contains("trust_pem = \"gateway-trust.pem\""));
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

    // Tests for the new --set parameter functionality

    #[test]
    fn init_with_set_string_override() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("test-gateway")
            .arg("--set")
            .arg("cert_pem=custom-certificate.pem")
            .arg("--output")
            .arg(temp_path.join("set-string.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("set-string.toml"))
            .expect("Failed to read config file");

        // Verify the --set override took effect
        assert!(
            config_content.contains("cert_pem = \"custom-certificate.pem\""),
            "Config should contain custom cert_pem, got: {}",
            config_content
        );
    }

    #[test]
    fn init_name_and_set_combination() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("custom-gateway")
            .arg("--set")
            .arg("trust_pem=override-trust.pem")
            .arg("--output")
            .arg(temp_path.join("name-and-set.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("name-and-set.toml"))
            .expect("Failed to read config file");

        // Verify the --name parameter affected cert_pem and key_pem
        assert!(
            config_content.contains("cert_pem = \"custom-gateway-cert.pem\""),
            "Config should contain name-based cert_pem, got: {}",
            config_content
        );
        assert!(
            config_content.contains("key_pem = \"custom-gateway-key.pem\""),
            "Config should contain name-based key_pem, got: {}",
            config_content
        );

        // Verify the --set parameter overrode trust_pem
        assert!(
            config_content.contains("trust_pem = \"override-trust.pem\""),
            "Config should contain override trust_pem, got: {}",
            config_content
        );
    }

    #[test]
    fn init_with_set_multiple_overrides() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("test-gateway")
            .arg("--set")
            .arg("cert_pem=custom-cert.pem")
            .arg("--set")
            .arg("key_pem=custom-key.pem")
            .arg("--set")
            .arg("trust_pem=custom-trust.pem")
            .arg("--output")
            .arg(temp_path.join("set-multiple.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("set-multiple.toml"))
            .expect("Failed to read config file");

        // Verify all --set overrides took effect
        assert!(config_content.contains("cert_pem = \"custom-cert.pem\""));
        assert!(config_content.contains("key_pem = \"custom-key.pem\""));
        assert!(config_content.contains("trust_pem = \"custom-trust.pem\""));
    }

    #[test]
    fn init_with_set_simple_field_override() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("test-gateway")
            .arg("--set")
            .arg("crls_pem=custom-crl.pem")
            .arg("--output")
            .arg(temp_path.join("set-simple.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("set-simple.toml"))
            .expect("Failed to read config file");

        // Verify the override took effect
        assert!(
            config_content.contains("crls_pem = \"custom-crl.pem\""),
            "Config should contain custom CRL, got: {}",
            config_content
        );
    }

    #[test]
    fn init_with_set_boolean_and_number_types() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("test-gateway")
            .arg("--set")
            .arg("telemetry_sample_ratio=0.5")
            .arg("--output")
            .arg(temp_path.join("set-types.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("set-types.toml"))
            .expect("Failed to read config file");

        // Verify numeric type detection works (no quotes around numbers)
        assert!(
            config_content.contains("telemetry_sample_ratio = 0.5"),
            "Float should not have quotes, got: {}",
            config_content
        );
    }

    #[test]
    fn init_with_set_invalid_syntax_fails() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("test-gateway")
            .arg("--set")
            .arg("invalid_no_equals_sign")
            .arg("--output")
            .arg(temp_path.join("should-not-exist.toml"))
            .output()
            .expect("Failed to execute command");

        // Command should fail due to invalid --set syntax
        assert!(
            !output.status.success(),
            "Command should have failed with invalid --set syntax, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Verify no config file was created
        assert!(
            !temp_path.join("should-not-exist.toml").exists(),
            "Config file should not have been created on error"
        );
    }

    #[test]
    fn init_with_set_quotes_in_string_values() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("test-gateway")
            .arg("--set")
            .arg("cert_pem=path/with\"quotes\"/cert.pem")
            .arg("--output")
            .arg(temp_path.join("set-quotes.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content = fs::read_to_string(temp_path.join("set-quotes.toml"))
            .expect("Failed to read config file");

        // NOTE: TOML serializers can use single quotes to avoid escaping double quotes,
        // which is valid TOML. We just verify the value is present correctly.
        assert!(
            config_content.contains("cert_pem = 'path/with\"quotes\"/cert.pem'")
                || config_content.contains("cert_pem = \"path/with\\\"quotes\\\"/cert.pem\""),
            "Quotes should be handled properly in TOML, got: {}",
            config_content
        );
    }

    #[test]
    fn probe_accepts_set_parameters() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        // First create a config file
        let binary_path = get_binary_path();
        let init_output = Command::new(&binary_path)
            .arg("init")
            .arg("--output")
            .arg(temp_path.join("probe-config.toml"))
            .output()
            .expect("Failed to execute init command");

        assert!(init_output.status.success());

        // Test that probe accepts --set parameters syntax (probe will fail to connect but shouldn't fail argument parsing)
        let probe_output = Command::new(&binary_path)
            .arg("probe")
            .arg("--config")
            .arg(temp_path.join("probe-config.toml"))
            .arg("--set")
            .arg("crls_pem=test-crl.pem")
            .arg("--timeout")
            .arg("100") // Quick timeout to fail fast
            .arg("/ip4/127.0.0.1/tcp/1234")
            .output()
            .expect("Failed to execute probe command");

        let stderr = String::from_utf8_lossy(&probe_output.stderr);

        // The probe should fail with a connection-related error, not argument parsing error
        // If it fails due to missing certificate files, that's expected behavior
        assert!(
            stderr.contains("Connection") ||
            stderr.contains("timeout") ||
            stderr.contains("refused") ||
            stderr.contains("No such file or directory") ||  // Certificate files don't exist
            stderr.contains("Configuration error"), // Expected if cert files don't exist
            "Probe should fail due to connection/config issues, not argument parsing. stderr: {}",
            stderr
        );
    }
}
