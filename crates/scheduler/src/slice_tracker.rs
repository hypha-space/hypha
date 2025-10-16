use std::sync::Arc;

use hypha_messages::{api, data};
use hypha_network::request_response::{
    RequestResponseError, RequestResponseInterface, RequestResponseInterfaceExt,
};
use libp2p::PeerId;
use tokio::sync::Mutex;

/// SliceTracker tracks the distribution of slices of a dataset.
///
/// In data-parallel training, multiple participants train on different partitions of a data set.
/// Participants can request slices of data. To ensure that each participant receives a unique slice,
/// the SliceTracker maintains a list of available slices and assigns them to participants as they request them.
pub struct SliceTracker<TBehaviour>
where
    TBehaviour: RequestResponseInterface<api::Codec> + Clone + Send + Sync + 'static,
{
    network: TBehaviour,
    data_provider: PeerId,
    dataset: String,
    available_slices: Arc<Mutex<Vec<u64>>>,
}

impl<TBehaviour> SliceTracker<TBehaviour>
where
    TBehaviour: RequestResponseInterface<api::Codec> + Clone + Send + Sync + 'static,
{
    pub fn new(
        network: TBehaviour,
        data_provider: PeerId,
        dataset: String,
        num_slices: u64,
    ) -> Self {
        Self {
            network,
            data_provider,
            dataset,
            available_slices: Arc::new(Mutex::new((0..num_slices).collect())),
        }
    }

    pub async fn run(self) -> Result<impl Future<Output = ()>, RequestResponseError> {
        let d = self.dataset.clone();
        let data_provider = self.data_provider;
        let available_slices = self.available_slices.clone();

        let future = self
            .network
            .on::<api::Codec, _>(move |req: &api::Request| {
                matches!(
                    req,
                    api::Request::Data(
                        data::Request { dataset }
                    ) if dataset == &d
                )
            })
            .into_stream()
            .await?
            .respond_with_concurrent(None, move |(peer_id, _)| {
                tracing::debug!(%peer_id, "Received data slice request");
                let available_slices = available_slices.clone();

                async move {
                    // Pick an available slie
                    // To consider: We should have a form of verification of slice usage because these requests aren't idempotent.
                    // I.e., repeated and erroneous requests can exhaust available slices without using them.
                    // We could, e.g. reserve a slice and only consider it used once a subsequent request confirms that.
                    // We also might need to reset the available slice for another round of picking.
                    match available_slices.lock().await.pop() {
                        Some(index) => {
                            tracing::debug!(%peer_id, "Picked data slice {}", index);
                            api::Response::Data(data::Response::Success {
                                data_provider,
                                index,
                            })
                        }
                        None => api::Response::Data(data::Response::Error(
                            "No available slices".to_string(),
                        )),
                    }
                }
            });

        Ok(future)
    }
}
