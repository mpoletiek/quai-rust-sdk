#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_abi::{AbiCoder,AbiType,AbiInterface,AbiValue,TypedData};
fuzz_target!(|data:&[u8]| {
 if data.len()>65_536{return;}
 if let Ok(text)=std::str::from_utf8(data){if let Ok(ty)=AbiType::parse(text){if let Ok(value)=AbiValue::default_for(ty){let encoded=value.encode().unwrap();assert_eq!(AbiCoder::decode(std::slice::from_ref(value.abi_type()),&encoded).unwrap(),vec![value.value().clone()]);}}}
 let _=AbiInterface::from_json(data);
 if let Ok(doc)=TypedData::from_json(data){if let Ok(rpc)=doc.to_rpc_json(){assert_eq!(TypedData::from_json(rpc.as_bytes()).unwrap().signing_hash(),doc.signing_hash());}}
 for expression in ["uint256","bytes","string","uint256[]","(address,bytes,uint256[])","bytes[][]","uint256[0][]"]{
  let ty=AbiType::parse(expression).unwrap();
  if let Ok(values)=AbiCoder::decode(std::slice::from_ref(&ty),data){assert_eq!(AbiCoder::encode(&[ty],&values).unwrap(),data);}
 }
});
