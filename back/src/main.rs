use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::{routing::get, Extension, Router};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use shuttle_axum::ShuttleAxum;
use shuttle_runtime::SecretStore;
use std::borrow::Borrow;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::{mpsc::UnboundedSender, RwLock};
use tower_http::auth::AddAuthorizationLayer;
#[derive(Serialize, Deserialize, Debug)]
struct Msg {
    name: String,
    uid: Option<usize>,
    message: String,
}
#[derive(Serialize, Deserialize, Debug)]
struct Msg1 {
    name: String,
    message: String,
}
type Users = Arc<RwLock<HashMap<usize, UnboundedSender<Message>>>>;
static NEXT_USERID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

#[shuttle_runtime::main]
async fn axum(#[shuttle_runtime::Secrets] secrets: SecretStore) -> ShuttleAxum {
    // set up router with secrets
    let secret = secrets.get("BEARER").unwrap_or("Bear".to_owned());

    let static_folder = PathBuf::new();
    let router = router(secret, static_folder);
    Ok(router.into())
}

fn router(secret: String, static_folder: PathBuf) -> Router {
    // initialize the users k/v store and allow the static files to be served
    let users = Users::default();
    // return a new router with a websocket route
    let admin = Router::new()
        .route("/disconnect/:user_id", get(disconnect_user))
        .layer(AddAuthorizationLayer::bearer(&secret));

    Router::new()
        .route("/ws", get(ws_handler))
        .nest("/admin", admin)
        .layer(Extension(users)) // used for sharing state across handler functions
}

// "impl IntoResponse" means we want our function to return a websocket connection
async fn ws_handler(ws: WebSocketUpgrade, Extension(state): Extension<Users>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}
async fn handle_socket(stream: WebSocket, state: Users) {
    // When a new user enters the chat (opens the websocket connection), assign then a user ID
    let my_id = NEXT_USERID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // By splitting the websocket into a receiver and sender, we can send and receive at the same time.
    let (mut sender, mut receiver) = stream.split();

    // Create a new channel for aync task management (stored in users hashmap)
    let (tx, mut rx) = unbounded_channel::<Message>();

    // If a message has been received, send the message (expect on error)
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            sender.send(msg).await.expect("Error while sending message");
        }
        sender.close().await.unwrap();
    });
    // If there's a message and the message is OK, broadcast it along all available open websocket connections
    while let Some(Ok(result)) = receiver.next().await {
        // let _res: Msg1 =
        //     serde_json::from_str(result.clone().into_text().unwrap().borrow()).unwrap();
        // println!("{:#?}", _res);
        if let Ok(result) = enrich_results(result, my_id) {
            broadcast_msg(result, &state).await;
        }
    }
}
async fn broadcast_msg(msg: Message, users: &Users) {
    if let Message::Text(msg) = msg {
        for (&_uid, tx) in users.read().await.iter() {
            tx.send(Message::Text(msg.clone()))
                .expect("Failed to send message")
        }
    }
}
fn enrich_results(result: Message, id: usize) -> Result<Message, serde_json::Error> {
    match result {
        Message::Text(msg) => {
            let mut msg: Msg = serde_json::from_str(&msg)?;
            msg.uid = Some(id);
            let msg = serde_json::to_string(&msg)?;
            Ok(Message::Text(msg))
        }
        _ => Ok(result),
    }
}
async fn disconnect_user(
    Path(user_id): Path<usize>,
    Extension(users): Extension<Users>,
) -> impl IntoResponse {
    disconnect(user_id, &users).await;
    "Done"
}
async fn disconnect(my_id: usize, users: &Users) {
    println!("Goodbye user {}", my_id);
    users.write().await.remove(&my_id);
    println!("Disconnected {my_id}");
}
