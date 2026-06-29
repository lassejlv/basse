# Build a static agent binary, then ship it on a minimal base.
FROM golang:1.26-alpine AS build
WORKDIR /src

# No external dependencies (stdlib only), so just copy and build.
COPY go.mod ./
COPY main.go ./
COPY internal ./internal

ARG VERSION=dev
RUN CGO_ENABLED=0 go build -trimpath -ldflags "-s -w -X main.version=${VERSION}" -o /agent .

# Runs as root (the default) on purpose: the agent reads the mounted
# /var/run/docker.sock, which is owned root:docker on the host. A :nonroot
# image could not read it.
FROM gcr.io/distroless/static-debian12
COPY --from=build /agent /agent

ENV BASSE_AGENT_PORT=8888
EXPOSE 8888

# Self-probing healthcheck — the image has no curl/wget.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD ["/agent", "healthcheck"]

ENTRYPOINT ["/agent"]
CMD ["serve"]
