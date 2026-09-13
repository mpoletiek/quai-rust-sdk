// Offline runtime byte hashes from the pinned reference; no deployment claim.
import {writeFileSync} from 'node:fs';
import {keccak256} from '../compatibility/node_modules/quais/lib/esm/index.js';
const vectors=['0x','0x00','0x602a60005260206000f3'].map(code=>({code,hash:keccak256(code)}));
writeFileSync(new URL('fixtures/contract-code.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',qualification:'Offline byte hashes only; no contract deployment or semantics claim.',vectors},null,2)+'\n');
