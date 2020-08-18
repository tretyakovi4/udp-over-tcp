use err_context::{BoxedErrorExt as _, ResultExt as _};
use std::convert::TryFrom;
use std::fmt;
use std::io;
use std::net::SocketAddrV4;
use structopt::StructOpt;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};

#[derive(Debug, StructOpt)]
#[structopt(name = "udp2tcp", about = "Listen for incoming UDP and forward to TCP")]
struct Options {
    /// The IP and UDP port to bind to and accept incoming connections on.
    udp_listen_addr: SocketAddrV4,

    /// The IP and TCP port to forward all UDP traffic to.
    tcp_forward_addr: SocketAddrV4,

    #[structopt(flatten)]
    tcp_options: udp_over_tcp::TcpOptions,
}

#[derive(Debug)]
pub enum Udp2TcpError {
    ConnectTcp(io::Error),
}

impl fmt::Display for Udp2TcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Udp2TcpError::*;
        match self {
            ConnectTcp(_) => "Failed to connect to TCP forward address".fmt(f),
        }
    }
}

impl std::error::Error for Udp2TcpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use Udp2TcpError::*;
        match self {
            ConnectTcp(e) => Some(e),
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let options = Options::from_args();
    if let Err(error) = run(options).await {
        log::error!("Error: {}", error.display("\nCaused by: "));
        std::process::exit(1);
    }
}

async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut tcp_stream = TcpStream::connect(options.tcp_forward_addr)
        .await
        .map_err(Udp2TcpError::ConnectTcp)?;
    log::info!("Connected to {}/TCP", options.tcp_forward_addr);
    udp_over_tcp::apply_tcp_options(&tcp_stream, &options.tcp_options)?;

    let mut udp_socket = UdpSocket::bind(options.udp_listen_addr)
        .await
        .with_context(|_| format!("Failed to bind UDP socket to {}", options.udp_listen_addr))?;
    log::info!("Listening on {}/UDP", udp_socket.local_addr().unwrap());

    let mut buffer = [0u8; 2 + 1024 * 64];
    let (udp_read_len, udp_peer_addr) = udp_socket
        .recv_from(&mut buffer[2..])
        .await
        .context("Failed receiving the first packet")?;
    log::info!(
        "Incoming connection from {}/UDP, forwarding to {}/TCP",
        udp_peer_addr,
        options.tcp_forward_addr
    );

    udp_socket
        .connect(udp_peer_addr)
        .await
        .with_context(|_| format!("Failed to connect UDP socket to {}", udp_peer_addr))?;

    let datagram_len = u16::try_from(udp_read_len).unwrap();
    buffer[..2].copy_from_slice(&datagram_len.to_be_bytes()[..]);
    tcp_stream
        .write_all(&buffer[..2 + udp_read_len])
        .await
        .context("Failed writing to TCP")?;
    log::trace!("Forwarded {} bytes UDP->TCP", udp_read_len);

    udp_over_tcp::process_udp_over_tcp(udp_socket, tcp_stream).await;
    log::trace!(
        "Closing forwarding for {}/UDP <-> {}/TCP",
        udp_peer_addr,
        options.tcp_forward_addr,
    );

    Ok(())
}