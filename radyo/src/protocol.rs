use anyhow::Result;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::future::Future;
use crate::call::incoming_call_handler;

pub const ALPN: &[u8] = b"radyo/1.0";

#[derive(Debug, Clone)]
pub struct RadyoProtocol;

impl ProtocolHandler for RadyoProtocol {
    fn accept(&self, conn: Connection) -> impl Future<Output = Result<(), AcceptError>> + Send {
        async move {
            // Spawn each call handler concurrently to allow multiple calls
            tokio::spawn(async move {
                incoming_call_handler(conn).await;
            });
            Ok(())
        }
    }
}
