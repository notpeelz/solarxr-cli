use std::cell::LazyCell;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use solarxr_protocol::data_feed::{DataFeedMessage, DataFeedMessageHeader};
use solarxr_protocol::datatypes::TransactionId;
use solarxr_protocol::flatbuffers::{FlatBufferBuilder, WIPOffset};
use solarxr_protocol::rpc::{
    DetectStayAlignedRelaxedPoseRequest, DetectStayAlignedRelaxedPoseRequestArgs,
    EnableStayAlignedRequest, EnableStayAlignedRequestArgs, HeightRequest, HeightRequestArgs,
    ResetRequest, ResetRequestArgs, ResetStayAlignedRelaxedPoseRequest,
    ResetStayAlignedRelaxedPoseRequestArgs, ResetType, RpcMessage, RpcMessageHeader,
    RpcMessageHeaderArgs, SetPauseTrackingRequest, SetPauseTrackingRequestArgs, SettingsRequest,
    SettingsRequestArgs, StayAlignedRelaxedPose, TrackingPauseStateRequest,
    TrackingPauseStateRequestArgs,
};
use solarxr_protocol::{MessageBundle, MessageBundleArgs, flatbuffers};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::select;
use tokio::sync::{Mutex, MutexGuard, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, instrument, trace, warn};

pub use solarxr_protocol as proto;

type Result<T, E = io::Error> = core::result::Result<T, E>;

pub mod owned {
    use solarxr_protocol::flatbuffers::Table;
    use std::sync::Arc;

    use super::proto;
    use crate::owned;

    pub struct RpcMessageHeader {
        pub(crate) buf: Arc<Vec<u8>>,
        pub(crate) loc: usize,
    }

    impl RpcMessageHeader {
        pub fn as_flatbuf(&'_ self) -> proto::rpc::RpcMessageHeader<'_> {
            unsafe {
                proto::rpc::RpcMessageHeader::init_from_table(Table::new(&self.buf, self.loc))
            }
        }
    }

    pub struct DataFeedUpdate {
        pub(crate) buf: Arc<Vec<u8>>,
        pub(crate) loc: usize,
    }

    impl DataFeedUpdate {
        pub fn as_flatbuf(&'_ self) -> solarxr_protocol::data_feed::DataFeedUpdate<'_> {
            unsafe {
                solarxr_protocol::data_feed::DataFeedUpdate::init_from_table(Table::new(
                    &self.buf, self.loc,
                ))
            }
        }
    }

    pub struct HeightResponse {
        pub(crate) msg: owned::RpcMessageHeader,
    }

    impl HeightResponse {
        pub(crate) fn new(msg: owned::RpcMessageHeader) -> Self {
            Self { msg }
        }

        pub fn as_flatbuf(&'_ self) -> proto::rpc::HeightResponse<'_> {
            self.msg.as_flatbuf().message_as_height_response().unwrap()
        }
    }
}

fn create_rpc_msg<'bldr: 'b, 'b>(
    fbb: &'bldr mut FlatBufferBuilder<'bldr>,
    items: &'b [WIPOffset<RpcMessageHeader<'bldr>>],
) -> &'bldr [u8] {
    let rpc_msgs = fbb.create_vector(items);

    let msg = proto::MessageBundle::create(
        fbb,
        &MessageBundleArgs {
            rpc_msgs: Some(rpc_msgs),
            ..Default::default()
        },
    );
    fbb.finish(msg, None);
    fbb.finished_data()
}

fn create_data_feed_msg<'bldr: 'b, 'b>(
    fbb: &'bldr mut FlatBufferBuilder<'bldr>,
    items: &'b [WIPOffset<DataFeedMessageHeader<'bldr>>],
) -> &'bldr [u8] {
    let data_feed_msgs = fbb.create_vector(items);

    let msg = solarxr_protocol::MessageBundle::create(
        fbb,
        &MessageBundleArgs {
            data_feed_msgs: Some(data_feed_msgs),
            ..Default::default()
        },
    );
    fbb.finish(msg, None);
    fbb.finished_data()
}

pub struct SolarXRClient<
    const FB_BUF_SIZE: usize = 1024,
    const DATA_FEED_UPDATE_BUF_SIZE: usize = 16,
> {
    state: Arc<ClientState<FB_BUF_SIZE, DATA_FEED_UPDATE_BUF_SIZE>>,
    pump_task: Option<(JoinHandle<Result<()>>, oneshot::Sender<()>)>,
}

