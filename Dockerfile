FROM rust:bookworm AS builder

WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src
COPY ./src ./src
RUN --mount=type=cache,target=/usr/local/cargo,from=rust:bookworm,source=/usr/local/cargo \
    --mount=type=cache,target=target \
    cargo build --release && mv ./target/release/gwm2mqtt ./gwm2mqtt

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    openssl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/app
RUN mkdir -p /usr/src/app/data /usr/src/app/data/logs
COPY --from=builder /usr/src/app/gwm2mqtt ./gwm2mqtt

ENV REFRESH_INTERVAL=15

CMD ["./gwm2mqtt"]
