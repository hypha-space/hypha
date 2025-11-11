use std::{fs, path::PathBuf, process::Command};

use tempfile::TempDir;

/// Integration tests for the CLI init command functionality.
///
/// These tests verify that the hypha-scheduler binary correctly:
/// 1. Handles --name parameter for certificate naming
/// 2. Supports --set parameter for configuration overrides
/// 3. Generates valid TOML configuration files
/// 4. Works with complex nested structures and array notation
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

    // ===== Basic Configuration Tests =====

    #[test]
    fn init_creates_valid_toml_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--output")
            .arg(temp_path.join("basic.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let config_content =
            fs::read_to_string(temp_path.join("basic.toml")).expect("Failed to read config file");

        // Verify the file is valid TOML with essential fields
        let parsed_toml: toml::Table = config_content
            .parse()
            .expect("Generated config should be valid TOML");

        assert!(parsed_toml.contains_key("cert_pem"));
        assert!(parsed_toml.contains_key("key_pem"));
        assert!(parsed_toml.contains_key("trust_pem"));
        assert!(parsed_toml.contains_key("gateway_addresses"));
        assert!(parsed_toml.contains_key("listen_addresses"));
    }

    #[test]
    fn init_with_default_name_uses_scheduler_prefix() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--output")
            .arg(temp_path.join("default-name.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("default-name.toml"))
            .expect("Failed to read config file");

        // When no --name is provided, should use default "scheduler" prefix
        assert!(config_content.contains("cert_pem = \"scheduler-cert.pem\""));
        assert!(config_content.contains("key_pem = \"scheduler-key.pem\""));
        assert!(config_content.contains("trust_pem = \"scheduler-trust.pem\""));
    }

    // ===== --name Parameter Tests =====

    #[test]
    fn init_with_custom_name_parameter() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("custom-scheduler")
            .arg("--output")
            .arg(temp_path.join("custom-name.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("custom-name.toml"))
            .expect("Failed to read config file");

        // Verify certificate names use the custom prefix
        assert!(config_content.contains("cert_pem = \"custom-scheduler-cert.pem\""));
        assert!(config_content.contains("key_pem = \"custom-scheduler-key.pem\""));
        assert!(config_content.contains("trust_pem = \"custom-scheduler-trust.pem\""));
    }

    #[test]
    fn init_with_name_and_no_output_creates_default_filename() {
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

        assert!(output.status.success());

        // Verify the default output filename pattern was used
        let expected_filename = temp_path.join("my-scheduler-config.toml");
        assert!(expected_filename.exists());

        let config_content =
            fs::read_to_string(&expected_filename).expect("Failed to read config file");

        assert!(config_content.contains("cert_pem = \"my-scheduler-cert.pem\""));
        assert!(config_content.contains("key_pem = \"my-scheduler-key.pem\""));
        assert!(config_content.contains("trust_pem = \"my-scheduler-trust.pem\""));
    }

    // ===== Basic --set Parameter Tests =====

    #[test]
    fn init_with_set_string_override() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--set")
            .arg("cert_pem=custom-certificate.pem")
            .arg("--output")
            .arg(temp_path.join("set-string.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("set-string.toml"))
            .expect("Failed to read config file");

        assert!(config_content.contains("cert_pem = \"custom-certificate.pem\""));
    }

    #[test]
    fn init_with_set_numeric_types() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--set")
            .arg("telemetry_sample_ratio=0.9")
            .arg("--output")
            .arg(temp_path.join("set-numeric.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("set-numeric.toml"))
            .expect("Failed to read config file");

        // Verify numeric values don't have quotes
        assert!(config_content.contains("telemetry_sample_ratio = 0.9"));
    }

    #[test]
    fn init_with_multiple_set_overrides() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--set")
            .arg("cert_pem=custom-cert.pem")
            .arg("--set")
            .arg("key_pem=custom-key.pem")
            .arg("--set")
            .arg("trust_pem=custom-trust.pem")
            .arg("--output")
            .arg(temp_path.join("multiple-sets.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("multiple-sets.toml"))
            .expect("Failed to read config file");

        assert!(config_content.contains("cert_pem = \"custom-cert.pem\""));
        assert!(config_content.contains("key_pem = \"custom-key.pem\""));
        assert!(config_content.contains("trust_pem = \"custom-trust.pem\""));
    }

    // ===== Nested Structure Tests =====

    // NOTE: Updated paths to match current configuration structure after refactoring.
    // The scheduler configuration now uses scheduler.job.model.* instead of scheduler.model.*
    // and scheduler.job.dataset.* instead of scheduler.dataset.* due to the Job enum wrapper.
    #[test]
    fn init_with_nested_scheduler_configuration() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--set")
            .arg("scheduler.job.model.repository=hypha-space/custom-model")
            .arg("--set")
            .arg("scheduler.job.model.revision=main")
            .arg("--set")
            .arg("scheduler.job.dataset.dataset=custom-dataset")
            .arg("--output")
            .arg(temp_path.join("nested-config.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("nested-config.toml"))
            .expect("Failed to read config file");

        assert!(config_content.contains("repository = \"hypha-space/custom-model\""));
        assert!(config_content.contains("revision = \"main\""));
        assert!(config_content.contains("dataset = \"custom-dataset\""));
    }

    // ===== Array Index Notation Tests =====

    #[test]
    fn init_with_array_index_notation() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--set")
            .arg("scheduler.job.model.repository=hypha-space/lenet")
            .arg("--set")
            .arg("scheduler.job.model.filenames.0=config.json")
            .arg("--set")
            .arg("scheduler.job.model.filenames.1=model.safetensors")
            .arg("--set")
            .arg("scheduler.job.model.filenames.2=custom-file.py")
            .arg("--output")
            .arg(temp_path.join("array-notation.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("array-notation.toml"))
            .expect("Failed to read config file");

        assert!(config_content.contains("repository = \"hypha-space/lenet\""));
        assert!(config_content.contains("\"config.json\""));
        assert!(config_content.contains("\"model.safetensors\""));
        assert!(config_content.contains("\"custom-file.py\""));
        assert!(config_content.contains("filenames = ["));
    }

    #[test]
    fn init_with_complex_model_configuration() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        // Test the original failing command that now works
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--set")
            .arg("scheduler.job.model.repository=hypha-space/lenet")
            .arg("--set")
            .arg("scheduler.job.model.type=vision-classification")
            .arg("--set")
            .arg("scheduler.job.model.filenames.0=config.json")
            .arg("--set")
            .arg("scheduler.job.model.filenames.1=model.safetensors")
            .arg("--set")
            .arg("scheduler.job.model.filenames.2=custom-file.py")
            .arg("--set")
            .arg("scheduler.job.model.filenames.3=requirements.txt")
            .arg("--output")
            .arg(temp_path.join("complex-model.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("complex-model.toml"))
            .expect("Failed to read config file");

        assert!(config_content.contains("repository = \"hypha-space/lenet\""));
        assert!(config_content.contains("\"config.json\""));
        assert!(config_content.contains("\"model.safetensors\""));
        assert!(config_content.contains("\"custom-file.py\""));
        assert!(config_content.contains("\"requirements.txt\""));
    }

    // ===== Combined Parameter Tests =====

    #[test]
    fn init_with_name_and_set_combination() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--name")
            .arg("combined-scheduler")
            .arg("--set")
            .arg("trust_pem=override-trust.pem")
            .arg("--set")
            .arg("scheduler.job.model.repository=hypha-space/combined")
            .arg("--output")
            .arg(temp_path.join("name-and-set.toml"))
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success());

        let config_content = fs::read_to_string(temp_path.join("name-and-set.toml"))
            .expect("Failed to read config file");

        // Verify --name affected cert files
        assert!(config_content.contains("cert_pem = \"combined-scheduler-cert.pem\""));
        assert!(config_content.contains("key_pem = \"combined-scheduler-key.pem\""));

        // Verify --set overrode trust_pem and set repository
        assert!(config_content.contains("trust_pem = \"override-trust.pem\""));
        assert!(config_content.contains("repository = \"hypha-space/combined\""));
    }

    // ===== Error Handling Tests =====

    #[test]
    fn init_with_invalid_set_syntax_fails() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();
        let output = Command::new(&binary_path)
            .arg("init")
            .arg("--set")
            .arg("invalid_no_equals_sign")
            .arg("--output")
            .arg(temp_path.join("should-not-exist.toml"))
            .output()
            .expect("Failed to execute command");

        // Command should fail due to invalid --set syntax
        assert!(!output.status.success());

        // Verify no config file was created
        assert!(!temp_path.join("should-not-exist.toml").exists());
    }

    // ===== Other Command Compatibility Tests =====

    #[test]
    fn probe_accepts_set_parameters() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let binary_path = get_binary_path();

        // First create a config file
        let init_output = Command::new(&binary_path)
            .arg("init")
            .arg("--output")
            .arg(temp_path.join("probe-config.toml"))
            .output()
            .expect("Failed to execute init command");

        assert!(init_output.status.success());

        // Test that probe accepts --set parameters (will fail due to connection, not args)
        let probe_output = Command::new(&binary_path)
            .arg("probe")
            .arg("--config")
            .arg(temp_path.join("probe-config.toml"))
            .arg("--set")
            .arg("crls_pem=test-crl.pem")
            .arg("--timeout")
            .arg("100") // Quick timeout
            .arg("/ip4/127.0.0.1/tcp/1234")
            .output()
            .expect("Failed to execute probe command");

        let stderr = String::from_utf8_lossy(&probe_output.stderr);

        // Should fail with connection error, not argument parsing error
        assert!(
            stderr.contains("Connection")
                || stderr.contains("timeout")
                || stderr.contains("refused")
                || stderr.contains("No such file or directory")
                || stderr.contains("Configuration error"),
            "Expected connection/config error, got: {}",
            stderr
        );
    }
}
