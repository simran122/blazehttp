# Go harness: net/http.ReadResponse from the Go release in the image (standard library, no module dependencies).
# golang:1-bookworm resolved to Go 1.27.3 (latest stable) at this digest.
FROM golang:1-bookworm@sha256:dc0963f32053e4bc451d24d87e3892d42d01021b61f093e98eb95c0352bb84b0

# Go default toolchain settings (design §16.2) with the baseline stated explicitly: GOARM64=v8.0 is the default.
ENV CGO_ENABLED=0 GOARM64=v8.0

WORKDIR /src
COPY go.mod main.go ./
RUN go vet ./... \
 && test -z "$(gofmt -l .)" \
 && go build -trimpath -o /usr/local/bin/harness . \
 && { echo "net/http $(go env GOVERSION)"; go version; echo "flags go default, GOARM64=${GOARM64}"; } > /versions.txt

ENTRYPOINT ["harness"]
