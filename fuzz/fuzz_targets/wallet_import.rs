#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_keystore::{Keystore,KdfLimits};
use quai_payments::PaymentCode;
use quai_wallet::{Mnemonic,Language,ExtendedPrivateKey,ExtendedPublicKey};
use quai_crypto::{PublicKey,RecoverableSignature,SchnorrSignature};
fuzz_target!(|data:&[u8]| {
 if data.len()>65_536{return;}
 if data.starts_with(b"QPAYABK1") {
  // Public toy fixture keys only; fixed-cost derivation, no attacker-selected KDF.
  static OWNER:std::sync::OnceLock<quai_payments::PrivatePaymentCode>=std::sync::OnceLock::new();
  static PEER:std::sync::OnceLock<PaymentCode>=std::sync::OnceLock::new();
  let owner=OWNER.get_or_init(||quai_payments::PrivatePaymentCode::from_seed(&[1;16],0).unwrap());
  let peer=PEER.get_or_init(||quai_payments::PrivatePaymentCode::from_seed(&[2;16],0).unwrap().public_code().clone());
  for zone in [0x00,0x11,0x22] { for direction in [quai_payments::PaymentDirection::Send,quai_payments::PaymentDirection::Receive] {
   let scope=quai_wallet::discovery::NetworkScope{chain_id:quai_consensus::U256::from(15000),genesis:quai_primitives::Hash32::from_bytes([1;32]),zone:quai_primitives::Zone::from_byte(zone).unwrap()};
   if let Ok(book)=quai_wallet::payment_allocation::PaymentAllocationBook::from_state(data,scope,owner,peer.clone(),direction){assert_eq!(book.export_state(),data);assert_eq!(book.scope(),scope);}
  }}
 }
 if data.starts_with(b"QADDRBK1") {
  static DESCRIPTORS: std::sync::OnceLock<Vec<(quai_wallet::discovery::NetworkScope,quai_wallet::AccountPublic)>>=std::sync::OnceLock::new();
  let descriptors=DESCRIPTORS.get_or_init(|| {
   let fixture:serde_json::Value=serde_json::from_str(include_str!("../../test-infra/fixtures/address-allocation.json")).unwrap();
   fixture["vectors"].as_array().unwrap().iter().map(|row|{
    let coin=if row["coin"]==969 {quai_wallet::CoinType::Qi}else{quai_wallet::CoinType::Quai};
    let account=quai_wallet::AccountPublic::import(row["accountXpub"].as_str().unwrap(),coin,row["account"].as_u64().unwrap() as u32).unwrap();
    let scope=quai_wallet::discovery::NetworkScope{chain_id:quai_consensus::U256::from(15000),genesis:quai_primitives::Hash32::from_bytes([1;32]),zone:quai_primitives::Zone::from_byte(row["zone"].as_u64().unwrap() as u8).unwrap()};(scope,account)
   }).collect()
  });
  for (scope,account) in descriptors {if let Ok(book)=quai_wallet::allocation::AddressAllocationBook::from_state(data,*scope,account.clone()){assert_eq!(book.export_state(),data);assert_eq!(book.scope(),*scope);}}
 }
 if let Ok(backup)=quai_wallet::full_backup::EncryptedWalletBackup::from_bytes(data){assert_eq!(backup.as_bytes(),data);}
 let _=Keystore::from_json(data,KdfLimits::default()); // no attacker-selected expensive KDF execution
 if let Ok(code)=PaymentCode::from_bytes(data){assert_eq!(PaymentCode::from_base58(&code.to_base58()).unwrap(),code);}
 if let Ok(address)=quai_wallet::metadata::PublicAddress::from_metadata(data){assert_eq!(address.export_metadata(),data);assert_eq!(PublicKey::from_sec1_bytes(address.public_key()).unwrap().address(),address.address());}
 let _=PublicKey::from_sec1_bytes(data);
 if let Ok(signature)=<&[u8;65]>::try_from(data){let _=RecoverableSignature::from_quais_bytes(signature);}
 if let Ok(signature)=<&[u8;64]>::try_from(data){let _=SchnorrSignature::from_bytes(signature);}
 if let Ok(text)=std::str::from_utf8(data){
  let _=PaymentCode::from_base58(text);
  let _=ExtendedPrivateKey::import(text);
  let _=ExtendedPublicKey::import(text);
  if data.len()<=4096{let _=Mnemonic::parse(Language::English,text);}
 }
});
