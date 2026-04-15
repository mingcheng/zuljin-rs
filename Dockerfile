# Stage 1: Dependencies - Cache Rust dependencies separately for faster rebuilds
FROM rust:alpine AS dependencies
LABEL maintainer="mingcheng <mingcheng@apache.org>"

RUN apk add --no-cache build-base musl-dev pkgconfig

RUN rustup default stable && rustup update stable

WORKDIR /build

# Copy only dependency manifests first to leverage Docker layer caching
COPY Cargo.toml Cargo.lock ./

# Build dependencies with a dummy source, then remove it
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Stage 2: Builder - Build the actual application
FROM dependencies AS builder

COPY src/ src/

# Touch main.rs so cargo detects the source change
RUN touch src/main.rs && \
    cargo build --release && \
    strip target/release/zuljin-rs

# Stage 3: Runtime - Minimal image
FROM alpine AS runtime

ARG TZ=Asia/Shanghai
ENV TZ=${TZ}

RUN apk add --no-cache tzdata ca-certificates curl && \
    ln -snf /usr/share/zoneinfo/$TZ /etc/localtime && \
    echo $TZ > /etc/timezone

COPY --from=builder /build/target/release/zuljin-rs /bin/zuljin-rs

# Create a non-root user and the default upload directory
RUN addgroup -g 1000 zuljin && \
    adduser -D -u 1000 -G zuljin zuljin && \
    mkdir -p /data/uploads && \
    chown -R zuljin:zuljin /data

USER zuljin

EXPOSE 3000

STOPSIGNAL SIGTERM

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/healthz || exit 1

VOLUME /data/uploads

ENTRYPOINT ["/bin/zuljin-rs"]

CMD ["serve", "-b", "0.0.0.0:3000", "-d", "/data/uploads", "-t", "zuljin-rs"]
