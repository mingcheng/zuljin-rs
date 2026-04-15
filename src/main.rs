mod bucket;
mod client;
mod http;
mod meta;
mod utils;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::HeaderValue;
use axum::routing::{delete, get, post};
use bucket::Bucket;
use chrono::Local;
use clap::{Args, Parser, Subcommand};
use client::Client;
use http::AppState;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_TIME: &str = compile_time::datetime_str!();

#[derive(Parser)]
#[command(name = "zuljin", about = "File upload and download service")]
struct Cli {
    /// Enable verbose (debug-level) console output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

/// Common arguments for CLI commands that talk to a remote Zuljin server.
#[derive(Args)]
struct RemoteArgs {
    /// Server address (also reads ZULJIN_SERVER env)
    #[arg(
        short,
        long,
        default_value = "http://127.0.0.1:3000",
        env = "ZULJIN_SERVER"
    )]
    server: String,
    /// Auth token (also reads ZULJIN_TOKEN env)
    #[arg(long, env = "ZULJIN_TOKEN")]
    token: String,
    /// Print raw JSON response from server
    #[arg(long)]
    raw: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the HTTP server
    Serve {
        /// Bind address
        #[arg(short, long, default_value = "127.0.0.1:3000")]
        bind: String,
        /// Upload directory
        #[arg(short, long, default_value = "uploads")]
        dir: String,
        /// Max upload size in MB
        #[arg(short, long, default_value_t = 250)]
        max_size: usize,
        /// Auth token (also reads ZULJIN_TOKEN env)
        #[arg(short, long, env = "ZULJIN_TOKEN")]
        token: String,
        /// Directory for log files (monthly rotation, e.g. logs/2026-04.log)
        #[arg(long, env = "ZULJIN_LOG_DIR")]
        log_dir: Option<String>,
    },
    /// Upload a file to the server
    Upload {
        /// File to upload
        #[arg(short, long)]
        file: String,
        #[command(flatten)]
        remote: RemoteArgs,
    },
    /// Download a file from the server
    Download {
        /// File key (e.g. 2026_03_30/1743408000.png)
        #[arg(short, long)]
        key: String,
        /// Server address (also reads ZULJIN_SERVER env)
        #[arg(
            short,
            long,
            default_value = "http://127.0.0.1:3000",
            env = "ZULJIN_SERVER"
        )]
        server: String,
        /// Output file path (defaults to current dir with original filename)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Show file metadata from the server
    Info {
        /// File key (e.g. 2026_03_30/1743408000.png)
        #[arg(short, long)]
        key: String,
        #[command(flatten)]
        remote: RemoteArgs,
    },
    /// Show disk space of the server
    Disk {
        #[command(flatten)]
        remote: RemoteArgs,
    },
    /// Delete a file from the server
    Delete {
        /// File key (e.g. 2026_03_30/1743408000.png)
        #[arg(short, long)]
        key: String,
        #[command(flatten)]
        remote: RemoteArgs,
    },
}

/// Unwrap a `Result`, printing the error with a label and exiting on failure.
fn unwrap_or_exit<T>(result: Result<T, String>, label: &str) -> T {
    result.unwrap_or_else(|e| {
        eprintln!("{label}: {e}");
        std::process::exit(1);
    })
}

/// Initialize the tracing/logging subsystem.
///
/// Log level priority: `ZULJIN_LOG` env > `RUST_LOG` env > `--verbose` flag > default (`info`).
/// - `log_dir`: if `Some`, a file layer writes to `<log_dir>/YYYY-MM.log`.
fn init_tracing(verbose: bool, log_dir: Option<&str>) {
    let env_filter = EnvFilter::try_from_env("ZULJIN_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| {
            if verbose {
                EnvFilter::new("debug")
            } else {
                EnvFilter::new("info")
            }
        });

    let console_layer = fmt::layer().with_target(false);

    if let Some(dir) = log_dir {
        std::fs::create_dir_all(dir).expect("Failed to create log directory");
        let filename = format!("{}.log", Local::now().format("%Y-%m"));
        let file_appender = tracing_appender::rolling::never(dir, filename);
        let file_layer = fmt::layer()
            .with_writer(file_appender)
            .with_ansi(false)
            .with_target(false);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .init();
    }
}

