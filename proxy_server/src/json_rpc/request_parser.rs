use std::fmt::Display;

use axum::{
    body::{Bytes, HttpBody},
    http::StatusCode,
};

#[derive(Clone)]
pub struct JsonrpcRequestParser;

impl JsonrpcRequestParser {
    pub async fn get_req_body<B>(body: B) -> Result<Bytes, (StatusCode, String)>
    where
        B: HttpBody<Data = Bytes>,
        B::Error: Display,
    {
        let bytes = match hyper::body::to_bytes(body).await {
            Ok(bytes) => bytes,
            Err(err) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("failed to read request body: {}", err),
                ));
            }
        };
        Ok(bytes)
    }
}
