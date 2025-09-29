use std::{collections::HashMap, fs::File, mem::transmute, path::Path};

use memmap2::{Mmap, MmapOptions};
use safetensors::{
    SafeTensorError, SafeTensors, View,
    tensor::{Metadata, TensorInfo, TensorView},
};
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};

#[derive(Error, Debug)]
pub enum TensorError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("SafeTensors error: {0}")]
    SafeTensorsError(#[from] SafeTensorError),
}

pub struct TensorData {
    buffer: Mmap,
}

impl TensorData {
    pub fn try_open(path: &Path) -> Result<Self, std::io::Error> {
        let file = File::open(path)?;
        let buffer = unsafe { MmapOptions::new().map(&file)? };

        Ok(TensorData { buffer })
    }

    pub fn tensors(&self) -> Result<Vec<(String, TensorView<'_>)>, SafeTensorError> {
        let safetensors = SafeTensors::deserialize(&self.buffer)?;

        Ok(safetensors.tensors())
    }
}

// The following code is adapted from Safetensors' 'tensor.rs'.
// There, 'serialize_to_file' is very close to what we need, only for a more abstract 'Write' implementation.

pub async fn serialize<
    'data,
    I: IntoIterator<Item = (String, TensorView<'data>)>,
    W: AsyncWrite + Unpin,
>(
    data: I,
    data_info: &Option<HashMap<String, String>>,
    writer: &mut W,
) -> Result<(), SafeTensorError> {
    let (
        PreparedData {
            n, header_bytes, ..
        },
        tensors,
    ) = prepare(data, data_info)?;
    writer.write_all(n.to_le_bytes().as_ref()).await?;
    writer.write_all(&header_bytes).await?;
    for tensor in tensors {
        writer.write_all(tensor.data().as_ref()).await?;
    }
    writer.flush().await?;
    Ok(())
}

struct PreparedData {
    n: u64,
    header_bytes: Vec<u8>,
}

fn prepare<'data, I: IntoIterator<Item = (String, TensorView<'data>)>>(
    data: I,
    data_info: &Option<HashMap<String, String>>,
    // ) -> Result<(Metadata, Vec<&'hash TensorView<'data>>, usize), SafeTensorError> {
) -> Result<(PreparedData, Vec<TensorView<'data>>), SafeTensorError> {
    // Make sure we're sorting by descending dtype alignment
    // Then by name
    let mut data: Vec<_> = data.into_iter().collect();
    data.sort_by(|(lname, left), (rname, right)| {
        right.dtype().cmp(&left.dtype()).then(lname.cmp(rname))
    });

    let mut tensors: Vec<TensorView<'_>> = Vec::with_capacity(data.len());
    let mut hmetadata = Vec::with_capacity(data.len());
    let mut offset = 0;
    let data: Vec<_> = data.into_iter().collect();
    for (name, tensor) in data {
        let n = tensor.data_len();
        let tensor_info = TensorInfo {
            dtype: tensor.dtype(),
            shape: tensor.shape().to_vec(),
            data_offsets: (offset, offset + n),
        };
        offset += n;
        hmetadata.push((name.to_string(), tensor_info));
        tensors.push(tensor);
    }

    // HACK! 'Metadata::new' is private but we have to create an
    // instance here. We do this by transmuting an identical object.
    let metadata = SafetensorMetadata::new(data_info.clone(), hmetadata)?;
    let metadata: Metadata = unsafe { transmute(metadata) };

    let mut metadata_buf = serde_json::to_string(&metadata)?.into_bytes();
    // Force alignment to 8 bytes.
    let extra = (8 - metadata_buf.len() % 8) % 8;
    metadata_buf.extend(vec![b' '; extra]);

    let n: u64 = metadata_buf.len() as u64;

    Ok((
        PreparedData {
            n,
            header_bytes: metadata_buf,
        },
        tensors,
    ))
}

// HACK! This has to match 'safetensors::tensor::Metadata'
// so that we can transmute it.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SafetensorMetadata {
    metadata: Option<HashMap<String, String>>,
    tensors: Vec<TensorInfo>,
    index_map: HashMap<String, usize>,
}

impl SafetensorMetadata {
    fn new(
        metadata: Option<HashMap<String, String>>,
        tensors: Vec<(String, TensorInfo)>,
    ) -> Result<Self, SafeTensorError> {
        let mut index_map = HashMap::with_capacity(tensors.len());

        let tensors: Vec<_> = tensors
            .into_iter()
            .enumerate()
            .map(|(index, (k, tensor))| {
                index_map.insert(k, index);
                tensor
            })
            .collect();

        let metadata = Self {
            metadata,
            tensors,
            index_map,
        };
        // metadata.validate()?;
        Ok(metadata)
    }
}
