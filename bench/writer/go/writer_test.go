// Comparison benchmark only (docs/notes/request-writer.md §8.4): Go net/http (*Request).Write on the same `get` and
// `post1k` requests as crates/engine-core/tests/support/write_rings.rs, into a reused 8 KiB bufio.Writer over
// io.Discard with one Flush per request (as Go's Transport writeLoop does). The body is one reusable reader reset per
// op, so every allocation reported is Request.Write's own.
// Run: go test -run '^$' -bench . -benchmem -count 5 (in golang:1.27.3-bookworm, see ../README.md).
package writer

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

const (
	variants    = 64
	postBodyLen = 1024
)

func baseRequest(method, target string) *http.Request {
	return &http.Request{
		Method:     method,
		URL:        &url.URL{Scheme: "http", Host: "127.0.0.1:3999", Opaque: target},
		Host:       "127.0.0.1:3999",
		Proto:      "HTTP/1.1",
		ProtoMajor: 1,
		ProtoMinor: 1,
		Header: http.Header{
			"Accept":          {"application/json, text/plain, */*"},
			"User-Agent":      {"blazehttp/0.0.0"},
			"Accept-Encoding": {"gzip, br"},
		},
	}
}

func getRing() []*http.Request {
	ring := make([]*http.Request, variants)
	for i := range ring {
		ring[i] = baseRequest("GET", fmt.Sprintf("/data/%02d", i))
	}
	return ring
}

// post1kBody mirrors write_rings::json_body: a valid 1 KiB JSON object, an array of items padded with whitespace.
func post1kBody(i int) []byte {
	item := fmt.Sprintf(`{"id":%d,"name":"item-%d","tags":["a","b"],"v":1.5}`, i, i)
	items := make([]string, (postBodyLen-16)/(len(item)+1))
	for j := range items {
		items[j] = item
	}
	body := []byte(`{"items":[` + strings.Join(items, ","))
	for len(body) < postBodyLen-2 {
		body = append(body, ' ')
	}
	return append(body, ']', '}')
}

func post1kRing() ([]*http.Request, [][]byte) {
	ring := make([]*http.Request, variants)
	bodies := make([][]byte, variants)
	for i := range ring {
		r := baseRequest("POST", "/api/items")
		r.Header.Set("Content-Type", "application/json")
		r.Header.Set("X-Request-Id", fmt.Sprintf("7f3c9a2e-%04d-4b8d-9e1f-2a6c8d0b4e%02d", i, i))
		r.Header.Set("Authorization", fmt.Sprintf("Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJiZW5jaCJ9.c2lnbmF0dXJl%02d", i))
		bodies[i] = post1kBody(i)
		r.ContentLength = int64(len(bodies[i]))
		ring[i] = r
	}
	return ring, bodies
}

// rewindBody is a request body that is reset in place, so re-arming it per op allocates nothing.
type rewindBody struct{ bytes.Reader }

func (*rewindBody) Close() error { return nil }

func run(b *testing.B, ring []*http.Request, bodies [][]byte) {
	w := bufio.NewWriterSize(io.Discard, 8192)
	body := &rewindBody{}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		r := ring[i%len(ring)]
		if bodies != nil {
			body.Reset(bodies[i%len(bodies)])
			r.Body = body
		}
		if err := r.Write(w); err != nil {
			b.Fatal(err)
		}
		if err := w.Flush(); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkGet(b *testing.B) {
	run(b, getRing(), nil)
}

func BenchmarkPost1k(b *testing.B) {
	ring, bodies := post1kRing()
	run(b, ring, bodies)
}
