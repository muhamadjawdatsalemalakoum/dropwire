# Build iroh-relay from the iroh monorepo at a pinned tag.
# VERIFY (ARCHITECTURE.md §13.7): set IROH_REF to the tag matching the app's iroh 1.0.x.
FROM rust:1-bookworm AS build
ARG IROH_REF=v1.0.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends git build-essential cmake perl \
 && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch ${IROH_REF} https://github.com/n0-computer/iroh /src
WORKDIR /src
RUN cargo build --release -p iroh-relay --features server --bin iroh-relay

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/iroh-relay /usr/local/bin/iroh-relay
EXPOSE 80 443 9889/udp 9090
ENTRYPOINT ["iroh-relay"]
CMD ["--config-path", "/config/relay.toml"]
