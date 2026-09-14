import {FetchRequest,hexlify} from 'quais';
import {writeFileSync} from 'node:fs';
const values=[['none',null],['text','hello π'],['text',''],['bytes','0x0001ff'],['bytes','0x'],['json',{count:'9007199254740993',ok:true}]];
const bodies=values.map(([kind,input])=>{const req=new FetchRequest('https://example.invalid/public');req.body=kind==='bytes'?Uint8Array.from(Buffer.from(input.slice(2),'hex')):input;return {kind,input,method:req.method,body:req.body===null?null:hexlify(req.body),contentType:req.getHeader('content-type')??null};});
const data=[];for(const uri of ['data:,hello%20world','data:application/json,%7B%22ok%22%3Atrue%7D','data:;base64,AAH/','data:,a+b%20c','data:,','data:,%gg']){const r=await new FetchRequest(uri).send();data.push({uri,rustReject:uri==='data:,%gg',status:r.statusCode,body:r.body===null?null:hexlify(r.body),contentType:r.getHeader('content-type')??null});}
writeFileSync(new URL('../fixtures/fetch.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',bodies,data},null,2)+'\n');
