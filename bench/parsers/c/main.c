/* C harness of the cross-language response-head parser bench: picohttpparser and llhttp.
 * Command line and output protocol: bench/parsers/README.md ("Harness protocol"). */
#define _POSIX_C_SOURCE 200809L /* clock_gettime */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "engine.h"

#define WARM_UP_NS 100000000ull
#define BLOCK 1000
#define ALLOC_SAMPLE 10000
#define MAX_HEADS 256

typedef struct {
    const char *at;
    size_t len;
} head_t;

/* Allocation counter: the build links with -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc, so every call made by
 * the harness and both parsers lands here. */
static unsigned long long allocations;
void *__real_malloc(size_t size);
void *__real_calloc(size_t n, size_t size);
void *__real_realloc(void *p, size_t size);
void *__wrap_malloc(size_t size) { allocations++; return __real_malloc(size); }
void *__wrap_calloc(size_t n, size_t size) { allocations++; return __real_calloc(n, size); }
void *__wrap_realloc(void *p, size_t size) { allocations++; return __real_realloc(p, size); }

_Noreturn void die(const char *what) {
    fprintf(stderr, "error: %s\n", what);
    exit(1);
}

static unsigned long long now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (unsigned long long)t.tv_sec * 1000000000ull + (unsigned long long)t.tv_nsec;
}

static char *read_file(const char *path, size_t *len) {
    FILE *f = fopen(path, "rb");
    if (f == NULL) die("cannot open workload file");
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *bytes = malloc((size_t)size);
    if (bytes == NULL || fread(bytes, 1, (size_t)size, f) != (size_t)size) die("cannot read workload file");
    fclose(f);
    *len = (size_t)size;
    return bytes;
}

/* Each head ends at its first CRLF CRLF. */
static size_t split_heads(const char *bytes, size_t len, head_t *heads) {
    size_t count = 0, start = 0;
    for (size_t i = 0; i + 4 <= len; i++) {
        if (memcmp(bytes + i, "\r\n\r\n", 4) == 0) {
            if (count == MAX_HEADS) die("too many heads");
            heads[count++] = (head_t){bytes + start, i + 4 - start};
            start = i + 4;
            i += 3;
        }
    }
    if (start != len) die("trailing bytes after the last head");
    return count;
}

static void print_hex(const char *bytes, size_t len) {
    for (size_t i = 0; i < len; i++) printf("%02x", (unsigned char)bytes[i]);
}

static void check(const engine_t *engine, const head_t *heads, size_t count) {
    static description_t d;
    for (size_t h = 0; h < count; h++) {
        engine->describe(heads[h].at, heads[h].len, &d);
        printf("head %d %zu ", d.status, d.head_len);
        if (d.content_length < 0) printf("- "); else printf("%lld ", (long long)d.content_length);
        printf("%zu\n", d.num_fields);
        for (size_t i = 0; i < d.num_fields; i++) {
            printf("field ");
            print_hex(d.fields[i].name, d.fields[i].name_len);
            printf(" ");
            print_hex(d.fields[i].value, d.fields[i].value_len);
            printf("\n");
        }
    }
    unsigned long long before = allocations;
    volatile uint64_t sink = 0;
    for (size_t i = 0; i < ALLOC_SAMPLE; i++) sink += engine->parse(heads[i % count].at, heads[i % count].len);
    printf("allocs %g\n", (double)(allocations - before) / ALLOC_SAMPLE);
}

static uint64_t block(const engine_t *engine, const head_t *heads, size_t count, size_t *next) {
    uint64_t sink = 0;
    for (int i = 0; i < BLOCK; i++) {
        sink += engine->parse(heads[*next].at, heads[*next].len);
        *next = (*next + 1) % count;
    }
    return sink;
}

static void time_engine(const engine_t *engine, const head_t *heads, size_t count, unsigned long long run_ns) {
    size_t next = 0;
    uint64_t sink = 0;
    unsigned long long warm = now_ns();
    while (now_ns() - warm < WARM_UP_NS) sink += block(engine, heads, count, &next);
    unsigned long long parsed = 0, start = now_ns(), elapsed = 0;
    while ((elapsed = now_ns() - start) < run_ns) {
        sink += block(engine, heads, count, &next);
        parsed += BLOCK;
    }
    elapsed = now_ns() - start;
    printf("ns %.4f %llu\n", (double)elapsed / (double)parsed, (unsigned long long)sink);
}

static void feed(const engine_t *engine, const head_t *heads, size_t count) {
    if (count != 1) die("a feed workload holds exactly one head");
    size_t head_len = 0;
    unsigned long long start = now_ns();
    const char *mode = engine->feed(heads[0].at, heads[0].len, &head_len);
    unsigned long long ns = now_ns() - start;
    printf("feed %llu %s %zu\n", ns, mode, head_len);
}

int main(int argc, char **argv) {
    if (argc < 4) die("usage: <picohttpparser|llhttp> <check|time|feed> <workload file> [run ms]");
    const engine_t *engines[] = {&pico_engine, &llhttp_engine};
    const engine_t *engine = NULL;
    for (size_t i = 0; i < sizeof engines / sizeof engines[0]; i++) {
        if (strcmp(argv[1], engines[i]->name) == 0) engine = engines[i];
    }
    if (engine == NULL) die("unknown parser");
    size_t len = 0;
    char *bytes = read_file(argv[3], &len);
    static head_t heads[MAX_HEADS];
    size_t count = split_heads(bytes, len, heads);
    if (count == 0) die("empty workload");
    if (strcmp(argv[2], "check") == 0) {
        check(engine, heads, count);
    } else if (strcmp(argv[2], "time") == 0) {
        unsigned long long ms = argc > 4 ? strtoull(argv[4], NULL, 10) : 300;
        time_engine(engine, heads, count, ms * 1000000ull);
    } else if (strcmp(argv[2], "feed") == 0) {
        feed(engine, heads, count);
    } else {
        die("unknown mode");
    }
    free(bytes);
    return 0;
}
