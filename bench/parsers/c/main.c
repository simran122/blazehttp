/* C harness of the cross-language response-head parser bench: picohttpparser and llhttp.
 * Command line and output protocol: bench/parsers/README.md ("Harness protocol"). */
#define _POSIX_C_SOURCE 200809L /* clock_gettime */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "engine.h"

#define ALLOC_SAMPLE 10000
#define MAX_HEADS 256
/* One hour: keeps ms * 1e6 far below 2^64. */
#define MAX_RUN_MS 3600000ull

/* Allocation counter: the build links with -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc, so every call made by
 * the harness and both parsers lands here. */
static unsigned long long allocations;
void *__real_malloc(size_t size);
void *__real_calloc(size_t n, size_t size);
void *__real_realloc(void *p, size_t size);
void *__wrap_malloc(size_t size) {
    allocations++;
    return __real_malloc(size);
}
void *__wrap_calloc(size_t n, size_t size) {
    allocations++;
    return __real_calloc(n, size);
}
void *__wrap_realloc(void *p, size_t size) {
    allocations++;
    return __real_realloc(p, size);
}

_Noreturn void die(const char *what) {
    fprintf(stderr, "error: %s\n", what);
    exit(1);
}

unsigned long long now_ns(void) {
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
    if (count == 0 || start != len) die("a workload is one or more heads, each ending at its first CRLF CRLF");
    return count;
}

static void print_hex(const char *bytes, size_t len) {
    for (size_t i = 0; i < len; i++) printf("%02x", (unsigned char)bytes[i]);
}

static void check(const engine_t *engine, const head_t *heads, size_t count) {
    static field_list_t list;
    for (size_t h = 0; h < count; h++) {
        facts_t f = engine->parse(heads[h].at, heads[h].len);
        engine->fields(heads[h].at, heads[h].len, &list);
        printf("head %d %zu ", f.status, f.head_len);
        if (f.length_kind == LENGTH_BYTES) printf("%llu ", (unsigned long long)f.length);
        else printf("%s ", f.length_kind == LENGTH_CHUNKED ? "chunked" : "-");
        printf("%d %zu\n", f.keep_alive ? 1 : 0, list.count);
        for (size_t i = 0; i < list.count; i++) {
            printf("field ");
            print_hex(list.fields[i].name, list.fields[i].name_len);
            printf(" ");
            print_hex(list.fields[i].value, list.fields[i].value_len);
            printf("\n");
        }
    }
    unsigned long long before = allocations;
    volatile uint64_t sink = 0;
    for (size_t i = 0; i < ALLOC_SAMPLE; i++) sink += fold(engine->parse(heads[i % count].at, heads[i % count].len));
    printf("allocs %g\n", (double)(allocations - before) / ALLOC_SAMPLE);
}

static void feed(const engine_t *engine, const head_t *heads, size_t count) {
    if (count != 1) die("a feed workload holds exactly one head");
    size_t head_len = 0;
    unsigned long long start = now_ns();
    const char *mode = engine->feed(heads[0].at, heads[0].len, &head_len);
    unsigned long long ns = now_ns() - start;
    printf("feed %llu %s %zu\n", ns, mode, head_len);
}

static unsigned long long run_ms(int argc, char **argv) {
    if (argc < 5) die("time needs a run time in ms from 1 to 3600000");
    char *end = NULL;
    errno = 0;
    unsigned long long ms = strtoull(argv[4], &end, 10);
    if (errno != 0 || end == argv[4] || *end != '\0' || ms == 0 || ms > MAX_RUN_MS || argv[4][0] == '-') {
        die("time needs a run time in ms from 1 to 3600000");
    }
    return ms;
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
    if (strcmp(argv[2], "check") == 0) {
        check(engine, heads, count);
    } else if (strcmp(argv[2], "time") == 0) {
        unsigned long long ms = run_ms(argc, argv);
        uint64_t sink = 0;
        double ns = engine->time(heads, count, ms * 1000000ull, &sink);
        printf("ns %.4f %llu\n", ns, (unsigned long long)sink);
    } else if (strcmp(argv[2], "feed") == 0) {
        feed(engine, heads, count);
    } else {
        die("unknown mode");
    }
    free(bytes);
    return 0;
}
