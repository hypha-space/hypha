use std::sync::Arc;

use hypha_messages::{api, data};
use hypha_network::request_response::{
    RequestResponseError, RequestResponseInterface, RequestResponseInterfaceExt,
};
use libp2p::PeerId;
use thiserror::Error;
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::tracker::slice::SliceTracker;

#[derive(Debug, Error)]
pub enum DataSchedulerError {
    #[error("Disconnected")]
    Disconnected,
    #[error("Network error")]
    NetworkError(#[from] RequestResponseError),
}

/// DataScheduler manges the distribution of slices of a dataset.
///
/// In data-parallel training, multiple participants train on different partitions of a data set.
/// Participants can request slices of data. To ensure that each participant receives a unique slice,
/// the DataScheduler uses a `SliceTracker` that maintains a list of available slices and
/// their stats to assigns slices to participants as they request them.
pub struct DataScheduler<TBehaviour>
where
    TBehaviour: RequestResponseInterface<api::Codec> + Clone + Send + Sync + 'static,
{
    network: TBehaviour,
    data_provider: PeerId,
    dataset: String,
    slice_tracker: Arc<Mutex<SliceTracker>>,
}

impl<TBehaviour> DataScheduler<TBehaviour>
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
            slice_tracker: Arc::new(Mutex::new(SliceTracker::new(num_slices))),
        }
    }

    pub async fn run(
        self,
        cancel_token: CancellationToken,
    ) -> Result<JoinHandle<()>, DataSchedulerError> {
        let d = self.dataset.clone();
        let data_provider = self.data_provider;
        let tracker = self.slice_tracker.clone();
        let stream_handle = tokio::spawn({
            self.network
                .on::<api::Codec, _>(move |req: &api::Request| {
                    matches!(
                        req,
                        api::Request::Data(
                            data::Request { dataset }
                        ) if dataset == &d
                    )
                })
                .into_stream()
                .await
                .map_err(DataSchedulerError::from)?
                .respond_with_concurrent(None, move |request| {
                    let tracker = tracker.clone();
                    async move {
                        let peer_id = request.0;
                        tracing::debug!(%peer_id, "Received data slice request");
                        let index = tracker.lock().await.next(&peer_id);
                        tracing::debug!(%peer_id, "Picked data slice {}", index);
                        api::Response::Data(data::Response::Success {
                            data_provider,
                            index,
                        })
                    }
                })
        });

        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = stream_handle => {
                    tracing::info!("Data handler finished.");
                },
                _ = cancel_token.cancelled() => {
                    tracing::info!("Job is completed.");
                }
            }
        });
        Ok(handle)
    }
}
