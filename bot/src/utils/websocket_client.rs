use std::{collections::HashMap, future::ready, sync::Arc, time::Duration};

use futures::{SinkExt, StreamExt};
use futures_util::stream::BoxStream;
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use solana_client::rpc_config::RpcProgramAccountsConfig;
use solana_rpc_client_api::{
    error_object::RpcErrorObject,
    response::{Response, RpcKeyedAccount, SlotInfo},
};
use solana_sdk::pubkey::Pubkey;
use tokio::{
    sync::{broadcast, mpsc, Mutex},
    task::JoinHandle,
    time::sleep,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        protocol::{frame::coding::CloseCode, CloseFrame},
        Message,
    },
};

#[derive(Debug)]
pub enum WebsocketError {
    AlreadyConnected,
    NotConnected,
    SubscriptionFailed(String),
    ConnectionCouldNotBeEstablished(String),

    ConnectionError(tokio_tungstenite::tungstenite::Error),
    MessageParseError(serde_json::error::Error),
}

impl ToString for WebsocketError {
    fn to_string(&self) -> String {
        match self {
            Self::AlreadyConnected => "AlreadyConnected".to_string(),
            Self::NotConnected => "NotConnected".to_string(),
            Self::SubscriptionFailed(msg) => format!("SubscriptionFailed: {}", msg),
            Self::ConnectionCouldNotBeEstablished(msg) => {
                format!("ConnectionCouldNotBeEstablished: {}", msg)
            }
            Self::ConnectionError(e) => format!("SendError: {}", e.to_string()),
            Self::MessageParseError(e) => format!("MessageParseError: {}", e.to_string()),
        }
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for WebsocketError {
    fn from(value: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::ConnectionError(value)
    }
}

impl From<serde_json::error::Error> for WebsocketError {
    fn from(value: serde_json::error::Error) -> Self {
        Self::MessageParseError(value)
    }
}

#[derive(Clone, PartialEq, Debug)]
enum SubscribeParams {
    Slot,
    Program {
        program_id: Pubkey,
        config: RpcProgramAccountsConfig,
    },
}

impl SubscribeParams {
    pub fn notification_into_unsub_method(method: String) -> &'static str {
        match method.as_str() {
            "slotNotification" => "slotUnsubscribe",
            "programNotification" => "programUnsubscribe",
            _ => unreachable!(),
        }
    }

    pub fn build_unsubscribe_request(method: String, subscription_id: u64) -> String {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": [subscription_id]
        })
        .to_string()
    }

    pub fn build_subscribe_request_and_method(&self, request_id: u64) -> (String, String) {
        let m: String;
        let r = match self {
            Self::Slot => {
                m = "slotSubscribe".to_string();
                json!({ "jsonrpc": "2.0", "id": request_id, "method": m })
            }
            Self::Program { program_id, config } => {
                m = "programSubscribe".to_string();
                json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": m,
                    "params": [
                        program_id.to_string(),
                        config,
                    ],
                })
            }
        }
        .to_string();
        (r, m)
    }
}

pub type NotificationSender = mpsc::UnboundedSender<Value>;
pub type NotificationReceiver = mpsc::UnboundedReceiver<Value>;
pub type SubscriptionStatusSender =
    mpsc::Sender<Result<(u64, NotificationReceiver), WebsocketError>>;
pub type UnsubscriptionStatusSender = mpsc::Sender<()>;

type SubscribeRequest = (SubscribeParams, SubscriptionStatusSender);
pub type UnsubscribeRequest = (u64, UnsubscriptionStatusSender);

pub type SubscribeResponse<'a, T> = (u64, BoxStream<'a, T>);

pub struct PendingSubscription {
    method: String,
    status_sender: SubscriptionStatusSender,
}

#[derive(Debug)]
pub struct ActiveSubscription {
    method: String,
    notification_sender: NotificationSender,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ConnectionStatus {
    Connected,
    Reconnecting,
    // Only until first connection
    Disconnected,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self::Disconnected
    }
}

pub struct WebsocketClient {
    url: String,
    connection_status: Mutex<ConnectionStatus>,

    unsubscribe_sender: broadcast::Sender<UnsubscribeRequest>,
    subscribe_sender: broadcast::Sender<SubscribeRequest>,
    reconnect_sender: broadcast::Sender<mpsc::Sender<()>>,
}

impl WebsocketClient {
    pub fn new(url: String) -> Self {
        let (subscribe_sender, _) = broadcast::channel(100);
        let (unsubscribe_sender, _) = broadcast::channel(100);
        let (reconnect_sender, _) = broadcast::channel(1);

        Self {
            connection_status: Default::default(),
            url,
            subscribe_sender,
            unsubscribe_sender,
            reconnect_sender,
        }
    }

