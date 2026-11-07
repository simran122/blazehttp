# C++ harness: Boost.Beast (header-only) from the latest Boost release.
# Debian 12 (GCC 12.2), the same compiler image as the C harness (bench/parsers/c/Dockerfile).
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc

ARG BOOST_VERSION=1.92.0
ARG BOOST_SHA256=ea7b982002cc9dfbe59b0b217b206f470dc75f3de0bb2973d844118934d82411
# Same CPU baseline as every other harness (design §16.2): generic Armv8-A.
ARG CXXFLAGS="-O3 -march=armv8-a -flto=auto -std=c++20"

RUN apt-get update \
 && apt-get install -y --no-install-recommends g++ curl xz-utils ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
# Only the headers are needed (Beast, Asio buffers and Boost.System are header-only).
RUN curl -fsSL -o boost.tar.xz \
      "https://github.com/boostorg/boost/releases/download/boost-${BOOST_VERSION}/boost-${BOOST_VERSION}-b2-nodocs.tar.xz" \
 && echo "${BOOST_SHA256}  boost.tar.xz" | sha256sum -c - \
 && tar -xJf boost.tar.xz "boost-${BOOST_VERSION}/boost" && mv "boost-${BOOST_VERSION}" boost-root && rm boost.tar.xz

COPY main.cpp ./
RUN g++ $CXXFLAGS -Wall -Wextra -Werror -isystem boost-root main.cpp -o /usr/local/bin/harness \
 && { echo "Boost.Beast boost-${BOOST_VERSION} (BOOST_BEAST_VERSION $(grep -o 'define BOOST_BEAST_VERSION [0-9]*' boost-root/boost/beast/version.hpp | cut -d' ' -f3))"; \
      g++ --version | head -n 1; echo "flags ${CXXFLAGS}"; } > /versions.txt

ENTRYPOINT ["harness"]
