use crate::config::{ConfigError, ConnectConfig};
use crate::proto::{frdrpc, litrpc, lnrpc, looprpc, routerrpc, taprpc};
use crate::tls;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::Duration;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status};

/// Adds the hex macaroon to every request.
#[derive(Clone)]
pub struct Macaroon(MetadataValue<Ascii>);

impl tonic::service::Interceptor for Macaroon {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        req.metadata_mut().insert("macaroon", self.0.clone());
        Ok(req)
    }
}

pub type Svc = InterceptedService<Channel, Macaroon>;

/// One authenticated HTTP/2 connection to litd/LND. Cheap to clone; every
/// accessor returns a client sharing the same connection.
#[derive(Clone)]
pub struct LitdClient {
    channel: Channel,
    macaroon: Macaroon,
}

impl LitdClient {
    pub fn lightning(&self) -> lnrpc::lightning_client::LightningClient<Svc> {
        lnrpc::lightning_client::LightningClient::with_interceptor(self.channel.clone(), self.macaroon.clone())
            .max_decoding_message_size(64 * 1024 * 1024)
    }
    pub fn router(&self) -> routerrpc::router_client::RouterClient<Svc> {
        routerrpc::router_client::RouterClient::with_interceptor(self.channel.clone(), self.macaroon.clone())
    }
    /// litd sub-server status (litd's own RPC port only).
    pub fn status(&self) -> litrpc::status_client::StatusClient<Svc> {
        litrpc::status_client::StatusClient::with_interceptor(self.channel.clone(), self.macaroon.clone())
    }
    pub fn taproot_assets(&self) -> taprpc::taproot_assets_client::TaprootAssetsClient<Svc> {
        taprpc::taproot_assets_client::TaprootAssetsClient::with_interceptor(
            self.channel.clone(),
            self.macaroon.clone(),
        )
    }
    pub fn loop_client(&self) -> looprpc::swap_client_client::SwapClientClient<Svc> {
        looprpc::swap_client_client::SwapClientClient::with_interceptor(self.channel.clone(), self.macaroon.clone())
    }
    pub fn faraday(&self) -> frdrpc::faraday_server_client::FaradayServerClient<Svc> {
        frdrpc::faraday_server_client::FaradayServerClient::with_interceptor(
            self.channel.clone(),
            self.macaroon.clone(),
        )
    }
    /// The wallet unlocker answers before a macaroon exists (create/unlock).
    pub fn unlocker(&self) -> lnrpc::wallet_unlocker_client::WalletUnlockerClient<Channel> {
        lnrpc::wallet_unlocker_client::WalletUnlockerClient::new(self.channel.clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("transport: {0}")]
    Transport(#[from] tonic::transport::Error),
}

/// Connect with the node's pinned certificate and macaroon.
pub async fn connect(cfg: &ConnectConfig) -> Result<LitdClient, ConnectError> {
    let mac_hex = cfg.read_macaroon_hex()?;
    let macaroon =
        Macaroon(mac_hex.parse().map_err(|_| ConfigError::Endpoint("macaroon is not valid metadata".into()))?);
    let channel = channel(&cfg.endpoint, cfg.read_cert_der()?).await?;
    Ok(LitdClient { channel, macaroon })
}

/// The wallet unlocker (GenSeed / InitWallet / UnlockWallet) answers before
/// any macaroon exists, so it needs only the pinned certificate.
pub async fn connect_unlocker(
    cfg: &ConnectConfig,
) -> Result<lnrpc::wallet_unlocker_client::WalletUnlockerClient<Channel>, ConnectError> {
    let channel = channel(&cfg.endpoint, cfg.read_cert_der()?).await?;
    Ok(lnrpc::wallet_unlocker_client::WalletUnlockerClient::new(channel))
}

async fn channel(endpoint: &str, cert: Vec<u8>) -> Result<Channel, ConnectError> {
    let (host, port) = endpoint
        .rsplit_once(':')
        .and_then(|(h, p)| p.parse::<u16>().ok().map(|p| (h.trim_matches(['[', ']']).to_string(), p)))
        .ok_or_else(|| ConfigError::Endpoint(endpoint.to_string()))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(tls::client_config(cert)));

    let connector = tower::service_fn(move |_uri: http::Uri| {
        let tls = tls.clone();
        let (host, port) = (host.clone(), port);
        async move {
            let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
            tcp.set_nodelay(true)?;
            // The name is irrelevant under an exact pin; LND certs always carry "localhost".
            let name = ServerName::try_from("localhost").expect("static name").to_owned();
            let stream = tls.connect(name, tcp).await?;
            Ok::<_, std::io::Error>(TokioIo::new(stream))
        }
    });

    // Plain-http URI: TLS is done by our connector, not by tonic.
    Ok(Endpoint::from_shared(format!("http://{endpoint}"))?
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .connect_with_connector(connector)
        .await?)
}
