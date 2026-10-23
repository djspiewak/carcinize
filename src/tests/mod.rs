use axum::Router;
use tokio::{net::TcpListener, task::JoinHandle};
use url::Url;

pub(crate) mod contracts;
mod github_fake;
mod lifecycle;
mod registration;
mod transport;

pub(crate) struct Server {
    pub(crate) base: Url,
    task: JoinHandle<()>,
}

impl Server {
    pub(crate) async fn start(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake");
        let base = Url::parse(&format!(
            "http://{}/",
            listener.local_addr().expect("address")
        ))
        .expect("URL");
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("fake server");
        });
        Self { base, task }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
