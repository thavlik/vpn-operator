FROM rust:1.67.1-slim-bullseye AS builder
RUN rustup toolchain install nightly \
    && rustup default nightly
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
WORKDIR /vpn-operator
COPY Cargo.lock .
COPY types types
WORKDIR /vpn-operator/operator
COPY operator/Cargo.toml .
RUN mkdir src \
    && echo 'fn main() { panic!("Dummy image called!"); }' > src/main.rs \
    && cargo build \
    && rm -rf src
COPY operator/src src
RUN touch -a -m ./src/main.rs \
    && cargo build
FROM debian:bullseye-slim
WORKDIR /
COPY --from=builder /vpn-operator/operator/target/debug/vpn-operator .
ENV RUST_BACKTRACE=full
CMD ["/vpn-operator"]