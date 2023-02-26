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
WORKDIR /
RUN cargo new ytdl-operator
WORKDIR /ytdl-operator
COPY Cargo.toml .
COPY Cargo.lock .
RUN cargo build \
    && rm src/*.rs \
    && rm ./target/debug/ytdl-operator*
COPY src ./src
RUN cargo build
FROM debian:bullseye-slim
WORKDIR /
COPY --from=builder /ytdl-operator/target/debug/ytdl-operator .
CMD ["/ytdl-operator"]