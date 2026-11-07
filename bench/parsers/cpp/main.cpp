// C++ harness of the cross-language response-head parser bench: Boost.Beast, through its two parser APIs.
// Command line and output protocol: bench/parsers/README.md ("Harness protocol").
//
// - beast-response-parser: http::response_parser<http::empty_body>, the API Beast's own clients use; it stores
//   every field in an http::fields container (one allocation per field).
// - beast-basic-parser: a class derived from http::basic_parser<false> (Beast's callback API, no container); the
//   field callback does nothing on the timed path and records spans for the differential check.
// Both derive the framing facts themselves (content_length(), chunked(), keep_alive()). A Beast parser handles
// one message, so each head gets a new parser.
#include <boost/beast/core/error.hpp>
#include <boost/beast/http.hpp>

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <new>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace http = boost::beast::http;
using boost::beast::error_code;

namespace {

// The feed inputs reach 60 KB; Beast's default header limit (8 KiB) would reject them.
constexpr std::uint32_t header_limit = 1u << 20;
constexpr auto warm_up = std::chrono::milliseconds(100);
constexpr int block = 1000;
constexpr int alloc_sample = 10000;

unsigned long long allocations = 0;

[[noreturn]] void die(std::string_view what) {
    std::fprintf(stderr, "error: %.*s\n", static_cast<int>(what.size()), what.data());
    std::exit(1);
}

using field_list = std::vector<std::pair<std::string_view, std::string_view>>;

struct description {
    int status;
    std::size_t head_len;
    std::optional<std::uint64_t> content_length;
    field_list fields;
};

// Status, head length and framing facts folded into one number (the timed path's result).
template <class Parser>
std::uint64_t fold(Parser const& p, int status, std::size_t head_len) {
    std::uint64_t length = p.chunked() ? 1ull << 40 : p.content_length().value_or(0);
    return static_cast<std::uint64_t>(status) + head_len + length + (p.keep_alive() ? 1 : 0);
}

template <class Parser>
std::size_t put_head(Parser& p, std::string_view head) {
    error_code ec;
    std::size_t n = p.put(boost::asio::buffer(head.data(), head.size()), ec);
    if (ec || !p.is_header_done()) die(ec ? ec.message() : "header not complete");
    return n;
}

class span_parser final : public http::basic_parser<false> {
public:
    explicit span_parser(field_list* fields) : fields_(fields) { header_limit(::header_limit); }
    int status = 0;

private:
    using sv = boost::beast::string_view;
    field_list* fields_;

