use std::{
    fmt::Display,
    io::{Read, Seek, SeekFrom, Write},
};

use crate::varint::{read_variable_unsigned_integer, write_variable_unsigned_integer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId(pub u64);

impl From<u64> for RecordId {
    fn from(value: u64) -> Self {
        RecordId(value)
    }
}

impl Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A record in the store.
///
/// This struct stores the fundamental information to manage each record in our
/// store.
///
/// In physical memory, a record has the following shape:
/// ```plain
/// [header][payload][next_offset]
/// ```
/// where [`header`](Self::header) and [`next_offset`](Self::next_offset) do not
/// occupy a fixed amount of memory when serialized, so we keep track of their
/// current size in [`Self::header_len`] and [`Self::next_offset_len`]
/// respectively.
pub struct Record {
    /// The position of the record in the stream
    pub stream_position: u64,
    /// The header data.
    pub header: RecordHeader,
    /// The current physical size of the record header ([`Self::header`]).
    pub header_len: u64,
    pub next_offset: u64,
    /// The current physical size of the offset to the next record ([`Self::header`]).
    pub next_offset_len: u64,
}

impl Record {
    pub fn id(&self) -> RecordId {
        self.header.id
    }

    pub(super) fn trailer_position(&self) -> std::io::Result<u64> {
        self.stream_position
            .checked_add(self.header_len)
            .and_then(|n| n.checked_add(self.header.payload_len))
            .ok_or_else(crate::position_overflow)
    }

    pub(super) fn end_position(&self) -> std::io::Result<u64> {
        self.trailer_position()?
            .checked_add(self.next_offset_len)
            .ok_or_else(crate::position_overflow)
    }

    // The reader starts at the storage header; links are relative to their own start.
    pub fn load_all<R: Read + Seek>(mut r: R) -> impl Iterator<Item = std::io::Result<Self>> {
        let mut next = read_stream_header(&mut r);
        std::iter::from_fn(move || {
            // Get the position of the next record.
            let position = match std::mem::replace(&mut next, Ok(None)) {
                Ok(Some(position)) => position,
                // no next record, stop
                Ok(None) => return None,
                // Error while reading next record position, error then stop
                Err(error) => return Some(Err(error)),
            };

            let (result, following) = match Self::read_next(&mut r, position) {
                Ok(r) => r,
                Err(e) => return Some(Err(e)),
            };
            next = following;
            Some(Ok(result))
        })
    }

    fn read_next(
        r: &mut (impl Read + Seek),
        position: u64,
    ) -> std::io::Result<(Self, std::io::Result<Option<u64>>)> {
        r.seek(SeekFrom::Start(position))?;
        let header = RecordHeader::read(r)?;
        let header_len = r.stream_position()?.checked_sub(position).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid header position")
        })?;
        let trailer = position
            .checked_add(header_len)
            .and_then(|n| n.checked_add(u64::from(header.payload_len)))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "position overflow")
            })?;
        r.seek(SeekFrom::Start(trailer))?;
        let next_offset = u64::try_from(read_variable_unsigned_integer(r)?).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid next offset")
        })?;
        let offset_len = r.stream_position()?.checked_sub(trailer).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid offset position")
        })?;
        let following = if next_offset == 0 {
            None
        } else {
            if u64::from(next_offset) < offset_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "overlapping records",
                ));
            }
            Some(trailer.checked_add(u64::from(next_offset)).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "position overflow")
            })?)
        };
        Ok((
            Self {
                stream_position: position,
                header_len,
                next_offset_len: offset_len,
                next_offset,
                header,
            },
            Ok(following),
        ))
    }
}

/// The header of a record.
///
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordHeader {
    pub id: RecordId,
    pub payload_len: u64,
}

impl RecordHeader {
    pub fn encoded_len(id: RecordId, payload_len: u64) -> std::io::Result<u64> {
        let mut bytes = Vec::new();
        write_variable_unsigned_integer(id.0 as u128, &mut bytes)?;
        write_variable_unsigned_integer(payload_len as u128, &mut bytes)?;
        Ok(bytes.len() as u64)
    }

    pub fn write(&self, w: &mut impl Write) -> std::io::Result<()> {
        write_variable_unsigned_integer(self.id.0 as u128, w)?;
        write_variable_unsigned_integer(self.payload_len as u128, w)
    }

    pub fn read(r: &mut impl Read) -> std::io::Result<Self> {
        let id = u64::try_from(read_variable_unsigned_integer(r)?).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid record id")
        })?;
        let payload_len = u64::try_from(read_variable_unsigned_integer(r)?).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid payload length")
        })?;
        Ok(Self {
            id: RecordId(id),
            payload_len,
        })
    }
}

fn read_stream_header(r: &mut (impl Read + Seek)) -> std::io::Result<Option<u64>> {
    let base = r.stream_position()?;
    let offset = read_first_record_offset(r)?;
    let header_len = r.stream_position()?.checked_sub(base).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid storage header")
    })?;
    if offset == 0 {
        Ok(None)
    } else {
        if u64::from(offset) < header_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "first record overlaps storage header",
            ));
        }
        base.checked_add(u64::from(offset))
            .map(Some)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "position overflow")
            })
    }
}

fn read_first_record_offset(r: &mut impl Read) -> std::io::Result<u64> {
    read_variable_unsigned_integer(r)
        .map(u64::try_from)?
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid first record offset",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn iterator_stops_after_header_or_record_error() {
        for bytes in [vec![], vec![1]] {
            let mut input = Cursor::new(bytes);
            let mut records = Record::load_all(&mut input);
            assert!(records.next().unwrap().is_err());
            assert!(records.next().is_none());
            assert!(records.next().is_none());
        }
    }

    #[test]
    fn iterator_stops_after_empty_storage() {
        let mut input = Cursor::new(vec![0]);
        let mut records = Record::load_all(&mut input);
        assert!(records.next().is_none());
        assert!(records.next().is_none());
    }
}
