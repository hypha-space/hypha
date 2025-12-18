//! Utility functions.

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_util::stream::Stream;
use libp2p::multiaddr::{Error, Multiaddr, Protocol};
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::{self, Sleep},
};

use crate::IpNet;

/// Returns the CIDR containing the first IP protocol in the multiaddr, if any.
pub fn find_containing_cidr(addr: &Multiaddr, cidrs: &[IpNet]) -> Option<IpNet> {
    addr.iter()
        .find_map(|protocol| match protocol {
            Protocol::Ip4(ip) => Some(std::net::IpAddr::V4(ip)),
            Protocol::Ip6(ip) => Some(std::net::IpAddr::V6(ip)),
            _ => None,
        })
        .and_then(|ip| cidrs.iter().find(|net| net.contains(&ip)).cloned())
}

/// Converts a `SocketAddr` to a libp2p `MultiAddr`.
pub fn multiaddr_from_socketaddr(addr: &SocketAddr) -> Result<Multiaddr, Error> {
    let mut multiaddr = Multiaddr::from(addr.ip());
    multiaddr.push(Protocol::Tcp(addr.port()));
    Ok(multiaddr)
}

/// Converts a `SocketAddr` to a libp2p `MultiAddr` using QUIC.
pub fn multiaddr_from_socketaddr_quic(addr: &SocketAddr) -> Result<Multiaddr, Error> {
    if addr.is_ipv6() {
        format!("/ip6/{}/udp/{}/quic-v1", addr.ip(), addr.port()).parse()
    } else {
        format!("/ip4/{}/udp/{}/quic-v1", addr.ip(), addr.port()).parse()
    }
}

/// A stream adapter that batches items from an underlying stream.
///
/// `Batched` collects items from a stream and emits them in batches based on two triggers:
/// - **Count trigger**: When the number of buffered items reaches the specified `limit`
/// - **Time trigger**: When the specified `wait` duration has elapsed since the first item in the current batch
#[pin_project]
pub struct Batched<S: Stream> {
    #[pin]
    inner: S,
    /// NOTE: The timer for the current batch, waking when the batch is ready to be emitted
    #[pin]
    delay: Option<Sleep>,
    buffer: Vec<S::Item>,
    limit: usize,
    wait: Duration,
}

impl<S: Stream> Batched<S> {
    /// Creates a new windowed stream adapter
    ///
    /// # Example
    /// ```ignore
    /// let windowed = Window::new(task_stream, 10, Duration::from_secs(5));
    /// // Emits batches of up to 10 items or after 5 seconds, whichever comes first
    /// ```
    pub fn new(stream: S, limit: usize, wait: Duration) -> Self {
        Batched {
            inner: stream,
            buffer: Vec::with_capacity(limit),
            limit,
            wait,
            delay: None,
        }
    }
}

impl<S: Stream> Stream for Batched<S> {
    type Item = Vec<S::Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        tracing::trace!("Polling windowed_stream");
        let mut this = self.project();

        loop {
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(item)) => {
                    if this.buffer.is_empty() {
                        // NOTE: Set the delay when the first item arrives.
                        this.delay.set(Some(time::sleep(*this.wait)));
                    }
                    this.buffer.push(item);

                    // NOTE: Count-based trigger - flush immediately when buffer reaches limit
                    // This ensures we don't exceed the desired batch size.
                    if this.buffer.len() >= *this.limit {
                        this.delay.set(None);
                        let out = std::mem::take(this.buffer);
                        return Poll::Ready(Some(out));
                    }

                    // Continue to see if more items are ready in the inner stream.
                    continue;
                }
                Poll::Ready(None) => {
                    if this.buffer.is_empty() {
                        // NOTE: Inner stream is exhausted.
                        return Poll::Ready(None);
                    } else {
                        // NOTE: Flush any remaining items.
                        this.delay.set(None);
                        let out = std::mem::take(this.buffer);
                        return Poll::Ready(Some(out));
                    }
                }
                Poll::Pending => {
                    // Inner stream is not ready, check the timer.
                    if this.buffer.is_empty() {
                        // No buffered items, so nothing to do.
                        return Poll::Pending;
                    }

                    // Poll the delay timer.
                    if let Some(delay) = this.delay.as_mut().as_pin_mut()
                        && delay.poll(cx).is_ready()
                    {
                        // Timer has fired, flush the buffer.
                        this.delay.set(None);
                        let out = std::mem::take(this.buffer);
                        return Poll::Ready(Some(out));
                    }

                    // Timer has not fired yet, and inner stream is pending.
                    return Poll::Pending;
                }
            }
        }
    }
}

/// Async reader that enforces an exact byte count.
///
/// Returns `UnexpectedEof` if the underlying stream ends before the declared length.
#[pin_project]
pub struct FixedAsyncRead<R> {
    #[pin]
    inner: R,
    remaining: u64,
}

impl<R> FixedAsyncRead<R> {
    /// Creates a new reader that enforces `expected_len` bytes.
    pub fn new(inner: R, expected_len: u64) -> Self {
        Self {
            inner,
            remaining: expected_len,
        }
    }

    /// Returns the number of bytes left to read.
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Returns the wrapped reader.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead> AsyncRead for FixedAsyncRead<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut this = self.project();

        if *this.remaining == 0 {
            return Poll::Ready(Ok(()));
        }

        let available = (*this.remaining).min(buf.remaining() as u64) as usize;
        let mut limited = buf.take(available);

