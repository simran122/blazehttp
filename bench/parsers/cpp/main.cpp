// C++ harness of the cross-language response-head parser bench: Boost.Beast, through its two parser APIs.
// Command line and output protocol: bench/parsers/README.md ("Harness protocol").
//
// - beast-response-parser: http::response_parser<http::empty_body>, the API Beast's own clients use; it stores
//   every field in an http::fields container (one allocation per field plus one for the container).
// - beast-basic-parser: a class derived from http::basic_parser<false> (Beast's callback API, no container); the
//   field callback records each name/value span into a reused list on the timed path, as every parser in the bench
//   delivers its fields, and `check` prints from that same path.
// Both derive the framing facts themselves (content_length(), chunked(), keep_alive()). A Beast parser handles
// one message, so each head gets a new parser. Each engine is a type, so every loop calls it directly.
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

using field_list = std::vector<std::pair<std::string, std::string>>;

// What the timed path produces for one head (printed by `check` as the head line).
struct facts {
    int status;
    std::size_t head_len;
    bool chunked;
    std::optional<std::uint64_t> content_length;
    bool keep_alive;
};

std::uint64_t fold(facts const& f) {
    std::uint64_t length = f.chunked ? 1ull << 40 : f.content_length.value_or(0);
    return static_cast<std::uint64_t>(f.status) + f.head_len + length + (f.keep_alive ? 1 : 0);
}

template <class Parser>
facts facts_of(Parser const& p, int status, std::size_t head_len) {
    auto length = p.content_length();
    return {status, head_len, p.chunked(), length ? std::optional<std::uint64_t>(*length) : std::nullopt,
            p.keep_alive()};
}

template <class Parser>
std::size_t put_head(Parser& p, std::string_view head) {
    error_code ec;
    std::size_t n = p.put(boost::asio::buffer(head.data(), head.size()), ec);
    if (ec || !p.is_header_done()) die(ec ? ec.message() : "header not complete");
    return n;
}

// Name/value spans into the parsed bytes; reserved once and reused, so recording them never allocates.
using span_list = std::vector<std::pair<std::string_view, std::string_view>>;

class span_parser final : public http::basic_parser<false> {
public:
    explicit span_parser(span_list& spans) : spans_(spans) {
        header_limit(::header_limit);
        spans_.clear();
    }
    int status = 0;

private:
    using sv = boost::beast::string_view;
    span_list& spans_;

    void on_request_impl(http::verb, sv, sv, int, error_code&) override {}
    void on_response_impl(int code, sv, int, error_code&) override { status = code; }
    void on_field_impl(http::field, sv name, sv value, error_code&) override {
        spans_.emplace_back(name, value);
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
        if (ec || !p.is_header_done() || end != input.size()) {
            die(ec ? ec.message() : "header completed at the wrong byte");
        }
        return consumed;
    }
    die("header never completed");
}

struct beast_response_parser {
    static constexpr std::string_view name = "beast-response-parser";
    static facts parse(std::string_view head) {
        response_parser p;
        std::size_t n = put_head(p, head);
        return facts_of(p, p.get().result_int(), n);
    }
    static field_list fields(std::string_view head) {
        response_parser p;
        put_head(p, head);
        field_list out;
        for (auto const& f : p.get()) out.emplace_back(f.name_string(), f.value());
        return out;
    }
    static std::size_t feed(std::string_view input) {
        response_parser p;
        return feed_parser(p, input);
    }
};

struct beast_basic_parser {
    static constexpr std::string_view name = "beast-basic-parser";
    // The connection's reused span list (the harness is single-threaded).
    static span_list& reused() {
        static span_list spans = [] {
            span_list s;
            s.reserve(100);
            return s;
        }();
        return spans;
    }
    static facts parse(std::string_view head) {
        span_parser p(reused());
        std::size_t n = put_head(p, head);
        return facts_of(p, p.status, n);
    }
    static field_list fields(std::string_view head) {
        parse(head);
        field_list out;
        for (auto const& [name, value] : reused()) out.emplace_back(name, value);
        return out;
    }
    static std::size_t feed(std::string_view input) {
        span_parser p(reused());
        return feed_parser(p, input);
    }
};

