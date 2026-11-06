// Boost.Beast + Asio C++20 coroutines, single-threaded io_context, 50 keep-alive connections
// (one coroutine per connection). Body parsed with Boost.JSON, top-level "id" checked == 42.
#include <boost/asio.hpp>
#include <boost/beast.hpp>
#include <boost/json.hpp>
#include <algorithm>
#include <chrono>
#include <cstdio>
#include <vector>

namespace asio = boost::asio;
namespace beast = boost::beast;
namespace http = beast::http;
using tcp = asio::ip::tcp;
using clk = std::chrono::steady_clock;

constexpr int CONCURRENCY = 50, TOTAL = 30000, WARMUP = 3000;

struct Run { long n; long next = 0; std::vector<double> lat; };

asio::awaitable<void> worker(tcp::socket& sock, Run& r) {
  beast::flat_buffer buf;
  http::request<http::empty_body> req{http::verb::get, "/data", 11};
  req.set(http::field::host, "127.0.0.1:3999");
  req.keep_alive(true);
  while (r.next < r.n) {
    long i = r.next++;
    auto t = clk::now();
    co_await http::async_write(sock, req, asio::use_awaitable);
    http::response<http::string_body> res;
    co_await http::async_read(sock, buf, res, asio::use_awaitable);
    auto v = boost::json::parse(res.body());
    if (v.as_object().at("id").as_int64() != 42) throw std::runtime_error("bad body");
    r.lat[i] = std::chrono::duration<double, std::milli>(clk::now() - t).count();
  }
}

int main() {
  asio::io_context ioc{1};
  std::vector<tcp::socket> socks;
  tcp::endpoint ep{asio::ip::make_address("127.0.0.1"), 3999};
  for (int i = 0; i < CONCURRENCY; i++) {
    socks.emplace_back(ioc).connect(ep);
    socks.back().set_option(tcp::no_delay(true));
  }
  auto bench = [&](long n) {
    Run r{n}; r.lat.resize(n);
    auto t0 = clk::now();
    for (auto& s : socks) asio::co_spawn(ioc, worker(s, r), [](std::exception_ptr e) { if (e) std::rethrow_exception(e); });
    ioc.run(); ioc.restart();
    double el = std::chrono::duration<double>(clk::now() - t0).count();
    std::sort(r.lat.begin(), r.lat.end());
    return std::tuple{long(n / el), r.lat[n / 2], r.lat[n * 99 / 100]};
  };
  bench(WARMUP);
  auto [rps, p50, p99] = bench(TOTAL);
  std::printf("{\"name\":\"C++ Boost.Beast\",\"rps\":%ld,\"p50\":\"%.2f\",\"p99\":\"%.2f\"}\n", rps, p50, p99);
}
