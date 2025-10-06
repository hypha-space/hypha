use tracing::Subscriber;
use tracing_subscriber::Layer;

pub struct NoopLayer;

impl<S> Layer<S> for NoopLayer where S: Subscriber {}
