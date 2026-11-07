# C harness: picohttpparser (no releases: pinned commit) and llhttp (latest release, generated C).
# Build: see bench/parsers/README.md. linux/arm64 only for now (x64 baseline decided with the x64 host).
# Debian 12 (GCC 12.2). Debian 13 / GCC 14 is the newer toolchain: switch once the image can be pulled on the
# bench host (docs/perf/journal.md, 2026-11-07 entry).
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc

ARG PICO_COMMIT=dd573d8fa9264c646a400e96ce9f07dea6f156e4
ARG PICO_C_SHA256=8aacb701e60a3a5c600f51845d9554463773eafed88f9c4f47472ed5dac6a942
ARG PICO_H_SHA256=1fc9074dd12418b2b91e55ef3a8279bccf8d3577b790dea6f2f908ab2a157f1c
ARG LLHTTP_VERSION=9.4.3
ARG LLHTTP_SHA256=1eb813c7437b31a87496a1cd3ed79f00746720f5e7e29c79b42c02cb69f36c39
# Same CPU baseline as every other harness (design §16.2): generic Armv8-A. SIMD as upstream ships it for this target:
# picohttpparser's only SIMD path is SSE4.2 (x86), so arm64 runs its scalar code; llhttp compiles its NEON span
# (__ARM_NEON is defined for Armv8-A) but uses it only under LENIENT_HEADER_VALUE_RELAXED, so the strict default
# measured here is scalar.
ARG CFLAGS="-O3 -march=armv8-a -flto -std=c17"

RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc libc6-dev curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
RUN curl -fsSLO "https://raw.githubusercontent.com/h2o/picohttpparser/${PICO_COMMIT}/picohttpparser.c" \
 && curl -fsSLO "https://raw.githubusercontent.com/h2o/picohttpparser/${PICO_COMMIT}/picohttpparser.h" \
 && curl -fsSL -o llhttp.tar.gz "https://github.com/nodejs/llhttp/archive/refs/tags/release/v${LLHTTP_VERSION}.tar.gz" \
 && printf '%s  %s\n' "$PICO_C_SHA256" picohttpparser.c "$PICO_H_SHA256" picohttpparser.h "$LLHTTP_SHA256" llhttp.tar.gz \
    | sha256sum -c - \
 && tar -xzf llhttp.tar.gz && mv "llhttp-release-v${LLHTTP_VERSION}" llhttp

COPY engine.h main.c pico.c llhttp_engine.c ./
# Upstream sources are compiled as published; the harness itself must be warning-free.
RUN gcc $CFLAGS -I. -Illhttp/include -c picohttpparser.c llhttp/src/api.c llhttp/src/http.c llhttp/src/llhttp.c \
 && gcc $CFLAGS -Wall -Wextra -Werror -I. -Illhttp/include -c main.c pico.c llhttp_engine.c \
 && gcc $CFLAGS -o /usr/local/bin/harness ./*.o -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc \
 && { echo "picohttpparser ${PICO_COMMIT}"; echo "llhttp ${LLHTTP_VERSION}"; gcc --version | head -n 1; \
      echo "flags ${CFLAGS}"; } > /versions.txt

ENTRYPOINT ["harness"]
