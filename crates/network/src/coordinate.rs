// pub enum CoordinateAction {
//     Request(
//         PeerId,
//         CoordinateRequest,
//         oneshot::Sender<CoordinateResponse>,
//     ),
// }

// pub trait CoordinateBehavior {
//     fn coordinate(
//         &mut self,
//     ) -> &mut request_response::cbor::Behaviour<CoordinateRequest, CoordinateResponse>;
// }

// pub trait CoordinateDriver<TBehavior>: SwarmDriver<TBehavior>
// where
//     TBehavior: NetworkBehaviour + CoordinateBehavior,
// {
//     fn pending_coordinate_requests(
//         &self,
//     ) -> Arc<Mutex<HashMap<OutboundRequestId, oneshot::Sender<CoordinateResponse>>>>;

//     async fn process_coordinate_action(&mut self, action: CoordinateAction) {
//         match action {
//             CoordinateAction::Request(peer_id, request, tx) => {
//                 tracing::info!(peer_id=%peer_id.clone(),"Requesting");

//                 let request_id = self
//                     .swarm()
//                     .behaviour_mut()
//                     .coordinate()
//                     .send_request(&peer_id, request);

//                 self.pending_coordinate_requests()
//                     .lock()
//                     .await
//                     .insert(request_id, tx);
//             }
//         }
//     }

//     async fn process_coordinate_response(
//         &mut self,
//         request_id: OutboundRequestId,
//         response: CoordinateResponse,
//     ) {
//         tracing::info!(request_id=%request_id,"Received response");
//         let tx = self
//             .pending_coordinate_requests()
//             .lock()
//             .await
//             .remove(&request_id)
//             .expect("Request ID not found");

//         tx.send(response).ok();
//     }
// }

// pub trait CoordinateAdapter<TBehavior>: SwarmAdapter<TBehavior>
// where
//     TBehavior: NetworkBehaviour + CoordinateBehavior,
//     Self::Driver: CoordinateDriver<TBehavior>,
// {
//     fn coordinate_action_sender(&self) -> UnboundedSender<CoordinateAction>;

//     async fn request(
//         &mut self,
//         peer_id: PeerId,
//         request: CoordinateRequest,
//     ) -> Result<CoordinateResponse, OutboundFailure> {
//         let (tx, rx) = oneshot::channel();

//         self.coordinate_action_sender()
//             .send(CoordinateAction::Request(peer_id, request, tx))
//             .map_err(|_| OutboundFailure::DialFailure)?;

//         rx.await.map_err(|_| OutboundFailure::DialFailure)
//     }
// }
