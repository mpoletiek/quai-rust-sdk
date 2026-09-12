// Public-only test vectors. Low KDF costs are intentional for deterministic tests.
import { encryptKeystoreJsonSync, decryptKeystoreJsonSync, computeAddress, Mnemonic, HDNodeWallet, keccak256, getBytes, hexlify } from 'quais';
import { pbkdf2Sync, scryptSync, createCipheriv } from 'node:crypto';
import { writeFileSync } from 'node:fs';
const salt=Buffer.alloc(32,0x11),iv=Buffer.alloc(16,0x22),uuid=Buffer.alloc(16,0x33),mnemonicIv=Buffer.alloc(16,0x44);
const encrypt=(key,iv,input)=>{const c=createCipheriv(`aes-${key.length*8}-ctr`,key,iv);return Buffer.concat([c.update(input),c.final()]);};
const vectors=[];
function add(name,account,password){
 let json=JSON.parse(encryptKeystoreJsonSync(account,password,{salt,iv,uuid,scrypt:{N:16,r:1,p:1}}));
 if(account.mnemonic){
   const p=typeof password==='string'?Buffer.from(password.normalize('NFKC')):password;
   const derived=scryptSync(p,salt,64,{N:16,r:1,p:1});
   json['x-quais'].mnemonicCounter=mnemonicIv.toString('hex');
   json['x-quais'].mnemonicCiphertext=encrypt(derived.subarray(32),mnemonicIv,getBytes(account.mnemonic.entropy)).toString('hex');
   json['x-quais'].gethFilename='public-fixture';
 }
 const decoded=decryptKeystoreJsonSync(JSON.stringify(json),password);
 vectors.push({name,password:typeof password==='string'?{text:password}:{bytes:Buffer.from(password).toString('hex')},expected:decoded,json});
}
for(const [scalar,password] of [[805,'PUBLIC password'],[130,'① Café Å'],[1,new Uint8Array([0,255,1,128])]]){
 const privateKey='0x'+BigInt(scalar).toString(16).padStart(64,'0');add(`raw-${scalar}`,{address:computeAddress(privateKey),privateKey},password);
}
// Two derivation paths and all ten mnemonic languages.
const {wordlists}=await import('quais');
for(const [locale,words] of Object.entries(wordlists)){
 const entropy='0x'+'00'.repeat(16),path=locale==='en'?"m/44'/969'/1'/1/12":"m/44'/994'/0'/0/0";
 const mnemonic=Mnemonic.fromEntropy(entropy,'',words),wallet=HDNodeWallet.fromMnemonic(mnemonic,path);
 add(`mnemonic-${locale}`,{address:wallet.address,privateKey:wallet.privateKey,mnemonic:{entropy,path,locale}},'PUBLIC mnemonic');
}
for(const prf of ['sha256','sha512']){
 const privateKey='0x'+(805n).toString(16).padStart(64,'0'),password='PUBLIC PBKDF2';
 const derived=pbkdf2Sync(password,salt,64,32,prf),ciphertext=encrypt(derived.subarray(0,16),iv,getBytes(privateKey));
 const json={version:3,address:computeAddress(privateKey).slice(2).toLowerCase(),Crypto:{cipher:'aes-128-ctr',cipherparams:{iv:iv.toString('hex')},ciphertext:ciphertext.toString('hex'),kdf:'pbkdf2',kdfparams:{salt:salt.toString('hex'),c:64,dklen:32,prf:`hmac-${prf}`},mac:keccak256(Buffer.concat([derived.subarray(16),ciphertext])).slice(2)}};
 vectors.push({name:`pbkdf2-${prf}`,password:{text:password},expected:decryptKeystoreJsonSync(JSON.stringify(json),password),json});
}
writeFileSync(new URL('../../crates/quai-keystore/tests/fixtures/keystores.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',publicFixtureOnly:true,vectors},null,2)+'\n');
console.log(`${vectors.length} public keystore vectors`);
