/* One parser adapter of the C harness (bench/parsers/README.md, "Harness protocol"). */
#ifndef PARSERS_BENCH_ENGINE_H
#define PARSERS_BENCH_ENGINE_H

#include <stddef.h>
#include <stdint.h>

/* Covers the 99-field feed input (b). */
#define MAX_FIELDS 100

typedef struct {
    const char *name;
    size_t name_len;
    const char *value;
    size_t value_len;
} field_t;

/* What `check` prints for one head; content_length is -1 when absent. */
typedef struct {
    int status;
    size_t head_len;
    int64_t content_length;
    size_t num_fields;
    field_t fields[MAX_FIELDS];
} description_t;

typedef struct {
    const char *name;
    /* Timed path: one complete head in, status + head length + framing facts folded into one number. */
    uint64_t (*parse)(const char *head, size_t len);
    void (*describe)(const char *head, size_t len, description_t *out);
    /* Feeds `input` one byte per call; returns the partial-input mode and stores the head length at completion. */
    const char *(*feed)(const char *input, size_t len, size_t *head_len);
} engine_t;

extern const engine_t pico_engine;
extern const engine_t llhttp_engine;

/* Aborts the process with a message: every head in a workload is valid, so any failure is a harness bug. */
_Noreturn void die(const char *what);

#endif