    void on_request_impl(http::verb, sv, sv, int, error_code&) override {}
    void on_response_impl(int code, sv, int, error_code&) override { status = code; }
    void on_field_impl(http::field, sv name, sv value, error_code&) override {
        if (fields_ != nullptr) fields_->emplace_back(name, value);
    }
    void on_trailer_field_impl(http::field, sv, sv, error_code&) override {}
    void on_header_impl(error_code&) override {}
    void on_body_init_impl(boost::optional<std::uint64_t> const&, error_code&) override {}
    std::size_t on_body_impl(sv body, error_code&) override { return body.size(); }
    void on_chunk_header_impl(std::uint64_t, sv, error_code&) override {}
    std::size_t on_chunk_body_impl(std::uint64_t, sv body, error_code&) override { return body.size(); }
    void on_finish_impl(error_code&) override {}
};

struct response_parser : http::response_parser<http::empty_body> {
    response_parser() { header_limit(::header_limit); }
};

struct engine {
    std::string_view name;
    std::uint64_t (*parse)(std::string_view head);
    description (*describe)(std::string_view head);
};

std::optional<std::uint64_t> to_std(boost::optional<std::uint64_t> const& v) {
    return v ? std::optional<std::uint64_t>(*v) : std::nullopt;
}

constexpr engine engines[] = {
    {"beast-response-parser",
     [](std::string_view head) {
         response_parser p;
         std::size_t n = put_head(p, head);
         return fold(p, p.get().result_int(), n);
     },
     [](std::string_view head) {
         response_parser p;
         description d{0, put_head(p, head), to_std(p.content_length()), {}};
         d.status = static_cast<int>(p.get().result_int());
         // Field views point into the parser's container, so copy them into storage that outlives it.
         static std::vector<std::string> storage;
         storage.clear();
         storage.reserve(2 * std::distance(p.get().begin(), p.get().end()));
         for (auto const& f : p.get()) {
             storage.emplace_back(f.name_string());
             std::string_view name = storage.back();
             storage.emplace_back(f.value());
             d.fields.emplace_back(name, storage.back());
         }
         return d;
     }},
    {"beast-basic-parser",
     [](std::string_view head) {
         span_parser p(nullptr);
         std::size_t n = put_head(p, head);
         return fold(p, p.status, n);
     },
     [](std::string_view head) {
         description d{};
         span_parser p(&d.fields);
         d.head_len = put_head(p, head);
         d.status = p.status;
         d.content_length = to_std(p.content_length());
         return d;
     }},
};

std::vector<std::string_view> split_heads(std::string_view bytes) {
    std::vector<std::string_view> heads;
    while (!bytes.empty()) {
        std::size_t end = bytes.find("\r\n\r\n");
        if (end == std::string_view::npos) die("trailing bytes after the last head");
        heads.push_back(bytes.substr(0, end + 4));
        bytes.remove_prefix(end + 4);
    }
    return heads;
}

void print_hex(std::string_view s) {
    for (unsigned char c : s) std::printf("%02x", c);
}

void check(engine const& e, std::vector<std::string_view> const& heads) {
    for (std::string_view head : heads) {
        description d = e.describe(head);
        std::printf("head %d %zu ", d.status, d.head_len);
        if (d.content_length) std::printf("%llu ", static_cast<unsigned long long>(*d.content_length));
        else std::printf("- ");
        std::printf("%zu\n", d.fields.size());
        for (auto const& [name, value] : d.fields) {
            std::printf("field ");
            print_hex(name);
            std::printf(" ");
            print_hex(value);
            std::printf("\n");
        }
    }
    unsigned long long before = allocations;
    std::uint64_t volatile sink = 0;
    for (int i = 0; i < alloc_sample; i++) sink = sink + e.parse(heads[i % heads.size()]);
    std::printf("allocs %g\n", static_cast<double>(allocations - before) / alloc_sample);
}

std::uint64_t run_block(engine const& e, std::vector<std::string_view> const& heads, std::size_t& next) {
    std::uint64_t sink = 0;
    for (int i = 0; i < block; i++) {
        sink += e.parse(heads[next]);
        next = (next + 1) % heads.size();
    }
    return sink;
}

void time_engine(engine const& e, std::vector<std::string_view> const& heads, std::chrono::milliseconds run) {
    using clock = std::chrono::steady_clock;
    std::size_t next = 0;
    std::uint64_t sink = 0;
    for (auto warm = clock::now(); clock::now() - warm < warm_up;) sink += run_block(e, heads, next);
    unsigned long long parsed = 0;
    auto start = clock::now();
    while (clock::now() - start < run) {
        sink += run_block(e, heads, next);
        parsed += block;
    }
    double ns = std::chrono::duration<double, std::nano>(clock::now() - start).count();
    std::printf("ns %.4f %llu\n", ns / static_cast<double>(parsed), static_cast<unsigned long long>(sink));
}

// Beast consumes each complete line (start line, then field by field) and reports need_more for a partial one, so
// the caller drops the consumed bytes and calls again when more arrive; a partial line is parsed again from its
// start on every call (only the search for its end resumes), which is quadratic in the line length.
template <class Parser>
std::size_t feed_parser(Parser& p, std::string_view input) {
    std::size_t consumed = 0;
    for (std::size_t end = 1; end <= input.size(); end++) {
        error_code ec;
        consumed += p.put(boost::asio::buffer(input.data() + consumed, end - consumed), ec);
        if (ec == http::error::need_more) continue;
        if (ec || !p.is_header_done() || end != input.size()) die(ec ? ec.message() : "header completed at the wrong byte");
        return consumed;
    }
    die("header never completed");
}

void feed(std::string_view name, std::vector<std::string_view> const& heads) {
    if (heads.size() != 1) die("a feed workload holds exactly one head");
    auto start = std::chrono::steady_clock::now();
    std::size_t head_len = 0;
    if (name == "beast-response-parser") {
        response_parser p;
        head_len = feed_parser(p, heads[0]);
    } else {
        span_parser p(nullptr);
        head_len = feed_parser(p, heads[0]);
    }
    auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now() - start);
    std::printf("feed %lld line-restart %zu\n", static_cast<long long>(ns.count()), head_len);
}

}  // namespace

// Counts allocations made through operator new (Beast's containers use std::allocator).
void* operator new(std::size_t size) {
    allocations++;
    if (void* p = std::malloc(size == 0 ? 1 : size)) return p;
    throw std::bad_alloc();
}
void operator delete(void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }

int main(int argc, char** argv) {
    if (argc < 4) die("usage: <beast-response-parser|beast-basic-parser> <check|time|feed> <workload file> [run ms]");
    engine const* e = nullptr;
    for (engine const& candidate : engines) {
        if (candidate.name == argv[1]) e = &candidate;
    }
    if (e == nullptr) die("unknown parser");
    std::ifstream in(argv[3], std::ios::binary);
    if (!in) die("cannot open workload file");
    std::string bytes((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    std::vector<std::string_view> heads = split_heads(bytes);
    if (heads.empty()) die("empty workload");
    std::string_view mode = argv[2];
    if (mode == "check") {
        check(*e, heads);
    } else if (mode == "time") {
        long ms = argc > 4 ? std::strtol(argv[4], nullptr, 10) : 300;
        time_engine(*e, heads, std::chrono::milliseconds(ms));
    } else if (mode == "feed") {
        feed(e->name, heads);
    } else {
        die("unknown mode");
    }
    return 0;
}
