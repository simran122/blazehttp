/* One parser adapter of the C harness (bench/parsers/README.md, "Harness protocol"). */
#ifndef PARSERS_BENCH_ENGINE_H
#define PARSERS_BENCH_ENGINE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Covers the 99-field feed input (b); the same slot count as the Rust harness. */
#define MAX_FIELDS 100
#define WARM_UP_NS 100000000ull
#define BLOCK 1000

typedef struct {
    const char *at;
    size_t len;
} head_t;

typedef struct {
    const char *name;
    size_t name_len;
    const char *value;
    size_t value_len;
} field_t;

typedef struct {
    size_t count;
    field_t fields[MAX_FIELDS];
} field_list_t;

typedef enum { LENGTH_UNKNOWN, LENGTH_BYTES, LENGTH_CHUNKED } length_kind_t;

/* What the timed path produces for one head (printed by `check` as the head line). */
typedef struct {
    int status;
    size_t head_len;
    length_kind_t length_kind;
    uint64_t length;
    bool keep_alive;
} facts_t;

typedef struct {
    const char *name;
    /* The timed path: one complete head in, the facts out. */
    facts_t (*parse)(const char *head, size_t len);
    /* Times `parse` over `heads` (see ENGINE_TIMER); returns ns per head. */
    double (*time)(const head_t *heads, size_t count, unsigned long long run_ns, uint64_t *sink);
    /* The gate's view of the fields, outside the timed path. */
    void (*fields)(const char *head, size_t len, field_list_t *out);
    /* Feeds `input` one byte per call; returns the partial-input mode and stores the head length at completion. */
    const char *(*feed)(const char *input, size_t len, size_t *head_len);
} engine_t;

extern const engine_t pico_engine;
extern const engine_t llhttp_engine;

/* Aborts the process with a message: every head in a workload is valid, so any failure is a harness bug. */
_Noreturn void die(const char *what);
unsigned long long now_ns(void);

/* Folds the facts into one number so the timed work cannot be optimised away. */
static inline uint64_t fold(facts_t f) {
    uint64_t length = f.length_kind == LENGTH_CHUNKED ? 1ull << 40 : f.length;
    return (uint64_t)f.status + f.head_len + length + (uint64_t)f.keep_alive;
}

/* Defines `name`, a timing loop that calls `parse_fn` directly (no indirect call per head): 100 ms warm-up, then
 * blocks of BLOCK parses cycling through `heads` until `run_ns` has elapsed. */
#define ENGINE_TIMER(name, parse_fn)                                                                    \
    static double name(const head_t *heads, size_t count, unsigned long long run_ns, uint64_t *sink) {  \
        size_t next = 0;                                                                                \
        unsigned long long parsed = 0;                                                                  \
        unsigned long long warm = now_ns();                                                             \
        unsigned long long start = 0;                                                                   \
        for (int timed = 0; timed < 2; timed++) {                                                       \
            unsigned long long since = timed ? (start = now_ns()) : warm;                               \
            unsigned long long limit = timed ? run_ns : WARM_UP_NS;                                     \
            while (now_ns() - since < limit) {                                                          \
                for (int i = 0; i < BLOCK; i++) {                                                       \
                    *sink += fold(parse_fn(heads[next].at, heads[next].len));                           \
                    next = next + 1 == count ? 0 : next + 1;                                            \
                }                                                                                       \
                if (timed) parsed += BLOCK;                                                             \
            }                                                                                           \
        }                                                                                               \
        return (double)(now_ns() - start) / (double)parsed;                                             \
    }

#endif
