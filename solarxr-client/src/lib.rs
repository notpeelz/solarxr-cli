use std::cell::LazyCell;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use solarxr_protocol::datatypes::TransactionId;
use solarxr_protocol::flatbuffers::{FlatBufferBuilder, Table, WIPOffset};
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
use tracing::{debug, trace, warn};

pub use solarxr_protocol::datatypes::BodyPart;

type Result<T, E = io::Error> = core::result::Result<T, E>;

fn create_rpc_msg<'bldr: 'b, 'b>(
    fbb: &'bldr mut FlatBufferBuilder<'bldr>,
    items: &'b [WIPOffset<RpcMessageHeader<'bldr>>],
) -> &'bldr [u8] {
    let rpc_msgs = fbb.create_vector(items);

    let msg = solarxr_protocol::MessageBundle::create(
        fbb,
        &MessageBundleArgs {
            rpc_msgs: Some(rpc_msgs),
            ..Default::default()
        },
    );
    fbb.finish(msg, None);
    fbb.finished_data()
}

pub struct SolarXRClient<const BUF_SIZE: usize = 1024> {
    state: Arc<ClientState<BUF_SIZE>>,
    pump_task: Option<(JoinHandle<Result<()>>, oneshot::Sender<()>)>,
}

struct OwnedRpcMessageHeader {
    buf: Arc<Vec<u8>>,
    loc: usize,
}

impl OwnedRpcMessageHeader {
    fn as_flatbuf(&'_ self) -> RpcMessageHeader<'_> {
        unsafe { RpcMessageHeader::init_from_table(Table::new(&self.buf, self.loc)) }
    }
}

struct ClientState<const BUF_SIZE: usize> {
    stream_reader: Mutex<OwnedReadHalf>,
    stream_writer: Mutex<OwnedWriteHalf>,
    transactions_state: Arc<TransactionsState>,
    buf: Mutex<[u8; BUF_SIZE]>,
}

struct TransactionsState {
    transaction_id_counter: AtomicU32,
    transactions: Mutex<HashMap<u32, mpsc::Sender<OwnedRpcMessageHeader>>>,
}

const RPC_TIMEOUT: Duration = Duration::from_secs(2);

impl<const BUF_SIZE: usize> ClientState<BUF_SIZE> {
    async fn pump(&self) -> Result<()> {
        let mut stream = self.stream_reader.lock().await;
        let buf = self.buf.lock().await;

        let mut len = [0u8; 4];
        stream.read(&mut len).await?;
        let len = u32::from_le_bytes(len) as usize;
        if len < 4 || len > BUF_SIZE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, ""));
        }
        let len = len - 4;
        let mut buf = MutexGuard::map(buf, |x| &mut x[..len]);
        stream.read_exact(&mut buf).await?;

        let bundle = flatbuffers::root::<MessageBundle>(&*buf).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid flatbuffer data: {}", err.to_string()),
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
                        .send(OwnedRpcMessageHeader {
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

        Ok(())
    }
}

