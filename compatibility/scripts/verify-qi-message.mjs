// Verify a Rust example's public toy-key signature with the pinned JS backend.
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
import {computeAddress, getBytes, keccak256} from 'quais';
import {schnorr} from '@noble/curves/secp256k1';
const file=readFileSync(process.argv[2]);
assert.ok(file.length<65536);
const value=JSON.parse(file);
assert.equal(value.publicFixture,true);
assert.equal(computeAddress(value.publicKey).toLowerCase(),value.address.toLowerCase());
assert.ok(schnorr.verify(getBytes(value.signature),getBytes(keccak256(value.message)),getBytes(value.publicKey).slice(1)));
assert.equal(schnorr.verify(getBytes(value.signature),getBytes(keccak256(value.message+'00')),getBytes(value.publicKey).slice(1)),false);
console.log('Pinned JS verified Rust Qi message signature and rejected a changed message');
