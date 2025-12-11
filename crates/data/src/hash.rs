use std::{fs::File, io, path::Path};

use blake3::Hasher;

pub fn get_file_hash<P>(path: P) -> io::Result<String>
where
    P: AsRef<Path>,
{
    let mut file = File::open(path)?;
    let mut hasher = Hasher::new();

    let _n = io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();
    Ok(format!("blake3:{}", hash.to_hex()))
}
