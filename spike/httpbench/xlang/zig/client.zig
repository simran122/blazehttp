// Zig 0.17 std.http.Client: one shared Client (keep-alive pool of 50), 50 concurrent tasks via
// Io.Group.concurrent on the default (thread-based) std.Io -> effectively 50 OS threads doing
// blocking I/O. Body parsed with std.json into a struct, top-level id checked == 42.
const std = @import("std");
const Io = std.Io;

const URL = "http://127.0.0.1:3999/data";
const CONCURRENCY = 50;
const TOTAL = 30000;
const WARMUP = 3000;

const Payload = struct { id: i64 };

const Run = struct {
    client: *std.http.Client,
    io: Io,
    n: usize,
    next: std.atomic.Value(usize) = .init(0),
    lat: []f64,
};

fn worker(r: *Run) void {
    var buf: [8192]u8 = undefined;
    var arena_buf: [16384]u8 = undefined;
    while (true) {
        const i = r.next.fetchAdd(1, .monotonic);
        if (i >= r.n) return;
        const t = Io.Timestamp.now(r.io, .awake);
        var w: Io.Writer = .fixed(&buf);
        const res = r.client.fetch(.{ .location = .{ .url = URL }, .response_writer = &w }) catch |e| std.debug.panic("fetch: {t}", .{e});
        if (res.status != .ok) std.debug.panic("status {d}", .{@intFromEnum(res.status)});
        var fba: std.heap.FixedBufferAllocator = .init(&arena_buf);
        const p = std.json.parseFromSliceLeaky(Payload, fba.allocator(), w.buffered(), .{ .ignore_unknown_fields = true }) catch |e| std.debug.panic("json: {t}", .{e});
        if (p.id != 42) std.debug.panic("bad body", .{});
        r.lat[i] = @as(f64, @floatFromInt(t.untilNow(r.io, .awake).nanoseconds)) / 1e6;
    }
}

fn bench(client: *std.http.Client, io: Io, gpa: std.mem.Allocator, n: usize) !struct { u64, f64, f64 } {
    var r: Run = .{ .client = client, .io = io, .n = n, .lat = try gpa.alloc(f64, n) };
    defer gpa.free(r.lat);
    const t0 = Io.Timestamp.now(io, .awake);
    var g: Io.Group = .init;
    for (0..CONCURRENCY) |_| try g.concurrent(io, worker, .{&r});
    try g.await(io);
    const el = @as(f64, @floatFromInt(t0.untilNow(io, .awake).nanoseconds)) / 1e9;
    std.mem.sort(f64, r.lat, {}, std.sort.asc(f64));
    return .{ @intFromFloat(@as(f64, @floatFromInt(n)) / el), r.lat[n / 2], r.lat[n * 99 / 100] };
}

pub fn main(init: std.process.Init) !void {
    const io = init.io;
    const gpa = init.gpa;
    var client: std.http.Client = .{ .allocator = gpa, .io = io };
    client.connection_pool.free_size = CONCURRENCY;
    _ = try bench(&client, io, gpa, WARMUP);
    const rps, const p50, const p99 = try bench(&client, io, gpa, TOTAL);
    var out_buf: [256]u8 = undefined;
    var out: Io.File.Writer = .init(.stdout(), io, &out_buf);
    try out.interface.print("{{\"name\":\"Zig std.http.Client\",\"rps\":{d},\"p50\":\"{d:.2}\",\"p99\":\"{d:.2}\"}}\n", .{ rps, p50, p99 });
    try out.interface.flush();
}
