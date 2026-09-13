#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_abi::{AbiCoder,AbiType,AbiInterface,AbiValue,SolidityArtifact,TypedData};
fuzz_target!(|data:&[u8]| {
 if data.len()>65_536{return;}
 if let Ok(text)=std::str::from_utf8(data){if let Ok(ty)=AbiType::parse(text){if let Ok(value)=AbiValue::default_for(ty){let encoded=value.encode().unwrap();assert_eq!(AbiCoder::decode(std::slice::from_ref(value.abi_type()),&encoded).unwrap(),vec![value.value().clone()]);}}}
 if let Ok(text)=std::str::from_utf8(data){if let Ok(types)=text.split('\n').take(1025).map(AbiType::parse).collect::<Result<Vec<_>,_>>(){if let Ok(values)=AbiCoder::default_values(&types){let encoded=AbiCoder::encode(&types,&values).unwrap();assert_eq!(AbiCoder::decode(&types,&encoded).unwrap(),values);}}}
 if let Ok(value)=serde_json::from_slice::<serde_json::Value>(data){
  if let (Some(types),Some(values))=(value["types"].as_array(),value["values"].as_array()){
   if types.len()<=1024 {
    let types=types.iter().map(|v|v.as_str().ok_or(quai_abi::AbiError::Schema).and_then(AbiType::parse)).collect::<Result<Vec<_>,_>>();
    if let Ok(types)=types {if let Ok(packed)=quai_abi::solidity_packed(&types,values){
     assert!(packed.len()<=AbiCoder::encode(&types,values).unwrap().len());
     assert_eq!(quai_abi::solidity_packed_keccak256(&types,values).unwrap(),quai_crypto::keccak256(&packed));
     assert_eq!(quai_abi::solidity_packed_sha256(&types,values).unwrap(),quai_crypto::sha256(&packed));
    }}
   }
  }
 }
 let _=AbiInterface::from_json(data);
 let _=SolidityArtifact::from_json(data);
 if let Ok(doc)=TypedData::from_json(data){if let Ok(rpc)=doc.to_rpc_json(){assert_eq!(TypedData::from_json(rpc.as_bytes()).unwrap().signing_hash(),doc.signing_hash());}}
 for expression in ["uint256","bytes","string","uint256[]","(address,bytes,uint256[])","bytes[][]","uint256[0][]"]{
  let ty=AbiType::parse(expression).unwrap();
  if let Ok(values)=AbiCoder::decode(std::slice::from_ref(&ty),data){assert_eq!(AbiCoder::encode(&[ty],&values).unwrap(),data);}
 }
});
