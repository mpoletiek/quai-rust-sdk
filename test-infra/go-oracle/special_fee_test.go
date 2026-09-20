// SPDX-License-Identifier: MIT
package main

import (
	"github.com/dominant-strategies/go-quai/common"
	"github.com/dominant-strategies/go-quai/consensus/misc"
	"github.com/dominant-strategies/go-quai/core/types"
	"github.com/dominant-strategies/go-quai/params"
	"math"
	"math/big"
	"testing"
)

// The SDK uses a ceiling plus one gas unit after scaling. Check its bound
// against the pinned node's actual gas function, including threshold neighbors.
func TestSpecialFeeGasBound(t *testing.T) {
	for _, count := range []uint64{0, 1, 3269017, 3269018, 100000000, ^uint64(0)} {
		for _, shape := range [][2]int{{1, 1}, {2, 1}, {1, 1024}, {1024, 1024}} {
			tx := types.NewTx(&types.QiTx{TxIn: make(types.TxIns, shape[0]), TxOut: make(types.TxOuts, shape[1])})
			factor := math.Log(float64(count))
			actual := types.CalculateIntrinsicQiTxGas(tx, factor) + params.QiToQuaiConversionGas
			base := uint64(shape[0])*800 + uint64(shape[1])*9000 + 3000
			bound := base + 100000
			if factor >= 15 {
				bound = uint64(math.Ceil(factor*float64(base)/15)) + 1 + 100000
			}
			if bound < actual || bound-actual > 2 {
				t.Fatalf("shape=%v count=%d bound=%d actual=%d", shape, count, bound, actual)
			}
		}
	}
}

// The node applies two fee floors: execution charges QiToQuaiConversionGas once,
// while inclusion (CalculateBlockQiTxGas, used by the miner's filter and block
// validation) charges ETXGas for every Quai-ledger destination output. The SDK
// must bound the larger of the two, or it quotes a fee no miner will take: a
// mainnet wrap with twelve destination outputs needed 409,400 gas, was priced
// for 257,400, and stayed pooled and unmined while remaining valid.
func TestSpecialFeeInclusionGasBound(t *testing.T) {
	location := common.Location{0, 0}
	quaiLedger := make([]byte, 20)
	quaiLedger[1] = 0x12 // same zone, Quai ledger: a conversion output
	qiLedger := make([]byte, 20)
	qiLedger[1] = 0x80 // same zone, Qi ledger: ordinary change
	for _, count := range []uint64{0, 3269018, 100000000, ^uint64(0)} {
		// inputs, outputs, destination (Quai-ledger) outputs
		for _, shape := range [][3]int{
			{1, 1, 1}, {7, 10, 1}, {2, 6, 5}, {13, 6, 2}, {13, 16, 12}, {1024, 1024, 1024},
		} {
			outs := make(types.TxOuts, 0, shape[1])
			for i := 0; i < shape[1]; i++ {
				address := qiLedger
				if i < shape[2] {
					address = quaiLedger
				}
				outs = append(outs, types.TxOut{Denomination: 1, Address: address})
			}
			tx := types.NewTx(&types.QiTx{TxIn: make(types.TxIns, shape[0]), TxOut: outs})
			factor := math.Log(float64(count))
			actual := types.CalculateBlockQiTxGas(tx, factor, location)
			if execution := types.CalculateIntrinsicQiTxGas(tx, factor) + params.QiToQuaiConversionGas; execution > actual {
				actual = execution
			}
			base := uint64(shape[0])*800 + uint64(shape[1])*9000 + 3000
			intrinsic := base
			if factor >= 15 {
				intrinsic = uint64(math.Ceil(factor*float64(base)/15)) + 1
			}
			bound := intrinsic + params.QiToQuaiConversionGas
			if inclusion := intrinsic + uint64(shape[2])*params.ETXGas; inclusion > bound {
				bound = inclusion
			}
			if bound < actual || bound-actual > 2 {
				t.Fatalf("shape=%v count=%d bound=%d actual=%d", shape, count, bound, actual)
			}
		}
	}
}

// Post-SHA public quote RPCs pass a different difficulty argument from the
// internal fee estimator. Prove both conversions ignore that argument on this
// profile, also across the later conversion fee-reward fork.
func TestShaAnchoredFeeRatesIgnoreDifficultyArgument(t *testing.T) {
	for _, prime := range []int64{1755000, 2236999, 2237000, 2500000} {
		for _, shares := range []int64{0, 1, 4} {
			header := &types.WorkObjectHeader{}
			header.SetNumber(big.NewInt(3000000))
			header.SetPrimeTerminusNumber(big.NewInt(prime))
			sha := &types.PowShareDiffAndCount{}
			sha.SetDifficulty(new(big.Int).Lsh(big.NewInt(1), 100))
			sha.SetCount(new(big.Int).Lsh(big.NewInt(shares), 32))
			scrypt := &types.PowShareDiffAndCount{}
			scrypt.SetDifficulty(big.NewInt(1))
			scrypt.SetCount(big.NewInt(0))
			header.SetShaDiffAndCount(sha)
			header.SetScryptDiffAndCount(scrypt)
			body := types.EmptyWorkObjectBody()
			body.Header().SetAvgTxFees(big.NewInt(123456))
			block := types.NewWorkObject(header, body, nil)
			exchange := big.NewInt(1000000000000000)
			for _, amount := range []int64{0, 1, 123456, 1000000000000000} {
				for _, convert := range []func(*types.WorkObject, *big.Int, *big.Int, *big.Int) *big.Int{misc.QiToQuai, misc.QuaiToQi} {
					a := convert(block, exchange, big.NewInt(1), big.NewInt(amount))
					b := convert(block, exchange, new(big.Int).Lsh(big.NewInt(1), 200), big.NewInt(amount))
					if a.Cmp(b) != 0 {
						t.Fatalf("prime=%d shares=%d amount=%d: difficulty-dependent quote", prime, shares, amount)
					}
				}
			}
		}
	}
}
