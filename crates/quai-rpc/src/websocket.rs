use crate::{Endpoint, MAX_BATCH_CALLS, RpcError, Transport, transport::decode_response};
use futures_util::{SinkExt, StreamExt};
use quai_primitives::{Address, Hash32};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    task::AbortHandle,
    time::Instant,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Resource and deadline limits for a single explicit native WebSocket connection.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WsConfig {
    /// DNS, TCP, TLS and upgrade handshake deadline.
    pub connect_timeout: Duration,
    /// Entire RPC deadline including permit and command-queue waiting.
    pub request_timeout: Duration,
    /// Maximum decoded inbound or serialized outbound JSON message size.
    pub max_message_bytes: usize,
    /// Maximum incoming WebSocket frame payload size.
    pub max_frame_bytes: usize,
    /// Maximum queued plus outstanding RPC requests.
    pub max_in_flight: usize,
    /// Maximum established plus pending subscription registrations.
    pub max_subscriptions: usize,
    /// Maximum queued notifications per subscription; overflow is an explicit error.
    pub subscription_capacity: usize,
    /// Aggregate queued notification budget measured in encoded message bytes.
    pub max_notification_bytes: usize,
}
impl WsConfig {
    /// Replace `connect_timeout`.
    pub const fn with_connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }
    /// Replace `request_timeout`.
    pub const fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }
    /// Replace `max_message_bytes`.
    pub const fn with_max_message_bytes(mut self, max_message_bytes: usize) -> Self {
        self.max_message_bytes = max_message_bytes;
        self
    }
    /// Replace `max_frame_bytes`.
    pub const fn with_max_frame_bytes(mut self, max_frame_bytes: usize) -> Self {
        self.max_frame_bytes = max_frame_bytes;
        self
    }
    /// Replace `max_in_flight`.
    pub const fn with_max_in_flight(mut self, max_in_flight: usize) -> Self {
        self.max_in_flight = max_in_flight;
        self
    }
    /// Replace `max_subscriptions`.
    pub const fn with_max_subscriptions(mut self, max_subscriptions: usize) -> Self {
        self.max_subscriptions = max_subscriptions;
        self
    }
    /// Replace `subscription_capacity`.
    pub const fn with_subscription_capacity(mut self, subscription_capacity: usize) -> Self {
        self.subscription_capacity = subscription_capacity;
        self
    }
    /// Replace `max_notification_bytes`.
    pub const fn with_max_notification_bytes(mut self, max_notification_bytes: usize) -> Self {
        self.max_notification_bytes = max_notification_bytes;
        self
    }
}
impl Default for WsConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(10),
            max_message_bytes: 2 * 1024 * 1024,
            max_frame_bytes: 2 * 1024 * 1024,
            max_in_flight: 16,
            max_subscriptions: 16,
            subscription_capacity: 64,
            max_notification_bytes: 8 * 1024 * 1024,
        }
    }
}
impl WsConfig {
    fn validate(&self) -> Result<(), RpcError> {
        if self.connect_timeout.is_zero()
            || self.request_timeout.is_zero()
            || self.max_message_bytes == 0
            || self.max_frame_bytes == 0
            || self.max_frame_bytes > self.max_message_bytes
            || self.max_in_flight == 0
            || self.max_subscriptions == 0
            || self.subscription_capacity == 0
            || self.max_in_flight > 1024
            || self.max_subscriptions > 1024
            || self.subscription_capacity > 65_536
            || self.max_notification_bytes == 0
            || self.max_notification_bytes > 256 * 1024 * 1024
            || self
                .max_in_flight
                .checked_mul(self.max_message_bytes)
                .is_none_or(|bytes| bytes > 256 * 1024 * 1024)
            || self.max_message_bytes > 16 * 1024 * 1024
            || Instant::now().checked_add(self.connect_timeout).is_none()
            || Instant::now().checked_add(self.request_timeout).is_none()
        {
            return Err(RpcError::InvalidConfig);
        }
        Ok(())
    }
}

