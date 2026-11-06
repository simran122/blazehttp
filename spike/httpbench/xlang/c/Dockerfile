FROM debian:bookworm
RUN apt-get update -qq && apt-get install -y -qq gcc libcurl4-openssl-dev >/dev/null && rm -rf /var/lib/apt/lists/*
COPY client.c /src/
RUN gcc -O2 -o /usr/local/bin/client /src/client.c -lcurl
CMD ["client"]
