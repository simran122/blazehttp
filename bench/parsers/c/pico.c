/* picohttpparser adapter: phr_parse_response into a caller array, then the framing facts our parser folds
 * (picohttpparser does not derive them), with the same rules as the Rust httparse adapter. */
#include <stdbool.h>
#include <string.h>
#include <strings.h>

#include "engine.h"
#include "picohttpparser.h"

typedef struct {
    int64_t content_length; /* -1: absent */
    bool chunked;
    bool close;
} header_facts_t;

static bool name_is(const struct phr_header *h, const char *name) {
    size_t len = strlen(name);
    return h->name_len == len && strncasecmp(h->name, name, len) == 0;
}

static bool is_ows(char c) { return c == ' ' || c == '\t'; }

/* Whether the comma-separated list `value` holds the token `close` (ASCII case-insensitive). */
static bool has_close(const char *value, size_t len) {
    size_t i = 0;
    while (i <= len) {
        size_t start = i;
        while (i < len && value[i] != ',') i++;
        size_t a = start, b = i;
        while (a < b && is_ows(value[a])) a++;
        while (b > a && is_ows(value[b - 1])) b--;
        if (b - a == 5 && strncasecmp(value + a, "close", 5) == 0) return true;
        i++;
    }
    return false;
}

static header_facts_t derive_facts(const struct phr_header *headers, size_t count) {
    header_facts_t f = {-1, false, false};
    for (size_t i = 0; i < count; i++) {
        const struct phr_header *h = &headers[i];
        if (name_is(h, "content-length")) {
            int64_t n = 0;
            for (size_t j = 0; j < h->value_len; j++) {
                char c = h->value[j];
                if (c < '0' || c > '9' || n > (INT64_MAX - 9) / 10) die("invalid content-length");
                n = n * 10 + (c - '0');
            }
            f.content_length = n;
        } else if (name_is(h, "transfer-encoding")) {
            f.chunked = h->value_len == 7 && strncasecmp(h->value, "chunked", 7) == 0;
        } else if (name_is(h, "connection")) {
            f.close |= has_close(h->value, h->value_len);
        }
    }
    return f;
}

typedef struct {
    int status;
    size_t head_len;
    size_t num_headers;
    struct phr_header headers[MAX_FIELDS];
} parsed_t;

static int parse_into(const char *head, size_t len, size_t last_len, parsed_t *out) {
    int minor_version = 0;
    const char *msg = NULL;
    size_t msg_len = 0;
    out->num_headers = MAX_FIELDS;
    int r = phr_parse_response(head, len, &minor_version, &out->status, &msg, &msg_len, out->headers,
                               &out->num_headers, last_len);
    out->head_len = r > 0 ? (size_t)r : 0;
    return r;
}

static facts_t parse(const char *head, size_t len) {
    parsed_t p;
    if (parse_into(head, len, 0, &p) <= 0) die("picohttpparser did not parse the head");
    header_facts_t f = derive_facts(p.headers, p.num_headers);
    length_kind_t kind = f.chunked ? LENGTH_CHUNKED : f.content_length < 0 ? LENGTH_UNKNOWN : LENGTH_BYTES;
    return (facts_t){p.status, p.head_len, kind, kind == LENGTH_BYTES ? (uint64_t)f.content_length : 0,
                     !f.close && kind != LENGTH_UNKNOWN};
}

ENGINE_TIMER(time_parse, parse)

static void fields(const char *head, size_t len, field_list_t *out) {
    parsed_t p;
    if (parse_into(head, len, 0, &p) <= 0) die("picohttpparser did not parse the head");
    out->count = p.num_headers;
    for (size_t i = 0; i < p.num_headers; i++) {
        out->fields[i] = (field_t){p.headers[i].name, p.headers[i].name_len, p.headers[i].value, p.headers[i].value_len};
    }
}

/* Not resumable: each call re-parses from the first byte, but `last_len` lets it scan only the new bytes for the
 * end of the head while the head is incomplete, so the full parse runs once. */
static const char *feed(const char *input, size_t len, size_t *head_len) {
    parsed_t p;
    for (size_t end = 1; end <= len; end++) {
        int r = parse_into(input, end, end - 1, &p);
        if (r == -1) die("picohttpparser rejected a prefix");
        if ((r > 0) != (end == len)) die("picohttpparser completed at the wrong byte");
    }
    *head_len = p.head_len;
    return "restart-skip";
}

const engine_t pico_engine = {"picohttpparser", parse, time_parse, fields, feed};
