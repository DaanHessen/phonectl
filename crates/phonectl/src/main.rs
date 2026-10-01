use std::process::ExitCode;
use std::time::Duration;

use adb::HostKey;
use adb::discovery::{Service, find};
use clap::{Parser, Subcommand};
use phone::paths;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

mod config;
mod daemon;
mod setup;
mod waybar;

/// Exit codes, kept stable so scripts can rely on them.
mod exit {
    pub const OK: u8 = 0;
    pub const FAILED: u8 = 1;
    pub const NO_DAEMON: u8 = 3;
    pub const NOT_CONNECTED: u8 = 4;
    pub const PAIRING_REQUIRED: u8 = 6;
}

#[derive(Debug, Parser)]
#[command(name = "phonectl", version, about = "Use your Android phone from Linux")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon (the systemd user unit runs this).
    Daemon,
    /// Show the phone's status.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Waybar module: prints one JSON line per change, forever.
    Waybar,
    /// Print link events as they happen (JSON lines).
    Events,
    /// Ask the phone to connect now (sends a poke over Tailscale).
    Connect,
    /// Send the laptop clipboard to the phone now.
    Clip,
    /// Ring the phone at alarm volume (find it); `--stop` silences it.
    Ring {
        #[arg(long)]
        stop: bool,
    },
    /// Set the phone's ringer: normal, vibrate or silent.
    Ringer { mode: String },
    /// Control media playing on the phone: toggle, play, pause, next, previous.
    Media { action: String },
    /// Print the phone app's recent log (states and package names only).
    Diag,
    /// Post a test notification on the phone (to check mirroring).
    TestNotification,
    /// Install a new phone app build over the link (no ADB needed).
    Update {
        /// APK to install (default: the release build in this repo).
        #[arg(long)]
        apk: Option<std::path::PathBuf>,
    },
    /// Create the pairing key; install and set up the phone app over ADB.
    Setup {
        /// Replace the existing key (the phone must be paired again).
        #[arg(long)]
        new_key: bool,
        /// APK to install (default: the release build in this repo).
        #[arg(long)]
        apk: Option<std::path::PathBuf>,
        /// Only print the pairing string.
        #[arg(long)]
        no_adb: bool,
    },
    /// Talk to the phone over ADB with phonectl's own ADB stack (development).
    #[command(subcommand)]
    Adb(AdbCommand),
}

#[derive(Debug, Subcommand)]
enum AdbCommand {
    /// List phones visible on the network.
    Devices {
        #[arg(long, default_value = "5", value_name = "SECONDS")]
        timeout: u64,
    },
    /// Pair with a phone that is showing a pairing code.
    Pair {
        /// The six-digit code from Wireless debugging > Pair device.
        code: String,
        /// Pairing endpoint, if mDNS cannot find it (IP:PORT).
        #[arg(long)]
        address: Option<std::net::SocketAddr>,
        #[arg(long, default_value = "5", value_name = "SECONDS")]
        timeout: u64,
    },
    /// Connect and print what the phone reports about itself.
    Info {
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value = "5", value_name = "SECONDS")]
        timeout: u64,
    },
    /// Run a command on the phone.
    Shell {
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value = "5", value_name = "SECONDS")]
        timeout: u64,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PHONECTL_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();

    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("phonectl: cannot start: {e}");
            return ExitCode::from(exit::FAILED);
        }
    };

    let result = match cli.command {
        Command::Adb(command) => runtime.block_on(adb_command(command)).map_err(|e| {
            eprintln!("phonectl: {}", describe(&e));
            exit_code_for(&e)
        }),
        command => runtime.block_on(link_command(command)).map_err(|e| {
            eprintln!("phonectl: {e:#}");
            if e.to_string().contains("daemon is not running") { exit::NO_DAEMON } else { exit::FAILED }
        }),
    };
    match result {
        Ok(code) | Err(code) => ExitCode::from(code),
    }
}

