# C harness: picohttpparser (no releases: pinned commit) and llhttp (latest release, generated C).
# Build: see bench/parsers/README.md. linux/arm64 only for now (x64 baseline decided with the x64 host).
# Default compiler: GCC 14.2 from Debian 13, pulled through the ECR mirror of Docker Hub's official image (Docker Hub
# pulls hung on the bench host). The runner's `--cc gcc-12` builds the Debian 12 / GCC 12.2 variant instead.
ARG BASE=public.ecr.aws/docker/library/debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
FROM ${BASE}

ARG GCC_MAJOR=14
ARG GCC_DEB=14.2.0-19
ARG PICO_COMMIT=dd573d8fa9264c646a400e96ce9f07dea6f156e4
ARG PICO_C_SHA256=8aacb701e60a3a5c600f51845d9554463773eafed88f9c4f47472ed5dac6a942
ARG PICO_H_SHA256=1fc9074dd12418b2b91e55ef3a8279bccf8d3577b790dea6f2f908ab2a157f1c
ARG LLHTTP_VERSION=9.4.3
# The commit the release/v9.4.3 tag points to; the tarball is verified by its SHA-256 below.
ARG LLHTTP_COMMIT=0e815792b167a9bd8ace259b95b7da953776c288
ARG LLHTTP_SHA256=1eb813c7437b31a87496a1cd3ed79f00746720f5e7e29c79b42c02cb69f36c39
# Same CPU baseline as every other harness (design §16.2): generic Armv8-A. SIMD as upstream ships it for this target:
# picohttpparser's only SIMD path is SSE4.2 (x86), so arm64 runs its scalar code; llhttp compiles its NEON span
# (__ARM_NEON is defined for Armv8-A) and uses it in the strict header_value state too (llhttp.c L2789-2829), so
# header values are vectorised here; names stay scalar (tchar needs more ranges than its NEON path takes).
# -DNDEBUG removes assertions from the timed path, as a release build does.
ARG CFLAGS="-O3 -DNDEBUG -march=armv8-a -flto=auto -std=c17"

RUN apt-get update \
 && apt-get install -y --no-install-recommends "gcc-${GCC_MAJOR}=${GCC_DEB}" libc6-dev curl ca-certificates \
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
RUN CC="gcc-${GCC_MAJOR}" \
 && $CC $CFLAGS -I. -Illhttp/include -c picohttpparser.c llhttp/src/api.c llhttp/src/http.c llhttp/src/llhttp.c \
 && $CC $CFLAGS -Wall -Wextra -Werror -I. -Illhttp/include -c main.c pico.c llhttp_engine.c \
 && $CC $CFLAGS -o /usr/local/bin/harness ./*.o -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc \
 && { echo "picohttpparser ${PICO_COMMIT}"; echo "llhttp ${LLHTTP_VERSION} (tag commit ${LLHTTP_COMMIT})"; \
      $CC --version | head -n 1; echo "flags ${CFLAGS}"; } > /versions.txt

ENTRYPOINT ["harness"]
