FROM debian:trixie
RUN apt-get update -qq && apt-get install -y -qq g++ libboost-dev libboost-json-dev >/dev/null && rm -rf /var/lib/apt/lists/*
COPY client.cpp /src/
RUN g++ -std=c++20 -O2 -o /usr/local/bin/client /src/client.cpp -lboost_json
CMD ["client"]
