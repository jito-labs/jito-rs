use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{body::StreamBody, http::HeaderMap, response::IntoResponse, routing::post, Router};
use tokio::task::JoinHandle;

use crate::json_rpc::signer::{SignerContext, SignerMiddleware};

pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    println!("signal received, starting graceful shutdown");
}

#[derive(Debug, Clone)]
pub struct ProxyServerImpl {
    block_engine_url: String,
    client: reqwest::Client,
}

impl ProxyServerImpl {
    fn new(block_engine_url: String) -> Self {
        Self {
            block_engine_url,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        }
    }

    /// Method to handle the incoming json rpc requests
    async fn handle_jsonrpc(
        &self,
        headers: HeaderMap,
        payload: String,
    ) -> Result<impl IntoResponse, (http::StatusCode, String)> {
        println!("Sending request {:?} to {}", payload, self.block_engine_url);
        let response = self
            .client
            .post(self.block_engine_url.clone())
            .headers(headers)
            .body(payload)
            .send()
            .await
            .map_err(|err| (http::StatusCode::BAD_GATEWAY, err.to_string()))?;
        if response.status().is_success() {
            Ok(StreamBody::new(response.bytes_stream()))
        } else {
            Err((response.status(), response.status().to_string()))
        }
    }

    /// Creates and runs a json rpc server. The server binds to the given port of localhost
    pub async fn run(block_engine_url: String, keypair_path: String, port: u16) -> JoinHandle<()> {
        println!(
            "Starting proxy server at port: {}, with json_rpc_url: {} and keypair path: {}",
            port, block_engine_url, keypair_path
        );
        let handler = ProxyServerImpl::new(block_engine_url);
        let auth_context = SignerContext::new(keypair_path);

        let app = Router::new()
            .route(
                "/api/v1/bundles",
                post(|headers: HeaderMap, payload: String| async move {
                    handler.handle_jsonrpc(headers, payload).await
                }),
            )
            .layer(axum::middleware::from_fn(SignerMiddleware::sign))
            .layer(axum::Extension(auth_context));

        tokio::spawn(async move {
            axum::Server::bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))
                .serve(app.into_make_service())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .unwrap()
        })
    }
}