std::vector<std::string_view> split_heads(std::string_view bytes) {
    std::vector<std::string_view> heads;
    while (!bytes.empty()) {
        std::size_t end = bytes.find("\r\n\r\n");
        if (end == std::string_view::npos) die("trailing bytes after the last head");
        heads.push_back(bytes.substr(0, end + 4));
        bytes.remove_prefix(end + 4);
    }
    if (heads.empty()) die("empty workload");
    return heads;
}

void print_hex(std::string_view s) {
    for (unsigned char c : s) std::printf("%02x", c);
}

template <class Engine>
void check(std::vector<std::string_view> const& heads) {
    for (std::string_view head : heads) {
        facts f = Engine::parse(head);
        field_list fields = Engine::fields(head);
        std::printf("head %d %zu ", f.status, f.head_len);
        if (f.chunked) std::printf("chunked ");
        else if (f.content_length) std::printf("%llu ", static_cast<unsigned long long>(*f.content_length));
        else std::printf("- ");
        std::printf("%d %zu\n", f.keep_alive ? 1 : 0, fields.size());
        for (auto const& [name, value] : fields) {
            std::printf("field ");
            print_hex(name);
            std::printf(" ");
            print_hex(value);
            std::printf("\n");
        }
    }
    unsigned long long before = allocations;
    std::uint64_t volatile sink = 0;
    for (int i = 0; i < alloc_sample; i++) sink = sink + fold(Engine::parse(heads[i % heads.size()]));
    std::printf("allocs %g\n", static_cast<double>(allocations - before) / alloc_sample);
}

template <class Engine>
std::uint64_t run_block(std::vector<std::string_view> const& heads, std::size_t& next) {
    std::uint64_t sink = 0;
    for (int i = 0; i < block; i++) {
        sink += fold(Engine::parse(heads[next]));
        next = next + 1 == heads.size() ? 0 : next + 1;
    }
    return sink;
}

template <class Engine>
void time_engine(std::vector<std::string_view> const& heads, std::chrono::milliseconds run) {
    using clock = std::chrono::steady_clock;
    std::size_t next = 0;
    std::uint64_t sink = 0;
    for (auto warm = clock::now(); clock::now() - warm < warm_up;) sink += run_block<Engine>(heads, next);
    unsigned long long parsed = 0;
    auto start = clock::now();
    while (clock::now() - start < run) {
        sink += run_block<Engine>(heads, next);
        parsed += block;
    }
    double ns = std::chrono::duration<double, std::nano>(clock::now() - start).count();
    std::printf("ns %.4f %llu\n", ns / static_cast<double>(parsed), static_cast<unsigned long long>(sink));
}

template <class Engine>
void feed(std::vector<std::string_view> const& heads) {
    if (heads.size() != 1) die("a feed workload holds exactly one head");
    auto start = std::chrono::steady_clock::now();
    std::size_t head_len = Engine::feed(heads[0]);
    auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now() - start);
    std::printf("feed %lld line-restart %zu\n", static_cast<long long>(ns.count()), head_len);
}

std::chrono::milliseconds run_ms(int argc, char** argv) {
    if (argc < 5) die("time needs a positive integer run time in ms");
    std::string_view text = argv[4];
    char* end = nullptr;
    unsigned long long ms = std::strtoull(argv[4], &end, 10);
    if (text.empty() || text.front() == '-' || *end != '\0' || ms == 0) die("time needs a positive integer run time in ms");
    return std::chrono::milliseconds(ms);
}

template <class Engine>
void run(std::string_view mode, int argc, char** argv, std::vector<std::string_view> const& heads) {
    if (mode == "check") check<Engine>(heads);
    else if (mode == "time") time_engine<Engine>(heads, run_ms(argc, argv));
    else if (mode == "feed") feed<Engine>(heads);
    else die("unknown mode");
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
    std::ifstream in(argv[3], std::ios::binary);
    if (!in) die("cannot open workload file");
    std::string bytes((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    std::vector<std::string_view> heads = split_heads(bytes);
    std::string_view name = argv[1];
    if (name == beast_response_parser::name) run<beast_response_parser>(argv[2], argc, argv, heads);
    else if (name == beast_basic_parser::name) run<beast_basic_parser>(argv[2], argc, argv, heads);
    else die("unknown parser");
    return 0;
}
