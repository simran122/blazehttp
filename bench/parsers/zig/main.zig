//! Zig harness of the cross-language response-head parser bench: std.http (the pinned Zig release).
//! Command line and output protocol: bench/parsers/README.md ("Harness protocol").
//!
//! std.http.Client reads a response head in two steps, reproduced here: `std.http.HeadParser.feed` finds the end
//! of the head (resumable, vectorised), then `std.http.Client.Response.Head.parse` splits the lines and derives
//! status, Content-Length, Transfer-Encoding and keep-alive. Neither takes an allocator.
const std = @import("std");
const Io = std.Io;
const Head = std.http.Client.Response.Head;

const warm_up_ns = 100 * std.time.ns_per_ms;
const block = 1000;

fn fail(comptime fmt: []const u8, args: anytype) noreturn {
    std.debug.print("error: " ++ fmt ++ "\n", args);
    std.process.exit(1);
}

/// The head's length (through the blank line) and its parsed form.
fn read(bytes: []const u8) struct { usize, Head } {
    var hp: std.http.HeadParser = .{};
    const len = hp.feed(bytes);
    if (hp.state != .finished) fail("HeadParser did not find the end of the head", .{});
    const head = Head.parse(bytes[0..len]) catch |e| fail("Head.parse: {t}", .{e});
    return .{ len, head };
}

/// Timed path: status, head length and framing facts folded into one number.
fn parse(bytes: []const u8) u64 {
    const len, const head = read(bytes);
    const length: u64 = if (head.transfer_encoding == .chunked) 1 << 40 else head.content_length orelse 0;
    return @as(u64, @backingInt(head.status)) + len + length + @intFromBool(head.keep_alive);
}

/// Splits a workload file into heads: each head ends at its first CRLF CRLF.
fn splitHeads(gpa: std.mem.Allocator, bytes: []const u8) ![]const []const u8 {
    var heads: std.ArrayList([]const u8) = .empty;
    var rest = bytes;
    while (rest.len > 0) {
        const end = (std.mem.find(u8, rest, "\r\n\r\n") orelse fail("trailing bytes after the last head", .{})) + 4;
        try heads.append(gpa, rest[0..end]);
        rest = rest[end..];
    }
    return heads.toOwnedSlice(gpa);
}

fn printHex(w: *Io.Writer, bytes: []const u8) !void {
    for (bytes) |b| try w.print("{x:0>2}", .{b});
}

fn check(w: *Io.Writer, heads: []const []const u8) !void {
    for (heads) |bytes| {
        const len, const head = read(bytes);
        var count: usize = 0;
        var counter = head.iterateHeaders();
        while (counter.next()) |_| count += 1;
        try w.print("head {d} {d} ", .{ @backingInt(head.status), len });
        if (head.content_length) |n| try w.print("{d} ", .{n}) else try w.writeAll("- ");
        try w.print("{d}\n", .{count});
        var it = head.iterateHeaders();
        while (it.next()) |field| {
            try w.writeAll("field ");
            try printHex(w, field.name);
            try w.writeAll(" ");
            try printHex(w, field.value);
            try w.writeAll("\n");
        }
    }
    // Neither HeadParser.feed nor Head.parse takes an allocator: zero allocations by construction.
    try w.writeAll("allocs 0\n");
}

fn runBlock(heads: []const []const u8, next: *usize) u64 {
    var sink: u64 = 0;
    for (0..block) |_| {
        sink +%= parse(heads[next.*]);
        next.* = (next.* + 1) % heads.len;
    }
    return sink;
}

fn elapsedNs(io: Io, since: Io.Timestamp) u64 {
    return @intCast(since.untilNow(io, .awake).nanoseconds);
}

fn time(io: Io, w: *Io.Writer, heads: []const []const u8, run_ns: u64) !void {
    var next: usize = 0;
    var sink: u64 = 0;
    const warm = Io.Timestamp.now(io, .awake);
    while (elapsedNs(io, warm) < warm_up_ns) sink +%= runBlock(heads, &next);
    var parsed: u64 = 0;
    const start = Io.Timestamp.now(io, .awake);
    while (elapsedNs(io, start) < run_ns) {
        sink +%= runBlock(heads, &next);
        parsed += block;
    }
    const ns: f64 = @floatFromInt(elapsedNs(io, start));
    try w.print("ns {d:.4} {d}\n", .{ ns / @as(f64, @floatFromInt(parsed)), sink });
}

/// HeadParser resumes at any byte; Head.parse runs once on the complete head.
fn feed(io: Io, w: *Io.Writer, heads: []const []const u8) !void {
    if (heads.len != 1) fail("a feed workload holds exactly one head", .{});
    const input = heads[0];
    const start = Io.Timestamp.now(io, .awake);
    var hp: std.http.HeadParser = .{};
    var len: usize = 0;
    while (hp.state != .finished and len < input.len) len += hp.feed(input[len .. len + 1]);
    if (hp.state != .finished or len != input.len) fail("HeadParser completed at the wrong byte", .{});
    const head = Head.parse(input[0..len]) catch |e| fail("Head.parse: {t}", .{e});
    std.mem.doNotOptimizeAway(head.keep_alive);
    try w.print("feed {d} incremental {d}\n", .{ elapsedNs(io, start), len });
}

pub fn main(init: std.process.Init) !void {
    const io = init.io;
    const arena = init.arena.allocator();
    const args = try init.minimal.args.toSlice(arena);
    if (args.len < 4 or !std.mem.eql(u8, args[1], "zig-std-http")) {
        fail("usage: zig-std-http <check|time|feed> <workload file> [run ms]", .{});
    }
    const bytes = try Io.Dir.cwd().readFileAlloc(io, args[3], arena, .unlimited);
    const heads = try splitHeads(arena, bytes);
    if (heads.len == 0) fail("empty workload", .{});

    var buf: [64 * 1024]u8 = undefined;
    var out: Io.File.Writer = .init(.stdout(), io, &buf);
    const w = &out.interface;
    const mode = args[2];
    if (std.mem.eql(u8, mode, "check")) {
        try check(w, heads);
    } else if (std.mem.eql(u8, mode, "time")) {
        const ms = if (args.len > 4) try std.fmt.parseInt(u64, args[4], 10) else 300;
        try time(io, w, heads, ms * std.time.ns_per_ms);
    } else if (std.mem.eql(u8, mode, "feed")) {
        try feed(io, w, heads);
    } else {
        fail("unknown mode {s}", .{mode});
    }
    try w.flush();
}
