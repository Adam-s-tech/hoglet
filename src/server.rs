//! The HTTP listener: `axum::serve` plus the limits it does not set.
//!
//! A client that opens a connection and dribbles a header (or a body) holds a
//! socket and a task for as long as it likes under plain `axum::serve`. Here
//! the header must arrive within [`HEADER_READ_TIMEOUT`], a whole request
//! must complete within [`REQUEST_TIMEOUT`], and open connections are capped.
//! Each request also carries its TCP peer address (`security::PeerAddr`) for
//! the login throttle.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tower_http::timeout::TimeoutLayer;

use crate::security::PeerAddr;

/// Time a client has to send a complete request header.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Time a request has, header to last response byte. Longer than the query
/// engine's own 30 s deadline.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Open connections accepted at once; further clients wait in the OS backlog.
pub const MAX_CONNECTIONS: usize = 4096;
/// How long shutdown waits for in-flight requests.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Serve `router` on `listener` until `shutdown` resolves, then drain.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    serve_with(
        listener,
        router,
        shutdown,
        HEADER_READ_TIMEOUT,
        REQUEST_TIMEOUT,
        MAX_CONNECTIONS,
    )
    .await
}

/// [`serve`] with explicit limits (tests use short ones).
pub async fn serve_with(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()>,
    header_read_timeout: Duration,
    request_timeout: Duration,
    max_connections: usize,
) -> std::io::Result<()> {
    let router = router.layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        request_timeout,
    ));
    let graceful = GracefulShutdown::new();
    let connections = Arc::new(Semaphore::new(max_connections));
    tokio::pin!(shutdown);
    loop {
        let permit = tokio::select! {
            permit = connections.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
            () = &mut shutdown => break,
        };
        let (stream, peer): (_, SocketAddr) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Out of descriptors or a reset handshake: back off, keep serving.
                    tracing::warn!(%error, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let _ = stream.set_nodelay(true);
        let service = TowerToHyperService::new(
            router.clone().layer(axum::Extension(PeerAddr(peer.ip()))),
        );
        let mut builder = Builder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(header_read_timeout);
        builder.http2().timer(TokioTimer::new());
        let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
        let connection = graceful.watch(connection.into_owned());
        tokio::spawn(async move {
            let _ = connection.await;
            drop(permit);
        });
    }
    drop(listener);
    if tokio::time::timeout(DRAIN_TIMEOUT, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("shutdown: connections still open after {DRAIN_TIMEOUT:?}, closing");
    }
    Ok(())
}
