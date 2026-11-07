# Zig harness: std.http head parsing from the pinned Zig release (latest stable; same release as the spike).
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc

ARG ZIG_VERSION=0.17.1
ARG ZIG_SHA256=a1546d5646eb184437918d6635ca6b762a0e889eac09cef720ee7a1ed3fe7795
# Zig builds for the host CPU unless told otherwise; pin the same generic Armv8-A baseline as every other harness.
ARG ZIG_FLAGS="-OReleaseFast -target aarch64-linux -mcpu=baseline"

RUN apt-get update \
 && apt-get install -y --no-install-recommends curl xz-utils ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && curl -fsSL -o /tmp/zig.tar.xz "https://ziglang.org/download/${ZIG_VERSION}/zig-aarch64-linux-${ZIG_VERSION}.tar.xz" \
 && echo "${ZIG_SHA256}  /tmp/zig.tar.xz" | sha256sum -c - \
 && tar -xJf /tmp/zig.tar.xz -C /opt && rm /tmp/zig.tar.xz \
 && ln -s "/opt/zig-aarch64-linux-${ZIG_VERSION}/zig" /usr/local/bin/zig

WORKDIR /src
COPY main.zig ./
RUN zig fmt --check main.zig \
 && zig build-exe $ZIG_FLAGS main.zig -femit-bin=/usr/local/bin/harness \
 && { echo "std.http zig-${ZIG_VERSION}"; echo "zig $(zig version)"; echo "flags ${ZIG_FLAGS}"; } > /versions.txt

ENTRYPOINT ["harness"]
