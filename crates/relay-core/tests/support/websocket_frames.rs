use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, WebSocket};
use std::time::Duration;

pub(super) async fn receive_websocket_data_with_timeout(
    socket: &mut WebSocket,
    timeout_duration: Duration,
) -> Message {
    tokio::time::timeout(timeout_duration, async {
        loop {
            match socket
                .next()
                .await
                .expect("websocket closed before a data message")
                .expect("websocket message failed")
            {
                message @ (Message::Text(_) | Message::Binary(_)) => return message,
                Message::Ping(payload) => {
                    socket.send(Message::Pong(payload)).await.unwrap();
                }
                Message::Pong(_) => {}
                message => panic!("websocket closed before a data message: {message:?}"),
            }
        }
    })
    .await
    .expect("websocket data message timed out")
}
