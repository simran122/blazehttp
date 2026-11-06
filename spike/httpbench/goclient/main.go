package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sort"
	"sync"
	"sync/atomic"
	"time"
)

const (
	url         = "http://127.0.0.1:3999/data"
	total       = 30000
	concurrency = 50
	warmup      = 3000
)

type payload struct {
	ID    int              `json:"id"`
	Name  string           `json:"name"`
	Items []map[string]any `json:"items"`
}

func run(client *http.Client, n int) (int, float64, float64) {
	lat := make([]float64, n)
	var next int64 = -1
	var wg sync.WaitGroup
	start := time.Now()
	for w := 0; w < concurrency; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for {
				i := atomic.AddInt64(&next, 1)
				if i >= int64(n) {
					return
				}
				t := time.Now()
				resp, err := client.Get(url)
				if err != nil {
					panic(err)
				}
				b, _ := io.ReadAll(resp.Body)
				resp.Body.Close()
				var p payload
				if err := json.Unmarshal(b, &p); err != nil || p.ID != 42 {
					panic("bad body")
				}
				lat[i] = float64(time.Since(t).Microseconds()) / 1000
			}
		}()
	}
	wg.Wait()
	elapsed := time.Since(start).Seconds()
	sort.Float64s(lat)
	return int(float64(n) / elapsed), lat[n/2], lat[n*99/100]
}

func main() {
	tr := &http.Transport{MaxIdleConns: concurrency, MaxIdleConnsPerHost: concurrency, MaxConnsPerHost: concurrency}
	client := &http.Client{Transport: tr}
	run(client, warmup)
	rps, p50, p99 := run(client, total)
	fmt.Printf("{\"name\":\"Go net/http\",\"rps\":%d,\"p50\":\"%.2f\",\"p99\":\"%.2f\"}\n", rps, p50, p99)
}
