use libp2p::identity;

pub fn generate_ed25519(secret_key_seed: u8) -> Result<identity::Keypair, identity::DecodingError> {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    identity::Keypair::ed25519_from_bytes(bytes)
}
