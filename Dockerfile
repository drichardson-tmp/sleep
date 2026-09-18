FROM rust:1.96.0-slim-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/sleep-telemetry /usr/local/bin/sleep-telemetry
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/sleep-telemetry"]
