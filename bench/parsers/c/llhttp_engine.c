/* llhttp adapter (strict mode, the default). llhttp derives the framing facts itself (content_length, F_CHUNKED,
 * llhttp_should_keep_alive). Like every parser in the bench, the timed path delivers each field to the caller: the
 * field callbacks record name/value spans into a reused list (OWS trimmed), and on_headers_complete pauses the
 * parser so the head length is known. `check` prints from this same path. */
#include <stdbool.h>

#include "engine.h"
#include "llhttp.h"

static int pause_at_head_end(llhttp_t *parser) {
    (void)parser;
    return HPE_PAUSED;
}

/* Runs `parser` over `head`; returns the head length (bytes up to and including the blank line), or 0 when the
 * head is not complete yet. */
static size_t execute(llhttp_t *parser, const char *data, size_t len) {
    llhttp_errno_t err = llhttp_execute(parser, data, len);
    if (err == HPE_OK) return 0;
    if (err != HPE_PAUSED) die(llhttp_get_error_reason(parser));
    return (size_t)(llhttp_get_error_pos(parser) - data);
}

/* Field spans over contiguous input: a span split across calls (1-byte feeds) is extended by the next call. */
static int on_field(llhttp_t *parser, const char *at, size_t len) {
    field_list_t *list = parser->data;
    field_t *last = list->count > 0 ? &list->fields[list->count - 1] : NULL;
    if (last != NULL && last->value == NULL) {
        last->name_len += len;
        return 0;
    }
    if (list->count == MAX_FIELDS) return -1;
    list->fields[list->count++] = (field_t){at, len, NULL, 0};
    return 0;
}

static int on_value(llhttp_t *parser, const char *at, size_t len) {
    field_list_t *list = parser->data;
    field_t *last = &list->fields[list->count - 1];
    if (last->value == NULL) last->value = at;
    last->value_len += len;
    return 0;
}

/* Ends a value: llhttp hands it with its trailing OWS (found by the gate's `gate-framing` head), which RFC 9112 §5
 * excludes from the field value, so it is trimmed here as the Rust split does. An empty value gets a non-NULL
 * pointer, so the next on_field starts a new field. */
static int on_value_complete(llhttp_t *parser) {
    field_list_t *list = parser->data;
    field_t *last = &list->fields[list->count - 1];
    if (last->value == NULL) last->value = "";
    while (last->value_len > 0 && (last->value[last->value_len - 1] == ' ' || last->value[last->value_len - 1] == '\t')) {
        last->value_len--;
    }
    return 0;
}

static const llhttp_settings_t settings = {
    .on_header_field = on_field,
    .on_header_value = on_value,
    .on_header_value_complete = on_value_complete,
    .on_headers_complete = pause_at_head_end,
};

/* Parses one complete head, delivering its fields into `list`. */
static facts_t parse_into(const char *head, size_t len, field_list_t *list) {
    llhttp_t parser;
    llhttp_init(&parser, HTTP_RESPONSE, &settings);
    list->count = 0;
    parser.data = list;
    size_t head_len = execute(&parser, head, len);
    if (head_len == 0) die("llhttp did not complete the head");
    length_kind_t kind = (parser.flags & F_CHUNKED)          ? LENGTH_CHUNKED
                         : (parser.flags & F_CONTENT_LENGTH) ? LENGTH_BYTES
                                                             : LENGTH_UNKNOWN;
    return (facts_t){llhttp_get_status_code(&parser), head_len, kind, kind == LENGTH_BYTES ? parser.content_length : 0,
                     llhttp_should_keep_alive(&parser) != 0};
}

/* The connection's reused field list (the harness is single-threaded). */
static field_list_t reused;

static facts_t parse(const char *head, size_t len) { return parse_into(head, len, &reused); }

ENGINE_TIMER(time_parse, parse)

static void fields(const char *head, size_t len, field_list_t *out) { parse_into(head, len, out); }

/* Resumable: each call continues from the parser's saved state, delivering field spans as they arrive. */
static const char *feed(const char *input, size_t len, size_t *head_len) {
    llhttp_t parser;
    llhttp_init(&parser, HTTP_RESPONSE, &settings);
    reused.count = 0;
    parser.data = &reused;
    for (size_t i = 0; i < len; i++) {
        size_t at = execute(&parser, input + i, 1);
        if ((at != 0) != (i + 1 == len)) die("llhttp completed at the wrong byte");
    }
    *head_len = len;
    return "incremental";
}

const engine_t llhttp_engine = {"llhttp", parse, time_parse, fields, feed};
