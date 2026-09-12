// Explicit declaration-level parity tracking. No API is silently removed from scope.
import {readFileSync,writeFileSync,existsSync} from 'node:fs';
import {createHash} from 'node:crypto';
import assert from 'node:assert/strict';
const inventoryURL=new URL('../api-inventory.json',import.meta.url);
const trackerURL=new URL('../parity.json',import.meta.url);
const bytes=readFileSync(inventoryURL),inventory=JSON.parse(bytes);
const inventorySha256=createHash('sha256').update(bytes).digest('hex');
const required=[];
for(const entry of inventory.exports) {
  required.push({id:entry.id,module:entry.subpath,symbol:entry.exportedName,behavior:'export/constructor',signatures:entry.target.signatures??[]});
  for(const [kind,members] of [['instance',entry.instanceMembers],['static',entry.staticMembers]]) {
    for(const member of members??[]) required.push({id:`${entry.id}:${kind}:${member.name}`,module:entry.subpath,symbol:`${entry.exportedName}.${member.name}`,behavior:kind,signatures:member.signatures??[]});
  }
}
assert.equal(new Set(required.map(x=>x.id)).size,required.length,'nonunique reference IDs');
const previous=existsSync(trackerURL)?JSON.parse(readFileSync(trackerURL)):null;
if(process.argv.includes('--refresh')) {
  const old=new Map((previous?.entries??[]).map(x=>[x.id,x]));
  const entries=required.map(row=>({...row,reference_version:inventory.reference,rust_api:[],feature:'unmapped',status:'pending',test_ids:[],docs:[],deviation:null,...old.get(row.id),...row}));
  const removed=[...old.keys()].filter(id=>!entries.some(x=>x.id===id));
  assert.deepEqual(removed,[],'removed reference entries require explicit scope review');
  writeFileSync(trackerURL,JSON.stringify({schemaVersion:1,reference:inventory.reference,inventorySha256,scope:'Every inventoried export and public class member; declaration tracking must be supplemented by semantic, overload, node, security and platform acceptance. Implemented does not mean release-qualified.',entries},null,2)+'\n');
  console.log(`Retained ${entries.length} declaration rows; new rows remain pending.`);
} else {
  assert.ok(previous,'run parity.mjs --refresh to initialize the tracker');
  assert.equal(previous.inventorySha256,inventorySha256,'inventory drift requires review and tracker refresh');
  assert.deepEqual(previous.entries.map(x=>x.id),required.map(x=>x.id),'reference denominator drift');
  const statuses={};
  for(const entry of previous.entries) {
    assert.ok(['pending','partial','implemented','deviation'].includes(entry.status));
    if(entry.status!=='pending') {
      assert.ok(entry.test_ids.length && entry.docs.length,`${entry.id} needs tests/docs`);
      assert.ok(entry.rust_api.length || entry.deviation,`${entry.id} needs API or deviation`);
      for(const path of [...entry.test_ids,...entry.docs]) {
        const file=path.split('#')[0];
        assert.ok(!file.startsWith('/') && !file.split('/').includes('..'),`${entry.id} invalid evidence path`);
        assert.ok(existsSync(new URL(`../../${file}`,import.meta.url)),`${entry.id} missing evidence: ${file}`);
      }
    }
    statuses[entry.status]=(statuses[entry.status]??0)+1;
  }
  console.log(JSON.stringify({declarationRows:required.length,statuses,releaseQualification:false}));
}
