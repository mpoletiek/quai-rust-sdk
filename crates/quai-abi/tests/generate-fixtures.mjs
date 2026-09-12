// Public test key 1 only. Never fund any fixture key/address.
import { writeFileSync } from 'node:fs';
import { TypedDataEncoder, SigningKey, computeAddress } from '../../../compatibility/node_modules/quais/lib/esm/index.js';
const field = (name, type) => ({name, type});
const vectors = [];
const rejects = [];
const policy = [];
function add(id, types, value, domain = {}) {
  const encoder = TypedDataEncoder.from(types);
  const digest = TypedDataEncoder.hash(domain, types, value);
  const key = new SigningKey('0x' + '00'.repeat(31) + '01');
  vectors.push({id, domain, types, value, primaryType: encoder.primaryType,
    encodedType: encoder.encodeType(encoder.primaryType), encoded: encoder.encode(value),
    structHash: encoder.hash(value), domainHash: TypedDataEncoder.hashDomain(domain),
    preimage: TypedDataEncoder.encode(domain,types,value), digest,
    signature: key.sign(digest).serialized, signer: computeAddress(key.publicKey)});
}
const mailTypes = {
  Person: [field('name','string'),field('wallet','address')],
  Mail: [field('from','Person'),field('to','Person'),field('contents','string')]
};
add('official-mail',mailTypes,{from:{name:'Cow',wallet:'0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826'},to:{name:'Bob',wallet:'0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB'},contents:'Hello, Bob!'},{name:'Ether Mail',version:'1',chainId:1,verifyingContract:'0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC'});
add('empty-struct',{Empty:[]},{});
add('all-domain-fields',{Empty:[]},{},{name:'Quai 🦆',version:'α',chainId:'0x'+'ff'.repeat(32),verifyingContract:'0x'+'00'.repeat(20),salt:'0x'+'12'.repeat(32)});
add('null-domain',{Empty:[]},{},{name:null,version:null,chainId:null,verifyingContract:null,salt:null});
add('unicode-bytes',{Data:[field('text','string'),field('raw','bytes'),field('enabled','bool'),field('disabled','bool')]},{text:'NUL\u0000, é ≠ é; 🦆',raw:'0x0001ff',enabled:true,disabled:false});
add('empty-dynamics',{Data:[field('text','string'),field('raw','bytes'),field('items','bytes[]')]},{text:'',raw:'0x',items:[]});
for(let bits=8;bits<=256;bits+=8) {
  add('integer-width-'+bits,{Numbers:[field('minimum','int'+bits),field('maximum','int'+bits),field('unsigned','uint'+bits),field('zero','uint'+bits)]},
    {minimum:(-(1n<<BigInt(bits-1))).toString(),maximum:((1n<<BigInt(bits-1))-1n).toString(),unsigned:((1n<<BigInt(bits))-1n).toString(),zero:0});
}
add('integer-hex-and-number',{Numbers:[field('a','int256'),field('b','uint64'),field('c','int64'),field('d','uint256')]},{a:'-0x80',b:9007199254740991,c:-9007199254740991,d:'0XFF'});
add('fixed-bytes',{Data:Array.from({length:32},(_,i)=>field('b'+i,'bytes'+(i+1)))},Object.fromEntries(Array.from({length:32},(_,i)=>['b'+i,'0x'+'a5'.repeat(i+1)])));
add('arrays',{Arrays:[field('grid','int16[2][]'),field('bytes','bytes[][2]'),field('flags','bool[2][2]'),field('none','uint8[0]')]},{grid:[[-1,2],[32767,-32768]],bytes:[['0x','0x1234'],[]],flags:[[true,false],[false,true]],none:[]});
add('struct-arrays',{Root:[field('z','Zoo[]'),field('a','Alpha[2]')],Zoo:[field('name','string'),field('a','Alpha')],Alpha:[field('value','uint32')]},{z:[{name:'first',a:{value:3}},{name:'second',a:{value:4}}],a:[{value:5},{value:6}]});
add('dependency-diamond',{Root:[field('z','Z'),field('a','A')],Z:[field('child','Leaf')],A:[field('child','Leaf')],Leaf:[field('number','uint8')]},{z:{child:{number:1}},a:{child:{number:2}}});
function reject(id,types,value={},domain={}) {
  try { TypedDataEncoder.hash(domain,types,value); throw new Error('unexpected acceptance '+id); }
  catch(error) { if(error.message.startsWith('unexpected acceptance')) throw error; rejects.push({id,types,value,domain,referenceRejects:true}); }
}
const scalar=(type)=>({Root:[field('value',type)]});
for(const type of ['int','uint','uint0','uint7','uint264','uint08','int255','bytes0','bytes33','bytes01','unknown','uint8[','uint8[-1]']) reject('type-'+type,scalar(type),{value:0});
reject('self-cycle',{Root:[field('children','Root[]')]},{children:[]});
reject('mutual-cycle',{A:[field('b','B')],B:[field('a','A')]});
reject('duplicate-field',{Root:[field('x','uint8'),field('x','bool')]},{x:1});
reject('ambiguous',{A:[],B:[]});
for(const [type,value] of [['uint8',256],['uint8',-1],['int8',128],['int8',-129],['uint256','0x1'+'00'.repeat(32)],['int256',(-(1n<<255n)-1n).toString()],['uint64',9007199254740992],['uint8',1.5],['uint8','no'],['bytes2','0x01'],['bytes','0x0'],['bytes','0xgg'],['address','0x1234'],['uint8[2]',[1]],['uint8[2]',[1,2,3]],['uint8[]',4]]) reject('value-'+rejects.length,scalar(type),{value});
reject('bad-domain-chain', {Root:[]},{},{chainId:-1});
reject('bad-domain-salt',{Root:[]},{},{salt:'0x12'});
reject('unknown-domain',{Root:[]},{},{random:'x'});

function stricter(id,types,value,domain={}) {
 let referenceAccepts;
 try { TypedDataEncoder.hash(domain,types,value); referenceAccepts=true; } catch { referenceAccepts=false; }
 policy.push({id,types,value,domain,referenceAccepts});
}
stricter('numeric-domain-name',{Root:[]},{},{name:42});
stricter('truthy-bool',scalar('bool'),{value:'false'});
stricter('missing-bool',scalar('bool'),{});
stricter('extra-field',scalar('uint8'),{value:1,notSigned:'danger'});
stricter('invalid-identifier',{'bad name':[field('x','uint8')]},{x:1});
stricter('noncanonical-array-width',scalar('uint8[02]'),{value:[1,2]});
stricter('null-unknown-domain',{Root:[]},{},{unknown:null});
stricter('integer-whitespace',scalar('uint8'),{value:' 1 '});
writeFileSync(new URL('fixtures/typed-data.json',import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',vectors,rejects,strictPolicy:policy},null,2)+'\n');
console.log(`${vectors.length} valid, ${rejects.length} shared rejection, ${policy.length} strict policy cases`);
