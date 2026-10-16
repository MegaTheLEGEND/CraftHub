# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src

# Dependency layer: build the workspace's dependencies against stub sources first,
# so editing src/ or ui/ does not recompile every crate.
COPY Cargo.toml Cargo.lock build.rs ./
COPY crates/core/Cargo.toml crates/core/Cargo.toml
RUN mkdir -p src crates/core/src ui i18n \
    && echo 'fn main() {}' > src/main.rs \
    && touch crates/core/src/lib.rs \
    && cargo build --release --locked || true

COPY crates crates
COPY src src
COPY ui ui
COPY i18n i18n
RUN touch src/main.rs crates/core/src/lib.rs \
    && cargo build --release --locked \
    && mkdir -p /out/data \
    && cp target/release/crafthub /out/crafthub

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /out/crafthub /crafthub
COPY --from=build --chown=nonroot:nonroot /out/data /data
ENV DATA_DIR=/data BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["/crafthub", "healthcheck"]
ENTRYPOINT ["/crafthub"]
