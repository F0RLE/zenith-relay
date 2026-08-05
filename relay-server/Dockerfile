FROM rust:1.97.1-bookworm AS build
WORKDIR /build
COPY crates/relay-core crates/relay-core
COPY relay-server relay-server
RUN cargo build --manifest-path relay-server/Cargo.toml --release --locked

FROM debian:bookworm-slim
RUN useradd --system --home /var/lib/zenith-relay --create-home relay
COPY --from=build /build/relay-server/target/release/zenith-relay-server /usr/local/bin/zenith-relay-server
USER relay
VOLUME ["/var/lib/zenith-relay"]
EXPOSE 14999
ENTRYPOINT ["/usr/local/bin/zenith-relay-server"]