/// Wait for a shutdown signal (SIGINT, SIGTERM, or SIGQUIT).
///
/// On Unix the function listens for SIGINT (Ctrl-C), SIGTERM (Docker / systemd
/// stop), and SIGQUIT.  The first signal received triggers a graceful shutdown:
/// in-flight requests are allowed to complete while new connections are refused.
async fn shutdown_signal() {
    let ctrl_c = signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            signal::unix::signal(signal::unix::SignalKind::terminate()).expect("install SIGTERM handler");
        let mut sigquit =
            signal::unix::signal(signal::unix::SignalKind::quit()).expect("install SIGQUIT handler");

        tokio::select! {
            _ = ctrl_c => info!("Received SIGINT, starting graceful shutdown…"),
            _ = sigterm.recv() => info!("Received SIGTERM, starting graceful shutdown…"),
            _ = sigquit.recv() => info!("Received SIGQUIT, starting graceful shutdown…"),
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("install Ctrl-C handler");
        info!("Received Ctrl-C, starting graceful shutdown…");
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            bind,
            dir,
            max_size,
            token,
            log_dir,
        } => {
            init_tracing(cli.verbose, log_dir.as_deref());

            let bucket = Arc::new(Bucket::new(&dir)?);
            info!(directory = %bucket.path.display(), "Upload directory ready");
            info!("Token auth enabled");
            tracing::debug!(token = %token, "Configured token");

            info!(address = %bind, max_size_mb = max_size, "Starting server");

            let state = AppState {
                bucket,
                token: Some(token),
            };

            let app = Router::new()
                .route("/healthz", get(http::healthz))
                .route("/upload", post(http::upload))
                .route("/get/{*path}", get(http::download))
                .route("/info/{*path}", get(http::file_info))
                .route("/disk", get(http::disk_info))
                .route("/delete/{*path}", delete(http::delete_file))
                .with_state(state)
                .layer(DefaultBodyLimit::disable())
                .layer(RequestBodyLimitLayer::new(max_size * 1024 * 1024))
                .layer(SetResponseHeaderLayer::overriding(
                    axum::http::header::SERVER,
                    HeaderValue::from_static(const_format::formatcp!(
                        "{}/{} (built {})",
                        PKG_NAME,
                        PKG_VERSION,
                        BUILD_TIME
                    )),
                ));

            let listener = TcpListener::bind(&bind).await?;
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await
        }
        Commands::Upload { file, remote } => {
            init_tracing(cli.verbose, None);
            let path = Path::new(&file);
            if !path.exists() {
                eprintln!("Error: file '{}' does not exist", file);
                std::process::exit(1);
            }

            let client = Client::new(&remote.server, Some(remote.token));
            if remote.raw {
                let raw = unwrap_or_exit(client.upload_raw(&file).await, "Upload failed");
                println!("{raw}");
            } else {
                let results = unwrap_or_exit(client.upload(&file).await, "Upload failed");
                for r in &results {
                    println!("Uploaded: {}", r.key);
                    println!("Size:     {}", utils::format_size(r.size as u64));
                }
            }
            Ok(())
        }
        Commands::Download {
            key,
            server,
            output,
        } => {
            init_tracing(cli.verbose, None);
            let client = Client::new(&server, None);
            let content = unwrap_or_exit(client.download(&key).await, "Download failed");

            let out_path = output.unwrap_or_else(|| {
                Path::new(&key)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("download.bin")
                    .to_string()
            });

            std::fs::write(&out_path, &content)?;
            println!("Downloaded to: {}", out_path);
            println!("Size: {}", utils::format_size(content.len() as u64));
            Ok(())
        }
        Commands::Info { key, remote } => {
            init_tracing(cli.verbose, None);
            let client = Client::new(&remote.server, Some(remote.token));
            if remote.raw {
                let raw = unwrap_or_exit(client.info_raw(&key).await, "Info failed");
                println!("{raw}");
            } else {
                let info = unwrap_or_exit(client.info(&key).await, "Info failed");
                println!("Key:          {}", info.key);
                println!("Size:         {}", utils::format_size(info.size as u64));
                println!(
                    "Content-Type: {}",
                    info.content_type.as_deref().unwrap_or("unknown")
                );
                println!(
                    "Extension:    {}",
                    info.extension.as_deref().unwrap_or("unknown")
                );
            }
            Ok(())
        }
        Commands::Disk { remote } => {
            init_tracing(cli.verbose, None);
            let client = Client::new(&remote.server, Some(remote.token));
            if remote.raw {
                let raw = unwrap_or_exit(client.disk_raw().await, "Disk info failed");
                println!("{raw}");
            } else {
                let info = unwrap_or_exit(client.disk().await, "Disk info failed");
                println!("Upload directory: {}", info.path);
                println!("File count:       {}", info.file_count);
                println!("Directory usage:  {}", info.used_human);
                println!("Disk total:       {}", info.total_human);
                println!("Disk available:   {}", info.available_human);
                println!("OS:               {}", info.os);
                println!("Arch:             {}", info.arch);
            }
            Ok(())
        }
        Commands::Delete { key, remote } => {
            init_tracing(cli.verbose, None);
            let client = Client::new(&remote.server, Some(remote.token));
            if remote.raw {
                let raw = unwrap_or_exit(client.delete_raw(&key).await, "Delete failed");
                println!("{raw}");
            } else {
                let result = unwrap_or_exit(client.delete(&key).await, "Delete failed");
                println!("Deleted: {}", result.key);
            }
            Ok(())
        }
    }
}
