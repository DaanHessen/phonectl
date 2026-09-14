use std::process::ExitCode;
use std::time::Duration;

use adb::HostKey;
use adb::discovery::{Service, find};
use clap::{Parser, Subcommand};
use phone::paths;

/// Exit codes, kept stable so scripts can rely on them.
mod exit {
    pub const OK: u8 = 0;
    pub const FAILED: u8 = 1;
    pub const NOT_CONNECTED: u8 = 4;
    pub const PAIRING_REQUIRED: u8 = 6;
}

#[derive(Debug, Parser)]
#[command(name = "phonectl", version, about = "Use your Android phone from Linux")]
struct Cli {
    /// Only talk to this phone (mDNS instance name, see `phonectl devices`).
    #[arg(long, global = true)]
    device: Option<String>,

    /// How long to look for the phone on the network.
    #[arg(long, global = true, default_value = "5", value_name = "SECONDS")]
    timeout: u64,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List phones visible on the network.
    Devices,
    /// Pair with a phone that is showing a pairing code.
    Pair {
        /// The six-digit code from Wireless debugging > Pair device.
        code: String,
        /// Pairing endpoint, if mDNS cannot find it (IP:PORT).
        #[arg(long)]
        address: Option<std::net::SocketAddr>,
    },
    /// Connect and print what the phone reports about itself.
    Info,
    /// Run a command on the phone.
    Shell {
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PHONECTL_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("phonectl: cannot start: {e}");
            return ExitCode::from(exit::FAILED);
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("phonectl: {}", describe(&e));
            ExitCode::from(exit_code_for(&e))
        }
    }
}

/// Turns a library error into something the user can act on.
fn describe(error: &adb::Error) -> String {
    match error {
        adb::Error::NotFound => "no phone found. Is Wireless debugging on, and is the phone on this \
network? Settings > System > Developer options > Wireless debugging"
            .to_string(),
        adb::Error::Unauthorized => {
            "the phone has not authorised this computer. Run `phonectl pair <code>` with the code \
from Wireless debugging > Pair device with pairing code"
                .to_string()
        }
        adb::Error::PairingRejected => {
            "pairing failed: wrong code, or the pairing dialog was closed".to_string()
        }
        adb::Error::Tls(detail) => format!(
            "the phone rejected our key ({detail}). Pair again with `phonectl pair <code>`"
        ),
        other => other.to_string(),
    }
}

fn exit_code_for(error: &adb::Error) -> u8 {
    match error {
        adb::Error::NotFound | adb::Error::ConnectionClosed => exit::NOT_CONNECTED,
        adb::Error::Unauthorized | adb::Error::PairingRejected | adb::Error::Tls(_) => {
            exit::PAIRING_REQUIRED
        }
        _ => exit::FAILED,
    }
}

async fn run(cli: Cli) -> Result<u8, adb::Error> {
    let timeout = Duration::from_secs(cli.timeout);
    match &cli.command {
        Command::Devices => devices(timeout).await,
        Command::Pair { code, address } => pair(code, *address, timeout).await,
        Command::Info => info(cli.device.as_deref(), timeout).await,
        Command::Shell { command } => shell(cli.device.as_deref(), &command.join(" "), timeout).await,
    }
}

fn host_key() -> Result<HostKey, adb::Error> {
    HostKey::load_or_generate(&paths::adb_key())
}

async fn devices(timeout: Duration) -> Result<u8, adb::Error> {
    let mut seen = Vec::new();
    for service in [Service::Connect, Service::Pairing] {
        let mut found = adb::discovery::watch(service)?;
        let deadline = tokio::time::Instant::now() + timeout / 2;
        while let Ok(Some(device)) = tokio::time::timeout_at(deadline, found.recv()).await {
            if !seen.iter().any(|d: &adb::discovery::Discovered| d.instance == device.instance) {
                seen.push(device);
            }
        }
    }

    if seen.is_empty() {
        println!("no phones found");
        return Ok(exit::NOT_CONNECTED);
    }
    for device in &seen {
        let what = match device.service {
            Service::Connect => "ready to connect",
            Service::Pairing => "waiting to pair",
        };
        println!("{}  {}  {what}", device.instance, device.address);
    }
    Ok(exit::OK)
}

async fn pair(
    code: &str,
    address: Option<std::net::SocketAddr>,
    timeout: Duration,
) -> Result<u8, adb::Error> {
    let address = match address {
        Some(address) => address,
        None => find(Service::Pairing, None, timeout).await?.address,
    };

    let key = host_key()?;
    let info = adb::pairing::pair(address, code, &key, &paths::key_comment()).await?;
    println!("paired with {}", info.text());
    println!("our key is stored on the phone as {}", paths::key_comment());
    Ok(exit::OK)
}

async fn info(device: Option<&str>, timeout: Duration) -> Result<u8, adb::Error> {
    let key = host_key()?;
    let connected = phone::discover_and_connect(device, &key, &paths::key_comment(), timeout).await?;

    println!("device     {}", connected.discovered.instance);
    println!("address    {}", connected.discovered.address);
    if let Some(model) = connected.banner.model() {
        println!("model      {model}");
    }
    for (name, value) in &connected.banner.properties {
        if name != "ro.product.model" {
            println!("{name:<10} {value}", name = name.trim_start_matches("ro.product."));
        }
    }
    println!("features   {}", connected.banner.features.iter().cloned().collect::<Vec<_>>().join(", "));
    Ok(exit::OK)
}

async fn shell(device: Option<&str>, command: &str, timeout: Duration) -> Result<u8, adb::Error> {
    let key = host_key()?;
    let connected = phone::discover_and_connect(device, &key, &paths::key_comment(), timeout).await?;
    let output = phone::shell(&connected.connection, &connected.banner, command).await?;

    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    Ok(output.exit_code.unwrap_or(exit::OK))
}
