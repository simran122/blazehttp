package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
)

type item struct {
	I      int    `json:"i"`
	Label  string `json:"label"`
	Active bool   `json:"active"`
}

func main() {
	items := make([]item, 20)
	for i := range items {
		items[i] = item{I: i, Label: fmt.Sprintf("item-%d", i), Active: i%2 == 0}
	}
	body, _ := json.Marshal(map[string]any{"id": 42, "name": "campaign", "items": items})
	length := strconv.Itoa(len(body))

	http.HandleFunc("/data", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "application/json")
		w.Header().Set("content-length", length)
		w.Write(body)
	})
	fmt.Println("ready")
	http.ListenAndServe("127.0.0.1:3999", nil)
}
