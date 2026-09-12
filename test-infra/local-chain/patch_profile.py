#!/usr/bin/env python3
"""Apply explicit acceleration/prefund patches to an isolated pinned source copy only."""
import pathlib
import sys
root=pathlib.Path(sys.argv[1]).resolve()
if not root.is_relative_to(pathlib.Path('/tmp')) or root.name!='source' or root==pathlib.Path('/tmp/quai-sdk-research-go'):
    raise SystemExit('requires an isolated /tmp/.../source copy')
def replace(path,old,new):
    file=root/path;text=file.read_text()
    if text.count(old)!=1:raise SystemExit('unexpected baseline: '+path)
    file.write_text(text.replace(old,new))
alloc=pathlib.Path(__file__).with_name('public-alloc.json').read_bytes()
(root/'params/genesis_alloc.json').write_bytes(alloc)
replace('params/config.go','0xa1f3f565b62c83fcc0e61dfd219409fb06526570934a5d94b5db5afdad13f1da','0x11035b9cfa8bdea8f0274f9d39723f337941659d95972a303384258fe7a8988a')
replace('params/protocol_params.go','TimeToStartTx              uint64 = 15 * BlocksPerDay','TimeToStartTx              uint64 = 0 // SDK DISPOSABLE PROFILE: execution from first block')
replace('cmd/utils/flags.go','\tif nodeLocation.Equal(common.Location{0, 0}) {\n\t\tcfg.GenesisAllocs, err = params.VerifyGenesisAllocs','\tif viper.GetString(EnvironmentFlag.Name) == params.LocalName {\n\t\tcfg.DefaultGenesisHash = cfg.Genesis.ToBlock(0).Hash()\n\t}\n\tif nodeLocation.Equal(common.Location{0, 0}) {\n\t\tcfg.GenesisAllocs, err = params.VerifyGenesisAllocs')
replace('cmd/go-quai/start.go','\t"context"','\t"context"\n\t"errors"')
replace('cmd/go-quai/start.go','\tnetwork := viper.GetString(utils.EnvironmentFlag.Name)','\tnetwork := viper.GetString(utils.EnvironmentFlag.Name)\n\tif network != params.LocalName || !viper.GetBool(utils.SoloFlag.Name) || viper.GetString(utils.IPAddrFlag.Name) != "127.0.0.1" {\n\t\treturn errors.New("SDK disposable profile requires local, solo and loopback")\n\t}')
# The pinned Blake3 engine list has one entry, while this post-Kawpow index unconditionally selects index1.
replace('core/bodydb.go','if bc.chainConfig.IndexAddressUtxos {','if bc.chainConfig.IndexAddressUtxos && len(bc.engine) > int(types.Kawpow) {')
# A deterministic bootstrap UTXO is committed as part of block1's MuHash/state, never an RPC mock.
# This deliberately changes local consensus allocation. It is not an unmodified-chain result.
needle='\t\taddressOutpointMap := make(map[[20]byte][]*types.OutpointAndDenomination)\n'
insert='''\t\t// SDK DISPOSABLE PROFILE: public-fixture Qi allocation at block1.
        fixtureHash := common.HexToHash("0x0080008011111111111111111111111111111111111111111111111111111111")
        fixtureAddress := common.HexToAddress("0x00edf2d16afbc028fb1e879559b07997af79539f", nodeLocation)
        fixtureUtxo := &types.UtxoEntry{Denomination: 14, Address: fixtureAddress.Bytes(), Lock: big.NewInt(0)}
        utxosCreate = append(utxosCreate, types.UTXOHash(fixtureHash, 0, fixtureUtxo))
        if !setRoots {
            if err := rawdb.CreateUTXO(batch, fixtureHash, 0, fixtureUtxo); err != nil { return nil, 0, nil, err }
            if hc.config.IndexAddressUtxos {
                index := map[[20]byte][]*types.OutpointAndDenomination{fixtureAddress.Bytes20(): {{TxHash:fixtureHash, Index:0, Denomination:14, Lock:big.NewInt(0)}}}
                if err := rawdb.WriteAddressUTXOs(batch, hc.Database(), index); err != nil { return nil, 0, nil, err }
            }
        }
'''
replace('core/headerchain_validation.go',needle,insert+needle)
print('Applied explicit development-only activation, allocation, index guard and Qi bootstrap patches')
