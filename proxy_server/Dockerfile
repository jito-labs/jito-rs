# syntax=docker/dockerfile:1.4.0
FROM rust:1.73-slim-bullseye as builder

RUN set -x \
    && apt-get -qq update \
    && apt-get -qq -y install \
    clang \
    cmake \
    libudev-dev \
    unzip \
    libssl-dev \
    pkg-config \
    zlib1g-dev \
    curl

RUN rustup component add rustfmt && update-ca-certificates

ENV HOME=/home/root
WORKDIR $HOME/app
COPY . .

# cache these directories for reuse
# see: https://docs.docker.com/build/cache/#use-the-dedicated-run-cache
RUN --mount=type=cache,mode=0777,target=/home/root/app/target \
    --mount=type=cache,mode=0777,target=/usr/local/cargo/registry \
    --mount=type=cache,mode=0777,target=/usr/local/cargo/git \
    RUSTFLAGS="-C target-cpu=native" cargo build --release && cp target/release/jito-* ./;

FROM debian:bullseye-slim as base_image
# read in build arg from ./b, default to false
ARG debug=false

RUN apt-get -qq update && apt-get -qq -y install ca-certificates libssl1.1 && rm -rf /var/lib/apt/lists/*;

FROM base_image as proxy_server
ENV APP="jito-block-engine-proxy-server"
WORKDIR /app
COPY --from=builder /home/root/app/${APP} ./
ENTRYPOINT ["/app/jito-block-engine-proxy-server"]
