FROM xlang-zigbase
COPY client.zig /src/
RUN cd /src && zig build-exe -OReleaseFast client.zig -femit-bin=/usr/local/bin/client
CMD ["client"]
