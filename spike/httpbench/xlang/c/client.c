// libcurl multi interface: 50 reusable easy handles, one thread, curl_multi_poll loop.
// JSON: tiny hand-written scanner (json_top_level_int) that walks the body, skips strings and
// nested values, and returns the integer value of the top-level "id" key. It is not a validating parser.
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define URL "http://127.0.0.1:3999/data"
#define CONCURRENCY 50
#define TOTAL 30000
#define WARMUP 3000

typedef struct { CURL *h; char buf[8192]; size_t len; double t0; long idx; } Slot;

static double now_ms(void) {
  struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1e3 + ts.tv_nsec / 1e6;
}

static size_t on_data(char *p, size_t sz, size_t n, void *ud) {
  Slot *s = ud; size_t k = sz * n;
  if (s->len + k >= sizeof s->buf) return 0; // body larger than buffer -> transfer error
  memcpy(s->buf + s->len, p, k); s->len += k;
  return k;
}

// Returns value of top-level integer key `key`, or -1. Skips strings (with escapes) and tracks depth.
static long json_top_level_int(const char *j, size_t n, const char *key) {
  size_t klen = strlen(key); int depth = 0;
  for (size_t i = 0; i < n; i++) {
    char c = j[i];
    if (c == '{' || c == '[') depth++;
    else if (c == '}' || c == ']') depth--;
    else if (c == '"') {
      size_t start = ++i;
      while (i < n && j[i] != '"') { if (j[i] == '\\') i++; i++; }
      if (depth != 1 || i - start != klen || memcmp(j + start, key, klen)) continue;
      size_t k = i + 1; int colon = 0;
      while (k < n && (j[k] == ' ' || j[k] == ':')) colon |= j[k++] == ':'; // only a key is followed by ':'
      if (!colon) continue;
      return strtol(j + k, NULL, 10);
    }
  }
  return -1;
}

static int cmp(const void *a, const void *b) {
  double x = *(const double *)a, y = *(const double *)b; return (x > y) - (x < y);
}

static Slot slots[CONCURRENCY];

static void start(CURLM *m, Slot *s, long idx) {
  s->len = 0; s->idx = idx; s->t0 = now_ms();
  curl_multi_remove_handle(m, s->h); // re-adding restarts the same easy handle (keeps its connection)
  curl_multi_add_handle(m, s->h);
}

static void run(CURLM *m, long n, long *rps, double *p50, double *p99) {
  double *lat = malloc(sizeof(double) * n);
  long next = 0, done = 0; int running;
  double t0 = now_ms();
  for (int i = 0; i < CONCURRENCY && next < n; i++) start(m, &slots[i], next++);
  while (done < n) {
    curl_multi_perform(m, &running);
    CURLMsg *msg; int q;
    while ((msg = curl_multi_info_read(m, &q))) {
      if (msg->msg != CURLMSG_DONE) continue;
      Slot *s; curl_easy_getinfo(msg->easy_handle, CURLINFO_PRIVATE, (char **)&s);
      if (msg->data.result != CURLE_OK || json_top_level_int(s->buf, s->len, "id") != 42) {
        fprintf(stderr, "bad response: %s\n", curl_easy_strerror(msg->data.result)); exit(1);
      }
      lat[s->idx] = now_ms() - s->t0; done++;
      if (next < n) start(m, s, next++); else curl_multi_remove_handle(m, s->h);
    }
    if (done < n) curl_multi_poll(m, NULL, 0, 100, NULL);
  }
  double el = (now_ms() - t0) / 1e3;
  qsort(lat, n, sizeof(double), cmp);
  *rps = (long)(n / el); *p50 = lat[n / 2]; *p99 = lat[n * 99 / 100];
  free(lat);
}

int main(void) {
  // self-check of the scanner: nested "id" must be ignored, string contents skipped
  const char *t = "{\"a\":{\"id\":1},\"s\":\"\\\"id\\\":7\",\"id\": 42}";
  if (json_top_level_int(t, strlen(t), "id") != 42) { fprintf(stderr, "scanner self-check failed\n"); return 1; }

  curl_global_init(CURL_GLOBAL_DEFAULT);
  CURLM *m = curl_multi_init();
  curl_multi_setopt(m, CURLMOPT_MAX_HOST_CONNECTIONS, (long)CONCURRENCY);
  for (int i = 0; i < CONCURRENCY; i++) {
    CURL *h = slots[i].h = curl_easy_init();
    curl_easy_setopt(h, CURLOPT_URL, URL);
    curl_easy_setopt(h, CURLOPT_WRITEFUNCTION, on_data);
    curl_easy_setopt(h, CURLOPT_WRITEDATA, &slots[i]);
    curl_easy_setopt(h, CURLOPT_PRIVATE, &slots[i]);
    curl_easy_setopt(h, CURLOPT_TCP_NODELAY, 1L);
  }
  long rps; double p50, p99;
  run(m, WARMUP, &rps, &p50, &p99);
  run(m, TOTAL, &rps, &p50, &p99);
  printf("{\"name\":\"C libcurl multi\",\"rps\":%ld,\"p50\":\"%.2f\",\"p99\":\"%.2f\"}\n", rps, p50, p99);
  return 0;
}
