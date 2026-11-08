# HTTP/1.1 parser and writer speed: research note (input to Tasks 4c, 5b and P1a)

Design §5.2 (strict rules, unchanged), §6 (learn from proven code, never copy), §7 (speed programme), §13 (code
structure and patterns), §16 (gates). Builds on `docs/notes/response-parser.md` (§8.4 budget, §8.6 cross-language
race), `docs/notes/simd-scanning-research.md` (SIMD survey; not repeated here) and, on branch `p0/writer`,
`docs/notes/request-writer.md` and `docs/perf/journal.md`. Access date for every link: **2026-11-08**. **Verified** =
checked against the pinned source, the primary text of a paper, or an official doc page; **unverified** = everything
else (unverified ideas go only to `docs/perf/ideas.md`). This note is research only: no benchmark was run for it.

## 0. The problem in numbers

| Component | Gate (owner) | Today (dev, Linux arm64) | Gap |
|---|---|---|---|
| `ResponseParser` + per-field spans | faster than every parser with equal work | httparse 1.16–2.02× faster, picohttpparser 1.21–1.78× faster (parser note §8.6) | 1.2–2.0× |
| `write_request` | faster than hyper's encoder and Go's `Request.Write` with equal work; `post1k` ≤ 2.3× copy floor | `post1k` 2.51–2.58× floor; 1.16–1.22× hyper encode-only (journal 2026-11-08) | ≈ 10% on the floor ratio; ≈ 20% vs hyper encode-only |

Constraints that stay: the §5.2 checks, `#![forbid(unsafe_code)]` in `engine-core`, `unsafe` SIMD only in
`engine-simd`.

## 1. References (pinned)

_(filled in below)_

## 2. Parsers: why each one is fast

_(in progress)_

## 3. Writers and serialisers: why each one is fast

_(in progress)_

## 4. Papers and algorithms

_(in progress)_

## 5. Our parser: candidates

_(in progress)_

## 6. Our writer: candidates

_(in progress)_

## 7. Measurement plan

_(in progress)_

## 8. Decision table

_(in progress)_

## 9. Sources

_(in progress)_
