use std::path::Path;

use tokio::{
    fs::File,
    io::{self, AsyncWrite},
};

pub async fn serialize_file<P: AsRef<Path>, W: AsyncWrite + Unpin>(
    path: P,
    writer: &mut W,
) -> Result<(), io::Error> {
    let mut file = File::open(path).await?;
    io::copy(&mut file, writer).await?;

    Ok(())
}
