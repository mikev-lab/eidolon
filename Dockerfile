# Multi-stage zero-dependency Rust build for eidolon-server
FROM rust:1.80-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app
COPY . .

RUN cargo build --release -p eidolon-server

FROM alpine:3.20

RUN apk add --no-cache ca-certificates

WORKDIR /app
COPY --from=builder /app/target/release/eidolon-server /app/eidolon-server

EXPOSE 8000/udp 8001/udp 8002/udp

ENTRYPOINT ["/app/eidolon-server"]
