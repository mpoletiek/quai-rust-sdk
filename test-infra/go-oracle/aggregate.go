// SPDX-License-Identifier: MIT
package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"os"

	"github.com/btcsuite/btcd/btcec/v2"
	"github.com/btcsuite/btcd/btcec/v2/schnorr"
	"github.com/btcsuite/btcd/btcec/v2/schnorr/musig2"
)

type aggregateVector struct {
	ID            string   `json:"id"`
	PublicKeys    []string `json:"publicKeys"`
	Aggregate     string   `json:"aggregatePublicKey"`
	Digest        string   `json:"digest"`
	JSSignature   string   `json:"jsSignature"`
	RustSignature string   `json:"rustSignature"`
}
type aggregateResult struct {
	ID                    string `json:"id"`
	OrderedPublicKeyMatch bool   `json:"orderedPublicKeyMatch"`
	JSSignatureVerified   bool   `json:"jsSignatureVerified"`
	RustSignatureVerified bool   `json:"rustSignatureVerified"`
	Error                 string `json:"error,omitempty"`
}

func verifyAggregate(v aggregateVector) (r aggregateResult) {
	r.ID = v.ID
	if len(v.PublicKeys) < 2 || len(v.PublicKeys) > 1024 {
		r.Error = "invalid bounded key count"
		return
	}
	keys := make([]*btcec.PublicKey, len(v.PublicKeys))
	for i, value := range v.PublicKeys {
		b, err := hexBytes(value, 33)
		if err != nil || len(b) != 33 {
			r.Error = "invalid public key bytes"
			return
		}
		keys[i], err = btcec.ParsePubKey(b)
		if err != nil {
			r.Error = "invalid public key"
			return
		}
	}
	aggregate, _, _, err := musig2.AggregateKeys(keys, false)
	if err != nil {
		r.Error = "Go ordered aggregation failed"
		return
	}
	expected, err := hexBytes(v.Aggregate, 33)
	if err != nil || len(expected) != 33 {
		r.Error = "invalid expected aggregate"
		return
	}
	r.OrderedPublicKeyMatch = bytes.Equal(aggregate.FinalKey.SerializeCompressed(), expected)
	digest, err := hexBytes(v.Digest, 32)
	if err != nil || len(digest) != 32 {
		r.Error = "invalid digest"
		return
	}
	verify := func(value string) bool {
		b, err := hexBytes(value, 64)
		if err != nil || len(b) != 64 {
			return false
		}
		sig, err := schnorr.ParseSignature(b)
		return err == nil && sig.Verify(digest, aggregate.FinalKey)
	}
	r.JSSignatureVerified = verify(v.JSSignature)
	r.RustSignatureVerified = verify(v.RustSignature)
	return
}
func aggregatePassed(r aggregateResult) bool {
	return r.Error == "" && r.OrderedPublicKeyMatch && r.JSSignatureVerified && r.RustSignatureVerified
}
func verifyAggregates(path string) ([]aggregateResult, string, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, "", errors.New("cannot open aggregate fixture")
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, maxFixtureBytes+1))
	if err != nil || len(data) > maxFixtureBytes {
		return nil, "", errors.New("cannot read bounded aggregate fixture")
	}
	var fixture struct {
		SchemaVersion int               `json:"schemaVersion"`
		Vectors       []aggregateVector `json:"vectors"`
	}
	if json.Unmarshal(data, &fixture) != nil || fixture.SchemaVersion != 1 || len(fixture.Vectors) == 0 {
		return nil, "", errors.New("invalid aggregate fixture")
	}
	results := make([]aggregateResult, 0, len(fixture.Vectors))
	seen := make(map[string]bool)
	for _, v := range fixture.Vectors {
		if v.ID == "" || seen[v.ID] {
			return nil, "", errors.New("aggregate IDs must be unique/nonempty")
		}
		seen[v.ID] = true
		results = append(results, verifyAggregate(v))
	}
	hash := sha256.Sum256(data)
	return results, hex.EncodeToString(hash[:]), nil
}
