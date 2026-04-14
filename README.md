# Zuljin-rs

A file upload and download service built with Rust and Axum, following a classic client-server architecture.

## Features

- **Upload** -- auto-detects file type via magic bytes / MIME hints, stores as `YYYY_mm_dd/<timestamp>.<ext>`
- **Download** -- public access by key, no token required
- **Delete** -- remove files by key
- **File info** -- inspect size, MIME type, and extension
- **Disk space** -- check available storage on the server
- **Token auth** -- optional Bearer token for upload, info, disk, and delete operations
- **Environment variables** -- configure server address and token via `ZULJIN_SERVER` / `ZULJIN_TOKEN`
- **Logging** -- verbose console output (`-v`), env-based log level (`ZULJIN_LOG`), monthly log files (`--log-dir`)

## Build

```bash
cargo build            # debug
cargo build --release  # release
```

## Usage

### Start the server

```bash
# Custom bind address, directory, size limit, and token
cargo run -- serve -b 0.0.0.0:8080 -d /data/uploads -m 500 -t my-secret
```

The token can also be set via the `ZULJIN_TOKEN` environment variable:

```bash
export ZULJIN_TOKEN=my-secret
cargo run -- serve
```

### curl examples

```bash
# Upload (with token)
curl -H "Authorization: Bearer my-secret" \
     -F "file=@photo.jpg" http://127.0.0.1:3000/upload

# Download (public)
curl -O http://127.0.0.1:3000/get/2026_03_30/1743408000.jpg

# File info (with token)
curl -H "Authorization: Bearer my-secret" \
     http://127.0.0.1:3000/info/2026_03_30/1743408000.jpg

# Disk space (with token)
curl -H "Authorization: Bearer my-secret" \
     http://127.0.0.1:3000/disk

# Delete (with token)
curl -X DELETE -H "Authorization: Bearer my-secret" \
     http://127.0.0.1:3000/delete/2026_03_30/1743408000.jpg
```

### CLI client

The CLI acts as an HTTP client. Use `--server` (or `ZULJIN_SERVER`) to point to a remote server and `--token` (or `ZULJIN_TOKEN`) for authentication.

```bash
# Upload
cargo run -- upload -f /path/to/file.jpg

# Upload to a remote server with token
cargo run -- upload -f /path/to/file.jpg -s http://192.168.1.100:3000 --token my-secret

# Download
cargo run -- download -k "2026_03_30/1743408000.jpg" -o output.jpg

# File info
cargo run -- info -k "2026_03_30/1743408000.jpg"

# Disk space
cargo run -- disk

# Delete
cargo run -- delete -k "2026_03_30/1743408000.jpg"

# Help
cargo run -- --help
```

## Logging

All commands support a `--verbose` / `-v` flag for debug-level console output. The `serve` command also accepts `--log-dir` (or `ZULJIN_LOG_DIR` env) to write monthly log files (e.g. `logs/2026-04.log`).

Log level priority: `ZULJIN_LOG` env > `RUST_LOG` env > `--verbose` flag > default (`info`).

```bash
# Debug output on console
cargo run -- -v serve

# Set log level via environment variable
ZULJIN_LOG=debug cargo run -- serve

# Write monthly log files to ./logs/
cargo run -- serve --log-dir ./logs

# Combine: debug level + file logging
ZULJIN_LOG=debug cargo run -- serve --log-dir ./logs
```

Logged events include file uploads (key, size, original name), file deletions, download requests (debug level), and authentication failures (warn level).

## Environment variables

| Variable         | Description                                             | Default                         |
| ---------------- | ------------------------------------------------------- | ------------------------------- |
| `ZULJIN_TOKEN`   | Bearer token for protected endpoints                    | *(none, auth disabled)*         |
| `ZULJIN_SERVER`  | Server address used by CLI commands                     | `http://127.0.0.1:3000`         |
| `ZULJIN_LOG`     | Log level filter (e.g. `debug`, `info,zuljin_rs=trace`) | `info`                          |
| `ZULJIN_LOG_DIR` | Directory for monthly log files (serve only)            | *(none, file logging disabled)* |

