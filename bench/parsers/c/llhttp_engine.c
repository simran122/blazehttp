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

static uint64_t parse(const char *head, size_t len) {
    llhttp_t parser;
    llhttp_init(&parser, HTTP_RESPONSE, &timed_settings);
    size_t head_len = execute(&parser, head, len);
    if (head_len == 0) die("llhttp did not complete the head");
    uint64_t length = (parser.flags & F_CHUNKED) ? 1ull << 40
                      : (parser.flags & F_CONTENT_LENGTH) ? parser.content_length
                                                          : 0;
    return (uint64_t)llhttp_get_status_code(&parser) + head_len + length + (uint64_t)llhttp_should_keep_alive(&parser);
}

/* Field spans of one contiguous head (each callback gets the whole span; consecutive calls extend it). */
static int on_field(llhttp_t *parser, const char *at, size_t len) {
    description_t *d = parser->data;
    field_t *last = d->num_fields > 0 ? &d->fields[d->num_fields - 1] : NULL;
    if (last != NULL && last->value == NULL) {
        last->name_len += len;
        return 0;
    }
    if (d->num_fields == MAX_FIELDS) return -1;
    d->fields[d->num_fields++] = (field_t){at, len, NULL, 0};
    return 0;
}

static int on_value(llhttp_t *parser, const char *at, size_t len) {
    description_t *d = parser->data;
    field_t *last = &d->fields[d->num_fields - 1];
    if (last->value == NULL) last->value = at;
    last->value_len += len;
    return 0;
}

/* Closes a field whose value is empty (no on_header_value call), so the next on_field starts a new field. */
static int on_value_complete(llhttp_t *parser) {
    description_t *d = parser->data;
    field_t *last = &d->fields[d->num_fields - 1];
    if (last->value == NULL) last->value = "";
    return 0;
}

static const llhttp_settings_t describe_settings = {
    .on_header_field = on_field,
    .on_header_value = on_value,
    .on_header_value_complete = on_value_complete,
    .on_headers_complete = pause_at_head_end,
};

static void describe(const char *head, size_t len, description_t *out) {
    llhttp_t parser;
    llhttp_init(&parser, HTTP_RESPONSE, &describe_settings);
    out->num_fields = 0;
    parser.data = out;
    out->head_len = execute(&parser, head, len);
    if (out->head_len == 0) die("llhttp did not complete the head");
    out->status = llhttp_get_status_code(&parser);
    out->content_length = (parser.flags & F_CONTENT_LENGTH) ? (int64_t)parser.content_length : -1;
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

const engine_t llhttp_engine = {"llhttp", parse, describe, feed};