async fn link_command(command: Command) -> anyhow::Result<u8> {
    match command {
        Command::Daemon => {
            daemon::run(config::Config::load()?).await?;
            Ok(exit::OK)
        }
        Command::Waybar => {
            waybar::run().await?;
            Ok(exit::OK)
        }
        Command::Setup { new_key, apk, no_adb } => {
            setup::run(&config::Config::load()?, setup::Options { new_key, apk, no_adb }).await?;
            Ok(exit::OK)
        }
        Command::Status { json } => {
            let reply = request("status").await?;
            let data = &reply["data"];
            if json {
                println!("{}", serde_json::to_string_pretty(data)?);
            } else {
                print_status(data);
            }
            Ok(if data["state"] == "connected" { exit::OK } else { exit::NOT_CONNECTED })
        }
        Command::Connect => {
            request("poke").await?;
            println!("poked the phone");
            Ok(exit::OK)
        }
        Command::Clip => {
            let reply = request("clip").await?;
            if reply["ok"] == true {
                Ok(exit::OK)
            } else {
                anyhow::bail!("{}", reply["error"].as_str().unwrap_or("failed"))
            }
        }
        Command::Diag => {
            let stream = connect_daemon().await?;
            let (read, mut write) = stream.into_split();
            write.write_all(b"{\"method\":\"call\",\"params\":{\"method\":\"diag\"}}\n").await?;
            let line = BufReader::new(read).lines().next_line().await?.unwrap_or_default();
            let reply: serde_json::Value = serde_json::from_str(&line)?;
            for l in reply["data"].as_array().into_iter().flatten() {
                println!("{}", l.as_str().unwrap_or(""));
            }
            if reply["ok"] != true {
                anyhow::bail!("{}", reply["error"].as_str().unwrap_or("failed"));
            }
            Ok(exit::OK)
        }
        Command::TestNotification => phone_call("test_notification", serde_json::Value::Null).await,
        Command::Ring { stop } => phone_call("ring", serde_json::json!({"on": !stop})).await,
        Command::Ringer { mode } => phone_call("ringer", serde_json::json!({"mode": mode})).await,
        Command::Media { action } => {
            let action = if action == "toggle" { "play_pause".to_string() } else { action };
            phone_call("media_action", serde_json::json!({"action": action})).await
        }
        Command::Update { apk } => {
            let apk = apk.or_else(setup::default_apk).ok_or_else(|| anyhow::anyhow!("no APK; build it or pass --apk"))?;
            let stream = connect_daemon().await?;
            let (read, mut write) = stream.into_split();
            let request = serde_json::json!({"method": "update", "params": {"path": apk.canonicalize()?}});
            write.write_all(format!("{request}\n").as_bytes()).await?;
            let line = BufReader::new(read).lines().next_line().await?.unwrap_or_default();
            let reply: serde_json::Value = serde_json::from_str(&line)?;
            if reply["ok"] != true {
                anyhow::bail!("{}", reply["error"].as_str().unwrap_or("failed"));
            }
            println!("sent {} to the phone; it installs and reconnects in a few seconds", apk.display());
            println!("(the first update asks for confirmation on the phone)");
            Ok(exit::OK)
        }
        Command::Events => {
            let stream = connect_daemon().await?;
            let (read, mut write) = stream.into_split();
            write.write_all(b"{\"method\":\"subscribe\"}\n").await?;
            let mut lines = BufReader::new(read).lines();
            while let Some(line) = lines.next_line().await? {
                println!("{line}");
            }
            Ok(exit::OK)
        }
        Command::Adb(_) => unreachable!(),
    }
}

async fn phone_call(method: &str, params: serde_json::Value) -> anyhow::Result<u8> {
    let stream = connect_daemon().await?;
    let (read, mut write) = stream.into_split();
    let request = serde_json::json!({"method": "call", "params": {"method": method, "params": params}});
    write.write_all(format!("{request}\n").as_bytes()).await?;
    let line = BufReader::new(read).lines().next_line().await?.unwrap_or_default();
    let reply: serde_json::Value = serde_json::from_str(&line)?;
    if reply["ok"] == true {
        Ok(exit::OK)
    } else {
        anyhow::bail!("{}", reply["error"].as_str().unwrap_or("failed"))
    }
}

async fn connect_daemon() -> anyhow::Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(paths::daemon_socket())
        .await
        .map_err(|_| anyhow::anyhow!("the phonectl daemon is not running (systemctl --user start phonectl)"))
}

async fn request(method: &str) -> anyhow::Result<serde_json::Value> {
    let stream = connect_daemon().await?;
    let (read, mut write) = stream.into_split();
    write.write_all(format!("{{\"method\":\"{method}\"}}\n").as_bytes()).await?;
    let line = BufReader::new(read).lines().next_line().await?.unwrap_or_default();
    Ok(serde_json::from_str(&line)?)
}

fn print_status(s: &serde_json::Value) {
    let name = s["phone"]["name"].as_str().unwrap_or("phone");
    let state = s["state"].as_str().unwrap_or("?");
    match s["transport"].as_str() {
        Some(t) => println!("{name}: {state} over {t}"),
        None => println!("{name}: {state}"),
    }
    let st = &s["status"];
    if let Some(level) = st["battery"]["level"].as_i64() {
        let charging = if st["battery"]["charging"] == true { ", charging" } else { "" };
        println!("battery    {level}%{charging}");
    }
    if let Some(t) = st["network"]["type"].as_str() {
        let detail = match t {
            "cellular" => format!("{} (signal {}/4)", st["network"]["cellular"].as_str().unwrap_or("?"), st["network"]["cellular_level"]),
            "wifi" => format!("wi-fi (signal {}/4)", st["network"]["wifi_level"]),
            other => other.to_string(),
        };
        println!("network    {detail}");
    }
    if let Some(call) = st["call"].as_str().filter(|c| *c != "idle") {
        println!("call       {call}");
    }
    println!("notifs     {}", s["notifications"]);
    if let Some(e) = s["error"].as_str() {
        println!("last error {e}");
    }
}

async fn adb_command(command: AdbCommand) -> Result<u8, adb::Error> {
    match command {
        AdbCommand::Devices { timeout } => devices(Duration::from_secs(timeout)).await,
        AdbCommand::Pair { code, address, timeout } => pair(&code, address, Duration::from_secs(timeout)).await,
        AdbCommand::Info { device, timeout } => info(device.as_deref(), Duration::from_secs(timeout)).await,
        AdbCommand::Shell { device, timeout, command } => {
            shell(device.as_deref(), &command.join(" "), Duration::from_secs(timeout)).await
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
            "the phone has not authorised this computer. Run `phonectl adb pair <code>` with the code \
from Wireless debugging > Pair device with pairing code"
                .to_string()
        }
        adb::Error::PairingRejected => {
            "pairing failed: wrong code, or the pairing dialog was closed".to_string()
        }
        adb::Error::Tls(detail) => format!(
            "the phone rejected our key ({detail}). Pair again with `phonectl adb pair <code>`"
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
