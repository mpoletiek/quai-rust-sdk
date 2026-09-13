//! Canonical bounded public ancestry cursor. Storage is not a consensus proof.
use super::*;
const MAGIC: &[u8; 8] = b"QHEAD001";
const HEADER: usize = 47;
/// Maximum encoded public head cursor: 4,096 numbered hashes and fixed metadata.
pub const MAX_HEAD_STATE_BYTES: usize = HEADER + 40 * 4096;

impl HeadTracker {
    /// Encode the retained public ancestry, zone, genesis and replay bounds.
    /// Persist atomically with the application of the corresponding head update.
    /// The bytes contain no keys and are not authenticated chain evidence.
    pub fn export_state(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER + self.anchors.len() * 40);
        bytes.extend_from_slice(MAGIC);
        bytes.push(self.zone.byte());
        bytes.extend_from_slice(self.genesis.bytes());
        for value in [self.retain, self.page_size, self.anchors.len()] {
            bytes.extend_from_slice(&(value as u16).to_be_bytes());
        }
        for anchor in &self.anchors {
            bytes.extend_from_slice(&anchor.number.to_be_bytes());
            bytes.extend_from_slice(anchor.hash.bytes());
        }
        bytes
    }
    /// Restore canonical bounded ancestry against an explicit expected network.
    /// Rejects malformed, noncontiguous or duplicate anchors before returning a
    /// cursor. `poll` revalidates genesis and a common canonical anchor before
    /// accepting further updates. Local storage integrity remains caller-owned.
    pub fn from_state(
        bytes: &[u8],
        expected_zone: Zone,
        expected_genesis: Hash32,
    ) -> Result<Self, ProviderError> {
        let invalid = || ProviderError::InvalidRequest("invalid head replay state");
        if !(HEADER + 40..=MAX_HEAD_STATE_BYTES).contains(&bytes.len())
            || &bytes[..8] != MAGIC
            || bytes[8] != expected_zone.byte()
            || &bytes[9..41] != expected_genesis.bytes()
        {
            return Err(invalid());
        }
        let retain = u16::from_be_bytes([bytes[41], bytes[42]]) as usize;
        let page_size = u16::from_be_bytes([bytes[43], bytes[44]]) as usize;
        let count = u16::from_be_bytes([bytes[45], bytes[46]]) as usize;
        if !(2..=4096).contains(&retain)
            || !(1..=256).contains(&page_size)
            || count == 0
            || count > retain
            || bytes.len() != HEADER + count * 40
        {
            return Err(invalid());
        }
        let mut anchors = VecDeque::with_capacity(count);
        let mut seen = BTreeSet::new();
        let mut previous: Option<u64> = None;
        for entry in bytes[HEADER..].chunks_exact(40) {
            let number = u64::from_be_bytes(entry[..8].try_into().map_err(|_| invalid())?);
            let hash = Hash32::from_bytes(entry[8..].try_into().map_err(|_| invalid())?);
            if hash == Hash32::ZERO
                || !seen.insert(hash)
                || (number == 0 && hash != expected_genesis)
                || previous.is_some_and(|old| old.checked_add(1) != Some(number))
            {
                return Err(invalid());
            }
            anchors.push_back(BlockReference { number, hash });
            previous = Some(number);
        }
        // Also applies the constructor's trusted identity validation.
        let mut result = Self::new(
            expected_zone,
            expected_genesis,
            *anchors.front().ok_or_else(invalid)?,
            retain,
            page_size,
        )?;
        result.anchors = anchors;
        Ok(result)
    }
}
