use std::{fs::File, io, path::Path};

use blake3::Hasher;

pub fn get_file_hash<P>(path: P) -> io::Result<u64>
where
    P: AsRef<Path>,
{
    let mut file = File::open(path)?;
    let mut hasher = Hasher::new();

    let _n = io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();

    let mut buf = [0u8; 8];
    buf.copy_from_slice(&hash.as_bytes()[..8]);
    Ok(u64::from_le_bytes(buf))
}
