# SyncTube server image (the Windows client is not built here).

FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p server --bin server \
    && cp target/release/server /server

FROM debian:bookworm-slim
RUN useradd --system --no-create-home --shell /usr/sbin/nologin synctube
COPY --from=builder /server /usr/local/bin/synctube-server
USER synctube
ENV PORT=8080 RUST_LOG=info
EXPOSE 8080
CMD ["/usr/local/bin/synctube-server"]
