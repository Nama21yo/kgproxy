FROM rust:1-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
ARG CARGO_BUILD_JOBS=1
RUN cargo build --release --jobs "${CARGO_BUILD_JOBS}"

FROM rust:1-slim
COPY --from=builder /app/target/release/kgproxy /usr/local/bin/kgproxy
EXPOSE 8080
CMD ["kgproxy"]
