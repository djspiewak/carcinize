use crate::github::USER_AGENT;
use crate::{
    error::{Failure, Result},
    http::{REQUEST_TIMEOUT, authorization, service_url},
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde_json::json;
use tokio::{net::TcpStream, task::JoinHandle};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(super) struct Feed {
    writer: SplitSink<Socket, Message>,
    reader: JoinHandle<Result<()>>,
}

impl Feed {
    pub async fn connect(url: &str, token: &str) -> Result<Self> {
        let url = websocket_url(url)?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| Failure::Protocol("invalid live feed request"))?;
        request
            .headers_mut()
            .insert("authorization", authorization("Bearer", token)?);
        request.headers_mut().insert(
            "user-agent",
            USER_AGENT
                .parse()
                .map_err(|_| Failure::Protocol("invalid user agent"))?,
        );
        let (socket, _) =
            tokio::time::timeout(REQUEST_TIMEOUT, tokio_tungstenite::connect_async(request))
                .await
                .map_err(|_| Failure::Protocol("live feed connection timed out"))?
                .map_err(|_| Failure::Protocol("live feed connection failed"))?;
        let (writer, mut reader) = socket.split();
        let reader = tokio::spawn(async move {
            while let Some(message) = reader.next().await {
                match message {
                    Ok(Message::Close(_)) => break,
                    Err(_) => return Err(Failure::Protocol("live feed disconnected")),
                    _ => {}
                }
            }
            Ok(())
        });
        Ok(Self { writer, reader })
    }

    pub async fn send(&mut self, step: Uuid, lines: &[String]) -> Result<()> {
        let mut start = 1;
        for batch in lines.chunks(100) {
            if self.reader.is_finished() {
                return Err(Failure::Protocol("live feed closed before logs were sent"));
            }
            let message =
                json!({"count":batch.len(),"value":batch,"stepId":step,"startLine":start});
            tokio::time::timeout(
                REQUEST_TIMEOUT,
                self.writer.send(Message::Text(message.to_string().into())),
            )
            .await
            .map_err(|_| Failure::Protocol("live feed send timed out"))?
            .map_err(|_| Failure::Protocol("live feed send failed"))?;
            start += batch.len();
        }
        Ok(())
    }

    pub async fn close(&mut self) -> Result<()> {
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), self.writer.close()).await;
        self.reader.abort();
        match (&mut self.reader).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(error.into()),
        }
        result
            .map_err(|_| Failure::Protocol("live feed close timed out"))?
            .map_err(|_| Failure::Protocol("live feed close failed"))
    }
}

fn websocket_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value).map_err(|_| Failure::Protocol("invalid live feed URL"))?;
    // Validate the corresponding HTTP URL with the same TLS/authority policy.
    let http_scheme = match url.scheme() {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        _ => return Err(Failure::Protocol("unsupported live feed URL scheme")),
    };
    url.set_scheme(http_scheme)
        .map_err(|_| Failure::Protocol("invalid live feed URL"))?;
    service_url(url.as_str())?;
    url.set_scheme(if http_scheme == "https" { "wss" } else { "ws" })
        .map_err(|_| Failure::Protocol("invalid live feed URL"))?;
    Ok(url)
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
