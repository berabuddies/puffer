//! Minimal DER reader for the small, fixed ASN.1 shapes used by Firefox NSS
//! credential decryption.
//!
//! This is deliberately *not* a general-purpose ASN.1 library: it understands
//! only definite-length encodings and the handful of tags the Firefox key
//! material uses (SEQUENCE, OCTET STRING, OBJECT IDENTIFIER, INTEGER). Keeping
//! it tiny avoids a heavyweight dependency for a structure we fully control.

use anyhow::{bail, Context, Result};

pub(crate) const TAG_INTEGER: u8 = 0x02;
pub(crate) const TAG_OCTET_STRING: u8 = 0x04;
pub(crate) const TAG_OID: u8 = 0x06;
pub(crate) const TAG_SEQUENCE: u8 = 0x30;

/// One parsed DER TLV node, borrowing its contents from the source slice.
pub(crate) struct Der<'a> {
    pub(crate) tag: u8,
    pub(crate) contents: &'a [u8],
}

impl<'a> Der<'a> {
    /// Reads the node's contents as a nested sequence of TLV children.
    pub(crate) fn reader(&self) -> DerReader<'a> {
        DerReader::new(self.contents)
    }

    /// Interprets the contents as a big-endian unsigned integer (small values only).
    pub(crate) fn as_usize(&self) -> Result<usize> {
        let mut value = 0usize;
        for &byte in self.contents {
            value = value
                .checked_shl(8)
                .and_then(|shifted| shifted.checked_add(byte as usize))
                .context("DER: integer too large")?;
        }
        Ok(value)
    }
}

/// Cursor over a DER byte slice yielding TLV nodes in order.
pub(crate) struct DerReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> DerReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Reads the next TLV node, advancing the cursor past it.
    pub(crate) fn next(&mut self) -> Result<Der<'a>> {
        let tag = *self.data.get(self.pos).context("DER: unexpected end (tag)")?;
        self.pos += 1;
        let first = *self.data.get(self.pos).context("DER: unexpected end (length)")?;
        self.pos += 1;
        let len = if first & 0x80 == 0 {
            first as usize
        } else {
            let count = (first & 0x7f) as usize;
            if count == 0 || count > 4 {
                bail!("DER: unsupported length form ({count} bytes)");
            }
            let mut len = 0usize;
            for _ in 0..count {
                let byte = *self
                    .data
                    .get(self.pos)
                    .context("DER: unexpected end (long length)")?;
                self.pos += 1;
                len = (len << 8) | byte as usize;
            }
            len
        };
        let end = self
            .pos
            .checked_add(len)
            .context("DER: length overflow")?;
        let contents = self
            .data
            .get(self.pos..end)
            .context("DER: truncated contents")?;
        self.pos = end;
        Ok(Der { tag, contents })
    }

    /// Reads the next node and asserts it carries the expected tag.
    pub(crate) fn expect(&mut self, tag: u8) -> Result<Der<'a>> {
        let node = self.next()?;
        if node.tag != tag {
            bail!("DER: expected tag {tag:#04x}, found {:#04x}", node.tag);
        }
        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_short_form_sequence_with_children() {
        // SEQUENCE { INTEGER 1, OCTET STRING "ab" }
        let data = [0x30, 0x07, 0x02, 0x01, 0x01, 0x04, 0x02, b'a', b'b'];
        let mut reader = DerReader::new(&data);
        let seq = reader.expect(TAG_SEQUENCE).unwrap();
        let mut inner = seq.reader();
        assert_eq!(inner.expect(TAG_INTEGER).unwrap().as_usize().unwrap(), 1);
        assert_eq!(inner.expect(TAG_OCTET_STRING).unwrap().contents, b"ab");
    }

    #[test]
    fn reads_long_form_length() {
        // OCTET STRING of 200 bytes uses one long-form length byte (0x81 0xC8).
        let mut data = vec![0x04, 0x81, 0xC8];
        data.extend(std::iter::repeat(0x41).take(200));
        let mut reader = DerReader::new(&data);
        let node = reader.expect(TAG_OCTET_STRING).unwrap();
        assert_eq!(node.contents.len(), 200);
    }

    #[test]
    fn rejects_truncated_contents() {
        let data = [0x04, 0x05, 0x41]; // claims 5 bytes, only 1 present
        let mut reader = DerReader::new(&data);
        assert!(reader.next().is_err());
    }
}