    pub async fn reconnect(&self) -> Result<(), WebsocketError> {
        let (status_sender, mut status_receiver) = mpsc::channel(1);

        self.reconnect_sender
            .send(status_sender)
            .map_err(|_| WebsocketError::NotConnected)?;

        status_receiver.recv().await;

        Ok(())
    }

    pub async fn program_subscribe(
        &self,
        program_id: Pubkey,
        config: RpcProgramAccountsConfig,
    ) -> Result<SubscribeResponse<Response<RpcKeyedAccount>>, WebsocketError> {
        self.subscribe(SubscribeParams::Program { program_id, config })
            .await
    }

    pub async fn slot_subscribe(&self) -> Result<SubscribeResponse<SlotInfo>, WebsocketError> {
        self.subscribe(SubscribeParams::Slot).await
    }

    async fn subscribe<'a, T: DeserializeOwned + Send + 'a>(
        &self,
        params: SubscribeParams,
    ) -> Result<(u64, BoxStream<'a, T>), WebsocketError> {
        let status = self.connection_status.lock().await.clone();

        let (subscription_id, receiver) = match status {
            ConnectionStatus::Disconnected => {
                return Err(WebsocketError::NotConnected);
            }
            _ => {
                let (status_sender, mut status_receiver) = mpsc::channel(1);

                self.subscribe_sender
                    .send((params, status_sender))
                    .map_err(|_| WebsocketError::NotConnected)?;

                if let Some(res) = status_receiver.recv().await {
                    res?
                } else {
                    return Err(WebsocketError::NotConnected);
                }
            }
        };

        let stream = UnboundedReceiverStream::new(receiver)
            .filter_map(|value| ready(serde_json::from_value::<T>(value).ok()))
            .boxed();

        Ok((subscription_id, stream))
    }

    pub async fn unsubscribe(&self, subscription_id: u64) {
        let status = self.connection_status.lock().await.clone();

        match status {
            ConnectionStatus::Connected => {
                let (status_sender, mut status_receiver) = mpsc::channel(1);

                self.unsubscribe_sender
                    .send((subscription_id, status_sender))
                    .ok();

                // if channel is closed, subscription is closed too
                status_receiver.recv().await;
            }
            _ => (),
        }
    }
}

