// Public-fixture, loopback-only helper for the explicitly isolated development chain.
package main

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/dominant-strategies/go-quai/common"
	"github.com/dominant-strategies/go-quai/core/types"
	"github.com/dominant-strategies/go-quai/crypto"
	"google.golang.org/protobuf/proto"
	"lukechampine.com/blake3"
)

func rpc(endpoint, method string, params []any, out any) error {
	body, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	request, err := http.NewRequestWithContext(ctx, "POST", endpoint, bytes.NewReader(body))
	if err != nil {
		return err
	}
	request.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: 10 * time.Second, Transport: &http.Transport{Proxy: nil}, CheckRedirect: func(*http.Request, []*http.Request) error { return fmt.Errorf("redirect forbidden") }}
	response, err := client.Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode != 200 {
		return fmt.Errorf("HTTP status %d", response.StatusCode)
	}
	raw, err := io.ReadAll(io.LimitReader(response.Body, 2*1024*1024+1))
	if err != nil {
		return err
	}
	if len(raw) > 2*1024*1024 {
		return fmt.Errorf("response too large")
	}
	var envelope struct {
		Version string          `json:"jsonrpc"`
		ID      int             `json:"id"`
		Result  json.RawMessage `json:"result"`
		Error   json.RawMessage `json:"error"`
	}
	if err = json.Unmarshal(raw, &envelope); err != nil {
		return err
	}
	if envelope.ID != 1 || envelope.Version != "2.0" {
		return fmt.Errorf("wrong id")
	}
	if len(envelope.Error) > 0 && string(envelope.Error) != "null" {
		return fmt.Errorf("local RPC error: %.500s", envelope.Error)
	}
	return json.Unmarshal(envelope.Result, out)
}
func run() error {
	if len(os.Args) < 2 {
		return fmt.Errorf("usage: keys | hash FILE | mine URL GENESIS COUNT")
	}
	switch os.Args[1] {
	case "keys":
		for _, scalar := range []int64{805, 130} {
			key, err := crypto.HexToECDSA(fmt.Sprintf("%064x", scalar))
			if err != nil {
				return err
			}
			fmt.Printf("%d %s\n", scalar, crypto.PubkeyToAddress(key.PublicKey, common.Location{0, 0}).Hex())
		}
		return nil
	case "hash":
		if len(os.Args) != 3 {
			return fmt.Errorf("hash needs file")
		}
		data, err := os.ReadFile(os.Args[2])
		if err != nil {
			return err
		}
		hash := blake3.Sum256(data)
		fmt.Printf("0x%x\n", hash)
		return nil
	case "mine":
		if len(os.Args) != 5 {
			return fmt.Errorf("mine needs URL GENESIS COUNT")
		}
		endpoint := os.Args[2]
		u, err := url.Parse(endpoint)
		if err != nil || u.Scheme != "http" || u.Hostname() != "127.0.0.1" || u.User != nil {
			return fmt.Errorf("requires loopback HTTP endpoint")
		}
		var chain string
		if err = rpc(endpoint, "quai_chainId", []any{}, &chain); err != nil {
			return err
		}
		if chain != "0x539" {
			return fmt.Errorf("refusing chain other than1337")
		}
		var genesis struct {
			Work struct {
				Hash string `json:"hash"`
			} `json:"woHeader"`
		}
		if err = rpc(endpoint, "quai_getHeaderByNumber", []any{"0x0"}, &genesis); err != nil {
			return err
		}
		if genesis.Work.Hash != os.Args[3] {
			return fmt.Errorf("genesis mismatch")
		}
		count, err := strconv.Atoi(os.Args[4])
		if err != nil || count < 1 || count > 100 {
			return fmt.Errorf("count out of bounds")
		}
		for block := 0; block < count; block++ {
			var encoded string
			if err = rpc(endpoint, "quai_getPendingHeader", []any{}, &encoded); err != nil {
				return err
			}
			if !strings.HasPrefix(encoded, "0x") || len(encoded) < 4 {
				return fmt.Errorf("malformed pending header")
			}
			raw, err := hex.DecodeString(encoded[2:])
			if err != nil {
				return err
			}
			wire := &types.ProtoWorkObject{}
			if err = proto.Unmarshal(raw, wire); err != nil {
				return err
			}
			wo := &types.WorkObject{}
			if err = wo.ProtoDecode(wire, common.Location{0, 0}, types.PEtxObject); err != nil {
				return err
			}
			if wo.Difficulty() == nil || wo.Difficulty().Sign() <= 0 {
				return fmt.Errorf("invalid work difficulty")
			}
			target := new(big.Int).Div(new(big.Int).Lsh(big.NewInt(1), 256), wo.Difficulty())
			start := time.Now()
			found := false
			var nonce uint64
			for nonce = 0; nonce < 100000000; nonce++ {
				wo.WorkObjectHeader().SetNonce(types.EncodeNonce(nonce))
				hash := wo.Hash()
				if new(big.Int).SetBytes(hash.Bytes()).Cmp(target) <= 0 {
					found = true
					break
				}
				if nonce%10000 == 0 && time.Since(start) > 60*time.Second {
					return fmt.Errorf("mining deadline")
				}
			}
			if !found {
				return fmt.Errorf("nonce budget exhausted")
			}
			wire, err = wo.ProtoEncode(types.PEtxObject)
			if err != nil {
				return err
			}
			raw, err = proto.Marshal(wire)
			if err != nil {
				return err
			}
			var result any
			if err = rpc(endpoint, "quai_receiveMinedHeader", []any{"0x" + hex.EncodeToString(raw)}, &result); err != nil {
				return err
			}
			fmt.Printf("submitted height=%d hash=%s nonce=%d seconds=%.3f gas=%d state=%d\n", wo.NumberU64(common.ZONE_CTX), wo.Hash().Hex(), nonce, time.Since(start).Seconds(), wo.GasLimit(), wo.StateLimit())
			deadline := time.Now().Add(10 * time.Second)
			for {
				var head string
				if err = rpc(endpoint, "quai_blockNumber", []any{}, &head); err != nil {
					return err
				}
				if !strings.HasPrefix(head, "0x") || len(head) < 3 {
					return fmt.Errorf("malformed block number")
				}
				n, parseErr := strconv.ParseUint(head[2:], 16, 64)
				if parseErr != nil {
					return fmt.Errorf("malformed block number")
				}
				if n >= wo.NumberU64(common.ZONE_CTX) {
					var canonical struct {
						Work struct {
							Hash string `json:"hash"`
						} `json:"woHeader"`
					}
					if err = rpc(endpoint, "quai_getHeaderByNumber", []any{fmt.Sprintf("0x%x", wo.NumberU64(common.ZONE_CTX))}, &canonical); err != nil {
						return err
					}
					if canonical.Work.Hash != wo.Hash().Hex() {
						return fmt.Errorf("mined header not canonical")
					}
					break
				}
				if time.Now().After(deadline) {
					return fmt.Errorf("submitted block not canonical before deadline")
				}
				time.Sleep(100 * time.Millisecond)
			}
			time.Sleep(time.Second)
		}
		return nil
	}
	return fmt.Errorf("unknown command")
}
func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
