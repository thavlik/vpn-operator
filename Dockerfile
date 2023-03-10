FROM rust:1.67.1-slim-bullseye AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        libssl-dev \
        pkg-config \
        wget \
    && rm -rf /var/lib/apt/lists/*
RUN rustup toolchain install nightly \
    && rustup default nightly
WORKDIR /
RUN cargo new vpn-operator
WORKDIR /vpn-operator
COPY Cargo.toml .
COPY Cargo.lock .
COPY types types
RUN cargo build \
    && rm src/*.rs \
    && rm ./target/debug/vpn-operator*
COPY src src
RUN touch -a -m src/main.rs \
    && cargo build
FROM debian:bullseye-slim
ENV RUST_BACKTRACE=1
WORKDIR /
COPY --from=builder /vpn-operator/target/debug/vpn-operator .
CMD ["/vpn-operator"]
