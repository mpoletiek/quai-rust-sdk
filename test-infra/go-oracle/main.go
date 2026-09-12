// SPDX-License-Identifier: MIT
// This development oracle links the separately licensed pinned go-quai library.
package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"runtime"
	"strings"

	"github.com/btcsuite/btcd/btcec/v2"
	"github.com/btcsuite/btcd/btcec/v2/schnorr/musig2"
	"github.com/dominant-strategies/go-quai/common"
	"github.com/dominant-strategies/go-quai/core/types"
	"github.com/dominant-strategies/go-quai/params"
	"google.golang.org/protobuf/proto"
)

const pinnedCommit = "f3f345c877300c044e3e0081a48bf3cf786fb9cc"
const maxFixtureBytes = 16 << 20
const maxWireBytes = 1 << 20

type fixture struct {
	SchemaVersion int      `json:"schemaVersion"`
	Reference     string   `json:"reference"`
	Vectors       []vector `json:"vectors"`
}
type vector struct {
	ID       string `json:"id"`
	Kind     string `json:"kind"`
	Unsigned string `json:"unsigned"`
	Digest   string `json:"digest"`
	Signed   string `json:"signed"`
	Hash     string `json:"hash"`
	Input    struct {
		From string `json:"from"`
	} `json:"input"`
}
type result struct {
	ID                             string `json:"id"`
	Kind                           string `json:"kind"`
	GoProtoDecode                  bool   `json:"goProtoDecode"`
	SigningBytesMatch              bool   `json:"signingBytesMatch"`
	SigningDigestMatch             bool   `json:"signingDigestMatch"`
	SignedBytesMatch               bool   `json:"signedBytesMatch"`
	TransactionHashMatch           bool   `json:"transactionHashMatch"`
	SignatureVerified              bool   `json:"signatureVerified"`
	SenderMatch                    *bool  `json:"senderMatch,omitempty"`
	KnownQiDataLengthPolicyFailure bool   `json:"knownQiDataLengthPolicyFailure,omitempty"`
	Note                           string `json:"note,omitempty"`
	Error                          string `json:"error,omitempty"`
}
type report struct {
	SchemaVersion          int               `json:"schemaVersion"`
	Oracle                 string            `json:"oracle"`
	PinnedGoCommit         string            `json:"pinnedGoCommit"`
	GoVersion              string            `json:"goVersion"`
	FixtureReference       string            `json:"fixtureReference"`
	FixtureSHA256          string            `json:"fixtureSha256"`
	NodeAcceptance         bool              `json:"nodeAcceptance"`
	Results                []result          `json:"results"`
	Aggregates             []aggregateResult `json:"aggregates,omitempty"`
	AggregateFixtureSHA256 string            `json:"aggregateFixtureSha256,omitempty"`
}

func hexBytes(value string, limit int) ([]byte, error) {
	if !strings.HasPrefix(value, "0x") || len(value) < 2 || (len(value)-2)%2 != 0 || (len(value)-2)/2 > limit {
		return nil, errors.New("invalid bounded hex fixture field")
	}
	b, err := hex.DecodeString(value[2:])
	if err != nil {
		return nil, errors.New("invalid hex fixture field")
	}
	return b, nil
}