struct ClientState<const FB_BUF_SIZE: usize, const DATA_FEED_UPDATE_BUF_SIZE: usize> {
    stream_reader: Mutex<OwnedReadHalf>,
    stream_writer: Mutex<OwnedWriteHalf>,
    data_feed_update_tx: Mutex<mpsc::Sender<owned::DataFeedUpdate>>,
    data_feed_update_rx: Mutex<mpsc::Receiver<owned::DataFeedUpdate>>,
    transactions_state: Arc<TransactionsState>,
    buf: Mutex<[u8; FB_BUF_SIZE]>,
}

struct TransactionsState {
    transaction_id_counter: AtomicU32,
    transactions: Mutex<HashMap<u32, mpsc::Sender<owned::RpcMessageHeader>>>,
}

const RPC_TIMEOUT: Duration = Duration::from_secs(2);

impl<const FB_BUF_SIZE: usize, const DATA_FEED_UPDATE_BUF_SIZE: usize>
    ClientState<FB_BUF_SIZE, DATA_FEED_UPDATE_BUF_SIZE>
{
    async fn pump(&self) -> Result<()> {
        let mut stream = self.stream_reader.lock().await;
        let buf = self.buf.lock().await;

        let mut len = [0u8; 4];
        stream.read_exact(&mut len).await?;
        let len = u32::from_le_bytes(len) as usize;
        if len < 4 || len > FB_BUF_SIZE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, ""));
        }
        let len = len - 4;
        let mut buf = MutexGuard::map(buf, |x| &mut x[..len]);
        stream.read_exact(&mut buf).await?;

        let bundle = flatbuffers::root::<MessageBundle>(&buf).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid flatbuffer data: {}", err),
            )
        })?;
        // Clone the buffer and share it with Arc so it can be used by all pending transactions
        let shared_buf = LazyCell::new(|| Arc::new(buf.to_vec()));
        if let Some(rpc_msgs) = bundle.rpc_msgs() {
            for msg in rpc_msgs {
                if let Some(tx_id) = msg.tx_id() {
                    trace!(
                        "pumped message (transaction {}): {:?}",
                        tx_id.id(),
                        msg.message_type()
                    );
                } else {
                    trace!("pumped message: {:?}", msg.message_type());
                }
                let Some(transaction_id) = msg.tx_id() else {
                    continue;
                };

                if let Some(tx) = &self
                    .transactions_state
                    .transactions
                    .lock()
                    .await
                    .get(&transaction_id.id())
                {
                    let res = tx
                        .send(owned::RpcMessageHeader {
                            buf: Arc::clone(&shared_buf),
                            loc: msg._tab.loc(),
                        })
                        .await;
                    if res.is_err() {
                        debug!("failed to complete transaction; receiver disconnected");
                    }
                } else {
                    warn!(
                        "unexpected message with transaction id {}",
                        transaction_id.id()
                    );
                }
            }
        }

        if let Some(data_feed_msgs) = bundle.data_feed_msgs() {
            for msg in data_feed_msgs {
                if let DataFeedMessage::DataFeedUpdate = msg.message_type() {
                    let tx = self.data_feed_update_tx.lock().await;
                    let msg = msg.message_as_data_feed_update().unwrap();
                    match tx.try_send(owned::DataFeedUpdate {
                        buf: Arc::clone(&shared_buf),
                        loc: msg._tab.loc(),
                    }) {
                        Ok(_) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            // TODO: debounce to avoid spamming logs?
                            warn!("dropping data feed update as the buffer is full");
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

struct Transaction {
    id: TransactionId,
    rx: mpsc::Receiver<owned::RpcMessageHeader>,
    state: Arc<TransactionsState>,
}

// FIXME: use AsyncDrop
impl Drop for Transaction {
    fn drop(&mut self) {
        let id = self.id.id();
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut transactions = state.transactions.lock().await;
            transactions.remove(&id);
            trace!("cleaning up transaction {}", id);
        });
    }
}

// FIXME: use AsyncDrop
impl<const FB_BUF_SIZE: usize, const DATA_FEED_UPDATE_BUF_SIZE: usize> Drop
    for SolarXRClient<FB_BUF_SIZE, DATA_FEED_UPDATE_BUF_SIZE>
{
    fn drop(&mut self) {
        if let Some((pump_task, shutdown_tx)) = self.pump_task.take() {
            let _ = shutdown_tx.send(());
            tokio::spawn(async move {
                let _ = pump_task.await;
                trace!("pump task done");
            });
        }
    }
}

impl<const FB_BUF_SIZE: usize, const DATA_FEED_UPDATE_BUF_SIZE: usize>
    SolarXRClient<FB_BUF_SIZE, DATA_FEED_UPDATE_BUF_SIZE>
{
    pub fn new(stream: UnixStream) -> Self {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stream_reader, stream_writer) = stream.into_split();

        let (data_feed_update_tx, data_feed_update_rx) = mpsc::channel(DATA_FEED_UPDATE_BUF_SIZE);

        let state = Arc::new(ClientState {
            stream_reader: Mutex::new(stream_reader),
            stream_writer: Mutex::new(stream_writer),
            data_feed_update_tx: Mutex::new(data_feed_update_tx),
            data_feed_update_rx: Mutex::new(data_feed_update_rx),
            transactions_state: Arc::new(TransactionsState {
                transaction_id_counter: AtomicU32::new(0),
                transactions: Mutex::new(HashMap::new()),
            }),
            buf: Mutex::new([0u8; FB_BUF_SIZE]),
        });

        let pump_task = tokio::spawn({
            let state = Arc::clone(&state);
            async move {
                tokio::pin!(shutdown_rx);

                loop {
                    select! {
                        _ = &mut shutdown_rx => {
                            trace!("received shutdown signal");
                            return Ok(());
                        },
                        r = state.pump() => r?,
                    }
                }
            }
        });

        Self {
            state,
            pump_task: Some((pump_task, shutdown_tx)),
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        let len = (data.len() as u32 + 4).to_le_bytes();
        let mut stream = self.state.stream_writer.lock().await;
        trace!("sending message to server");
        stream.write_all(&len).await?;
        stream.write_all(data).await?;
        trace!("sent message to server");
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn new_transaction(&self) -> Transaction {
        let mut transactions = self.state.transactions_state.transactions.lock().await;
        let id = self
            .state
            .transactions_state
            .transaction_id_counter
            .fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel::<owned::RpcMessageHeader>(10);
        transactions.insert(id, tx);
        trace!("registered new transaction {}", id);
        Transaction {
            id: TransactionId::new(id),
            state: Arc::clone(&self.state.transactions_state),
            rx,
        }
    }

    #[instrument(level = "trace", skip(self))]
    async fn consume_message(
        &mut self,
        rx: &mut mpsc::Receiver<owned::RpcMessageHeader>,
        timeout: Duration,
    ) -> Result<owned::RpcMessageHeader> {
        let Ok(response) = tokio::time::timeout(timeout, rx.recv()).await else {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        };
        Ok(response.expect("transaction channel is closed"))
    }

    #[instrument(level = "trace", skip(self))]
    async fn reset_with_parts(
        &mut self,
        reset_type: ResetType,
        body_parts: &[proto::datatypes::BodyPart],
    ) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let body_parts = Some(fbb.create_vector(body_parts));
        let m = {
            let m = ResetRequest::create(
                &mut fbb,
                &ResetRequestArgs {
                    reset_type,
                    body_parts,
                },
            );
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: None,
                    message_type: RpcMessage::ResetRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;

        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn reset(&mut self, reset_type: ResetType) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let m = {
            let m = ResetRequest::create(
                &mut fbb,
                &ResetRequestArgs {
                    reset_type,
                    body_parts: None,
                },
            );
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: None,
                    message_type: RpcMessage::ResetRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;

        Ok(())
    }
}

macro_rules! impl_reset {
    ($name:ident; $reset_type:expr) => {
        pub async fn $name(&mut self) -> Result<()> {
            self.reset($reset_type).await
        }
    };
}

macro_rules! impl_reset_with_parts {
    ($name:ident; $reset_type:expr) => {
        #[allow(dead_code)]
        pub async fn $name(&mut self, body_parts: &[proto::datatypes::BodyPart]) -> Result<()> {
            self.reset_with_parts($reset_type, body_parts).await
        }
    };
}

impl<const FB_BUF_SIZE: usize, const DATA_FEED_UPDATE_BUF_SIZE: usize>
    SolarXRClient<FB_BUF_SIZE, DATA_FEED_UPDATE_BUF_SIZE>
{
    impl_reset!(reset_yaw; ResetType::Yaw);
    impl_reset!(reset_mounting; ResetType::Mounting);
    impl_reset!(reset_full; ResetType::Full);
    impl_reset_with_parts!(reset_yaw_with_parts; ResetType::Yaw);
    impl_reset_with_parts!(reset_mounting_with_parts; ResetType::Mounting);
    impl_reset_with_parts!(reset_full_with_parts; ResetType::Full);

    #[instrument(level = "trace", skip(self))]
    pub async fn pause_tracking_state(&mut self) -> Result<bool> {
        let mut fbb = FlatBufferBuilder::new();
        let mut transaction = self.new_transaction().await;
        let m = {
            let m = TrackingPauseStateRequest::create(
                &mut fbb,
                &TrackingPauseStateRequestArgs::default(),
            );
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: Some(&transaction.id),
                    message_type: RpcMessage::TrackingPauseStateRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;
        let response = self
            .consume_message(&mut transaction.rx, RPC_TIMEOUT)
            .await?;
        let response = response
            .as_flatbuf()
            .message_as_tracking_pause_state_response()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "received message isn't a TrackingPauseStateResponse",
                )
            })?;
        Ok(response.trackingPaused())
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn set_pause_tracking(&mut self, state: bool) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let m = {
            let m = SetPauseTrackingRequest::create(
                &mut fbb,
                &SetPauseTrackingRequestArgs {
                    pauseTracking: state,
                },
            );
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: None,
                    message_type: RpcMessage::SetPauseTrackingRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn save_stay_aligned_pose(&mut self, pose: StayAlignedRelaxedPose) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let m = {
            let m = DetectStayAlignedRelaxedPoseRequest::create(
                &mut fbb,
                &DetectStayAlignedRelaxedPoseRequestArgs { pose },
            );
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: None,
                    message_type: RpcMessage::DetectStayAlignedRelaxedPoseRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn reset_stay_aligned_pose(&mut self, pose: StayAlignedRelaxedPose) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let m = {
            let m = ResetStayAlignedRelaxedPoseRequest::create(
                &mut fbb,
                &ResetStayAlignedRelaxedPoseRequestArgs { pose },
            );
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: None,
                    message_type: RpcMessage::ResetStayAlignedRelaxedPoseRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn set_stay_aligned_enabled(&mut self, enabled: bool) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let m = {
            let m = EnableStayAlignedRequest::create(
                &mut fbb,
                &EnableStayAlignedRequestArgs { enable: enabled },
            );
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: None,
                    message_type: RpcMessage::EnableStayAlignedRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn stay_aligned_enabled(&mut self) -> Result<bool> {
        let mut fbb = FlatBufferBuilder::new();
        let mut transaction = self.new_transaction().await;
        let m = {
            let m = SettingsRequest::create(&mut fbb, &SettingsRequestArgs::default());
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: Some(&transaction.id),
                    message_type: RpcMessage::SettingsRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;
        let response = self
            .consume_message(&mut transaction.rx, RPC_TIMEOUT)
            .await?;
        let response = response
            .as_flatbuf()
            .message_as_settings_response()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "received message isn't a SettingsResponse",
                )
            })?;
        let Some(stay_aligned_settings) = response.stay_aligned() else {
            return Ok(false);
        };

        Ok(stay_aligned_settings.enabled())
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn get_height(&mut self) -> Result<owned::HeightResponse> {
        let mut fbb = FlatBufferBuilder::new();
        let mut transaction = self.new_transaction().await;
        let m = {
            let m = HeightRequest::create(&mut fbb, &HeightRequestArgs::default());
            RpcMessageHeader::create(
                &mut fbb,
                &RpcMessageHeaderArgs {
                    tx_id: Some(&transaction.id),
                    message_type: RpcMessage::HeightRequest,
                    message: Some(m.as_union_value()),
                },
            )
        };
        let data = create_rpc_msg(&mut fbb, &[m]);
        self.write(data).await?;
        let response = self
            .consume_message(&mut transaction.rx, RPC_TIMEOUT)
            .await?;
        if response.as_flatbuf().message_type() != RpcMessage::HeightResponse {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "received message isn't a HeightResponse",
            ));
        }
        Ok(owned::HeightResponse::new(response))
    }

    pub async fn next_data_feed_msg(&mut self) -> Option<owned::DataFeedUpdate> {
        let mut rx = self.state.data_feed_update_rx.lock().await;
        rx.recv().await
    }
}