        match this.inner.as_mut().poll_read(cx, &mut limited) {
            Poll::Ready(Ok(())) => {
                let read = limited.filled().len();
                if read == 0 {
                    return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
                }

                if read as u64 > *this.remaining {
                    return Poll::Ready(Err(io::Error::from(io::ErrorKind::InvalidData)));
                }

                *this.remaining -= read as u64;
                buf.advance(read);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// Async writer that enforces an exact byte count.
///
/// Returns an error if more than the declared length is written, or if shutdown
/// is attempted before the declared length is reached.
#[pin_project]
pub struct FixedAsyncWrite<W> {
    #[pin]
    inner: W,
    remaining: u64,
}

impl<W> FixedAsyncWrite<W> {
    /// Creates a new writer that enforces `expected_len` bytes.
    pub fn new(inner: W, expected_len: u64) -> Self {
        Self {
            inner,
            remaining: expected_len,
        }
    }

    /// Returns the number of bytes left to write.
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Returns the wrapped writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: AsyncWrite> AsyncWrite for FixedAsyncWrite<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();

        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        if buf.len() as u64 > *this.remaining {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "write exceeds declared length",
            )));
        }

        match this.inner.as_mut().poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                if written == 0 {
                    return Poll::Ready(Ok(0));
                }

                if written as u64 > *this.remaining {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "write exceeds declared length",
                    )));
                }

                *this.remaining -= written as u64;
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.project();
        if *this.remaining > 0 {
            return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
        }
        this.inner.poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IpNet;

    #[test]
    fn test_multiaddr_from_socketaddr_ipv4() {
        let addr = "127.0.0.1:8080".parse().unwrap();
        let multiaddr = multiaddr_from_socketaddr(&addr).unwrap();
        assert_eq!(multiaddr.to_string(), "/ip4/127.0.0.1/tcp/8080");
    }

    #[test]
    fn test_multiaddr_from_socketaddr_ipv6() {
        let addr = "[::1]:8080".parse().unwrap();
        let multiaddr = multiaddr_from_socketaddr(&addr).unwrap();
        assert_eq!(multiaddr.to_string(), "/ip6/::1/tcp/8080");
    }

    #[test]
    fn test_multiaddr_from_socketaddr_quic_ipv4() {
        let addr = "127.0.0.1:8080".parse().unwrap();
        let multiaddr = multiaddr_from_socketaddr_quic(&addr).unwrap();
        assert_eq!(multiaddr.to_string(), "/ip4/127.0.0.1/udp/8080/quic-v1");
    }

    #[test]
    fn test_multiaddr_from_socketaddr_quic_ipv6() {
        let addr = "[::1]:8080".parse().unwrap();
        let multiaddr = multiaddr_from_socketaddr_quic(&addr).unwrap();
        assert_eq!(multiaddr.to_string(), "/ip6/::1/udp/8080/quic-v1");
    }

    mod find_containing_cidr {
        use std::str::FromStr;

        use super::*;

        #[test]
        fn ip4_in_cidrs() {
            let cidrs = vec![
                IpNet::from_str("10.0.0.0/8").unwrap(),
                IpNet::from_str("127.0.0.0/8").unwrap(),
                IpNet::from_str("192.168.0.0/16").unwrap(),
            ];

            assert!(
                find_containing_cidr(&"/ip4/10.1.2.3/tcp/1".parse().unwrap(), &cidrs).is_some()
            );
            assert!(
                find_containing_cidr(&"/ip4/127.0.0.1/udp/62001/quic-v1".parse().unwrap(), &cidrs)
                    .is_some()
            );
        }

        #[test]
        fn multiaddr_matches_cidrs_ip4_not() {
            let cidrs = vec![
                IpNet::from_str("10.0.0.0/8").unwrap(),
                IpNet::from_str("127.0.0.0/8").unwrap(),
                IpNet::from_str("192.168.0.0/16").unwrap(),
            ];

            assert!(
                !find_containing_cidr(&"/ip4/8.8.8.8/tcp/1".parse().unwrap(), &cidrs).is_some()
            );
        }

        #[test]
        fn multiaddr_matches_cidrs_ip6() {
            let cidrs = vec![
                IpNet::from_str("fc00::/7").unwrap(),
                IpNet::from_str("2001:4860::/32").unwrap(),
            ];

            assert!(find_containing_cidr(&"/ip6/fc00::1/tcp/1".parse().unwrap(), &cidrs).is_some());
        }

        #[test]
        fn multiaddr_matches_cidrs_ip6_not() {
            let cidrs = vec![
                IpNet::from_str("fc00::/7").unwrap(),
                IpNet::from_str("2001:4860::/32").unwrap(),
            ];

            assert!(
                !find_containing_cidr(&"/ip6/2001:db8::1/tcp/1".parse().unwrap(), &cidrs).is_some()
            );
        }

        #[test]
        fn matching_cidr_returns_exact_range() {
            let cidrs = vec![
                IpNet::from_str("10.0.0.0/8").unwrap(),
                IpNet::from_str("192.168.0.0/16").unwrap(),
            ];

            let matched = find_containing_cidr(&"/ip4/192.168.1.10/tcp/1".parse().unwrap(), &cidrs)
                .expect("cidr should match");

            assert_eq!(matched, IpNet::from_str("192.168.0.0/16").unwrap());
        }
    }
}
