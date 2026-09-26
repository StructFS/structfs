#![cfg(feature = "streaming")]
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    time::Duration,
};
use structfs_http::{
    streaming::{AsyncHttpExecutor, AsyncReqwestExecutor},
    HttpRequest,
};
/// Bind a loopback listener, or `None` when the environment forbids it.
///
/// These tests drive a real socket on purpose — that is what proves the head
/// arrives before the body and that dropping the response closes the
/// connection. Sandboxes that deny `bind(2)` cannot run them, so they skip
/// with a visible note rather than failing.
fn loopback_listener() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => Some(listener),
        Err(error) => {
            eprintln!(
                "skipping: this environment denies TcpListener::bind on loopback ({error}); \
                 the streaming tests need a real socket"
            );
            None
        }
    }
}

#[tokio::test]
async fn forwards_request_exposes_head_early_and_disconnect_releases_body() {
    let Some(listener) = loopback_listener() else {
        return;
    };
    let address = listener.local_addr().unwrap();
    let (head_tx, head_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.contains("q=hello+world"));
        assert!(request.to_lowercase().contains("x-probe: yes"));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nX-Head: ready\r\n\r\n")
            .unwrap();
        head_rx.recv().unwrap();
        let closed = matches!(socket.read(&mut byte), Ok(0));
        closed_tx.send(closed).unwrap();
    });
    let executor = AsyncReqwestExecutor::new(Duration::from_secs(3)).unwrap();
    let mut request = HttpRequest::get(format!("http://{address}/stream"));
    request.query.insert("q".into(), "hello world".into());
    request.headers.insert("X-Probe".into(), "yes".into());
    let response = tokio::time::timeout(Duration::from_secs(2), executor.execute(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.headers["x-head"], "ready");
    drop(response);
    head_tx.send(()).unwrap();
    assert!(closed_rx.await.unwrap());
    server.join().unwrap();
}
#[tokio::test]
async fn bounded_error_bodies_and_transport_failure_do_not_require_full_buffering() {
    for (body, max, success, extra) in [
        ("abcdef", 3, false, 0),
        ("abc", 3, true, 0),
        ("abc", 64, false, 5),
    ] {
        let Some(listener) = loopback_listener() else {
            return;
        };
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(
                socket,
                "HTTP/1.1 500 Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len() + extra,
                body
            )
            .unwrap();
        });
        let response = AsyncReqwestExecutor::new(Duration::from_secs(2))
            .unwrap()
            .execute(HttpRequest::get(format!("http://{address}/")))
            .await
            .unwrap();
        assert_eq!(response.read_limited(max).await.is_ok(), success);
        server.join().unwrap();
    }
}