/// Common Quai subscription requests. The endpoint selects the shard; no zone parameter is added.
#[derive(Clone, Debug)]
pub enum WsSubscriptionKind {
    /// `quai_subscribe` with `["newHeads"]`.
    NewHeads,
    /// `quai_subscribe` with `["newPendingTransactions"]`.
    NewPendingTransactions,
    /// `quai_subscribe` with `["logs", filter]`; None topic slots are wildcards.
    Logs {
        /// Optional emitting-address filter; empty means omit address restriction.
        addresses: Vec<Address>,
        /// Up to four topic positions, each a wildcard or an ordered OR-list.
        topics: Vec<Option<Vec<Hash32>>>,
    },
    /// Explicit compatibility escape hatch for additional Quai events, not arbitrary RPC methods.
    Raw {
        /// Event name resolved by the node's subscription registry.
        name: String,
        /// Event arguments appended after its name. Maximum 16 arguments.
        arguments: Vec<Value>,
    },
}
impl WsSubscriptionKind {
    fn params(self) -> Result<Value, RpcError> {
        match self {
            Self::NewHeads => Ok(json!(["newHeads"])),
            Self::NewPendingTransactions => Ok(json!(["newPendingTransactions"])),
            Self::Logs { addresses, topics } => {
                if addresses.len() > 1024
                    || topics.len() > 4
                    || topics.iter().flatten().any(|slot| slot.len() > 1024)
                {
                    return Err(RpcError::InvalidConfig);
                }
                let mut filter = serde_json::Map::new();
                if !addresses.is_empty() {
                    filter.insert(
                        "address".into(),
                        json!(
                            addresses
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        ),
                    );
                }
                if !topics.is_empty() {
                    filter.insert(
                        "topics".into(),
                        Value::Array(
                            topics
                                .into_iter()
                                .map(|slot| {
                                    slot.map_or(Value::Null, |hashes| {
                                        json!(
                                            hashes
                                                .iter()
                                                .map(ToString::to_string)
                                                .collect::<Vec<_>>()
                                        )
                                    })
                                })
                                .collect(),
                        ),
                    );
                }
                Ok(json!(["logs", filter]))
            }
            Self::Raw {
                name,
                mut arguments,
            } => {
                if !valid_method(&name) || arguments.len() > 16 {
                    return Err(RpcError::InvalidConfig);
                }
                arguments.insert(0, Value::String(name));
                Ok(Value::Array(arguments))
            }
        }
    }
}

/// One explicit endpoint and one multiplexed session. Clones share its resource limits.
///
/// Disconnects are terminal: reconnect by creating another transport, then reconcile
/// application state before resubscribing. No request, including writes, is replayed.
#[derive(Clone)]
pub struct WsTransport {
    inner: Arc<Inner>,
}
struct Inner {
    endpoint: Endpoint,
    config: WsConfig,
    commands: mpsc::Sender<Command>,
    shutdown: watch::Sender<bool>,
    closed: watch::Receiver<Option<RpcError>>,
    permits: Arc<Semaphore>,
    subscription_permits: Arc<Semaphore>,
    next_id: Arc<AtomicU64>,
    task: AbortHandle,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }
}
impl fmt::Debug for WsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsTransport")
            .field("endpoint", &self.inner.endpoint)
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

