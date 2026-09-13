#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_keystore::{Keystore,KdfLimits};
use quai_payments::PaymentCode;
use quai_wallet::{Mnemonic,Language,ExtendedPrivateKey,ExtendedPublicKey};
use quai_crypto::{PublicKey,RecoverableSignature,SchnorrSignature};
fuzz_target!(|data:&[u8]| {
 if data.len()>65_536{return;}
 if data.first()==Some(&b'{') {
  use quai_wallet::full_backup::legacy::{import_quais_json,export_quais_json,LegacyWalletIdentity};
  static IDENTITY:std::sync::OnceLock<(Mnemonic,ExtendedPublicKey,ExtendedPublicKey)>=std::sync::OnceLock::new();
  let (mnemonic,quai,qi)=IDENTITY.get_or_init(||{
   let mnemonic=Mnemonic::from_entropy(Language::English,&[0;16]).unwrap();
   let quai=quai_wallet::HdWallet::from_mnemonic(&mnemonic,"",quai_wallet::CoinType::Quai).unwrap().root_public_key();
   let qi=quai_wallet::HdWallet::from_mnemonic(&mnemonic,"",quai_wallet::CoinType::Qi).unwrap().root_public_key();
   (mnemonic,quai,qi)
  });
  let scope=quai_wallet::discovery::NetworkScope{chain_id:quai_consensus::U256::from(9),genesis:quai_primitives::Hash32::from_bytes([1;32]),zone:quai_primitives::Zone::Cyprus1};
  for (coin,root) in [(quai_wallet::CoinType::Quai,quai),(quai_wallet::CoinType::Qi,qi)] {
   if let Ok(backup)=import_quais_json(data,scope,LegacyWalletIdentity{language:Language::English,passphrase:"",expected_root:root}) {
    if let Ok(json)=export_quais_json(&backup,scope,coin,mnemonic) {
     let again=import_quais_json(json.expose().as_bytes(),scope,LegacyWalletIdentity{language:Language::English,passphrase:"",expected_root:root}).unwrap();
     assert_eq!(again.scopes(),backup.scopes());assert_eq!(again.payment_exposures().count(),backup.payment_exposures().count());
    }
   }
  }
 }

 if data.starts_with(b"QQICUBK1") {
  let scope=quai_wallet::discovery::NetworkScope{chain_id:quai_consensus::U256::from(15000),genesis:quai_primitives::Hash32::from_bytes([1;32]),zone:quai_primitives::Zone::Cyprus1};
  let identity=quai_primitives::Hash32::from_bytes([7;32]);
  if let Ok(mut book)=quai_wallet::qi_custody::QiOperationBook::from_state(data,scope,identity){
   assert_eq!(book.export_state().unwrap(),data);assert_eq!(book.scope(),scope);assert_eq!(book.identity(),identity);
   static KEYS:std::sync::OnceLock<Vec<quai_crypto::SecretKey>>=std::sync::OnceLock::new();
   let keys=KEYS.get_or_init(||{
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../../test-infra/fixtures/qi-custody.json")).unwrap();
    let mut keys=std::collections::BTreeMap::new();
    for row in fixture["vectors"].as_array().unwrap(){for key in row["publicTestSecrets"].as_array().unwrap(){
     let bytes=quai_primitives::get_bytes(key.as_str().unwrap()).unwrap();let key=quai_crypto::SecretKey::from_bytes(bytes.as_slice().try_into().unwrap()).unwrap();keys.insert(key.public_key().address(),key);
    }}keys.into_values().collect()
   });
   // Fixed public toy ownership proofs only; arbitrary valid public origins may be unowned.
   let origins=keys.iter().map(quai_wallet::full_backup::BackupOrigin::from_private_key).collect();
   if let Ok(backup)=quai_wallet::full_backup::WalletBackup::capture_portable(quai_wallet::full_backup::PortableWalletCapture{qi:&[&book],..Default::default()},origins){
    let backup=quai_wallet::full_backup::WalletBackup::capture_portable(quai_wallet::full_backup::PortableWalletCapture{qi:&[&book],previous_inventory:Some(&backup),..Default::default()},keys.iter().map(quai_wallet::full_backup::BackupOrigin::from_private_key).collect()).unwrap();
    let restored=quai_wallet::qi_custody::QiOperationBook::from_backup(&backup,scope,identity).unwrap();
    let report=book.merge_backup(&backup).unwrap();assert_eq!(report.operations_added,0);assert_eq!(report.candidates_added,0);assert_eq!(book.export_state().unwrap(),restored.export_state().unwrap());
   }
  }
 }
 if data.starts_with(b"QACCTBK1") {
  // Fixed public toy owner; no attacker-selected derivation or KDF.
  let mut secret=[0;32];secret[30]=3;secret[31]=0x25;
  let key=quai_crypto::SecretKey::from_bytes(&secret).unwrap();
  let owner=key.public_key();
  let scope=quai_wallet::discovery::NetworkScope{chain_id:quai_consensus::U256::from(9),genesis:quai_primitives::Hash32::from_bytes([1;32]),zone:quai_primitives::Zone::Cyprus1};
  if let Ok(mut book)=quai_wallet::account_custody::AccountOperationBook::from_state(data,scope,owner){
   assert_eq!(book.export_state().unwrap(),data);assert_eq!(book.scope(),scope);
   // Ownership proof and public-state merge only; never execute a password KDF.
   let backup=quai_wallet::full_backup::WalletBackup::capture_portable(quai_wallet::full_backup::PortableWalletCapture{accounts:&[quai_wallet::full_backup::AccountCustodyCapture{book:&book,address:&quai_wallet::metadata::PublicAddress::imported(&owner).unwrap()}],..Default::default()},vec![quai_wallet::full_backup::BackupOrigin::from_private_key(&key)]).unwrap();
   let backup=quai_wallet::full_backup::WalletBackup::capture_portable(quai_wallet::full_backup::PortableWalletCapture{accounts:&[quai_wallet::full_backup::AccountCustodyCapture{book:&book,address:&quai_wallet::metadata::PublicAddress::imported(&owner).unwrap()}],previous_inventory:Some(&backup),..Default::default()},vec![quai_wallet::full_backup::BackupOrigin::from_private_key(&key)]).unwrap();
   let restored=quai_wallet::account_custody::AccountOperationBook::from_backup(&backup,scope,owner).unwrap();
   let report=book.merge_backup(&backup).unwrap();assert_eq!(report.operations_added,0);assert_eq!(report.candidates_added,0);assert_eq!(book.export_state().unwrap(),restored.export_state().unwrap());
  }
 }
 if data.starts_with(b"QPAYABK") {
  // Public toy fixture keys only; fixed-cost derivation, no attacker-selected KDF.
  static OWNER:std::sync::OnceLock<quai_payments::PrivatePaymentCode>=std::sync::OnceLock::new();
  static PEER:std::sync::OnceLock<PaymentCode>=std::sync::OnceLock::new();
  let owner=OWNER.get_or_init(||quai_payments::PrivatePaymentCode::from_seed(&[1;16],0).unwrap());
  let peer=PEER.get_or_init(||quai_payments::PrivatePaymentCode::from_seed(&[2;16],0).unwrap().public_code().clone());
  for zone in [0x00,0x11,0x22] { for direction in [quai_payments::PaymentDirection::Send,quai_payments::PaymentDirection::Receive] {
   let scope=quai_wallet::discovery::NetworkScope{chain_id:quai_consensus::U256::from(15000),genesis:quai_primitives::Hash32::from_bytes([1;32]),zone:quai_primitives::Zone::from_byte(zone).unwrap()};
   if let Ok(book)=quai_wallet::payment_allocation::PaymentAllocationBook::from_state(data,scope,owner,peer.clone(),direction){
    assert_eq!(book.export_state(),data);assert_eq!(book.scope(),scope);
    let backup=quai_wallet::full_backup::WalletBackup::capture_portable(quai_wallet::full_backup::PortableWalletCapture{payments:&[&book],..Default::default()},vec![quai_wallet::full_backup::BackupOrigin::from_seed(&[1;16]).unwrap()]).unwrap();
    let restored=quai_wallet::payment_allocation::PaymentAllocationBook::from_backup(&backup,scope,owner,peer.clone(),direction).unwrap();assert_eq!(restored.next_index(),book.next_index());
    let mut live=book.clone();let count=live.allocations().count();live.merge_backup(owner,&backup).unwrap();assert_eq!(live.next_index(),book.next_index());assert_eq!(live.allocations().count(),count);
    assert_eq!(quai_wallet::payment_allocation::PaymentAllocationBook::from_state(&live.export_state(),scope,owner,peer.clone(),direction).unwrap().export_state(),live.export_state());
    let again=quai_wallet::full_backup::WalletBackup::capture_portable(quai_wallet::full_backup::PortableWalletCapture{payments:&[&restored],previous_inventory:Some(&backup),..Default::default()},vec![quai_wallet::full_backup::BackupOrigin::from_seed(&[1;16]).unwrap()]).unwrap();
    assert_eq!(again.payment_exposures().count(),backup.payment_exposures().count());
    assert_eq!(quai_wallet::payment_allocation::PaymentAllocationBook::from_backup(&again,scope,owner,peer.clone(),direction).unwrap().next_index(),book.next_index());
   }
  }}
 }
 if data.starts_with(b"QADDRBK") {
  static DESCRIPTORS: std::sync::OnceLock<Vec<(quai_wallet::discovery::NetworkScope,quai_wallet::AccountPublic)>>=std::sync::OnceLock::new();
  let descriptors=DESCRIPTORS.get_or_init(|| {
   let fixture:serde_json::Value=serde_json::from_str(include_str!("../../test-infra/fixtures/address-allocation.json")).unwrap();
   fixture["vectors"].as_array().unwrap().iter().map(|row|{
    let coin=if row["coin"]==969 {quai_wallet::CoinType::Qi}else{quai_wallet::CoinType::Quai};
    let account=quai_wallet::AccountPublic::import(row["accountXpub"].as_str().unwrap(),coin,row["account"].as_u64().unwrap() as u32).unwrap();
    let scope=quai_wallet::discovery::NetworkScope{chain_id:quai_consensus::U256::from(15000),genesis:quai_primitives::Hash32::from_bytes([1;32]),zone:quai_primitives::Zone::from_byte(row["zone"].as_u64().unwrap() as u8).unwrap()};(scope,account)
   }).collect()
  });
  for (scope,account) in descriptors {if let Ok(mut book)=quai_wallet::allocation::AddressAllocationBook::from_state(data,*scope,account.clone()){
   assert_eq!(book.export_state(),data);assert_eq!(book.scope(),*scope);
   static SEED:std::sync::OnceLock<Vec<u8>>=std::sync::OnceLock::new();
   let seed=SEED.get_or_init(||{let f:serde_json::Value=serde_json::from_str(include_str!("../../test-infra/fixtures/allocation-merge.json")).unwrap();quai_primitives::get_bytes(f["hdSeed"].as_str().unwrap()).unwrap()});
   let backup=quai_wallet::full_backup::WalletBackup::capture_portable(quai_wallet::full_backup::PortableWalletCapture{allocations:&[&book],..Default::default()},vec![quai_wallet::full_backup::BackupOrigin::from_seed(seed).unwrap()]).unwrap();
   let count=book.allocations().count();let next=[book.next_index(false),book.next_index(true)];book.merge_backup(&backup).unwrap();assert_eq!(book.allocations().count(),count);assert_eq!([book.next_index(false),book.next_index(true)],next);
   assert_eq!(quai_wallet::allocation::AddressAllocationBook::from_state(&book.export_state(),*scope,account.clone()).unwrap().export_state(),book.export_state());
  }}
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
