# The co-op relay. Build from this folder:
#   docker build -t atc-relay .
#   docker run -d --restart unless-stopped -p 7341:7341 atc-relay
FROM rust:1-slim AS build
WORKDIR /relay
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=build /relay/target/release/atc-relay /usr/local/bin/atc-relay
EXPOSE 7341
USER nobody
CMD ["atc-relay", "--listen", "0.0.0.0:7341"]