/// Bounded notification receiver. Call `unsubscribe` for independent graceful removal.
///
/// Dropping an active subscription without unsubscribing closes the shared session
/// to prevent a hidden remote subscription or silent notification loss. Other
/// requests/subscriptions then receive explicit disconnect errors.
pub struct WsSubscription {
    id: String,
    receiver: mpsc::Receiver<QueuedNotification>,
    terminal: watch::Receiver<Option<RpcError>>,
    owner: WsTransport,
    armed: bool,
    finished: bool,
}
impl fmt::Debug for WsSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsSubscription")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}
impl Drop for WsSubscription {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.owner.inner.shutdown.send(true);
        }
    }
}
impl WsSubscription {
    /// Subscription identity supplied by the node. Treat as session-scoped untrusted data.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Receive the next notification, or one terminal error followed by None.
    /// Terminal errors take priority over buffered values once continuity is lost.
    pub async fn recv(&mut self) -> Result<Option<Value>, RpcError> {
        if self.finished {
            return Ok(None);
        }
        let mut closed_receiver = self.owner.inner.closed.clone();
        loop {
            let terminal = self.terminal.borrow().clone();
            let closed = self.owner.inner.closed.borrow().clone();
            if let Some(error) = terminal.or(closed) {
                self.finished = true;
                return Err(error);
            }
            tokio::select! {biased;
                result=self.terminal.changed()=>{if result.is_err(){self.finished=true;return Err(RpcError::Disconnected)}},
                result=closed_receiver.changed()=>{if result.is_err(){self.finished=true;return Err(RpcError::Disconnected)}},
                value=self.receiver.recv()=>{return match value {Some(value)=>Ok(Some(value.payload)),None=>{self.finished=true;Err(RpcError::Disconnected)}}},
            }
        }
    }
    /// Remove this subscription using exactly one `quai_unsubscribe` request.
    /// Any unsuccessful or cancelled unsubscribe closes the session during Drop.
    pub async fn unsubscribe(mut self) -> Result<(), RpcError> {
        let response = self
            .owner
            .request_inner(
                "quai_unsubscribe",
                json!([self.id]),
                None,
                Some(self.id.clone()),
            )
            .await?;
        if response != json!(true) {
            return Err(RpcError::InvalidResponse(
                "unsubscribe was not acknowledged",
            ));
        }
        self.armed = false;
        self.finished = true;
        Ok(())
    }
}

struct Registration {
    sender: mpsc::Sender<QueuedNotification>,
    terminal: watch::Sender<Option<RpcError>>,
    _permit: OwnedSemaphorePermit,
}
struct QueuedNotification {
    payload: Value,
    _permit: OwnedSemaphorePermit,
}
struct Pending {
    reply: oneshot::Sender<Result<Value, RpcError>>,
    registration: Option<Registration>,
    unsubscribe: Option<String>,
    _permit: OwnedSemaphorePermit,
    deadline: Instant,
}
struct Command {
    id: u64,
    body: String,
    pending: Pending,
}

