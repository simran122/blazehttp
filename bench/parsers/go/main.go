// Go harness of the cross-language response-head parser bench: net/http.ReadResponse.
// Command line and output protocol: bench/parsers/README.md ("Harness protocol").
//
// ReadResponse is the head parser Go's own client uses (Transport keeps one bufio.Reader per connection, reset
// between responses, as done here). It builds a *http.Response with a canonicalised Header map and derives the
// framing facts (ContentLength, TransferEncoding, Close); the body reader is created but never read.
package main

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"fmt"
	"maps"
	"net/http"
	"os"
	"runtime"
	"slices"
	"strconv"
	"testing/iotest"
	"time"
)

const (
	warmUp      = 100 * time.Millisecond
	block       = 1000
	allocSample = 10000
)

// parser reuses one bufio.Reader and one bytes.Reader across heads, as a keep-alive connection does.
type parser struct {
	src bytes.Reader
	br  *bufio.Reader
}

func newParser() *parser {
	p := &parser{}
	p.br = bufio.NewReader(&p.src)
	return p
}

func (p *parser) read(head []byte) (*http.Response, int) {
	p.src.Reset(head)
	p.br.Reset(&p.src)
	resp, err := http.ReadResponse(p.br, nil)
	if err != nil {
		fail("ReadResponse: " + err.Error())
	}
	return resp, len(head) - p.br.Buffered()
}

// parse is the timed path: status, head length and framing facts folded into one number.
func (p *parser) parse(head []byte) uint64 {
	resp, headLen := p.read(head)
	length := uint64(0)
	if len(resp.TransferEncoding) > 0 {
		length = 1 << 40
	} else if resp.ContentLength >= 0 {
		length = uint64(resp.ContentLength)
	}
	keepAlive := uint64(0)
	if !resp.Close {
		keepAlive = 1
	}
	return uint64(resp.StatusCode) + uint64(headLen) + length + keepAlive
}

func fail(msg string) {
	fmt.Fprintln(os.Stderr, "error: "+msg)
	os.Exit(1)
}

// splitHeads splits a workload file into heads: each head ends at its first CRLF CRLF.
func splitHeads(b []byte) [][]byte {
	var heads [][]byte
	for len(b) > 0 {
		end := bytes.Index(b, []byte("\r\n\r\n"))
		if end < 0 {
			fail("trailing bytes after the last head")
		}
		heads = append(heads, b[:end+4])
		b = b[end+4:]
	}
	return heads
}

func check(heads [][]byte) {
	p := newParser()
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	for _, head := range heads {
		resp, headLen := p.read(head)
		length := "-"
		if resp.ContentLength >= 0 {
			length = strconv.FormatInt(resp.ContentLength, 10)
		}
		// The Header map has no order: names are printed sorted, values in arrival order per name.
		names := slices.Sorted(maps.Keys(resp.Header))
		count := 0
		for _, values := range resp.Header {
			count += len(values)
		}
		fmt.Fprintf(out, "head %d %d %s %d\n", resp.StatusCode, headLen, length, count)
		for _, name := range names {
			for _, value := range resp.Header[name] {
				fmt.Fprintf(out, "field %s %s\n", hex.EncodeToString([]byte(name)), hex.EncodeToString([]byte(value)))
			}
		}
	}
	var before, after runtime.MemStats
	runtime.ReadMemStats(&before)
	for i := range allocSample {
		p.parse(heads[i%len(heads)])
	}
	runtime.ReadMemStats(&after)
	fmt.Fprintf(out, "allocs %g\n", float64(after.Mallocs-before.Mallocs)/allocSample)
}

func runBlock(p *parser, heads [][]byte, next *int) uint64 {
	sink := uint64(0)
	for range block {
		sink += p.parse(heads[*next])
		*next = (*next + 1) % len(heads)
	}
	return sink
}

func timeHeads(heads [][]byte, run time.Duration) {
	p := newParser()
	next, sink := 0, uint64(0)
	for warm := time.Now(); time.Since(warm) < warmUp; {
		sink += runBlock(p, heads, &next)
	}
	parsed := 0
	start := time.Now()
	for time.Since(start) < run {
		sink += runBlock(p, heads, &next)
		parsed += block
	}
	fmt.Printf("ns %.4f %d\n", float64(time.Since(start).Nanoseconds())/float64(parsed), sink)
}

// feed delivers the head one byte per Read: ReadResponse pulls lines through bufio and resumes where it stopped.
func feed(heads [][]byte) {
	if len(heads) != 1 {
		fail("a feed workload holds exactly one head")
	}
	input := heads[0]
	start := time.Now()
	br := bufio.NewReader(iotest.OneByteReader(bytes.NewReader(input)))
	if _, err := http.ReadResponse(br, nil); err != nil {
		fail("ReadResponse: " + err.Error())
	}
	ns := time.Since(start).Nanoseconds()
	fmt.Printf("feed %d incremental %d\n", ns, len(input)-br.Buffered())
}

func main() {
	if len(os.Args) < 4 || os.Args[1] != "go-net-http" {
		fail("usage: go-net-http <check|time|feed> <workload file> [run ms]")
	}
	b, err := os.ReadFile(os.Args[3])
	if err != nil {
		fail(err.Error())
	}
	heads := splitHeads(b)
	modes := map[string]func(){
		"check": func() { check(heads) },
		"time": func() {
			ms := 300
			if len(os.Args) > 4 {
				if ms, err = strconv.Atoi(os.Args[4]); err != nil {
					fail("run ms: " + err.Error())
				}
			}
			timeHeads(heads, time.Duration(ms)*time.Millisecond)
		},
		"feed": func() { feed(heads) },
	}
	mode, ok := modes[os.Args[2]]
	if !ok {
		fail("unknown mode " + os.Args[2])
	}
	mode()
}
