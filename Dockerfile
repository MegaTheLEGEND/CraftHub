# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
# Cache dependencies first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src ui && echo 'fn main(){}' > src/main.rs && touch ui/index.html \
    && cargo build --release && rm -rf src ui target/release/craft-hub*
COPY src ./src
COPY ui ./ui
RUN touch src/main.rs && cargo build --release \
    && mkdir -p /out/data && cp target/release/craft-hub /out/craft-hub

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /out/craft-hub /craft-hub
COPY --from=build --chown=65532:65532 /out/data /data
ENV BIND=0.0.0.0:8080 DATA_DIR=/data
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/craft-hub", "healthcheck"]
USER nonroot
ENTRYPOINT ["/craft-hub"]
