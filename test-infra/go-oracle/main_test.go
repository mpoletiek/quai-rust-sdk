// SPDX-License-Identifier: MIT
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"
)

func fixturePath(t *testing.T) string {
	t.Helper()
	path := os.Getenv("QUAI_ORACLE_FIXTURES")
	if path == "" {
		t.Fatal("run tests through run.py --test, which sets QUAI_ORACLE_FIXTURES")
	}
	return path
}

func TestPinnedFixtures(t *testing.T) {
	var output bytes.Buffer
	ok, err := run(fixturePath(t), &output)
	if err != nil || !ok {
		t.Fatalf("fixture verification failed: %v\n%s", err, output.String())
	}
	var report report
	if err := json.Unmarshal(output.Bytes(), &report); err != nil {
		t.Fatal(err)
	}
	if len(report.Results) < 11 || report.NodeAcceptance {
		t.Fatal("unexpected fixture count/acceptance claim")
	}
	policyFailures := 0
	for _, result := range report.Results {
		if result.KnownQiDataLengthPolicyFailure {
			policyFailures++
		}
	}
	if policyFailures != 2 {
		t.Fatalf("expected two known Qi policy failures, got %d", policyFailures)
	}
}

func TestAlteredExpectationsFail(t *testing.T) {
	data, err := os.ReadFile(fixturePath(t))
	if err != nil {
		t.Fatal(err)
	}
	var fixture fixture
	if json.Unmarshal(data, &fixture) != nil {
		t.Fatal("fixture decode")
	}
	for _, original := range fixture.Vectors {
		altered := original
		altered.Hash = "0x" + string(bytes.Repeat([]byte("00"), 32))
		if passed(verify(altered)) {
			t.Fatalf("accepted wrong transaction hash for %s", altered.ID)
		}
		altered = original
		altered.Digest = "0x" + string(bytes.Repeat([]byte("00"), 32))
		if passed(verify(altered)) {
			t.Fatalf("accepted wrong signing digest for %s", altered.ID)
		}
		altered = original
		changed, err := hexBytes(altered.Signed, maxWireBytes)
		if err != nil {
			t.Fatal(err)
		}
		changed[len(changed)-1] ^= 1
		altered.Signed = "0x" + hex.EncodeToString(changed)
		if verify(altered).SignatureVerified {
			t.Fatalf("accepted tampered signature for %s", altered.ID)
		}
		altered = original
		altered.Signed = "0x0800"
		if passed(verify(altered)) {
			t.Fatalf("accepted truncated signed bytes for %s", altered.ID)
		}
	}
}

func TestBoundedHexInput(t *testing.T) {
	for _, value := range []string{"", "0X00", "0x0", "0xzz", "0x0000"} {
		if _, err := hexBytes(value, 1); err == nil {
			t.Fatalf("accepted %q", value)
		}
	}
	if got, err := hexBytes("0x00", 1); err != nil || len(got) != 1 {
		t.Fatal("rejected valid byte")
	}
}
