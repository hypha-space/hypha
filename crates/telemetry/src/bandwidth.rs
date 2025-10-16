//! Bandwidth instrumentation for libp2p transports.
//!
//! Wrap a libp2p transport to collect bandwidth metrics via OpenTelemetry.
//! Implementation is based [libp2p metrics module](https://github.com/libp2p/rust-libp2p/blob/master/misc/metrics/src/bandwidth.rs) adjusted to support OTEL.
use std::{
    convert::TryFrom as _,
    io,
    io::{IoSlice, IoSliceMut},
    pin::Pin,
    task::{Context, Poll, ready},
};

use futures_io::{AsyncRead, AsyncWrite};
use futures_util::future::{MapOk, TryFutureExt};
use libp2p::{
    core::{
        Multiaddr,
        muxing::{StreamMuxer, StreamMuxerEvent},
        transport::{DialOpts, ListenerId, TransportError, TransportEvent},
    },
    identity::PeerId,
};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Meter},
};

use crate::attributes::Attributes;

/// Wrap a transport to collect bandwidth metrics via OpenTelemetry OTLP metrics.
#[derive(Debug, Clone)]
#[pin_project::pin_project]
pub struct Transport<T> {
    #[pin]
    transport: T,
    inbound_counter: Counter<u64>,
    outbound_counter: Counter<u64>,
}

impl<T> Transport<T> {
    /// Create a new bandwidth-instrumented transport.
    ///
    /// - `meter`: OpenTelemetry meter used to create the byte counters.
    pub fn new(transport: T, meter: &Meter) -> Self {
        // NOTE: Split metrics by direction to avoid maintaining a direction attribute.
        // Inbound counts bytes read; outbound counts bytes written.
        let inbound_counter = meter
            .u64_counter("hypha.bandwidth.inbound.bytes")
            .with_description("Inbound bandwidth usage by transport protocols")
            .build();
        let outbound_counter = meter
            .u64_counter("hypha.bandwidth.outbound.bytes")
            .with_description("Outbound bandwidth usage by transport protocols")
            .build();

        Self {
            transport,
            inbound_counter,
            outbound_counter,
        }
    }
}

