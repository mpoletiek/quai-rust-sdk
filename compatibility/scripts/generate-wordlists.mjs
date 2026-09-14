import {wordlists,Mnemonic,Wordlist,id} from 'quais';import {writeFileSync} from 'node:fs';
const languages=[];
for(const [locale,list] of Object.entries(wordlists)){
 const words=Array.from({length:2048},(_,i)=>list.getWord(i));const entropy='0x'+'00'.repeat(16),password='TREZOR';const mnemonic=Mnemonic.fromEntropy(entropy,password,list);
 const sample=words.slice(0,3);const phrases=[list.join(sample),' '+list.join(sample)+' ',sample.join('\t'),sample.join(''),sample.join('\u3000'),list.join(sample).normalize('NFC')];
 const splits=phrases.map(phrase=>({phrase,words:list.split(phrase)}));
 languages.push({locale,words,checksum:id(words.join('\n')+'\n'),owl:list._data??null,accents:list._accent??null,entropy,password,phrase:mnemonic.phrase,seed:mnemonic.computeSeed(),splits,join:list.join(sample)});
}
class Custom extends Wordlist {constructor(){super('custom');}getWord(i){if(i<0||i>=2048)throw Error('index');return 'word'+String(i).padStart(4,'0');}getWordIndex(w){const m=/^word([0-9]{4})$/.exec(w);return m&&+m[1]<2048?+m[1]:-1;}}
const list=new Custom();const custom=[16,20,24,28,32].map((n,i)=>{const entropy='0x'+(i?'a5':'00').repeat(n),password=i?'é 日本語':'',m=Mnemonic.fromEntropy(entropy,password,list);return {entropy,password,phrase:m.phrase,seed:m.computeSeed()};});
writeFileSync(new URL('../fixtures/wordlists.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',languages,custom},null,2)+'\n');console.log(`Generated ${languages.length} full wordlists, phrase conventions and ${custom.length} custom dictionary mnemonics`);