## Deployment

### Docker

Pre-built image is available on GitHub Container Registry:

```bash
docker pull ghcr.io/mingcheng/zuljin-rs
```

Run with custom token and persistent storage:

```bash
docker run -d \
  -p 3000:3000 \
  -e ZULJIN_TOKEN=my-secret \
  -v zuljin-uploads:/data/uploads \
  ghcr.io/mingcheng/zuljin-rs
```

Or build from source:

```bash
docker build -t zuljin-rs .

docker run -d \
  -p 3000:3000 \
  -e ZULJIN_TOKEN=my-secret \
  -v zuljin-uploads:/data/uploads \
  zuljin-rs
```

The Dockerfile uses a multi-stage build (Alpine-based) and runs as a non-root user (`zuljin`). By default the container:

- Binds to `0.0.0.0:3000`
- Stores uploads in `/data/uploads`
- Uses `zuljin-rs` as the default token (override via `ZULJIN_TOKEN`)

### Docker Compose

```bash
# Start
docker compose up -d

# Stop
docker compose down
```

The `compose.yaml` exposes port 3000 and uses a named volume `uploads` for persistent storage. Set `ZULJIN_TOKEN` in the environment section to enable authentication:

```yaml
services:
  zuljin:
    image: ghcr.io/mingcheng/zuljin-rs  # or use build: { context: . }
    ports:
      - "3000:3000"
    environment:
      ZULJIN_TOKEN: "my-secret"   # set your token here
    volumes:
      - uploads:/data/uploads
    restart: unless-stopped

volumes:
  uploads:
```

### Environment variables (deployment)

| Variable       | Description                          | Default in Dockerfile |
| -------------- | ------------------------------------ | --------------------- |
| `ZULJIN_TOKEN` | Bearer token for protected endpoints | `zuljin-rs`           |
| `TZ`           | Container timezone                   | `Asia/Shanghai`       |

The upload directory (`/data/uploads`) and bind address (`0.0.0.0:3000`) are set in the Dockerfile `CMD` and can be overridden by appending custom arguments:

```bash
docker run -d -p 8080:8080 -e ZULJIN_TOKEN=my-secret zuljin-rs \
  serve -b 0.0.0.0:8080 -d /data/uploads -m 500
```

## Testing

```bash
cargo test                                    # all tests
cargo test --bin zuljin-rs bucket::tests      # storage engine
cargo test --bin zuljin-rs http::tests        # HTTP handlers
cargo test --bin zuljin-rs meta::tests        # metadata detection
cargo test --bin zuljin-rs utils::tests       # utilities
cargo test -- --nocapture                     # show stdout
```

## Project structure

```
src/
├── main.rs    # CLI entry point, clap commands, server startup
├── http.rs    # HTTP handlers, API types, token auth
├── client.rs  # HTTP client used by CLI commands
├── bucket.rs  # Storage engine (naming, read/write, disk space, path traversal protection, file type detection)
├── meta.rs    # File metadata (size, MIME type via magic bytes)
└── utils.rs   # Helpers (human-readable file size formatting)
```

## API

| Method | Path            | Auth  | Description              |
| ------ | --------------- | ----- | ------------------------ |
| POST   | `/upload`       | Token | Upload files (multipart) |
| GET    | `/get/{key}`    | No    | Download file            |
| GET    | `/info/{key}`   | Token | File metadata            |
| GET    | `/disk`         | Token | Available disk space     |
| DELETE | `/delete/{key}` | Token | Delete file              |

All API responses (except download) use a unified JSON envelope:

```json
{
  "success": true,
  "data": { ... },
  "error": null
}
```

## File type detection

File extension and content type are detected in a single pass during upload,
avoiding redundant analysis. Detection priority:

1. **Magic bytes** -- `infer` library detects the file header signature (most accurate, also yields MIME type)
2. **MIME hint** -- Content-Type from the upload, reverse-mapped to extension via `mime_guess`
3. **Original filename** -- extension extracted from the uploaded filename
4. **Fallback** -- `.bin`
