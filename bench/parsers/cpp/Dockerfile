# C++ harness: Boost.Beast (header-only) from the latest Boost release.
# Same compiler image and choice as the C harness (bench/parsers/c/Dockerfile): GCC 14.2 by default, `--cc gcc-12`
# for the Debian 12 variant.
ARG BASE=public.ecr.aws/docker/library/debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
FROM ${BASE}

ARG GCC_MAJOR=14
ARG GCC_DEB=14.2.0-19
ARG BOOST_VERSION=1.92.0
# The commit the boost-1.92.0 tag points to; the release tarball is verified by its SHA-256 below.
ARG BOOST_COMMIT=afdfa32505af73e3d208144b3f623f0096cb62b6
ARG BOOST_SHA256=ea7b982002cc9dfbe59b0b217b206f470dc75f3de0bb2973d844118934d82411
# Same CPU baseline as every other harness (design §16.2): generic Armv8-A. -DNDEBUG removes Beast's BOOST_ASSERTs
# from the timed path, as a release build does.
ARG CXXFLAGS="-O3 -DNDEBUG -march=armv8-a -flto=auto -std=c++20"

RUN apt-get update \
 && apt-get install -y --no-install-recommends "g++-${GCC_MAJOR}=${GCC_DEB}" curl xz-utils ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
# Only the headers are needed (Beast, Asio buffers and Boost.System are header-only).
RUN curl -fsSL -o boost.tar.xz \
      "https://github.com/boostorg/boost/releases/download/boost-${BOOST_VERSION}/boost-${BOOST_VERSION}-b2-nodocs.tar.xz" \
 && echo "${BOOST_SHA256}  boost.tar.xz" | sha256sum -c - \
 && tar -xJf boost.tar.xz "boost-${BOOST_VERSION}/boost" && mv "boost-${BOOST_VERSION}" boost-root && rm boost.tar.xz

COPY main.cpp ./
RUN CXX="g++-${GCC_MAJOR}" \
 && $CXX $CXXFLAGS -Wall -Wextra -Werror -isystem boost-root main.cpp -o /usr/local/bin/harness \
 && { echo "Boost.Beast boost-${BOOST_VERSION} (tag commit ${BOOST_COMMIT}, BOOST_BEAST_VERSION $(grep -o 'define BOOST_BEAST_VERSION [0-9]*' boost-root/boost/beast/version.hpp | cut -d' ' -f3))"; \
      $CXX --version | head -n 1; echo "flags ${CXXFLAGS}"; } > /versions.txt

ENTRYPOINT ["harness"]
