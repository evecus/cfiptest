# ── Build stage ──────────────────────────────────────────────────────────────
FROM golang:1.22-alpine AS builder

WORKDIR /app
COPY go.mod go.sum ./
RUN go mod download

COPY main.go .
RUN CGO_ENABLED=0 GOOS=linux go build -ldflags="-s -w" -o cfiptest .

# ── Final stage ───────────────────────────────────────────────────────────────
FROM alpine:3.19

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app
COPY --from=builder /app/cfiptest .

# Default log to stdout-friendly path inside container
ENV CF_LOG_FILE=/tmp/cfip.log

ENTRYPOINT ["/app/cfiptest"]
