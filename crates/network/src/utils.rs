use libp2p::identity;

/// Generates a deterministic Ed25519 keypair based on a single byte seed.
///
/// The seed is used as the first byte of the 32-byte secret key input.
/// The remaining bytes are zero. This is useful for testing or scenarios
/// where reproducible keys are needed from a simple input.
///
/// NOTE: This method of key generation is **not cryptographically secure** for
/// production use cases where unpredictable keys are required. It's primarily
/// intended for deterministic testing or specific niche applications.
///
/// # Examples
///
/// ```
/// # use libp2p::identity;
/// use hypha_network::utils::generate_ed25519;
/// let keypair = generate_ed25519(42)?;
/// # Ok::<(), identity::DecodingError>(())
/// ```
pub fn generate_ed25519(seed: u8) -> Result<identity::Keypair, identity::DecodingError> {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;

    identity::Keypair::ed25519_from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_generate_keypair() {
        let keypair = generate_ed25519(42);

        assert!(keypair.is_ok());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_different_seeds_different_keys(seed1 in any::<u8>(), seed2 in any::<u8>()) {
             prop_assume!(seed1 != seed2, "Seeds must be different for this property test.");

             let keypair1 = generate_ed25519(seed1).expect("Generation should succeed");
             let keypair2 = generate_ed25519(seed2).expect("Generation should succeed");

             prop_assert_ne!(keypair1.public(), keypair2.public(),
                 "Different seeds ({}, {}) produced the same public key", seed1, seed2);
        }

         #[test]
        fn prop_same_seed_same_key(seed in any::<u8>()) {
             let keypair1 = generate_ed25519(seed).expect("Generation should succeed");
             let keypair2 = generate_ed25519(seed).expect("Generation should succeed");

             prop_assert_eq!(keypair1.public(), keypair2.public(),
                 "Same seed ({}) produced different public keys", seed);
        }
    }
}
