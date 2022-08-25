use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::channel::mpsc;
use futures::{Future, Sink, SinkExt, Stream, StreamExt};
use slv_input::index;
use slv_proto::{client, server};
use tokio::sync::broadcast;
use tokio::{fs, io, net};

use crate::{session, Options};

pub async fn init(
    options: Options,
    mut shutdown: broadcast::Receiver<()>,
    index: Arc<index::Store>,
) -> Result<impl Future<Output = ()>, InitError> {
    let options = Arc::new(options);

    let listener = net::TcpListener::bind(options.addr).await.map_err(InitError::BindTcp)?;
    let tls_config = if options.tls {
        Some(Arc::new(
            init_tls(&options.certs, options.key.as_ref().ok_or_else(InitError::RequiredKey)?)
                .await?,
        ))
    } else {
        None
    };

    Ok(async move {
        loop {
            let accept = tokio::select! {
                accept = listener.accept() => accept,
                _ = shutdown.recv() => break,
            };
            let (stream, addr) = match accept {
                Ok(ret) => ret,
                Err(err) => {
                    log::debug!("Error accepting new client: {err}");
                    continue;
                }
            };
            let tls_config = tls_config.clone();
            tokio::spawn(handle(
                Arc::clone(&options),
                stream,
                addr,
                tls_config,
                Arc::clone(&index),
            ));
        }
    })
}

async fn init_tls(
    cert_paths: &[impl AsRef<Path>],
    key_path: &Path,
) -> Result<rustls::ServerConfig, InitError> {
    let mut certs = Vec::new();
    for path in cert_paths {
        let cert_data = fs::read(path).await.map_err(InitError::ReadCert)?;
        for cert in rustls_pemfile::certs(&mut &cert_data[..]).map_err(InitError::ReadCert)? {
            certs.push(rustls::Certificate(cert));
        }
    }

    let key_data = fs::read(key_path).await.map_err(InitError::ReadKey)?;
    let mut key_data_cursor = &key_data[..];
    let key = loop {
        let key_pem = rustls_pemfile::read_one(&mut key_data_cursor).map_err(InitError::ReadKey)?;
        break match key_pem {
            Some(
                rustls_pemfile::Item::RSAKey(key)
                | rustls_pemfile::Item::ECKey(key)
                | rustls_pemfile::Item::PKCS8Key(key),
            ) => rustls::PrivateKey(key),
            Some(_) => continue,
            None => return Err(InitError::NoPrivateKey(key_path.to_path_buf())),
        };
    };

    let config = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(InitError::BuildTlsConfig)?;

    Ok(config)
}

async fn handle(
    options: Arc<Options>,
    stream: impl io::AsyncRead + io::AsyncWrite + Unpin,
    from: SocketAddr,
    tls_config: Option<Arc<rustls::ServerConfig>>,
    index: Arc<index::Store>,
) {
    let ret = if let Some(tls_config) = tls_config {
        match tokio_rustls::TlsAcceptor::from(tls_config).accept(stream).await {
            Ok(tls_stream) => handle_raw(options, tls_stream, from, index).await,
            Err(err) => Err(RunError::Tls(err)),
        }
    } else {
        handle_raw(options, stream, from, index).await
    };

    match ret {
        Ok(never) => match never {},
        Err(RunError::EndOfStream) => {}
        Err(err) => {
            log::debug!("Error handling connection from {from}: {err}");
        }
    }
}

async fn handle_raw(
    options: Arc<Options>,
    stream: impl io::AsyncRead + io::AsyncWrite + Unpin,
    _from: SocketAddr,
    index: Arc<index::Store>,
) -> Result<Infallible, RunError> {
    let mut sock = tokio_tungstenite::accept_async(stream).await.map_err(RunError::WebSocket)?;

    async fn recv_msg(
        sock: &mut (impl Stream<Item = tungstenite::Result<tungstenite::Message>> + Unpin),
    ) -> Result<client::Message, RunError> {
        let raw = loop {
            let recv = match sock.next().await {
                Some(recv) => recv.map_err(RunError::Receive)?,
                None => return Err(RunError::EndOfStream),
            };
            match recv {
                tungstenite::Message::Close(_) => return Err(RunError::EndOfStream),
                tungstenite::Message::Binary(raw) => break raw,
                _ => continue,
            }
        };

        let message: client::Message =
            slv_proto::decode::from_read(&raw[..]).map_err(RunError::Decode)?;
        Ok(message)
    }

    async fn send_msg(
        sock: &mut (impl Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin),
        message: server::Message,
    ) -> Result<(), RunError> {
        let raw = slv_proto::encode::to_vec(&message).map_err(RunError::Encode)?;
        sock.send(tungstenite::Message::Binary(raw)).await.map_err(RunError::Send)?;
        Ok(())
    }

    let handshake = recv_msg(&mut sock).await?;
    let handshake = match handshake {
        client::Message::Handshake(handshake) => handshake,
        _ => return Err(RunError::NoHandshake),
    };
    if handshake.token != options.auth_token {
        return Err(RunError::BadAuthToken);
    }

    let (send_tx, mut send_rx) = mpsc::unbounded();
    let (mut recv_tx, recv_rx) = mpsc::unbounded();

    tokio::spawn({
        let index2 = Arc::clone(&index);
        async move {
            let index3 = index2;
            session::handle(recv_rx, send_tx, &index3).await
        }
    });

    loop {
        tokio::select! {
            send = send_rx.next() => {
                match send {
                    Some(message) => {
                        send_msg(&mut sock, message).await?;
                    }
                    None => return Err(RunError::EndOfStream),
                }
            }
            recv = recv_msg(&mut sock) => {
                let message = recv?;
                _ = recv_tx.send(message).await; // if receiver is dropped, we will handle it in the next iteration during send_rx.next()
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("Failed to bind TCP socket: {0}")]
    BindTcp(io::Error),
    #[error("Cannot read TLS certificate file: {0}")]
    ReadCert(io::Error),
    #[error("Missing `--key` option but TLS is enabled.")]
    RequiredKey(),
    #[error("Cannot read TLS key file: {0}")]
    ReadKey(io::Error),
    #[error("No private key found in {0}")]
    NoPrivateKey(PathBuf),
    #[error("Cannot build TLS configuration: {0}")]
    BuildTlsConfig(rustls::Error),
}

#[derive(Debug, thiserror::Error)]
enum RunError {
    #[error("TLS protocol error: {0}")]
    Tls(io::Error),
    #[error("WebSocket protocol error: {0}")]
    WebSocket(tungstenite::Error),
    #[error("WebSocket send error: {0}")]
    Send(tungstenite::Error),
    #[error("WebSocket receive error: {0}")]
    Receive(tungstenite::Error),
    #[error("Server message encode error: {0}")]
    Encode(slv_proto::encode::Error),
    #[error("Client message decode error: {0}")]
    Decode(slv_proto::decode::Error),
    #[error("First message in a connection must be a handshake")]
    NoHandshake,
    #[error("Client sent an incorrect auth token")]
    BadAuthToken,
    #[error("End of stream")]
    EndOfStream,
}