impl<T, M> libp2p::core::Transport for Transport<T>
where
    T: libp2p::core::Transport<Output = (PeerId, M)>,
    M: StreamMuxer + Send + 'static,
    M::Substream: Send + 'static,
    M::Error: Send + Sync + 'static,
{
    type Output = (PeerId, Muxer<M>);
    type Error = T::Error;
    type ListenerUpgrade =
        MapOk<T::ListenerUpgrade, Box<dyn FnOnce((PeerId, M)) -> (PeerId, Muxer<M>) + Send>>;
    type Dial = MapOk<T::Dial, Box<dyn FnOnce((PeerId, M)) -> (PeerId, Muxer<M>) + Send>>;

    fn listen_on(
        &mut self,
        id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        self.transport.listen_on(id, addr)
    }

    fn remove_listener(&mut self, id: ListenerId) -> bool {
        self.transport.remove_listener(id)
    }

    fn dial(
        &mut self,
        addr: Multiaddr,
        dial_opts: DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        let metrics = ConnectionMetrics::from_counters_and_addr(
            self.inbound_counter.clone(),
            self.outbound_counter.clone(),
            &addr,
        );
        Ok(self
            .transport
            .dial(addr.clone(), dial_opts)?
            .map_ok(Box::new(move |(peer_id, stream_muxer)| {
                (peer_id, Muxer::new(stream_muxer, metrics.clone()))
            })))
    }

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        let this = self.project();
        match this.transport.poll(cx) {
            Poll::Ready(TransportEvent::Incoming {
                listener_id,
                upgrade,
                local_addr,
                send_back_addr,
            }) => {
                let metrics = ConnectionMetrics::from_counters_and_addr(
                    this.inbound_counter.clone(),
                    this.outbound_counter.clone(),
                    &send_back_addr,
                );
                Poll::Ready(TransportEvent::Incoming {
                    listener_id,
                    upgrade: upgrade.map_ok(Box::new(move |(peer_id, stream_muxer)| {
                        (peer_id, Muxer::new(stream_muxer, metrics.clone()))
                    })),
                    local_addr,
                    send_back_addr,
                })
            }
            Poll::Ready(other) => {
                let mapped = other.map_upgrade(|_upgrade| unreachable!("case already matched"));
                Poll::Ready(mapped)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Clone, Debug)]
struct ConnectionMetrics {
    inbound: Counter<u64>,
    outbound: Counter<u64>,
    // NOTE: Bake in shared attributes (protocols) once to avoid duplication across directions.
    protocol_attrs: Attributes,
}

impl ConnectionMetrics {
    fn from_counters_and_addr(
        inbound: Counter<u64>,
        outbound: Counter<u64>,
        addr: &Multiaddr,
    ) -> Self {
        Self {
            inbound,
            outbound,
            protocol_attrs: Attributes::new(vec![KeyValue::new(
                "protocols",
                protocol_stack_as_string(addr),
            )]),
        }
    }
}

/// Wraps around a [`StreamMuxer`] and counts the number of bytes that go through all the opened
/// streams.
#[derive(Clone)]
#[pin_project::pin_project]
pub struct Muxer<SMInner> {
    #[pin]
    inner: SMInner,
    metrics: ConnectionMetrics,
}

impl<SMInner> Muxer<SMInner> {
    fn new(inner: SMInner, metrics: ConnectionMetrics) -> Self {
        Self { inner, metrics }
    }
}

impl<SMInner> StreamMuxer for Muxer<SMInner>
where
    SMInner: StreamMuxer,
{
    type Substream = InstrumentedStream<SMInner::Substream>;
    type Error = SMInner::Error;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<StreamMuxerEvent, Self::Error>> {
        let this = self.project();
        this.inner.poll(cx)
    }

    fn poll_inbound(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Substream, Self::Error>> {
        let this = self.project();
        let inner = ready!(this.inner.poll_inbound(cx)?);
        let logged = InstrumentedStream {
            inner,
            metrics: this.metrics.clone(),
        };
        Poll::Ready(Ok(logged))
    }

    fn poll_outbound(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Self::Substream, Self::Error>> {
        let this = self.project();
        let inner = ready!(this.inner.poll_outbound(cx)?);
        let logged = InstrumentedStream {
            inner,
            metrics: this.metrics.clone(),
        };
        Poll::Ready(Ok(logged))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.inner.poll_close(cx)
    }
}

/// Wraps around an [`AsyncRead`] + [`AsyncWrite`] and logs the bandwidth that goes through it.
#[pin_project::pin_project]
pub struct InstrumentedStream<SMInner> {
    #[pin]
    inner: SMInner,
    metrics: ConnectionMetrics,
}

impl<SMInner: AsyncRead> AsyncRead for InstrumentedStream<SMInner> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let num_bytes = ready!(this.inner.poll_read(cx, buf))?;
        let attrs: Vec<KeyValue> = this.metrics.protocol_attrs.clone().into();
        this.metrics
            .inbound
            .add(u64::try_from(num_bytes).unwrap_or(u64::MAX), &attrs);
        Poll::Ready(Ok(num_bytes))
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let num_bytes = ready!(this.inner.poll_read_vectored(cx, bufs))?;
        let attrs: Vec<KeyValue> = this.metrics.protocol_attrs.clone().into();
        this.metrics
            .inbound
            .add(u64::try_from(num_bytes).unwrap_or(u64::MAX), &attrs);
        Poll::Ready(Ok(num_bytes))
    }
}

impl<SMInner: AsyncWrite> AsyncWrite for InstrumentedStream<SMInner> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let num_bytes = ready!(this.inner.poll_write(cx, buf))?;
        let attrs: Vec<KeyValue> = this.metrics.protocol_attrs.clone().into();
        this.metrics
            .outbound
            .add(u64::try_from(num_bytes).unwrap_or(u64::MAX), &attrs);
        Poll::Ready(Ok(num_bytes))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let num_bytes = ready!(this.inner.poll_write_vectored(cx, bufs))?;
        let attrs: Vec<KeyValue> = this.metrics.protocol_attrs.clone().into();
        this.metrics
            .outbound
            .add(u64::try_from(num_bytes).unwrap_or(u64::MAX), &attrs);
        Poll::Ready(Ok(num_bytes))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.project();
        this.inner.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.project();
        this.inner.poll_close(cx)
    }
}

