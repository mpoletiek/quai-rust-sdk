//! Shared bounded public custody framing; each book validates its semantic records.
use crate::metadata::StorageError;
use quai_consensus::MAX_TRANSACTION_BYTES;
type Result<T> = std::result::Result<T, StorageError>;
pub(crate) const MAX_CUSTODY_BYTES: usize = 16 * 1024 * 1024;
pub(crate) struct Writer(pub(crate) Vec<u8>);
impl Writer {
    pub(crate) fn put(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_CUSTODY_BYTES - self.0.len() {
            return Err(StorageError::Invalid);
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    pub(crate) fn blob(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_TRANSACTION_BYTES {
            return Err(StorageError::Invalid);
        }
        self.put(&(bytes.len() as u32).to_be_bytes())?;
        self.put(bytes)
    }
}
pub(crate) struct Reader<'a>(pub(crate) &'a [u8]);
impl<'a> Reader<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(n).ok_or(StorageError::Invalid)?;
        self.0 = tail;
        Ok(head)
    }
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| StorageError::Invalid)
    }
    pub(crate) fn byte(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    pub(crate) fn flag(&mut self) -> Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StorageError::Invalid),
        }
    }
    pub(crate) fn blob(&mut self) -> Result<&'a [u8]> {
        let n = u32::from_be_bytes(self.array()?) as usize;
        if n > MAX_TRANSACTION_BYTES {
            return Err(StorageError::Invalid);
        }
        self.take(n)
    }
}
