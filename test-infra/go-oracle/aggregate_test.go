// SPDX-License-Identifier: MIT
package main

import (
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"
)

func TestOrderedAggregateSignatures(t *testing.T) {
	path := os.Getenv("QUAI_AGGREGATE_FIXTURES")
	if path == "" {
		t.Skip("pass --aggregates with generated Rust signatures")
	}
	results, _, err := verifyAggregates(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(results) != 11 {
		t.Fatal("expected eleven ordered/duplicate vectors")
	}
	for _, r := range results {
		if !aggregatePassed(r) {
			t.Fatalf("aggregate failure: %+v", r)
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var input struct {
		Vectors []aggregateVector `json:"vectors"`
	}
	if json.Unmarshal(data, &input) != nil {
		t.Fatal("json")
	}
	for _, v := range input.Vectors {
		raw, err := hexBytes(v.RustSignature, 64)
		if err != nil {
			t.Fatal(err)
		}
		raw[63] ^= 1
		v.RustSignature = "0x" + hex.EncodeToString(raw)
		if verifyAggregate(v).RustSignatureVerified {
			t.Fatal("accepted tampered signature")
		}
	}
}