// NOTE: Duplicated from libp2p's metrics helper to avoid introducing a dependency cycle.
pub(crate) fn protocol_stack_as_string(ma: &Multiaddr) -> String {
    let len = ma
        .protocol_stack()
        .fold(0, |acc, proto| acc + proto.len() + 1);
    let mut protocols = String::with_capacity(len);
    for proto_tag in ma.protocol_stack() {
        protocols.push('/');
        protocols.push_str(proto_tag);
    }
    protocols
}

#[cfg(test)]
mod tests {
    use futures_util::io::AllowStdIo;

    use super::*;

    // Shared dummy muxer used across tests
    #[derive(Clone)]
    pub struct DummyMuxer;

    impl StreamMuxer for DummyMuxer {
        type Substream = AllowStdIo<std::io::Cursor<Vec<u8>>>;
        type Error = io::Error;

        fn poll(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<StreamMuxerEvent, Self::Error>> {
            Poll::Pending
        }

        fn poll_inbound(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<Self::Substream, Self::Error>> {
            Poll::Ready(Ok(AllowStdIo::new(std::io::Cursor::new(Vec::new()))))
        }

        fn poll_outbound(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<Self::Substream, Self::Error>> {
            Poll::Ready(Ok(AllowStdIo::new(std::io::Cursor::new(Vec::new()))))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    // Simple recorder/spy transport to assert delegation.
    #[derive(Default)]
    pub struct RecordedState {
        pub listen_on_calls: Vec<(ListenerId, Multiaddr)>,
        pub remove_listener_calls: Vec<ListenerId>,
        pub dial_calls: Vec<(Multiaddr, DialOpts)>,
    }

    pub struct RecordingTransport {
        pub state: std::rc::Rc<std::cell::RefCell<RecordedState>>,
        pub next_event: Option<
            TransportEvent<
                futures_util::future::Ready<Result<(PeerId, DummyMuxer), io::Error>>,
                io::Error,
            >,
        >,
    }

    impl RecordingTransport {
        pub fn new() -> Self {
            Self {
                state: Default::default(),
                next_event: None,
            }
        }

        pub fn new_incoming(send_back_addr: Multiaddr) -> Self {
            let listener_id = ListenerId::next();
            let local_addr: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
            let peer = PeerId::random();
            let upgrade = futures_util::future::ready(Ok((peer, DummyMuxer)));
            Self {
                state: Default::default(),
                next_event: Some(TransportEvent::Incoming {
                    listener_id,
                    upgrade,
                    local_addr,
                    send_back_addr,
                }),
            }
        }
    }

    impl libp2p::core::transport::Transport for RecordingTransport {
        type Output = (PeerId, DummyMuxer);
        type Error = io::Error;
        type ListenerUpgrade = futures_util::future::Ready<Result<Self::Output, Self::Error>>;
        type Dial = futures_util::future::Ready<Result<Self::Output, Self::Error>>;

        fn listen_on(
            &mut self,
            id: ListenerId,
            addr: Multiaddr,
        ) -> Result<(), TransportError<Self::Error>> {
            self.state.borrow_mut().listen_on_calls.push((id, addr));
            Ok(())
        }

        fn remove_listener(&mut self, id: ListenerId) -> bool {
            self.state.borrow_mut().remove_listener_calls.push(id);
            true
        }

        fn dial(
            &mut self,
            _addr: Multiaddr,
            _opts: DialOpts,
        ) -> Result<Self::Dial, TransportError<Self::Error>> {
            self.state
                .borrow_mut()
                .dial_calls
                .push((_addr.clone(), _opts));
            Ok(futures_util::future::ready(Ok((
                PeerId::random(),
                DummyMuxer,
            ))))
        }

        fn poll(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
            let this = Pin::get_mut(self);
            match this.next_event.take() {
                Some(ev) => Poll::Ready(ev),
                None => Poll::Pending,
            }
        }
    }

    mod protocol_stack {
        use super::*;

        #[test]
        fn returns_expected_path_for_multiaddr() {
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/9000/quic-v1".parse().unwrap();
            let s = protocol_stack_as_string(&addr);
            assert_eq!(s, "/ip4/tcp/quic-v1");
        }
    }

    mod instrumented_stream {
        use futures_executor::block_on;
        use futures_util::io::{AsyncReadExt, AsyncWriteExt};

        use super::*;
        use crate::testing::{build_test_provider, read_sum_u64};

        #[test]
        fn emits_inbound_bytes_on_read() {
            let (provider, exporter) = build_test_provider();
            use opentelemetry::metrics::MeterProvider as _;
            let inbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.inbound.bytes")
                .build();
            let outbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.outbound.bytes")
                .build();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1".parse().unwrap();
            let metrics = ConnectionMetrics::from_counters_and_addr(inbound, outbound, &addr);

            let inner = AllowStdIo::new(std::io::Cursor::new(vec![1u8, 2, 3, 4, 5]));
            let mut stream = InstrumentedStream { inner, metrics };
            let mut buf = vec![0u8; 3];
            let n = block_on(AsyncReadExt::read(&mut stream, &mut buf)).unwrap();
            assert_eq!(n, 3);
            provider.force_flush().unwrap();
            let finished = exporter.get_finished_metrics().unwrap();
            let sum = read_sum_u64(&finished, "hypha.bandwidth.inbound.bytes");
            assert_eq!(sum, Some(3));
        }

        #[test]
        fn emits_outbound_bytes_on_write() {
            let (provider, exporter) = build_test_provider();
            use opentelemetry::metrics::MeterProvider as _;
            let inbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.inbound.bytes")
                .build();
            let outbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.outbound.bytes")
                .build();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1".parse().unwrap();
            let metrics = ConnectionMetrics::from_counters_and_addr(inbound, outbound, &addr);

            let inner = AllowStdIo::new(std::io::Cursor::new(Vec::<u8>::new()));
            let mut stream = InstrumentedStream { inner, metrics };
            block_on(AsyncWriteExt::write_all(&mut stream, b"hello")).unwrap();
            block_on(AsyncWriteExt::flush(&mut stream)).unwrap();
            block_on(AsyncWriteExt::close(&mut stream)).unwrap();
            let buf = stream.inner.get_ref().get_ref();
            assert_eq!(buf.as_slice(), b"hello");
            provider.force_flush().unwrap();
            let finished = exporter.get_finished_metrics().unwrap();
            let sum = read_sum_u64(&finished, "hypha.bandwidth.outbound.bytes");
            assert_eq!(sum, Some(5));
        }

        #[test]
        fn emits_inbound_bytes_on_read_vectored() {
            let (provider, exporter) = build_test_provider();
            use opentelemetry::metrics::MeterProvider as _;
            let inbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.inbound.bytes")
                .build();
            let outbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.outbound.bytes")
                .build();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/2".parse().unwrap();
            let metrics = ConnectionMetrics::from_counters_and_addr(inbound, outbound, &addr);

            let inner = AllowStdIo::new(std::io::Cursor::new(vec![10u8, 20, 30, 40, 50]));
            let mut stream = InstrumentedStream { inner, metrics };
            let mut a = [0u8; 2];
            let mut b = [0u8; 3];
            let mut bufs = [IoSliceMut::new(&mut a), IoSliceMut::new(&mut b)];
            let n = block_on(AsyncReadExt::read_vectored(&mut stream, &mut bufs)).unwrap();
            assert_eq!(n, 5);
            assert_eq!(&a, &[10, 20]);
            assert_eq!(&b, &[30, 40, 50]);
            provider.force_flush().unwrap();
            let finished = exporter.get_finished_metrics().unwrap();
            let sum = read_sum_u64(&finished, "hypha.bandwidth.inbound.bytes");
            assert_eq!(sum, Some(5));
        }

        #[test]
        fn emits_outbound_bytes_on_write_vectored() {
            let (provider, exporter) = build_test_provider();
            use opentelemetry::metrics::MeterProvider as _;
            let inbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.inbound.bytes")
                .build();
            let outbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.outbound.bytes")
                .build();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/2".parse().unwrap();
            let metrics = ConnectionMetrics::from_counters_and_addr(inbound, outbound, &addr);

            let inner = AllowStdIo::new(std::io::Cursor::new(Vec::<u8>::new()));
            let mut stream = InstrumentedStream { inner, metrics };
            let b1 = [1u8, 2, 3];
            let b2 = [4u8, 5u8];
            let bufs_w = [IoSlice::new(&b1), IoSlice::new(&b2)];
            let written = block_on(AsyncWriteExt::write_vectored(&mut stream, &bufs_w)).unwrap();
            assert_eq!(written, 5);
            block_on(AsyncWriteExt::flush(&mut stream)).unwrap();
            block_on(AsyncWriteExt::close(&mut stream)).unwrap();
            let buf = stream.inner.get_ref().get_ref();
            assert_eq!(buf.as_slice(), &[1, 2, 3, 4, 5]);
            provider.force_flush().unwrap();
            let finished = exporter.get_finished_metrics().unwrap();
            let sum = read_sum_u64(&finished, "hypha.bandwidth.outbound.bytes");
            assert_eq!(sum, Some(5));
        }

        #[test]
        fn does_not_pollute_outbound_on_read() {
            let (provider, exporter) = build_test_provider();
            use opentelemetry::metrics::MeterProvider as _;
            let inbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.inbound.bytes")
                .build();
            let outbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.outbound.bytes")
                .build();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/10".parse().unwrap();
            let metrics =
                ConnectionMetrics::from_counters_and_addr(inbound.clone(), outbound.clone(), &addr);

            let inner = AllowStdIo::new(std::io::Cursor::new(vec![1u8, 2, 3]));
            let mut stream = InstrumentedStream { inner, metrics };
            let mut buf = vec![0u8; 3];
            let n = block_on(AsyncReadExt::read(&mut stream, &mut buf)).unwrap();
            assert_eq!(n, 3);

            provider.force_flush().unwrap();
            let finished = exporter.get_finished_metrics().unwrap();
            let inbound_sum = read_sum_u64(&finished, "hypha.bandwidth.inbound.bytes");
            let outbound_sum = read_sum_u64(&finished, "hypha.bandwidth.outbound.bytes");
            assert_eq!(inbound_sum, Some(3));
            assert_eq!(outbound_sum, None);
        }

        #[test]
        fn does_not_pollute_inbound_on_write() {
            let (provider, exporter) = build_test_provider();
            use opentelemetry::metrics::MeterProvider as _;
            let inbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.inbound.bytes")
                .build();
            let outbound = provider
                .meter("test")
                .u64_counter("hypha.bandwidth.outbound.bytes")
                .build();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/11".parse().unwrap();
            let metrics =
                ConnectionMetrics::from_counters_and_addr(inbound.clone(), outbound.clone(), &addr);

            let inner = AllowStdIo::new(std::io::Cursor::new(Vec::<u8>::new()));
            let mut stream = InstrumentedStream { inner, metrics };
            block_on(AsyncWriteExt::write_all(&mut stream, b"abcd")).unwrap();
            block_on(AsyncWriteExt::flush(&mut stream)).unwrap();
            block_on(AsyncWriteExt::close(&mut stream)).unwrap();

            provider.force_flush().unwrap();
            let finished = exporter.get_finished_metrics().unwrap();
            let inbound_sum = read_sum_u64(&finished, "hypha.bandwidth.inbound.bytes");
            let outbound_sum = read_sum_u64(&finished, "hypha.bandwidth.outbound.bytes");
            assert_eq!(inbound_sum, None);
            assert_eq!(outbound_sum, Some(4));
        }
    }

    mod transport {
        use std::{
            pin::Pin,
            task::{Context, Poll},
        };

        use futures_executor::block_on;
        use libp2p::core::transport::{Transport, TransportEvent};

        use super::*;

        #[test]
        fn wraps_dial_with_connection_metrics() {
            let meter = crate::metrics::global::meter();

            // Dial path wraps inner output with Muxer
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1000".parse().unwrap();
            let rec = RecordingTransport::new();
            let state = rec.state.clone();
            let mut t = super::super::Transport::new(rec, &meter);
            let dial_opts = DialOpts {
                role: libp2p::core::Endpoint::Dialer,
                port_use: libp2p::core::transport::PortUse::Reuse,
            };
            let mapped = t.dial(addr.clone(), dial_opts).expect("dial future");
            let _out = block_on(mapped).expect("mapped dial result");
            {
                let s = state.borrow();
                assert_eq!(s.dial_calls.len(), 1);
                let (ref a, ref opts) = s.dial_calls[0];
                assert_eq!(a, &addr);
                assert_eq!(opts.role, dial_opts.role);
                assert_eq!(opts.port_use, dial_opts.port_use);
            }
        }

        #[test]
        fn wraps_incoming_with_connection_metrics() {
            let meter = crate::metrics::global::meter();

            // Incoming path maps upgrade to Muxer
            let send_back_addr: Multiaddr = "/ip4/127.0.0.1/tcp/2000".parse().unwrap();
            let mut t = super::super::Transport::new(
                RecordingTransport::new_incoming(send_back_addr),
                &meter,
            );
            let waker = futures_task::noop_waker();
            let mut cx = Context::from_waker(&waker);
            let mut pinned = Pin::new(&mut t);
            match Transport::poll(pinned.as_mut(), &mut cx) {
                Poll::Ready(TransportEvent::Incoming { upgrade, .. }) => {
                    let _res = block_on(upgrade).expect("upgrade mapped");
                }
                other => panic!("unexpected: {:?}", other),
            }
        }

        #[test]
        fn delegates_listen_on_and_remove_listener() {
            let meter = crate::metrics::global::meter();
            let addr0: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
            let id = ListenerId::next();
            let rec = RecordingTransport::new();
            let state = rec.state.clone();
            let mut t = super::super::Transport::new(rec, &meter);
            Transport::listen_on(&mut t, id, addr0.clone()).unwrap();
            assert_eq!(state.borrow().listen_on_calls, vec![(id, addr0.clone())]);
            assert!(Transport::remove_listener(&mut t, id));
            assert_eq!(state.borrow().remove_listener_calls, vec![id]);
        }
    }

    mod muxer {
        use std::{pin::Pin, task::Context};

        use super::{DummyMuxer, *};

        #[test]
        fn returns_pending_on_poll() {
            let meter = crate::metrics::global::meter();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/3000".parse().unwrap();
            let metrics = ConnectionMetrics::from_counters_and_addr(
                meter.u64_counter("muxer.counter.in").build(),
                meter.u64_counter("muxer.counter.out").build(),
                &addr,
            );
            let mut mux = super::Muxer::new(DummyMuxer, metrics);
            let waker = futures_task::noop_waker();
            let mut cx = Context::from_waker(&waker);
            let _ = Pin::new(&mut mux).poll(&mut cx);
        }

        #[test]
        fn opens_inbound_and_outbound_streams() {
            let meter = crate::metrics::global::meter();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/3000".parse().unwrap();
            let metrics = ConnectionMetrics::from_counters_and_addr(
                meter.u64_counter("muxer.counter.in").build(),
                meter.u64_counter("muxer.counter.out").build(),
                &addr,
            );
            let mut mux = super::Muxer::new(DummyMuxer, metrics);
            let waker = futures_task::noop_waker();
            let mut cx = Context::from_waker(&waker);

            let mut inbound = match Pin::new(&mut mux).poll_inbound(&mut cx) {
                Poll::Ready(Ok(s)) => s,
                _ => panic!("unexpected inbound poll"),
            };
            futures_executor::block_on(futures_util::io::AsyncWriteExt::write_all(
                &mut inbound,
                b"abc",
            ))
            .unwrap();

            let mut outbound = match Pin::new(&mut mux).poll_outbound(&mut cx) {
                Poll::Ready(Ok(s)) => s,
                _ => panic!("unexpected outbound poll"),
            };
            futures_executor::block_on(futures_util::io::AsyncWriteExt::write_all(
                &mut outbound,
                b"xyz",
            ))
            .unwrap();
        }

        #[test]
        fn delegates_close() {
            let meter = crate::metrics::global::meter();
            let addr: Multiaddr = "/ip4/127.0.0.1/tcp/3000".parse().unwrap();
            let metrics = ConnectionMetrics::from_counters_and_addr(
                meter.u64_counter("muxer.counter.in").build(),
                meter.u64_counter("muxer.counter.out").build(),
                &addr,
            );
            let mut mux = super::Muxer::new(DummyMuxer, metrics);
            let waker = futures_task::noop_waker();
            let mut cx = Context::from_waker(&waker);
            let _ = Pin::new(&mut mux).poll_close(&mut cx);
        }
    }
}