impl WsTransport {
    /// Whether this local session has completed its handshake and has not
    /// requested shutdown or observed closure. A remote disconnect may not yet
    /// have been detected; this is not a network or subscription continuity proof.
    pub fn is_open(&self) -> bool {
        !*self.inner.shutdown.borrow() && self.inner.closed.borrow().is_none()
    }
    /// Connect to the exact WS(S) endpoint with verified TLS and no redirects or proxies.
    pub async fn connect(endpoint: Endpoint, config: WsConfig) -> Result<Self, RpcError> {
        config.validate()?;
        if !matches!(endpoint.scheme(), "ws" | "wss") {
            return Err(RpcError::InvalidConfig);
        }
        let mut request = endpoint
            .as_str()
            .into_client_request()
            .map_err(|_| RpcError::InvalidConfig)?;
        request.headers_mut().insert(
            "user-agent",
            concat!("quai-rust-sdk/", env!("CARGO_PKG_VERSION"))
                .parse()
                .map_err(|_| RpcError::InvalidConfig)?,
        );
        let websocket_config = WebSocketConfig::default()
            .read_buffer_size(16 * 1024)
            .write_buffer_size(0)
            .max_write_buffer_size(config.max_message_bytes + 1024)
            .max_message_size(Some(config.max_message_bytes))
            .max_frame_size(Some(config.max_frame_bytes));
        let (socket, _) = tokio::time::timeout(
            config.connect_timeout,
            connect_async_with_config(request, Some(websocket_config), false),
        )
        .await
        .map_err(|_| RpcError::Timeout)?
        .map_err(ws_error)?;
        let (commands, receiver) = mpsc::channel(config.max_in_flight);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (closed_sender, closed) = watch::channel(None);
        let next_id = Arc::new(AtomicU64::new(1));
        let actor = tokio::spawn(run(
            socket,
            receiver,
            shutdown_receiver,
            closed_sender,
            config.clone(),
            next_id.clone(),
        ));
        Ok(Self {
            inner: Arc::new(Inner {
                endpoint,
                config: config.clone(),
                commands,
                shutdown,
                closed,
                permits: Arc::new(Semaphore::new(config.max_in_flight)),
                subscription_permits: Arc::new(Semaphore::new(config.max_subscriptions)),
                next_id,
                task: actor.abort_handle(),
            }),
        })
    }
    /// Send an explicit close signal and await session termination, bounded by request_timeout.
    pub async fn shutdown(&self) -> Result<(), RpcError> {
        let _ = self.inner.shutdown.send(true);
        let mut closed = self.inner.closed.clone();
        let result = tokio::time::timeout(self.inner.config.request_timeout, async {
            loop {
                if closed.borrow().is_some() {
                    return;
                }
                if closed.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        if result.is_err() {
            self.inner.task.abort();
            return Err(RpcError::Timeout);
        }
        Ok(())
    }
    /// Register a bounded subscription; registration precedes delivery of its first notification.
    /// A cancelled in-flight registration closes the session rather than leaking an unknown server ID.
    pub async fn subscribe(&self, kind: WsSubscriptionKind) -> Result<WsSubscription, RpcError> {
        let params = kind.params()?;
        let deadline = Instant::now() + self.inner.config.request_timeout;
        let permit = tokio::time::timeout_at(
            deadline,
            self.inner.subscription_permits.clone().acquire_owned(),
        )
        .await
        .map_err(|_| RpcError::Timeout)?
        .map_err(|_| RpcError::Disconnected)?;
        let (sender, receiver) = mpsc::channel(self.inner.config.subscription_capacity);
        let (terminal_sender, terminal) = watch::channel(None);
        let result = self
            .request_at(
                "quai_subscribe",
                params,
                Some(Registration {
                    sender,
                    terminal: terminal_sender,
                    _permit: permit,
                }),
                None,
                deadline,
            )
            .await?;
        let id = subscription_id(&result)?.to_owned();
        Ok(WsSubscription {
            id,
            receiver,
            terminal,
            owner: self.clone(),
            armed: true,
            finished: false,
        })
    }
    async fn request_inner(
        &self,
        method: &str,
        params: Value,
        registration: Option<Registration>,
        unsubscribe: Option<String>,
    ) -> Result<Value, RpcError> {
        self.request_at(
            method,
            params,
            registration,
            unsubscribe,
            Instant::now() + self.inner.config.request_timeout,
        )
        .await
    }
    async fn request_at(
        &self,
        method: &str,
        params: Value,
        registration: Option<Registration>,
        unsubscribe: Option<String>,
        deadline: Instant,
    ) -> Result<Value, RpcError> {
        if !valid_method(method) || !(params.is_array() || params.is_object()) {
            return Err(RpcError::InvalidConfig);
        }
        if *self.inner.shutdown.borrow() {
            return Err(RpcError::Disconnected);
        }
        if let Some(error) = self.inner.closed.borrow().clone() {
            return Err(error);
        }
        let work = async {
            let permit = self
                .inner
                .permits
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| RpcError::Disconnected)?;
            let id = self
                .inner
                .next_id
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .map_err(|_| RpcError::RequestIdExhausted)?;
            let body = encode_request(id, method, params, self.inner.config.max_message_bytes)?;
            let (reply, response) = oneshot::channel();
            let pending = Pending {
                reply,
                registration,
                unsubscribe,
                _permit: permit,
                deadline,
            };
            self.inner
                .commands
                .send(Command { id, body, pending })
                .await
                .map_err(|_| RpcError::Disconnected)?;
            response.await.map_err(|_| RpcError::Disconnected)?
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| RpcError::Timeout)?
    }
}
impl Transport for WsTransport {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if endpoint != &self.inner.endpoint
            || matches!(method, "quai_subscribe" | "quai_unsubscribe")
        {
            return Err(RpcError::InvalidConfig);
        }
        self.request_inner(method, params, None, None).await
    }

    /// Issue a group of calls concurrently over the multiplexed session.
    ///
    /// This is not JSON-RPC array framing. The session is already pipelined:
    /// requests are correlated by ID and several may be in flight at once, so
    /// issuing a group concurrently collapses the same round trips that array
    /// framing would, without touching the dispatch path that performs the
    /// correlation and the duplicate/foreign/unknown-ID rejection. Array framing
    /// would require the actor to accept a response frame carrying many IDs,
    /// which is the one place a correlation mistake becomes a response-confusion
    /// bug, so it is deliberately not introduced for a latency win the existing
    /// path already provides.
    ///
    /// Concurrency is bounded by the session's in-flight permit count, so a
    /// group larger than that budget proceeds in waves rather than unbounded.
    /// Results keep their request positions. One deadline covers the whole
    /// group, matching the HTTP batch. A failure part-way through leaves
    /// acceptance of the already-sent calls unknown, exactly as the trait says.
    async fn request_batch(
        &self,
        endpoint: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<crate::BatchResult> {
        if endpoint != &self.inner.endpoint
            || requests.is_empty()
            || requests.len() > MAX_BATCH_CALLS
            || requests.iter().any(|(method, params)| {
                matches!(*method, "quai_subscribe" | "quai_unsubscribe")
                    || !(params.is_array() || params.is_object())
            })
        {
            return Some(Err(RpcError::InvalidConfig));
        }
        let deadline = Instant::now() + self.inner.config.request_timeout;
        let pending = requests
            .into_iter()
            .map(|(method, params)| self.request_at(method, params, None, None, deadline));
        Some(Ok(futures_util::future::join_all(pending).await))
    }
}

async fn run(
    mut socket: Socket,
    mut commands: mpsc::Receiver<Command>,
    mut shutdown: watch::Receiver<bool>,
    closed: watch::Sender<Option<RpcError>>,
    config: WsConfig,
    next_id: Arc<AtomicU64>,
) {
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut subscriptions: HashMap<String, Registration> = HashMap::new();
    // Subscription IDs this session has just unsubscribed. The node hands a
    // notification to its writer before it processes the unsubscribe, so a
    // notification for a cancelled subscription can legitimately arrive after
    // its own acknowledgement. Bounded and FIFO: it only has to cover frames
    // already in flight, never a growing history.
    let mut recently_unsubscribed: VecDeque<String> = VecDeque::new();
    let notification_bytes = Arc::new(Semaphore::new(config.max_notification_bytes));
    let mut cleanup = tokio::time::interval(Duration::from_millis(10).min(config.request_timeout));
    cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let failure = loop {
        tokio::select! {biased;
            _=shutdown.changed()=>{break RpcError::Disconnected},
            _=cleanup.tick()=>{
                let mut close=subscriptions.values().any(|subscription|subscription.sender.is_closed());
                let abandoned:Vec<_>=pending.iter().filter(|(_,request)|request.reply.is_closed()||Instant::now()>=request.deadline).map(|(id,_)|*id).collect();
                for id in abandoned {
                    if let Some(request)=pending.remove(&id) {
                        close|=request.registration.is_some()||request.unsubscribe.is_some();
                        let _=request.reply.send(Err(RpcError::Timeout));
                    }
                }
                if close {break RpcError::Disconnected}
            },
            command=commands.recv()=>{
                let Some(command)=command else {break RpcError::Disconnected};
                if command.pending.reply.is_closed()||Instant::now()>=command.pending.deadline {
                    let _=command.pending.reply.send(Err(RpcError::Timeout));continue
                }
                let result=tokio::select! {biased;
                    _=shutdown.changed()=>{let _=command.pending.reply.send(Err(RpcError::Disconnected));break RpcError::Disconnected},
                    result=tokio::time::timeout_at(command.pending.deadline,socket.send(Message::Text(command.body.into())))=>result,
                };
                match result {
                    Ok(Ok(()))=>{pending.insert(command.id,command.pending);},
                    Ok(Err(error))=>{let error=ws_error(error);let _=command.pending.reply.send(Err(error.clone()));break error},
                    Err(_)=>{let _=command.pending.reply.send(Err(RpcError::Timeout));break RpcError::Disconnected},
                }
            },
            message=socket.next()=>{
                let message=match message {Some(Ok(message))=>message,Some(Err(error))=>break ws_error(error),None=>break RpcError::Disconnected};
                let bytes=match &message {
                    Message::Text(text)=>text.as_str().as_bytes(),
                    Message::Binary(bytes)=>bytes.as_ref(),
                    Message::Close(_)=>break RpcError::Disconnected,
                    Message::Ping(_)|Message::Pong(_)=>{
                        let result=tokio::select! {biased;
                            _=shutdown.changed()=>break RpcError::Disconnected,
                            result=tokio::time::timeout(config.request_timeout,socket.flush())=>result,
                        };
                        match result {Ok(Ok(()))=>{},_=>break RpcError::Disconnected};continue
                    },
                    _=>break RpcError::InvalidResponse("unexpected WebSocket frame"),
                };
                if let Err(error)=dispatch(bytes,&mut pending,&mut subscriptions,&mut recently_unsubscribed,next_id.load(Ordering::Relaxed),&notification_bytes) {break error}
            },
        }
    };
    for (_, request) in pending {
        let _ = request.reply.send(Err(failure.clone()));
    }
    for (_, subscription) in subscriptions {
        if subscription.terminal.borrow().is_none() {
            let _ = subscription.terminal.send(Some(failure.clone()));
        }
    }
    commands.close();
    drop(commands);
    let _ = tokio::time::timeout(
        Duration::from_secs(1).min(config.request_timeout),
        socket.close(None),
    )
    .await;
    let _ = closed.send(Some(failure));
}

#[derive(Default)]
enum Slot {
    #[default]
    Absent,
    Present(Value),
}
fn slot<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Slot, D::Error> {
    Value::deserialize(deserializer).map(Slot::Present)
}
#[derive(Deserialize)]
struct WireEnvelope {
    jsonrpc: String,
    #[serde(default, deserialize_with = "slot")]
    id: Slot,
    #[serde(default, deserialize_with = "slot")]
    method: Slot,
    #[serde(default, deserialize_with = "slot")]
    params: Slot,
    #[serde(default, deserialize_with = "slot")]
    result: Slot,
    #[serde(default, deserialize_with = "slot")]
    error: Slot,
}
#[derive(Deserialize)]
struct Notification {
    subscription: String,
    result: Value,
}
#[derive(Deserialize)]
struct NotificationFrame {
    params: Notification,
}
/// Maximum subscription IDs remembered after unsubscribing, enough to cover
/// notifications already in flight when the acknowledgement was processed.
const MAX_RECENTLY_UNSUBSCRIBED: usize = 64;

fn dispatch(
    bytes: &[u8],
    pending: &mut HashMap<u64, Pending>,
    subscriptions: &mut HashMap<String, Registration>,
    recently_unsubscribed: &mut VecDeque<String>,
    next_id: u64,
    notification_bytes: &Arc<Semaphore>,
) -> Result<(), RpcError> {
    // Binary frames reach here as raw bytes. serde skips an ignored field's
    // contents without validating UTF-8, so without this check a frame that is
    // not JSON text was routed by its ID, then failed decoding as that
    // request's reply. Found by the ws_dispatch fuzz target.
    std::str::from_utf8(bytes)
        .map_err(|_| RpcError::InvalidResponse("malformed WebSocket envelope"))?;
    let envelope: WireEnvelope = serde_json::from_slice(bytes)
        .map_err(|_| RpcError::InvalidResponse("malformed WebSocket envelope"))?;
    if envelope.jsonrpc != "2.0" {
        return Err(RpcError::InvalidResponse("wrong protocol version"));
    }
    match (
        envelope.id,
        envelope.method,
        envelope.params,
        envelope.result,
        envelope.error,
    ) {
        (Slot::Present(id), Slot::Absent, Slot::Absent, _, _) => {
            let id = id
                .as_u64()
                .ok_or(RpcError::InvalidResponse("invalid response ID"))?;
            if id == 0 || id >= next_id {
                return Err(RpcError::InvalidResponse("unissued response ID"));
            }
            let result = decode_response(bytes, id);
            let Some(mut request) = pending.remove(&id) else {
                // IDs are never reused; this can only be a late/duplicate response to an abandoned/completed request.
                return result.map(|_| ()).or_else(|error| {
                    if matches!(error, RpcError::Remote(_)) {
                        Ok(())
                    } else {
                        Err(error)
                    }
                });
            };
            if let Some(registration) = request.registration.take() {
                if request.reply.is_closed() {
                    return Err(RpcError::Disconnected);
                }
                if let Ok(value) = &result {
                    let id = subscription_id(value)?.to_owned();
                    if subscriptions.contains_key(&id) {
                        return Err(RpcError::InvalidResponse("duplicate subscription ID"));
                    }
                    subscriptions.insert(id, registration);
                }
            }
            if let Some(id) = request.unsubscribe.take()
                && result.as_ref().is_ok_and(|value| value == &json!(true))
                && let Some(subscription) = subscriptions.remove(&id)
            {
                let _ = subscription
                    .terminal
                    .send(Some(RpcError::SubscriptionClosed));
                if recently_unsubscribed.len() == MAX_RECENTLY_UNSUBSCRIBED {
                    recently_unsubscribed.pop_front();
                }
                recently_unsubscribed.push_back(id);
            }
            let _ = request.reply.send(result);
            Ok(())
        }
        (
            Slot::Absent,
            Slot::Present(method),
            Slot::Present(params),
            Slot::Absent,
            Slot::Absent,
        ) if method == json!("quai_subscription") => {
            let _ = params;
            let notification: NotificationFrame = serde_json::from_slice(bytes)
                .map_err(|_| RpcError::InvalidResponse("malformed subscription notification"))?;
            let notification = notification.params;
            let Some(subscription) = subscriptions.get(&notification.subscription) else {
                // A notification that raced its own unsubscribe acknowledgement
                // is expected, not a protocol violation, and must not fail the
                // whole multiplexed session: doing so would cancel every other
                // subscription and every in-flight request on this connection.
                if recently_unsubscribed.contains(&notification.subscription) {
                    return Ok(());
                }
                return Err(RpcError::InvalidResponse("unknown subscription ID"));
            };
            let permit = match notification_bytes
                .clone()
                .try_acquire_many_owned(bytes.len() as u32)
            {
                Ok(permit) => permit,
                Err(_) => {
                    let _ = subscription
                        .terminal
                        .send(Some(RpcError::SubscriptionLagged));
                    return Err(RpcError::Disconnected);
                }
            };
            match subscription.sender.try_send(QueuedNotification {
                payload: notification.result,
                _permit: permit,
            }) {
                Ok(()) => Ok(()),
                Err(mpsc::error::TrySendError::Full(_)) => {
                    let _ = subscription
                        .terminal
                        .send(Some(RpcError::SubscriptionLagged));
                    Err(RpcError::Disconnected)
                }
                Err(mpsc::error::TrySendError::Closed(_)) => Err(RpcError::Disconnected),
            }
        }
        _ => Err(RpcError::InvalidResponse("unexpected WebSocket envelope")),
    }
}
/// Feed frames through `dispatch` against a synthetic session: requests
/// `1..next_id` outstanding, request 1 optionally subscribing, request 2
/// optionally unsubscribing from "0x1", subscription "0x1" optionally live and
/// "0x2" optionally recently unsubscribed, as `layout` selects. Asserts that a
/// reply only ever reaches the request whose ID the frame carries, and that
/// nothing unissued is ever pending.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_dispatch(layout: u8, frames: &[&[u8]]) {
    let permits = Arc::new(Semaphore::new(1 << 16));
    let permit = || {
        permits
            .clone()
            .try_acquire_owned()
            .expect("fixture permits")
    };
    let mut subscription_queues = Vec::new();
    let mut registration = || {
        let (sender, queue) = mpsc::channel(2);
        let (terminal, watched) = watch::channel(None);
        subscription_queues.push((queue, watched));
        Registration {
            sender,
            terminal,
            _permit: permit(),
        }
    };
    let next_id = 1 + u64::from(layout % 8);
    let mut pending = HashMap::new();
    let mut replies = HashMap::new();
    for id in 1..next_id {
        let (reply, receiver) = oneshot::channel();
        pending.insert(
            id,
            Pending {
                reply,
                registration: (layout & 0x10 != 0 && id == 1).then(&mut registration),
                unsubscribe: (layout & 0x20 != 0 && id == 2).then(|| "0x1".to_owned()),
                _permit: permit(),
                deadline: Instant::now(),
            },
        );
        replies.insert(id, receiver);
    }
    let mut subscriptions = HashMap::new();
    if layout & 0x40 != 0 {
        subscriptions.insert("0x1".to_owned(), registration());
    }
    let mut recently_unsubscribed = VecDeque::new();
    if layout & 0x80 != 0 {
        recently_unsubscribed.push_back("0x2".to_owned());
    }
    let notification_bytes = Arc::new(Semaphore::new(4096));
    for frame in frames {
        let result = dispatch(
            frame,
            &mut pending,
            &mut subscriptions,
            &mut recently_unsubscribed,
            next_id,
            &notification_bytes,
        );
        let frame_id = serde_json::from_slice::<Value>(frame)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_u64));
        for (id, receiver) in &mut replies {
            if receiver.try_recv().is_ok() {
                assert_eq!(Some(*id), frame_id, "a reply reached another request");
            }
        }
        assert!(pending.keys().all(|id| *id != 0 && *id < next_id));
        if result.is_err() {
            // A real session closes here.
            break;
        }
    }
}