func verify(v vector) (r result) {
	r.ID, r.Kind = v.ID, v.Kind
	// Fixture data is public, but prevent a backend panic from hiding later cases.
	defer func() {
		if recover() != nil {
			r.Error = "pinned Go implementation panicked while processing fixture"
		}
	}()
	if v.Kind != "quai" && v.Kind != "qi" {
		r.Error = "unsupported fixture kind"
		return
	}
	signed, err := hexBytes(v.Signed, maxWireBytes)
	if err != nil {
		r.Error = "invalid signed bytes"
		return
	}
	unsigned, err := hexBytes(v.Unsigned, maxWireBytes)
	if err != nil {
		r.Error = "invalid unsigned bytes"
		return
	}
	expectedDigest, err := hexBytes(v.Digest, 32)
	if err != nil || len(expectedDigest) != 32 {
		r.Error = "invalid digest"
		return
	}
	expectedHash, err := hexBytes(v.Hash, 32)
	if err != nil || len(expectedHash) != 32 {
		r.Error = "invalid transaction hash"
		return
	}
	var envelope types.ProtoTransaction
	if proto.Unmarshal(signed, &envelope) != nil {
		r.Error = "Go protobuf unmarshal failed"
		return
	}
	wantedType := uint64(types.QuaiTxType)
	if v.Kind == "qi" {
		wantedType = uint64(types.QiTxType)
	}
	if envelope.Type == nil || envelope.GetType() != wantedType {
		r.Error = "fixture kind does not match protobuf type"
		return
	}
	// Every currently supplied vector is Cyprus-1; explicit location is recorded
	// here rather than deriving validation context from an untrusted transaction.
	location := common.Location{0, 0}
	var tx types.Transaction
	if tx.ProtoDecode(&envelope, location) != nil {
		r.Error = "Go transaction ProtoDecode failed"
		return
	}
	r.GoProtoDecode = true
	signingBytes, err := proto.Marshal(tx.ProtoEncodeTxSigningData())
	if err != nil {
		r.Error = "Go signing protobuf marshal failed"
		return
	}
	r.SigningBytesMatch = bytes.Equal(signingBytes, unsigned)
	signer := types.NewSigner(tx.ChainId(), location)
	digest := signer.Hash(&tx)
	r.SigningDigestMatch = bytes.Equal(digest[:], expectedDigest)
	canonical, err := tx.ProtoEncode()
	if err != nil {
		r.Error = "Go signed transaction ProtoEncode failed"
		return
	}
	canonicalBytes, err := proto.Marshal(canonical)
	if err != nil {
		r.Error = "Go signed protobuf marshal failed"
		return
	}
	r.SignedBytesMatch = bytes.Equal(canonicalBytes, signed)
	actualHash := tx.Hash()
	r.TransactionHashMatch = bytes.Equal(actualHash[:], expectedHash)
	if v.Kind == "quai" {
		sender, err := types.Sender(signer, &tx)
		if err != nil {
			r.Error = "Go ECDSA sender recovery failed"
			return
		}
		expectedSender, err := hexBytes(v.Input.From, 20)
		if err != nil || len(expectedSender) != 20 {
			r.Error = "invalid fixture sender"
			return
		}
		equal := bytes.Equal(sender.Bytes(), expectedSender)
		r.SenderMatch = &equal
		r.SignatureVerified = equal
	} else {
		if len(tx.TxIn()) == 0 || len(tx.TxIn()) > 1024 {
			r.Error = "unsupported Qi input count"
			return
		}
		keys := make([]*btcec.PublicKey, len(tx.TxIn()))
		for i, input := range tx.TxIn() {
			var err error
			keys[i], err = btcec.ParsePubKey(input.PubKey)
			if err != nil {
				r.Error = "Go input public-key parse failed"
				return
			}
		}
		key := keys[0]
		if len(keys) > 1 {
			aggregate, _, _, err := musig2.AggregateKeys(keys, false)
			if err != nil {
				r.Error = "Go ordered input key aggregation failed"
				return
			}
			key = aggregate.FinalKey
		}
		r.SignatureVerified = tx.GetSchnorrSignature().Verify(digest[:], key)
		dataLen := len(tx.Data())
		if dataLen != 0 && dataLen != common.AddressLength && dataLen != params.MaxQiTxDataLength {
			r.KnownQiDataLengthPolicyFailure = true
			r.Note = "Wire/signature case only: data length violates pinned ProcessQiTx/ValidateQiTxInputs policy; those stateful functions are not invoked."
		}
	}
	return
}

func passed(r result) bool {
	return r.Error == "" && r.GoProtoDecode && r.SigningBytesMatch && r.SigningDigestMatch && r.SignedBytesMatch && r.TransactionHashMatch && r.SignatureVerified && (r.SenderMatch == nil || *r.SenderMatch)
}

func run(path string, output io.Writer, aggregatePaths ...string) (bool, error) {
	file, err := os.Open(path)
	if err != nil {
		return false, errors.New("cannot open fixture file")
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, maxFixtureBytes+1))
	if err != nil || len(data) > maxFixtureBytes {
		return false, errors.New("fixture exceeds read limit or cannot be read")
	}
	var input fixture
	if json.Unmarshal(data, &input) != nil || input.SchemaVersion != 1 || (input.Reference != "quais@1.0.0-alpha.57" && input.Reference != "quai-consensus local signing over quais@1.0.0-alpha.57 conversion vectors") || len(input.Vectors) == 0 {
		return false, errors.New("invalid or unsupported fixture schema/reference")
	}
	sum := sha256.Sum256(data)
	report := report{SchemaVersion: 1, Oracle: "pinned-go-quai-wire-and-signature", PinnedGoCommit: pinnedCommit, GoVersion: runtime.Version(), FixtureReference: input.Reference, FixtureSHA256: hex.EncodeToString(sum[:]), NodeAcceptance: false}
	success := true
	seen := make(map[string]bool)
	for _, v := range input.Vectors {
		if v.ID == "" || seen[v.ID] {
			return false, errors.New("fixture IDs must be nonempty and unique")
		}
		seen[v.ID] = true
		r := verify(v)
		report.Results = append(report.Results, r)
		success = success && passed(r)
	}
	if len(aggregatePaths) > 0 && aggregatePaths[0] != "" {
		results, hash, err := verifyAggregates(aggregatePaths[0])
		if err != nil {
			return false, err
		}
		report.Aggregates = results
		report.AggregateFixtureSHA256 = hash
		for _, r := range results {
			success = success && aggregatePassed(r)
		}
	}
	encoder := json.NewEncoder(output)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(report); err != nil {
		return false, errors.New("cannot write report")
	}
	return success, nil
}

func main() {
	path := flag.String("fixtures", "../../compatibility/fixtures/transactions.json", "public transaction fixture JSON")
	aggregates := flag.String("aggregates", "", "optional public ordered aggregation fixtures with Rust signatures")
	flag.Parse()
	success, err := run(*path, os.Stdout, *aggregates)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	if !success {
		os.Exit(1)
	}
}
