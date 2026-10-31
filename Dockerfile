FROM rust:1.99-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN groupadd -r -g 10001 clipsync && useradd -r -u 10001 -g clipsync clipsync \
    && mkdir /data && chown clipsync:clipsync /data
COPY --from=builder /build/target/release/rustclipsync-server /usr/local/bin/rustclipsync-server
USER clipsync
ENV RCS_LISTEN=0.0.0.0:8090 RCS_DATA_DIR=/data
EXPOSE 8090
VOLUME /data
HEALTHCHECK --interval=30s --timeout=3s CMD ["/usr/local/bin/rustclipsync-server", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/rustclipsync-server"]
