mod bucket;
mod http;
mod meta;
mod utils;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use bucket::Bucket;
use clap::{Parser, Subcommand};
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::limit::RequestBodyLimitLayer;

#[derive(Parser)]
#[command(name = "zuljin", about = "File upload and download service")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
    },
    /// Upload a file via CLI
    Upload {
        /// File to upload
        #[arg(short, long)]
        file: String,
        /// Upload directory
        #[arg(short, long, default_value = "uploads")]
        dir: String,
    },
    /// Download / read a file from the bucket
    Download {
        /// File key (e.g. 2026/03/1743408000000000.png)
        #[arg(short, long)]
        key: String,
        /// Upload directory
        #[arg(long, default_value = "uploads")]
        dir: String,
        /// Output file path (defaults to current dir with original filename)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Show file metadata
    Info {
        /// File key (e.g. 2026/03/1743408000000000.png)
        #[arg(short, long)]
        key: String,
        /// Upload directory
        #[arg(long, default_value = "uploads")]
        dir: String,
    },
    /// Show disk space for the upload directory
    Disk {
        /// Upload directory
        #[arg(short, long, default_value = "uploads")]
        dir: String,
    },
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            bind,
            dir,
            max_size,
        } => {
            let bucket = Arc::new(Bucket::new(&dir)?);
            println!("Upload directory: {}", bucket.path.display());
            println!("Listening on: {}", bind);

            let app = Router::new()
                .route("/", get(http::show_form))
                .route("/upload", post(http::upload))
                .route("/files/{*path}", get(http::download))
                .with_state(bucket)
                .layer(DefaultBodyLimit::disable())
                .layer(RequestBodyLimitLayer::new(max_size * 1024 * 1024));

            let listener = TcpListener::bind(&bind).await?;
            axum::serve(listener, app).await
        }
        Commands::Upload { file, dir } => {
            let bucket = Bucket::new(&dir)?;
            let path = Path::new(&file);
            if !path.exists() {
                eprintln!("Error: file '{}' does not exist", file);
                std::process::exit(1);
            }

            let data = std::fs::read(path)?;
            let original_name = path.file_name().and_then(|n| n.to_str());
            let key = bucket.save(data, None, original_name)?;
            println!("Uploaded: {}", key);
            println!("Full path: {}", bucket.path.join(&key).display());
            Ok(())
        }
        Commands::Download { key, dir, output } => {
            let bucket = Bucket::new(&dir)?;
            let content = bucket.get_content(&key)?;

            let out_path = match output {
                Some(p) => p,
                None => {
                    // Use the filename part of the key
                    Path::new(&key)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("download.bin")
                        .to_string()
                }
            };

            std::fs::write(&out_path, &content)?;
            println!("Downloaded to: {}", out_path);
            println!("Size: {}", utils::format_size(content.len() as u64));
            Ok(())
        }
        Commands::Info { key, dir } => {
            let bucket = Bucket::new(&dir)?;
            let meta = bucket.get_meta(&key)?;
            println!("Path:         {}", meta.path);
            println!("Size:         {}", utils::format_size(meta.size as u64));
            println!(
                "Content-Type: {}",
                meta.content_type.as_deref().unwrap_or("unknown")
            );
            println!(
                "Extension:    {}",
                meta.extension.as_deref().unwrap_or("unknown")
            );
            Ok(())
        }
        Commands::Disk { dir } => {
            let bucket = Bucket::new(&dir)?;
            println!("Upload directory: {}", bucket.path.display());
            match bucket.available_space() {
                Ok(space) => println!("Available space:  {}", utils::format_size(space)),
                Err(e) => eprintln!("Failed to get disk space: {}", e),
            }
            Ok(())
        }
    }
}
