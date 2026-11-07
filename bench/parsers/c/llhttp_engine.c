/* llhttp adapter (strict mode, the default). llhttp derives the framing facts itself (content_length, F_CHUNKED,
 * llhttp_should_keep_alive); the timed path registers only on_headers_complete, which pauses the parser so the
 * head length is known. Field callbacks are registered for the differential check only. */
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

static const llhttp_settings_t timed_settings = {.on_headers_complete = pause_at_head_end};

static facts_t parse(const char *head, size_t len) {
    llhttp_t parser;
    llhttp_init(&parser, HTTP_RESPONSE, &timed_settings);
    size_t head_len = execute(&parser, head, len);
    if (head_len == 0) die("llhttp did not complete the head");
    length_kind_t kind = (parser.flags & F_CHUNKED)          ? LENGTH_CHUNKED
                         : (parser.flags & F_CONTENT_LENGTH) ? LENGTH_BYTES
                                                             : LENGTH_UNKNOWN;
    return (facts_t){llhttp_get_status_code(&parser), head_len, kind, kind == LENGTH_BYTES ? parser.content_length : 0,
                     llhttp_should_keep_alive(&parser) != 0};
}

ENGINE_TIMER(time_parse, parse)

/* Field spans of one contiguous head (each callback gets the whole span; consecutive calls extend it). */
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

/* Closes a field whose value is empty (no on_header_value call), so the next on_field starts a new field. */
static int on_value_complete(llhttp_t *parser) {
    field_list_t *list = parser->data;
    field_t *last = &list->fields[list->count - 1];
    if (last->value == NULL) last->value = "";
    return 0;
}

static const llhttp_settings_t field_settings = {
    .on_header_field = on_field,
    .on_header_value = on_value,
    .on_header_value_complete = on_value_complete,
    .on_headers_complete = pause_at_head_end,
};

static void fields(const char *head, size_t len, field_list_t *out) {
    llhttp_t parser;
    llhttp_init(&parser, HTTP_RESPONSE, &field_settings);
    out->count = 0;
    parser.data = out;
    if (execute(&parser, head, len) == 0) die("llhttp did not complete the head");
    /* llhttp hands a value with its trailing OWS (found by the gate's `gate-framing` head); RFC 9112 §5 excludes OWS
     * from the field value, so a consumer trims it, and the field view does too (outside the timed path). */
    for (size_t i = 0; i < out->count; i++) {
        field_t *f = &out->fields[i];
        while (f->value_len > 0 && (f->value[f->value_len - 1] == ' ' || f->value[f->value_len - 1] == '\t')) {
            f->value_len--;
        }
    }
}

/* Resumable: each call continues from the parser's saved state. */
static const char *feed(const char *input, size_t len, size_t *head_len) {
    llhttp_t parser;
    llhttp_init(&parser, HTTP_RESPONSE, &timed_settings);
    for (size_t i = 0; i < len; i++) {
        size_t at = execute(&parser, input + i, 1);
        if ((at != 0) != (i + 1 == len)) die("llhttp completed at the wrong byte");
    }
    *head_len = len;
    return "incremental";
}

const engine_t llhttp_engine = {"llhttp", parse, time_parse, fields, feed};
