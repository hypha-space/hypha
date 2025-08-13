use std::path::{Path, PathBuf};

use futures_util::{AsyncReadExt, AsyncWriteExt};
use libp2p::Stream;
use tokio::{fs::File, io::copy};
use tokio_util::compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt};

const MAX_HEADER_SIZE: u64 = 1024 * 1024; // 1 MB

/// Send a file to a libp2p stream.
pub async fn send_file(
    header: &hypha_messages::ArtifactHeader,
    file_path: impl AsRef<Path>,
    stream: &mut Stream,
) -> Result<(), std::io::Error> {
    let file_path = file_path.as_ref();
    let mut file = File::open(file_path).await?;

    let mut header_bytes = Vec::new();
    ciborium::into_writer(&header, &mut header_bytes).expect("artifact header is serializable");

    // Add length delimiters for header and payload before writing them
    // This way we know how many bytes to expect for each of them.
    stream
        .write_all(&(header_bytes.len() as u64).to_le_bytes())
        .await?;
    stream.write_all(&header_bytes).await?;

    let file_metadata = file.metadata().await?;

    stream
        .write_all(&(file_metadata.len()).to_le_bytes())
        .await?;

    // TODO: fine-tune copy performance by using 'copy_buf' instead.
    // We should also check if memory-mapping the file increases performance.
    let nbytes = copy(&mut file, &mut stream.compat_write()).await?;

    tracing::trace!(file_path = %file_path.display(), bytes = nbytes, "Sent file");

    // TODO: assert nbytes = file_metadata.len()
    Ok(())
}

/// Receive a file from a libp2p stream.
pub async fn receive_file(
    work_dir: impl AsRef<Path>,
    stream: &mut Stream,
) -> Result<PathBuf, std::io::Error> {
    let work_dir = work_dir.as_ref();

    let mut buf = [0; 8];
    stream.read_exact(&mut buf).await?;

    let header_len = u64::from_le_bytes(buf);

    if header_len > MAX_HEADER_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Header too large",
        ));
    }

    let mut header_bytes = vec![0; header_len as usize];
    // TODO: use a maximum header size, don't blindly trust the provided value
    stream.read_exact(&mut header_bytes).await?;

    let header: hypha_messages::ArtifactHeader = ciborium::from_reader(header_bytes.as_slice())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    // TODO: Add header validation

    stream.read_exact(&mut buf).await?;
    let _payload_len = u64::from_le_bytes(buf);

    // NOTE: `task_id` is a `Uuid` and `epoch` is a `u64`. Their string representations are
    // restricted to ASCII hex-digits plus dashes (`[0-9a-f-]`) and never start with "/" or
    // include any path separator.  That guarantees the join below stays inside `work_dir`.
    // If either field type ever changes you MUST re-audit this line and potentially add explicit
    // validation, as it could otherwise be used to write files outside the sandbox.
    let result_path = work_dir.join(format!("result-{}-{}", header.task_id, header.epoch));
    let mut file = File::create(&result_path).await?;

    // TODO: fine-tune copy performance by using 'copy_buf' instead.
    // We should also check if memory-mapping the file increases performance.
    let nbytes = copy(&mut stream.compat(), &mut file).await?;
    tracing::trace!(file_path = %result_path.display(), bytes = nbytes, "Received file");

    // TODO: assert nbytes == payload_len
    Ok(result_path)
}
