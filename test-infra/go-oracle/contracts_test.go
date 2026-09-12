// SPDX-License-Identifier: MIT
package main

import (
	"encoding/hex"
	"encoding/json"
	"github.com/dominant-strategies/go-quai/common"
	quaicrypto "github.com/dominant-strategies/go-quai/crypto"
	"os"
	"strconv"
	"strings"
	"testing"
)

func TestExactContractAddresses(t *testing.T) {
	path := os.Getenv("QUAI_CONTRACT_FIXTURES")
	if path == "" {
		t.Skip("optional exact-code contract fixtures not configured")
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var file struct {
		Contracts []struct{ Sender, Nonce, Code, Exact, Legacy, Salt, CodeHash, Create2 string }
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if len(file.Contracts) != 30 {
		t.Fatal("expected 30 contract fixtures")
	}
	corrected := 0
	for _, v := range file.Contracts {
		decode := func(s string) []byte {
			b, e := hex.DecodeString(strings.TrimPrefix(s, "0x"))
			if e != nil {
				t.Fatal(e)
			}
			return b
		}
		sender := common.BytesToAddress(decode(v.Sender), common.Location{0, 0})
		nonce, err := strconv.ParseUint(v.Nonce, 10, 64)
		if err != nil {
			t.Fatal(err)
		}
		actual := quaicrypto.CreateAddress(sender, nonce, decode(v.Code), common.Location{0, 0})
		if !strings.EqualFold(actual.Hex(), v.Exact) {
			t.Fatalf("CREATE mismatch for nonce %s code %s", v.Nonce, v.Code)
		}
		if !strings.EqualFold(v.Exact, v.Legacy) {
			corrected++
		}
		var salt [32]byte
		copy(salt[:], decode(v.Salt))
		actual2 := quaicrypto.CreateAddress2(sender, salt, decode(v.CodeHash), common.Location{0, 0})
		if !strings.EqualFold(actual2.Hex(), v.Create2) {
			t.Fatal("CREATE2 mismatch")
		}
	}
	if corrected != 18 {
		t.Fatal("expected 18 explicit JS leading-zero divergences")
	}
}
