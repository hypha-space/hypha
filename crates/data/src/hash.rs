use std::{fs::File, io, path::Path};

use sha2::{Digest, Sha256};

pub fn get_file_sha256<P>(path: P) -> io::Result<String>
where
    P: AsRef<Path>,
{
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();

    let _n = io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();
    Ok(format!("sha256:{:x}", hash))
}