pub async fn create_persisted_websocket_connection(
    client: Arc<WebsocketClient>,
) -> Result<JoinHandle<Result<(), WebsocketError>>, WebsocketError> {
    let status = client.connection_status.lock().await;
    if *status == ConnectionStatus::Connected || *status == ConnectionStatus::Reconnecting {
        return Err(WebsocketError::AlreadyConnected);
    }
    drop(status);

    let handle: JoinHandle<Result<(), WebsocketError>> = tokio::spawn(async move {
        type RequestId = u64;
        type SubscriptionId = u64;

        // TODO:
        // Can potentially remain pending forever if websocket reconnects
        // while the subscription is pending
        let mut pending_subscriptions: HashMap<RequestId, PendingSubscription> = HashMap::new();
        let mut pending_reconnect: Option<mpsc::Sender<()>> = None;

        let mut subscribe_receiver = client.subscribe_sender.subscribe();
        let mut unsubscribe_receiver = client.unsubscribe_sender.subscribe();
        let mut reconnect_receiver = client.reconnect_sender.subscribe();

        loop {
            let mut request_id: RequestId = 1;

            println!("Connecting to ws");
            let mut conn_status = client.connection_status.lock().await;
            let (mut ws, _response) = connect_async(&client.url)
                .await
                .map_err(|e| WebsocketError::ConnectionCouldNotBeEstablished(e.to_string()))?;
            *conn_status = ConnectionStatus::Connected;
            drop(conn_status);

            if let Some(status_sender) = pending_reconnect {
                status_sender.send(()).await.ok();
                pending_reconnect = None;
            }

            let mut active_subscriptions: HashMap<SubscriptionId, ActiveSubscription> =
                HashMap::new();
            let mut pending_unsubscriptions: HashMap<RequestId, UnsubscriptionStatusSender> =
                HashMap::default();

            loop {
                tokio::select! {
                    Ok(status_sender) = reconnect_receiver.recv() =>
                    {
                        #[allow(unused_assignments)]
                        {
                            pending_reconnect = Some(status_sender);
                        }

                        let frame = CloseFrame { code: CloseCode::Normal, reason: "".into() };
                        ws.send(Message::Close(Some(frame))).await?;
                        ws.flush().await?;

                        break;
                    }
                    Ok((subscription_id, status_sender)) = unsubscribe_receiver.recv() => {
                        let Some(ActiveSubscription { method, .. }) = active_subscriptions.remove(&subscription_id) else {
                            status_sender.send(()).await.ok();
                            continue;
                        };

                        println!("Unsubcribing {}: rid {}", subscription_id, request_id);

                        let req = SubscribeParams::build_unsubscribe_request(method, request_id);
                        ws.send(Message::Text(req)).await?;

                        request_id += 1;
                    }
                    Ok((subscribe_params, status_sender)) = subscribe_receiver.recv() => {
                        let (req, method) = subscribe_params.build_subscribe_request_and_method(request_id);
                        ws.send(Message::Text(req)).await?;
                        println!("Subscribing {}: {}", &method, request_id);
                        pending_subscriptions.insert(request_id, PendingSubscription { method, status_sender });
                        request_id += 1;
                    }
                    _ = sleep(Duration::from_secs(5)) => {
                        ws.send(Message::Ping(vec![])).await?;
                    }
                    Some(msg) = ws.next() => {
                        let Ok(msg) = msg else {
                            println!("Websocket message error: {}", msg.err().unwrap().to_string());
                            break;
                        };
                        let text = match msg {
                            Message::Text(v) => v,
                            Message::Ping(data) => {
                                ws.send(Message::Pong(data)).await?;
                                continue;
                            }
                            Message::Close(reason) => {
                                dbg!(reason);
                                break;
                            }
                            _ => {
                                continue;
                            }
                        };
                        let response: Map<String, Value> = serde_json::from_str(&text)?;

                        match response.get("id").map(|id| id.as_u64()).flatten() {
                            Some(r_id) => {
                                let err = response.get("error").map(|error_object| {
                                    match serde_json::from_value::<RpcErrorObject>(error_object.clone()) {
                                        Ok(rpc_error_object) => {
                                            format!("{} ({})",  rpc_error_object.message, rpc_error_object.code)
                                        }
                                        Err(err) => format!(
                                            "Failed to deserialize RPC error response: {} [{}]",
                                            serde_json::to_string(error_object).unwrap(),
                                            err
                                        )
                                    }
                                });

                                if let Some(status_sender) = pending_unsubscriptions.remove(&r_id) {
                                    println!("confirming unsub {}", r_id);
                                    status_sender.send(()).await.ok();
                                    continue;
                                }

                                if let Some(PendingSubscription { method, status_sender }) = pending_subscriptions.remove(&r_id) {
                                    match err {
                                        Some(msg) => {
                                            status_sender.send(Err(WebsocketError::SubscriptionFailed(format!("{}: {}", msg, text.clone())))).await.ok();
                                            continue;
                                        }
                                        _ => {
                                            let Some(s_id) = response.get("result").map(|id| id.as_u64()).flatten() else {
                                                status_sender.send(Err(WebsocketError::SubscriptionFailed(format!("Invalid result field: {}", text.clone())))).await.ok();
                                                continue;
                                            };

                                            println!("Confirmed subscription {}, {}", &method, r_id);

                                            let (notification_sender, notification_receiver) = mpsc::unbounded_channel();

                                            if status_sender.send(Ok((s_id, notification_receiver))).await.is_ok() {
                                                active_subscriptions.insert(s_id, ActiveSubscription { method, notification_sender });
                                            }
                                        }
                                    }
                                    continue;
                                }
                            }
                            None => {
                                let Some(Value::Object(params)) = response.get("params") else {
                                    continue;
                                };

                                let s_id = params.get("subscription").map(|id| id.as_u64()).flatten();
                                let result = params.get("result");
                                let method = response.get("method").map(|m| m.as_str()).flatten();

                                match (s_id, result, method) {
                                    (Some(s_id), Some(result), Some(method)) => {
                                        let mut should_unsub = false;

                                        if let Some(subscription) = active_subscriptions.get(&s_id) {
                                            if !subscription.notification_sender.send(result.clone()).is_ok() {
                                                println!("Subscription no longer active, remove");
                                                active_subscriptions.remove(&s_id);
                                                should_unsub = true;
                                            }
                                        } else {
                                            should_unsub = true;
                                        }

                                        if should_unsub {
                                            println!("Subscription no longer active, unsub");
                                            let unsub_method = SubscribeParams::notification_into_unsub_method(method.to_string());
                                            let req = SubscribeParams::build_unsubscribe_request(unsub_method.to_string(), request_id);

                                            ws.send(Message::Text(req)).await?;

                                            request_id += 1;
                                        }
                                    }
                                    _ => ()
                                }
                            }
                        }

                    }
                }
            }

            *client.connection_status.lock().await = ConnectionStatus::Reconnecting;
        }
    });

    Ok(handle)
}
