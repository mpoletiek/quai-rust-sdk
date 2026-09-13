#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_abi::{AbiCoder,AbiType,AbiInterface,AbiValue,SolidityArtifact,TypedData,AbiParameter,AbiFormat,AbiResult};
fuzz_target!(|data:&[u8]| {
 if data.len()>65_536{return;}
 for event in [false,true] {
  if let Ok(p)=AbiParameter::from_json(data,event) {let json=p.format(AbiFormat::Json).unwrap();assert_eq!(AbiParameter::from_json(json.as_bytes(),event).unwrap(),p);}
  if let Ok(text)=std::str::from_utf8(data) {if let Ok(p)=AbiParameter::from_human_readable(text,event) {let json=p.format(AbiFormat::Json).unwrap();assert_eq!(AbiParameter::from_json(json.as_bytes(),event).unwrap(),p);}}
 }

 if let Ok(text)=std::str::from_utf8(data){if let Ok(ty)=AbiType::parse(text){if let Ok(value)=AbiValue::default_for(ty){let encoded=value.encode().unwrap();assert_eq!(AbiCoder::decode(std::slice::from_ref(value.abi_type()),&encoded).unwrap(),vec![value.value().clone()]);}}}
 if let Ok(text)=std::str::from_utf8(data){if let Ok(types)=text.split('\n').take(1025).map(AbiType::parse).collect::<Result<Vec<_>,_>>(){if let Ok(values)=AbiCoder::default_values(&types){let encoded=AbiCoder::encode(&types,&values).unwrap();assert_eq!(AbiCoder::decode(&types,&encoded).unwrap(),values);}}}
 if let Ok(value)=serde_json::from_slice::<serde_json::Value>(data){
  if let Some(parameter)=value.get("parameter") {
   if let Ok(p)=AbiParameter::from_json(&serde_json::to_vec(parameter).unwrap(),true) {
    if let Ok(walked)=p.walk(&value["value"],|_,v|Ok(v.clone())) {assert_eq!(p.walk(&walked,|_,v|Ok(v.clone())).unwrap(),walked);}
   }
  }
  if let (Some(items),Some(names))=(value.get("items").and_then(|v|v.as_array()),value.get("names").and_then(|v|v.as_array())) {
   if items.len()<=1025 && names.len()<=1025 {
    if let Ok(names)=names.iter().map(|v|if v.is_null(){Ok(None)}else{v.as_str().map(|s|Some(s.to_owned())).ok_or(())}).collect::<Result<Vec<_>,_>>() {
     if let Ok(result)=AbiResult::from_items(items.clone(),names) {assert_eq!(result.slice(0..items.len()).unwrap().values(),items);for (i,name) in result.names().iter().enumerate(){if let Some(name)=name {assert_eq!(result.get_value(name),Some(&items[i]));}}}
    }
   }
  }
  if let Some(document)=value.get("abi") {
   if let Ok(interface)=AbiInterface::from_json(&serde_json::to_vec(document).unwrap()) {
    if let Some(bytes)=value.get("data").and_then(|v|v.as_str()).and_then(|s|quai_primitives::get_bytes(s).ok()) {
     if let Ok(call)=interface.parse_call(&bytes){assert_eq!(call.function.encode_call(&call.arguments).unwrap(),bytes);assert_eq!(call.function.decode_call_named(&bytes).unwrap().values(),call.arguments);}
     let _=interface.parse_revert(&bytes);
     if let Some(topics)=value.get("topics").and_then(|v|v.as_array()) {
      if topics.len()<=5 {if let Ok(topics)=topics.iter().map(|v|v.as_str().ok_or(()).and_then(|s|s.parse::<quai_primitives::Hash32>().map_err(|_|()))).collect::<Result<Vec<_>,_>>() {let _=interface.parse_log(&topics,&bytes);}}
     }
    }
    if let Some(criteria)=value.get("criteria").and_then(|v|v.as_array()) {
     if criteria.len()<=1025 {
      let filters=criteria.iter().map(|v|if v.is_null(){Ok(quai_abi::AbiFilterValue::Any)}else if let Some(exact)=v.get("exact"){Ok(quai_abi::AbiFilterValue::Exact(exact))}else{v.get("anyOf").and_then(|v|v.as_array()).map(|v|quai_abi::AbiFilterValue::AnyOf(v)).ok_or(())}).collect::<Result<Vec<_>,_>>();
      if let Ok(filters)=filters {for event in interface.events().take(3) {if let Ok(topics)=event.encode_filter_topics(&filters){assert!(topics.len()<=4);assert_ne!(topics.last(),Some(&quai_abi::AbiFilterTopic::Any));}}}
     }
    }
   }
  }
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
 let _=AbiInterface::default().parse_revert(data);
 if let Ok(interface)=AbiInterface::from_json(data){
  for style in [AbiFormat::Full,AbiFormat::Minimal,AbiFormat::Json,AbiFormat::Signature] {
   for f in interface.functions().take(3) {let _=f.format(style);}
   for e in interface.errors().take(3) {let _=e.format(style);}
   for e in interface.events().take(3) {let _=e.format(style);}
   if let Some(c)=interface.constructor(){let _=c.format(style);}
  }

  if let Ok(json)=interface.format_json(){let restored=AbiInterface::from_json(json.as_bytes()).unwrap();assert_eq!(restored.format_json().unwrap(),json);}
  let _=interface.format_human_readable(false);let _=interface.format_human_readable(true);
 }
 if let Ok(text)=std::str::from_utf8(data){
  if let Ok(interface)=AbiInterface::from_human_readable(&[text]){
   let json=interface.format_json().unwrap();let restored=AbiInterface::from_json(json.as_bytes()).unwrap();
   for minimal in [false,true]{let formatted=interface.format_human_readable(minimal).unwrap();assert_eq!(formatted,restored.format_human_readable(minimal).unwrap());
    let refs:Vec<_>=formatted.iter().map(String::as_str).collect();if let Ok(again)=AbiInterface::from_human_readable(&refs){assert_eq!(again.format_human_readable(minimal).unwrap(),formatted);}
   }
  }
 }
 let _=SolidityArtifact::from_json(data);
 if let Ok(doc)=TypedData::from_json(data){if let Ok(rpc)=doc.to_rpc_json(){assert_eq!(TypedData::from_json(rpc.as_bytes()).unwrap().signing_hash(),doc.signing_hash());}}
 for expression in ["uint256","bytes","string","uint256[]","(address,bytes,uint256[])","bytes[][]","uint256[0][]"]{
  let ty=AbiType::parse(expression).unwrap();
  if let Ok(values)=AbiCoder::decode(std::slice::from_ref(&ty),data){assert_eq!(AbiCoder::encode(&[ty],&values).unwrap(),data);}
 }
});
