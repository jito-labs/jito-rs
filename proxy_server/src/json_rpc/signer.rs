use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use solana_sdk::{
    signature::{read_keypair_file, Signer},
    signer::keypair::Keypair,
};

use crate::json_rpc::consts::JsonRpcConsts;
use crate::json_rpc::request_parser::JsonrpcRequestParser;

#[derive(Clone)]
pub struct SignerContext {
    signer_keypair: Arc<Keypair>,
}

impl SignerContext {
    pub fn new(keypair_path: String) -> Self {
        Self {
            signer_keypair: Arc::new(
                read_keypair_file(keypair_path).expect("failed to read keypair file"),
            ),
        }
    }

    pub fn sign(&self, payload: &Bytes) -> Result<String, (StatusCode, String)> {
        Ok(self
            .signer_keypair
            .try_sign_message(payload)
            .map_err(|err| (StatusCode::PRECONDITION_FAILED, err.to_string()))?
            .to_string())
    }
}

#[derive(Clone)]
pub struct SignerMiddleware;

impl SignerMiddleware {
    fn sign_request(
        mut headers: HeaderMap,
        req: &Bytes,
        signer_context: &SignerContext,
    ) -> Result<HeaderMap, (StatusCode, String)> {
        let signed_payload = signer_context.sign(req)?;
        headers.insert(
            JsonRpcConsts::AUTHORIZATION_HEADER,
            signed_payload.parse().unwrap(),
        );
        headers.insert(
            JsonRpcConsts::SENDER_PUBKEY_HEADER,
            signer_context
                .signer_keypair
                .pubkey()
                .to_string()
                .parse()
                .unwrap(),
        );

        Ok(headers)
    }

    pub async fn sign(
        req: Request<Body>,
        next: Next<Body>,
    ) -> Result<Response, (StatusCode, String)> {
        let state = req
            .extensions()
            .get::<SignerContext>()
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    StatusCode::INTERNAL_SERVER_ERROR.to_string(),
                )
            })?
            .clone();

        let (mut parts, body) = req.into_parts();
        let bytes = JsonrpcRequestParser::get_req_body(body).await?;

        let headers = Self::sign_request(parts.headers, &bytes, &state)?;
        parts.headers = headers;

        let req = Request::from_parts(parts, Body::from(bytes));

        Ok(next.run(req).await)
    }
}
