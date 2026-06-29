FROM rust:1-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release

FROM rust:1-slim
COPY --from=builder /app/target/release/kgproxy /usr/local/bin/kgproxy
EXPOSE 8080
CMD ["kgproxy"]
