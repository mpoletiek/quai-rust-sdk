//! Strict bounded legacy schema. Secret text and partially built outputs are guarded.
use super::*;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

pub(super) struct Text(pub Zeroizing<String>);
impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct TextVisitor;
        impl Visitor<'_> for TextVisitor {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded text")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Text, E> {
                if v.len() > 4096 {
                    return Err(E::custom("text limit"));
                }
                Ok(Text(Zeroizing::new(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Text, E> {
                let v = Zeroizing::new(v);
                if v.len() > 4096 {
                    return Err(E::custom("text limit"));
                }
                Ok(Text(v))
            }
        }
        d.deserialize_string(TextVisitor)
    }
}
impl Serialize for Text {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
#[derive(Serialize)]
#[serde(transparent)]
pub(super) struct List<T>(pub Vec<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for List<T> {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct ListVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for ListVisitor<T> {
            type Value = List<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded address list")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut out = vec![];
                while let Some(item) = a.next_element()? {
                    if out.len() >= MAX_LEGACY_ADDRESSES {
                        return Err(de::Error::custom("address limit"));
                    }
                    out.push(item);
                }
                Ok(List(out))
            }
        }
        d.deserialize_seq(ListVisitor(PhantomData))
    }
}
#[derive(Serialize)]
#[serde(transparent)]
pub(super) struct Channels(pub BTreeMap<String, List<Row>>);
impl<'de> Deserialize<'de> for Channels {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct ChannelsVisitor;
        impl<'de> Visitor<'de> for ChannelsVisitor {
            type Value = Channels;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unique bounded channel map")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Channels, A::Error> {
                let mut out = BTreeMap::new();
                let mut total = 0usize;
                while let Some(key) = a.next_key::<String>()? {
                    if key.len() > 128 || out.len() >= MAX_LEGACY_CHANNELS || out.contains_key(&key)
                    {
                        return Err(de::Error::custom("channel limit or duplicate"));
                    }
                    let value: List<Row> = a.next_value()?;
                    total = total.saturating_add(value.0.len());
                    if total > MAX_LEGACY_ADDRESSES {
                        return Err(de::Error::custom("address limit"));
                    }
                    out.insert(key, value);
                }
                Ok(Channels(out))
            }
        }
        d.deserialize_map(ChannelsVisitor)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Document {
    pub version: u32,
    pub phrase: Text,
    pub coin_type: u16,
    pub addresses: List<Row>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_payment_code_info: Option<Channels>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Row {
    pub pub_key: String,
    pub address: String,
    pub account: u32,
    pub index: i64,
    pub zone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derivation_path: Option<Text>,
    #[serde(default)]
    pub last_synced_block: Option<LegacyCheckpoint>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum Status {
    Unknown,
    Unused,
    Used,
    AttemptedUse,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyCheckpoint {
    pub hash: String,
    pub number: u64,
}
pub(super) fn parse(bytes: &[u8]) -> Result<Document> {
    if bytes.len() > MAX_LEGACY_JSON_BYTES {
        return Err(WalletBackupError::InvalidInput);
    }
    let doc: Document =
        serde_json::from_slice(bytes).map_err(|_| WalletBackupError::InvalidInput)?;
    let count = doc.addresses.0.len()
        + doc
            .sender_payment_code_info
            .as_ref()
            .map(|c| c.0.values().map(|v| v.0.len()).sum::<usize>())
            .unwrap_or(0);
    if count > MAX_LEGACY_ADDRESSES {
        return Err(WalletBackupError::InvalidInput);
    }
    Ok(doc)
}
pub(super) fn encode(doc: &Document) -> Result<crate::SecretString> {
    struct Writer(Zeroizing<Vec<u8>>);
    impl std::io::Write for Writer {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(b.len()) > MAX_LEGACY_JSON_BYTES {
                return Err(std::io::ErrorKind::FileTooLarge.into());
            }
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(MAX_LEGACY_JSON_BYTES)
        .map_err(|_| WalletBackupError::Resources)?;
    let mut writer = Writer(Zeroizing::new(bytes));
    serde_json::to_writer(&mut writer, doc).map_err(|_| WalletBackupError::InvalidInput)?;
    // JSON is UTF-8; transfer the allocation instead of making an unguarded copy.
    let bytes = std::mem::take(&mut *writer.0);
    let text = String::from_utf8(bytes).map_err(|error| {
        let _guard = Zeroizing::new(error.into_bytes());
        WalletBackupError::InvalidInput
    })?;
    Ok(crate::SecretString(Zeroizing::new(text)))
}
