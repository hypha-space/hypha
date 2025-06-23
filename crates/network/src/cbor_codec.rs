use std::{io, marker::PhantomData};

use async_trait::async_trait;
use futures_util::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{StreamProtocol, request_response};
use serde::{Serialize, de::DeserializeOwned};

/// Codec for CBOR serialization and deserialization.
///
/// While `libp2p` provives a CBOR codec, it is a private implementation.
/// We need a public one to use it as parameter in `request_response.rs`
pub struct Codec<Req, Resp> {
    request_size_maximum: u64,
    response_size_maximum: u64,
    phantom: PhantomData<(Req, Resp)>,
}

impl<Req, Resp> Default for Codec<Req, Resp> {
    fn default() -> Self {
        Self {
            request_size_maximum: 1024 * 1024,
            response_size_maximum: 10 * 1024 * 1024,
            phantom: PhantomData,
        }
    }
}

impl<Req, Resp> Clone for Codec<Req, Resp> {
    fn clone(&self) -> Self {
        Self {
            request_size_maximum: self.request_size_maximum,
            response_size_maximum: self.response_size_maximum,
            phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<Req, Resp> request_response::Codec for Codec<Req, Resp>
where
    Req: Send + Serialize + DeserializeOwned,
    Resp: Send + Serialize + DeserializeOwned,
{
    type Protocol = StreamProtocol;
    type Request = Req;
    type Response = Resp;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut data = Vec::new();
        io.take(self.request_size_maximum)
            .read_to_end(&mut data)
            .await?;

        ciborium::from_reader(data.as_slice())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn read_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut data = Vec::new();
        io.take(self.response_size_maximum)
            .read_to_end(&mut data)
            .await?;

        ciborium::from_reader(data.as_slice())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut data = Vec::new();
        ciborium::into_writer(&req, &mut data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        io.write_all(data.as_ref()).await?;

        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut data = Vec::new();
        ciborium::into_writer(&res, &mut data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        io.write_all(data.as_ref()).await?;

        Ok(())
    }
}