fn subscription_id(value: &Value) -> Result<&str, RpcError> {
    value
        .as_str()
        .filter(|id| {
            !id.is_empty() && id.len() <= 256 && id.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .ok_or(RpcError::InvalidResponse("invalid subscription ID"))
}
fn valid_method(method: &str) -> bool {
    !method.is_empty()
        && method.len() <= 128
        && method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
}
fn ws_error(error: tokio_tungstenite::tungstenite::Error) -> RpcError {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Http(response) => RpcError::HttpStatus(response.status().as_u16()),
        Error::Capacity(_) => RpcError::ResponseTooLarge,
        Error::Protocol(_) | Error::Utf8(_) => {
            RpcError::InvalidResponse("WebSocket protocol error")
        }
        _ => RpcError::Disconnected,
    }
}
fn encode_request(id: u64, method: &str, params: Value, max: usize) -> Result<String, RpcError> {
    struct BoundedWriter {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit - self.bytes.len() {
                return Err(std::io::Error::other("request size limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit: max,
    };
    serde_json::to_writer(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .map_err(|_| RpcError::RequestTooLarge)?;
    String::from_utf8(writer.bytes).map_err(|_| RpcError::InvalidConfig)
}

#[cfg(all(test, feature = "fuzzing"))]
mod fuzz_harness_tests {
    #[test]
    fn the_dispatch_harness_routes_representative_sessions_without_panicking() {
        let frames: [&[u8]; 8] = [
            br#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#,
            br#"{"jsonrpc":"2.0","id":2,"result":true}"#,
            br#"{"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"x"}}"#,
            br#"{"jsonrpc":"2.0","method":"quai_subscription","params":{"subscription":"0x1","result":{}}}"#,
            br#"{"jsonrpc":"2.0","method":"quai_subscription","params":{"subscription":"0x2","result":{}}}"#,
            br#"{"jsonrpc":"2.0","id":9,"result":"0x1"}"#,
            br#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#,
            b"not json",
        ];
        // Invalid UTF-8 inside an unknown field: rejected before routing.
        super::fuzz_dispatch(0x01, &[b"{\"jsonrpc\":\"2.0\",\"id\":1,\"x\":\"\xc5\"}"]);
        for layout in 0..=u8::MAX {
            for start in 0..frames.len() {
                super::fuzz_dispatch(layout, &frames[start..]);
            }
        }
    }
}
