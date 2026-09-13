// Whole-wallet serialization vectors from the published SDK. All secrets are public toys.
import {QuaiHDWallet,QiHDWallet,HDNodeWallet,Mnemonic,wordlists,Zone} from 'quais';
import {writeFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const phrase='abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
const vectors=[];
function normalize(doc) {
  const out=structuredClone(doc);
  out.addresses.sort((a,b)=>a.address.toLowerCase().localeCompare(b.address.toLowerCase()));
  for (const row of out.addresses) {row.lastSyncedBlock=null;if(out.coinType===969)row.status='UNKNOWN';}
  for (const rows of Object.values(out.senderPaymentCodeInfo??{})) {
    rows.sort((a,b)=>a.address.toLowerCase().localeCompare(b.address.toLowerCase()));
    for (const row of rows) {row.lastSyncedBlock=null;row.status='UNKNOWN';}
  }
  return out;
}
async function record(name,wallet,language='en',passphrase='') {
  const doc=wallet.serialize();
  // Retain a synthetic stale checkpoint to verify that Rust discards observations.
  if(doc.coinType===969 && doc.addresses.length)doc.addresses[0].lastSyncedBlock={hash:'0x'+'11'.repeat(32),number:100};
  const canExport=language==='en' && passphrase==='';
  const canonicalExport=canExport?normalize(doc):null;
  if(canExport) {
    const restored=await (doc.coinType===969?QiHDWallet:QuaiHDWallet).deserialize(canonicalExport);
    assert.equal(restored.xPub(),wallet.xPub());
    assert.equal(restored.serialize().addresses.length,doc.addresses.length);
  }
  vectors.push({name,language,passphrase,expectedRoot:HDNodeWallet.fromExtendedKey(wallet.xPub()).neuter().extendedKey,canExport,document:doc,canonicalExport});
}
const quai=QuaiHDWallet.fromPhrase(phrase);
await quai.getNextAddress(0,Zone.Cyprus1);await quai.getNextAddress(1,Zone.Cyprus2);
await record('quai-multiple-accounts-and-zones',quai);
const qi=QiHDWallet.fromPhrase(phrase);
await qi.getNextAddress(0,Zone.Cyprus1);await qi.getNextChangeAddress(0,Zone.Cyprus1);
await qi.getNextAddress(1,Zone.Cyprus2);
await qi.importPrivateKey('0x'+(130n).toString(16).padStart(64,'0'));
const peer=QiHDWallet.fromSeed('0x'+'08'.repeat(32)).getPaymentCode();
qi.openChannel(peer);
await qi.getNextReceiveAddress(peer,Zone.Cyprus1,0);await qi.getNextSendAddress(peer,Zone.Cyprus1,0);
await qi.getNextReceiveAddress(peer,Zone.Cyprus2,1);await qi.getNextSendAddress(peer,Zone.Cyprus2,1);
await record('qi-all-origins-and-payment-accounts',qi);
await record('empty-quai-passphrase-identity',QuaiHDWallet.fromPhrase(phrase,'public-test-passphrase'),'en','public-test-passphrase');
const french=Mnemonic.fromEntropy('0x'+'00'.repeat(16),'',wordlists.fr);
const fr=QuaiHDWallet.fromMnemonic(french);await fr.getNextAddress(0,Zone.Cyprus1);
await record('french-identity',fr,'fr');
writeFileSync(new URL('../fixtures/legacy-wallets.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',publicTestFixturesOnly:true,vectors},null,2)+'\n');
console.log(`Generated ${vectors.length} legacy wallet migration vectors; compatible normalized exports restored by quais.js`);
