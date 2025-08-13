use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, broadcast};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::driver::Driver;

pub async fn try_new_parameter_server(token: CancellationToken) -> std::io::Result<Driver> {
    let (tasks_channel, _) = broadcast::channel(8);
    let (send_channel, _) = broadcast::channel(8);
    let (receive_channel, _) = broadcast::channel(8);
    let driver_tracker = TaskTracker::new();

    let tasks = Arc::new(Mutex::new(HashMap::new()));

    driver_tracker.spawn({
        let send_channel = send_channel.clone();
        let receive_channel = receive_channel.clone();
        let fut = async move {
            let mut inputs_rx = receive_channel.subscribe();

            tracing::info!("Waiting for input for workers");

            while let Ok((peer_id, input)) = inputs_rx.recv().await {
                tracing::info!(peer_id = %peer_id, "Received input");

                // TODO: aggregate parameters received from multiple workers

                let _ = send_channel.send((peer_id, input));

                tracing::info!(peer_id = %peer_id, "Sent merged parameters");
            }
        };

        async move {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::trace!("Received shutdown signal");
                }
                _ = fut => {
                    tracing::trace!("Finished processing input");
                }
            }
        }
    });

    driver_tracker.close();

    Ok(Driver::new(
        tasks,
        tasks_channel,
        send_channel,
        receive_channel,
        driver_tracker,
    ))
}