struct Transaction {
    id: TransactionId,
    rx: mpsc::Receiver<OwnedRpcMessageHeader>,
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
impl<const BUF_SIZE: usize> Drop for SolarXRClient<BUF_SIZE> {
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

impl<const BUF_SIZE: usize> SolarXRClient<BUF_SIZE> {
    pub fn new(stream: UnixStream) -> Self {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stream_reader, stream_writer) = stream.into_split();
        let state = Arc::new(ClientState {
            stream_reader: Mutex::new(stream_reader),
            stream_writer: Mutex::new(stream_writer),
            transactions_state: Arc::new(TransactionsState {
                transaction_id_counter: AtomicU32::new(0),
                transactions: Mutex::new(HashMap::new()),
            }),
            buf: Mutex::new([0u8; BUF_SIZE]),
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
                        r = state.pump() => if let Err(err) = r {
                            return Err(err);
                        },
                    }
                }
            }
        });

        Self {
            state,
            pump_task: Some((pump_task, shutdown_tx)),
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        let len = (data.len() as u32 + 4).to_le_bytes();
        let mut stream = self.state.stream_writer.lock().await;
        trace!("acquired stream lock for writing");
        stream.write_all(&len).await?;
        trace!("wrote len to server");
        stream.write_all(data).await?;
        trace!("wrote data to server");
        Ok(())
    }

    async fn new_transaction(&self) -> Transaction {
        let mut transactions = self.state.transactions_state.transactions.lock().await;
        let id = self
            .state
            .transactions_state
            .transaction_id_counter
            .fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel::<OwnedRpcMessageHeader>(10);
        transactions.insert(id, tx);
        trace!("registered new transaction {}", id);
        Transaction {
            id: TransactionId::new(id),
            state: Arc::clone(&self.state.transactions_state),
            rx,
        }
    }

    async fn consume_message(
        &mut self,
        rx: &mut mpsc::Receiver<OwnedRpcMessageHeader>,
        timeout: Duration,
    ) -> Result<OwnedRpcMessageHeader> {
        let response = tokio::time::timeout(timeout, rx.recv()).await;
        let response = match response {
            Ok(response) => response.expect("transaction channel is closed"),
            Err(_) => {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
        };
        Ok(response)
    }

    async fn reset_with_parts(
        &mut self,
        reset_type: ResetType,
        body_parts: &[BodyPart],
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
        pub async fn $name(&mut self, body_parts: &[BodyPart]) -> Result<()> {
            self.reset_with_parts($reset_type, body_parts).await
        }
    };
}

impl SolarXRClient {
    impl_reset!(reset_yaw; ResetType::Yaw);
    impl_reset!(reset_mounting; ResetType::Mounting);
    impl_reset!(reset_full; ResetType::Full);
    impl_reset_with_parts!(reset_yaw_with_parts; ResetType::Yaw);
    impl_reset_with_parts!(reset_mounting_with_parts; ResetType::Mounting);
    impl_reset_with_parts!(reset_full_with_parts; ResetType::Full);

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

    pub async fn save_stay_aligned_pose(&mut self, pose: StayAlignedPose) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let m = {
            let m = DetectStayAlignedRelaxedPoseRequest::create(
                &mut fbb,
                &DetectStayAlignedRelaxedPoseRequestArgs {
                    pose: match pose {
                        StayAlignedPose::Standing => StayAlignedRelaxedPose::STANDING,
                        StayAlignedPose::Sitting => StayAlignedRelaxedPose::SITTING,
                        StayAlignedPose::Flat => StayAlignedRelaxedPose::FLAT,
                    },
                },
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

    pub async fn reset_stay_aligned_pose(&mut self, pose: StayAlignedPose) -> Result<()> {
        let mut fbb = FlatBufferBuilder::new();
        let m = {
            let m = ResetStayAlignedRelaxedPoseRequest::create(
                &mut fbb,
                &ResetStayAlignedRelaxedPoseRequestArgs {
                    pose: match pose {
                        StayAlignedPose::Standing => StayAlignedRelaxedPose::STANDING,
                        StayAlignedPose::Sitting => StayAlignedRelaxedPose::SITTING,
                        StayAlignedPose::Flat => StayAlignedRelaxedPose::FLAT,
                    },
                },
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

    pub async fn get_height(&mut self) -> Result<HeightData> {
        let mut fbb = FlatBufferBuilder::new();
        let mut transaction = self.new_transaction().await;
        let m = {
            let m = HeightRequest::create(&mut fbb, &HeightRequestArgs {});
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
        let response = response
            .as_flatbuf()
            .message_as_height_response()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "received message isn't a HeightResponse",
                )
            })?;
        Ok(HeightData {
            min: response.min_height(),
            max: response.max_height(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum StayAlignedPose {
    Standing,
    Sitting,
    Flat,
}

#[derive(Debug, Clone, Copy)]
pub struct HeightData {
    pub min: f32,
    pub max: f32,
}
