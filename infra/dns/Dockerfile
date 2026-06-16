# Build iroh-dns-server from the iroh monorepo at a pinned tag.
# VERIFY (ARCHITECTURE.md §13.7): set IROH_REF to the tag matching the app's iroh 1.0.x.
FROM rust:1-bookworm AS build
ARG IROH_REF=v1.0.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends git build-essential cmake perl \
 && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch ${IROH_REF} https://github.com/n0-computer/iroh /src
WORKDIR /src
RUN cargo build --release -p iroh-dns-server --bin iroh-dns-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/iroh-dns-server /usr/local/bin/iroh-dns-server
EXPOSE 53/udp 53/tcp 80 443
ENTRYPOINT ["iroh-dns-server"]
CMD ["--config-path", "/config/config.toml"]
