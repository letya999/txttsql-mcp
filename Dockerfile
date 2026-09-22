FROM rust:1.96-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && apt-get clean
COPY --from=build /build/target/release/txttsql-mcp /usr/local/bin/txttsql-mcp
USER 65532:65532
WORKDIR /app
ENTRYPOINT ["/usr/local/bin/txttsql-mcp"]
CMD ["--config", "/app/config.toml"]
