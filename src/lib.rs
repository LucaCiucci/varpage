#![doc = include_str!("../README.md")]
#![doc = include_str!("../docs/format.md")]

use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Seek, SeekFrom, Write},
};

use crate::varint::{read_variable_unsigned_integer, write_variable_unsigned_integer};

pub mod varint;

mod gap_index;
mod record;
mod write_plan;
use gap_index::GapIndex;
pub use record::*;
use write_plan::WritePlan;

/// A store for variable-length records on a seekable stream.
///
/// Changes are applied directly to the backing stream. Crash safety, recovery
/// from interrupted writes, and recovery from write errors are outside the
/// current scope. If a mutation fails or is interrupted, the store may be
/// inconsistent. Rebuild it or restore it from another copy before reuse.
pub struct Storage<T> {
    first_stream_position: u64,
    storage_header_len: u64,
    rw: T,
    records: BTreeMap<u64, Record>,
    record_positions: HashMap<RecordId, u64>,
    gaps: GapIndex,
    zero_free_space: bool,
}

fn position_overflow() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "record position or size overflow",
    )
}

impl<T> Storage<T> {
    /// Borrow the backing stream while keeping this store open.
    ///
    /// For a backing `File`, call [`Self::flush`] before using this reference
    /// to sync the file. Changing stored bytes or truncating the stream through
    /// a shared reference can invalidate the in-memory record index.
    pub fn get_ref(&self) -> &T {
        &self.rw
    }

    /// Consume the store and return its backing stream.
    ///
    /// Call [`Self::flush`] first if you need to flush buffered writes and
    /// handle any errors. Taking the stream back performs no I/O.
    pub fn into_inner(self) -> T {
        self.rw
    }

    pub fn new_empty(mut rw: T) -> std::io::Result<Self>
    where
        T: Seek + Write,
    {
        let first_stream_position = rw.stream_position()?;
        write_variable_unsigned_integer(0, &mut rw)?;
        Ok(Self {
            first_stream_position,
            storage_header_len: 1,
            rw,
            records: BTreeMap::new(),
            record_positions: HashMap::new(),
            gaps: GapIndex::default(),
            zero_free_space: false,
        })
    }

    /// Open storage at the current stream position and index its records.
    pub fn open(mut rw: T) -> std::io::Result<Self>
    where
        T: Read + Seek,
    {
        let first_stream_position = rw.stream_position()?;
        read_variable_unsigned_integer(&mut rw)?;
        let storage_header_len = rw
            .stream_position()?
            .checked_sub(first_stream_position)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid storage header position",
                )
            })?;
        rw.seek(SeekFrom::Start(first_stream_position))?;
        let mut storage = Self {
            first_stream_position,
            storage_header_len,
            rw,
            records: BTreeMap::new(),
            record_positions: HashMap::new(),
            gaps: GapIndex::default(),
            zero_free_space: false,
        };
        let mut gap_start = first_stream_position
            .checked_add(1)
            .ok_or_else(position_overflow)?;
        for record in Record::load_all(&mut storage.rw) {
            let record = record?;
            let pos = record.stream_position;
            if storage
                .record_positions
                .insert(record.header.id, pos)
                .is_some()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate record id",
                ));
            }
            storage.gaps.set(pos, pos - gap_start);
            record.end_position()?;
            gap_start = record.trailer_position()? + 1;
            storage.records.insert(pos, record);
        }
        Ok(storage)
    }

    /// Enable or disable clearing free space after each successful write.
    ///
    /// When enabled, gaps between records and space released by the write are
    /// overwritten with zeroes as they are released. Existing gaps are not
    /// cleared when this setting is enabled. It takes effect on the next write.
    pub fn set_zero_free_space(&mut self, enabled: bool) {
        self.zero_free_space = enabled;
    }

    /// Read a record by ID, returning `None` if it does not exist.
    pub fn read(&mut self, record_id: impl Into<RecordId>) -> std::io::Result<Option<Vec<u8>>>
    where
        T: Read + Seek,
    {
        let Some(&index) = self.record_positions.get(&record_id.into()) else {
            return Ok(None);
        };
        let record = &self.records[&index];
        let position = record
            .stream_position
            .checked_add(record.header_len)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "record position overflow")
            })?;
        let len = usize::try_from(record.header.payload_len).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "payload length exceeds address space",
            )
        })?;
        let mut data = vec![0; len];
        self.rw.seek(SeekFrom::Start(position))?;
        self.rw.read_exact(&mut data)?;
        Ok(Some(data))
    }

    /// Return the absolute stream position just after the last live record.
    ///
    /// This does not remove gaps between records. After `vacuum`, it is the
    /// end of the packed storage and can be used as a truncation boundary.
    pub fn end_position(&self) -> std::io::Result<u64> {
        let overflow =
            || std::io::Error::new(std::io::ErrorKind::InvalidData, "storage position overflow");
        match self.records.last_key_value() {
            Some((_, record)) => record
                .stream_position
                .checked_add(record.header_len)
                .and_then(|n| n.checked_add(record.header.payload_len))
                .and_then(|n| n.checked_add(record.next_offset_len))
                .ok_or_else(overflow),
            None => self
                .first_stream_position
                .checked_add(self.storage_header_len)
                .ok_or_else(overflow),
        }
    }

    /// Pack live records together and return the new absolute end position.
    ///
    /// This does not truncate the underlying stream. If the stream is a file,
    /// the caller may flush the store, take the file back with
    /// [`Self::into_inner`], and truncate it to the returned position.
    /// When zeroing is enabled, the released part of the old record
    /// extent is overwritten with zeroes. An interrupted vacuum or a write
    /// error during vacuum can leave the store inconsistent.
    pub fn vacuum(&mut self) -> std::io::Result<u64>
    where
        T: Read + Write + Seek,
    {
        use std::io::{Cursor, Error, ErrorKind};

        let overflow = || Error::new(ErrorKind::InvalidInput, "record position or size overflow");
        let mut packed = Cursor::new(Vec::new());
        write_variable_unsigned_integer(if self.records.is_empty() { 0 } else { 1 }, &mut packed)?;
        let mut records = Vec::with_capacity(self.records.len());
        let mut old_end = self
            .first_stream_position
            .checked_add(1)
            .ok_or_else(overflow)?;
        for (index, record) in self.records.values().enumerate() {
            let payload_pos = record
                .stream_position
                .checked_add(record.header_len)
                .ok_or_else(overflow)?;
            let record_end = payload_pos
                .checked_add(record.header.payload_len)
                .and_then(|n| n.checked_add(record.next_offset_len))
                .ok_or_else(overflow)?;
            old_end = old_end.max(record_end);
            let len = usize::try_from(record.header.payload_len).map_err(|_| overflow())?;
            let mut data = vec![0; len];
            self.rw.seek(SeekFrom::Start(payload_pos))?;
            self.rw.read_exact(&mut data)?;
            let position = self
                .first_stream_position
                .checked_add(packed.position())
                .ok_or_else(overflow)?;
            let header_start = packed.position();
            record.header.write(&mut packed)?;
            let header_len = packed.position() - header_start;
            packed.write_all(&data)?;
            let next_offset = if index + 1 == self.records.len() {
                0
            } else {
                1
            };
            write_variable_unsigned_integer(next_offset, &mut packed)?;
            records.push(Record {
                stream_position: position,
                header_len,
                next_offset_len: 1,
                next_offset: next_offset as u64,
                header: record.header,
            });
        }
        let end = self
            .first_stream_position
            .checked_add(packed.position())
            .ok_or_else(overflow)?;
        self.rw.seek(SeekFrom::Start(self.first_stream_position))?;
        self.rw.write_all(packed.get_ref())?;
        if self.zero_free_space && end < old_end {
            let zeroes = [0u8; 8192];
            let mut remaining = old_end - end;
            while remaining > 0 {
                let count = remaining.min(zeroes.len() as u64) as usize;
                self.rw.write_all(&zeroes[..count])?;
                remaining -= count as u64;
            }
        }
        self.records = records
            .into_iter()
            .map(|r| (r.stream_position, r))
            .collect();
        self.storage_header_len = 1;
        self.record_positions = self
            .records
            .values()
            .map(|r| (r.header.id, r.stream_position))
            .collect();
        self.gaps = GapIndex::default();
        Ok(end)
    }

    /// Insert or replace a record, using the first free space that fits.
    ///
    /// Existing records stay in place when the replacement and its links fit.
    /// An interruption or write error can leave the store partly updated.
    pub fn write(&mut self, record_id: u64, data: &[u8]) -> std::io::Result<()>
    where
        T: Write + Seek,
    {
        let payload_len = u64::try_from(data.len()).map_err(|_| position_overflow())?;
        let id = RecordId(record_id);
        let plan = self.plan_write(id, payload_len)?;
        self.apply_write(plan, data)
    }

    /// Flush buffered writes through the backing stream.
    ///
    /// This forwards to [`Write::flush`].
    /// For a file, durability requires a separate filesystem sync.
    pub fn flush(&mut self) -> std::io::Result<()>
    where
        T: Write,
    {
        self.rw.flush()
    }

    fn apply_write(&mut self, plan: WritePlan, data: &[u8]) -> std::io::Result<()>
    where
        T: Write + Seek,
    {
        let record = plan.record;
        let position = record.stream_position;
        self.rw.seek(SeekFrom::Start(position))?;
        record.header.write(&mut self.rw)?;
        self.rw.write_all(data)?;
        write_variable_unsigned_integer(record.next_offset.into(), &mut self.rw)?;
        for link in &plan.links {
            self.rw.seek(SeekFrom::Start(link.trailer))?;
            write_variable_unsigned_integer(link.offset.into(), &mut self.rw)?;
        }
        if let Some((offset, _)) = plan.first_link {
            self.rw.seek(SeekFrom::Start(self.first_stream_position))?;
            write_variable_unsigned_integer(offset.into(), &mut self.rw)?;
        }
        for range in plan.released {
            self.rw.seek(SeekFrom::Start(range.start))?;
            let zeroes = [0u8; 8192];
            let mut remaining = range.end - range.start;
            while remaining > 0 {
                let count = remaining.min(zeroes.len() as u64) as usize;
                self.rw.write_all(&zeroes[..count])?;
                remaining -= count as u64;
            }
        }

        if let Some((_, len)) = plan.first_link {
            self.storage_header_len = len;
        }
        if let Some(old) = plan.old_position.filter(|&old| old != position) {
            self.records.remove(&old);
            self.gaps.set(old, 0);
        }
        for link in plan.links {
            let previous = self
                .records
                .get_mut(&link.record_position)
                .expect("existing predecessor");
            previous.next_offset = link.offset;
            previous.next_offset_len = link.len;
        }
        self.record_positions.insert(record.header.id, position);
        self.records.insert(position, record);
        for end in plan.gap_ends {
            let start = self.gap_start(self.predecessor(end, None))?;
            self.gaps.set(end, end - start);
        }
        Ok(())
    }

    fn predecessor(&self, pos: u64, excluded: Option<u64>) -> Option<&Record> {
        self.records
            .range(..pos)
            .rev()
            .map(|(_, r)| r)
            .find(|r| Some(r.stream_position) != excluded)
    }

    fn successor(&self, pos: u64, excluded: Option<u64>) -> Option<&Record> {
        use std::ops::Bound::{Excluded, Unbounded};
        self.records
            .range((Excluded(pos), Unbounded))
            .map(|(_, r)| r)
            .find(|r| Some(r.stream_position) != excluded)
    }

    // Canonical offsets of distance d >= 1 always fit in d bytes. A record
    // placed directly after this boundary needs only a one-byte incoming link.
    fn gap_start(&self, previous: Option<&Record>) -> std::io::Result<u64> {
        previous
            .map_or(Ok(self.first_stream_position), Record::trailer_position)?
            .checked_add(1)
            .ok_or_else(position_overflow)
    }

    /// Iterate over live records in stream order.
    pub fn records(&self) -> impl ExactSizeIterator<Item = &Record> + DoubleEndedIterator {
        self.records.values()
    }

    pub fn record_ids(&self) -> impl ExactSizeIterator<Item = RecordId> + DoubleEndedIterator {
        self.records.values().map(|r| r.id())
    }

    pub fn compute_total_payload_size(&self) -> u64 {
        self.records.values().map(|r| r.header.payload_len).sum()
    }

    pub fn compute_total_record_size(&self) -> u64 {
        self.records
            .values()
            .map(|r| r.header_len + r.header.payload_len + r.next_offset_len)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn check(storage: &mut Storage<Cursor<Vec<u8>>>, expected: &[(u64, &[u8])]) {
        storage
            .rw
            .seek(SeekFrom::Start(storage.first_stream_position))
            .unwrap();
        let offset =
            u64::try_from(read_variable_unsigned_integer(&mut storage.rw).unwrap()).unwrap();
        assert_ne!(offset, 0);
        let mut pos = storage.first_stream_position + offset;
        for (index, (id, data)) in expected.iter().enumerate() {
            storage.rw.seek(SeekFrom::Start(pos)).unwrap();
            let header = RecordHeader::read(&mut storage.rw).unwrap();
            assert_eq!(header.id.0, *id);
            let mut actual = vec![0; header.payload_len as usize];
            storage.rw.read_exact(&mut actual).unwrap();
            assert_eq!(&actual, data);
            let offset =
                u64::try_from(read_variable_unsigned_integer(&mut storage.rw).unwrap()).unwrap();
            if index + 1 == expected.len() {
                assert_eq!(offset, 0);
            } else {
                assert_ne!(offset, 0);
            }
            let header_len = storage.records().nth(index).unwrap().header_len;
            pos += header_len + header.payload_len + offset;
        }
    }

    #[test]
    fn writes_moves_and_reuses_gaps() {
        let mut cursor = Cursor::new(vec![42; 7]);
        cursor.set_position(7);
        let mut s = Storage::new_empty(cursor).unwrap();
        s.write(1, b"a").unwrap();
        s.write(2, b"bb").unwrap();
        s.write(3, b"ccc").unwrap();
        s.write(2, b"a much longer value").unwrap();
        check(
            &mut s,
            &[(1, b"a"), (3, b"ccc"), (2, b"a much longer value")],
        );
        s.write(4, b"x").unwrap();
        check(
            &mut s,
            &[
                (1, b"a"),
                (4, b"x"),
                (3, b"ccc"),
                (2, b"a much longer value"),
            ],
        );
        s.write(1, b"first grows substantially").unwrap();
        check(
            &mut s,
            &[
                (4, b"x"),
                (3, b"ccc"),
                (2, b"a much longer value"),
                (1, b"first grows substantially"),
            ],
        );
        assert_eq!(&s.rw.get_ref()[..7], &[42; 7]);
    }

    #[test]
    fn moved_first_record_leaves_reusable_space() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        assert_eq!(s.rw.get_ref(), &[0]);
        s.write(1, b"a").unwrap();
        s.write(2, b"b").unwrap();
        s.write(1, b"a longer value").unwrap();
        check(&mut s, &[(2, b"b"), (1, b"a longer value")]);
        s.rw.set_position(0);
        let loaded: Vec<_> = Record::load_all(&mut s.rw).map(Result::unwrap).collect();
        assert_eq!(
            loaded.iter().map(|r| r.header.id.0).collect::<Vec<_>>(),
            [2, 1]
        );
        s.write(3, b"c").unwrap();
        check(&mut s, &[(3, b"c"), (2, b"b"), (1, b"a longer value")]);
    }

    #[test]
    fn shrink_and_growth_across_varint_boundary() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        s.set_zero_free_space(true);
        s.write(64, &[7; 128]).unwrap();
        s.write(2, b"tail").unwrap();
        let original = s.records().next().unwrap().stream_position;
        let old_header = s.records().next().unwrap().header_len;
        let next = s.records().nth(1).unwrap().stream_position;
        s.write(64, &[9; 127]).unwrap();
        assert_eq!(s.records().next().unwrap().stream_position, original);
        assert_eq!(s.records().next().unwrap().header_len + 1, old_header);
        assert_eq!(s.records().nth(1).unwrap().stream_position, next);
        let end = (original + s.records().next().unwrap().header_len + 127) as usize;
        let trailer_end = end + s.records().next().unwrap().next_offset_len as usize;
        assert!(
            s.rw.get_ref()[trailer_end..next as usize]
                .iter()
                .all(|b| *b == 0)
        );
        check(&mut s, &[(64, &[9; 127]), (2, b"tail")]);
        s.write(64, &[3; 128]).unwrap();
        assert_eq!(s.records().next().unwrap().stream_position, original);
        s.write(64, &[4; 129]).unwrap();
        check(&mut s, &[(2, b"tail"), (64, &[4; 129])]);
    }

    #[test]
    fn header_roundtrips_large_values_and_rejects_bad_varints() {
        let header = RecordHeader {
            id: RecordId(u64::MAX),
            payload_len: u64::MAX,
        };
        let mut bytes = Vec::new();
        header.write(&mut bytes).unwrap();
        assert_eq!(RecordHeader::read(&mut &bytes[..]).unwrap(), header);
        assert_eq!(
            bytes.len() as u64,
            RecordHeader::encoded_len(header.id, header.payload_len).unwrap()
        );
        assert!(read_variable_unsigned_integer(&mut &[0xff; 20][..]).is_err());
    }

    #[test]
    fn rejects_out_of_range_variable_fields() {
        let mut bytes = Vec::new();
        write_variable_unsigned_integer(u64::MAX as u128 + 1, &mut bytes).unwrap();
        write_variable_unsigned_integer(0, &mut bytes).unwrap();
        assert_eq!(
            RecordHeader::read(&mut &bytes[..]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let mut bytes = Vec::new();
        write_variable_unsigned_integer(1, &mut bytes).unwrap();
        write_variable_unsigned_integer(u64::MAX as u128 + 1, &mut bytes).unwrap();
        assert_eq!(
            RecordHeader::read(&mut &bytes[..]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn reopen_and_read_records() {
        let mut cursor = Cursor::new(vec![42; 5]);
        cursor.set_position(5);
        let mut s = Storage::new_empty(cursor).unwrap();
        s.write(1, b"first").unwrap();
        s.write(2, b"second").unwrap();
        s.write(1, b"a longer first record").unwrap();
        let mut cursor = s.rw;
        cursor.set_position(5);
        let mut reopened = Storage::open(cursor).unwrap();
        assert_eq!(
            reopened.read(1).unwrap(),
            Some(b"a longer first record".to_vec())
        );
        assert_eq!(reopened.read(2).unwrap(), Some(b"second".to_vec()));
        reopened.write(3, b"new").unwrap();
        assert_eq!(reopened.read(3).unwrap(), Some(b"new".to_vec()));
    }

    #[test]
    fn reads_missing_empty_moved_and_resized_records() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        assert_eq!(s.read(1).unwrap(), None);
        s.write(1, &[7; 64]).unwrap();
        s.write(2, b"second").unwrap();
        assert_eq!(s.read(1).unwrap(), Some(vec![7; 64]));
        s.write(1, &[8; 63]).unwrap();
        assert_eq!(s.read(1).unwrap(), Some(vec![8; 63]));
        s.write(1, &[9; 100]).unwrap();
        assert_eq!(s.read(1).unwrap(), Some(vec![9; 100]));
        assert_eq!(s.read(2).unwrap(), Some(b"second".to_vec()));
        s.write(1, b"").unwrap();
        assert_eq!(s.read(1).unwrap(), Some(Vec::new()));
        assert_eq!(s.read(3).unwrap(), None);
    }

    #[test]
    fn trailer_link_grows_across_boundary_when_record_shrinks() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        s.set_zero_free_space(true);
        s.write(1, &[1; 128]).unwrap();
        s.write(2, b"next").unwrap();
        let first = s.records().next().unwrap().stream_position;
        let second = s.records().nth(1).unwrap().stream_position;
        s.write(1, b"x").unwrap();
        assert_eq!(s.records().next().unwrap().stream_position, first);
        assert_eq!(s.records().nth(1).unwrap().stream_position, second);
        assert_eq!(s.records().next().unwrap().next_offset_len, 2);
        let mut cursor = s.rw;
        cursor.set_position(0);
        let mut reopened = Storage::open(cursor).unwrap();
        assert_eq!(reopened.read(1).unwrap(), Some(b"x".to_vec()));
        assert_eq!(reopened.read(2).unwrap(), Some(b"next".to_vec()));
    }

    #[test]
    fn rejects_link_that_overlaps_its_encoding() {
        let mut bytes = vec![1];
        write_variable_unsigned_integer(1, &mut bytes).unwrap();
        write_variable_unsigned_integer(0, &mut bytes).unwrap();
        // Noncanonical two-byte encoding of positive one.
        bytes.extend_from_slice(&[0x81, 0x00]);
        assert_eq!(
            Storage::open(Cursor::new(bytes)).err().unwrap().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn first_offset_grows_and_shrinks_without_overwriting_records() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        s.set_zero_free_space(true);
        s.write(1, &[7; 128]).unwrap();
        s.write(2, b"second").unwrap();
        s.write(1, &[8; 200]).unwrap();
        assert!(s.records().next().unwrap().stream_position >= 128);
        let mut cursor = s.rw;
        cursor.set_position(0);
        let mut s = Storage::open(cursor).unwrap();
        assert_eq!(s.read(2).unwrap(), Some(b"second".to_vec()));
        assert_eq!(s.read(1).unwrap(), Some(vec![8; 200]));
        s.set_zero_free_space(true);
        s.write(3, b"new").unwrap();
        assert_eq!(s.records().next().unwrap().stream_position, 1);
        s.rw.set_position(0);
        let mut reopened = Storage::open(s.rw).unwrap();
        assert_eq!(reopened.read(3).unwrap(), Some(b"new".to_vec()));
        assert_eq!(reopened.read(2).unwrap(), Some(b"second".to_vec()));
    }

    #[test]
    fn rejects_first_offset_overlapping_its_encoding() {
        let bytes = vec![0x81, 0x00];
        assert_eq!(
            Storage::open(Cursor::new(bytes)).err().unwrap().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn reads_unsigned_format_and_supports_large_ids() {
        // Unsigned encoding: first offset 1, id 1, length 1, payload, end link.
        let mut s = Storage::open(Cursor::new(vec![1, 1, 1, b'a', 0])).unwrap();
        assert_eq!(s.read(1).unwrap(), Some(vec![b'a']));
        let large_id = u32::MAX as u64 + 1;
        s.write(large_id, b"large id").unwrap();
        s.write(u64::MAX, b"max id").unwrap();
        s.rw.set_position(0);
        let mut reopened = Storage::open(s.rw).unwrap();
        assert_eq!(reopened.read(1).unwrap(), Some(vec![b'a']));
        assert_eq!(reopened.read(large_id).unwrap(), Some(b"large id".to_vec()));
        assert_eq!(reopened.read(u64::MAX).unwrap(), Some(b"max id".to_vec()));
    }

    #[test]
    fn vacuum_compacts_and_reopens_with_nonzero_start() {
        let mut cursor = Cursor::new(vec![42; 7]);
        cursor.set_position(7);
        let mut s = Storage::new_empty(cursor).unwrap();
        s.set_zero_free_space(true);
        s.write(1, &[1; 80]).unwrap();
        s.write(2, b"second").unwrap();
        s.write(1, b"short").unwrap();
        let old_end = s.records().next_back().unwrap().stream_position;
        let end = s.vacuum().unwrap();
        assert!(end < old_end);
        assert_eq!(&s.rw.get_ref()[..7], &[42; 7]);
        assert!(s.rw.get_ref()[end as usize..].iter().all(|b| *b == 0));
        assert_eq!(s.read(1).unwrap(), Some(b"short".to_vec()));
        assert_eq!(s.read(2).unwrap(), Some(b"second".to_vec()));
        s.rw.set_position(7);
        let mut reopened = Storage::open(s.rw).unwrap();
        assert_eq!(reopened.read(1).unwrap(), Some(b"short".to_vec()));
        reopened.write(3, b"after").unwrap();
        assert_eq!(reopened.read(3).unwrap(), Some(b"after".to_vec()));
    }

    #[test]
    fn vacuum_empty_and_adjacent_empty_records() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        assert_eq!(s.vacuum().unwrap(), 1);
        s.write(1, b"").unwrap();
        s.write(2, b"").unwrap();
        let end = s.vacuum().unwrap();
        assert_eq!(end, s.rw.get_ref().len() as u64);
        s.rw.set_position(0);
        let mut reopened = Storage::open(s.rw).unwrap();
        assert_eq!(reopened.read(1).unwrap(), Some(vec![]));
        assert_eq!(reopened.read(2).unwrap(), Some(vec![]));
    }

    #[test]
    fn end_position_tracks_live_records_and_storage_header() {
        let mut cursor = Cursor::new(vec![9; 5]);
        cursor.set_position(5);
        let mut s = Storage::new_empty(cursor).unwrap();
        assert_eq!(s.end_position().unwrap(), 6);
        s.write(1, &[1; 80]).unwrap();
        let old = s.end_position().unwrap();
        s.write(1, b"a").unwrap();
        assert!(s.end_position().unwrap() < old);
        assert_eq!(s.end_position().unwrap(), s.vacuum().unwrap());
        let mut s = Storage::open(Cursor::new(vec![0x80, 0])).unwrap();
        assert_eq!(s.end_position().unwrap(), 2);
        s.vacuum().unwrap();
        assert_eq!(s.end_position().unwrap(), 1);
    }

    #[test]
    fn writes_only_changed_record_and_links() {
        use std::io::{Read, Seek, SeekFrom, Write};
        #[derive(Default)]
        struct Meter {
            inner: Cursor<Vec<u8>>,
            read: usize,
            written: usize,
        }
        impl Read for Meter {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.inner.read(buf)?;
                self.read += n;
                Ok(n)
            }
        }
        impl Write for Meter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let n = self.inner.write(buf)?;
                self.written += n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl Seek for Meter {
            fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
                self.inner.seek(pos)
            }
        }
        let mut s = Storage::new_empty(Meter::default()).unwrap();
        s.write(1, &vec![42; 10000]).unwrap();
        s.write(2, b"tiny").unwrap();
        s.rw.read = 0;
        s.rw.written = 0;
        s.write(2, b"x").unwrap();
        assert_eq!(s.rw.read, 0);
        assert!(s.rw.written < 20);
        s.rw.read = 0;
        s.rw.written = 0;
        s.write(3, b"new").unwrap();
        assert_eq!(s.rw.read, 0);
        assert!(s.rw.written < 20);
        assert_eq!(s.read(1).unwrap(), Some(vec![42; 10000]));
        s.rw.inner.set_position(0);
        let mut reopened = Storage::open(s.rw).unwrap();
        assert_eq!(reopened.read(2).unwrap(), Some(b"x".to_vec()));
        assert_eq!(reopened.read(3).unwrap(), Some(b"new".to_vec()));
    }

    #[test]
    fn writes_preserve_noncanonical_unmodified_headers_and_links() {
        // First record uses noncanonical encodings for its ID and next link.
        let bytes = vec![1, 0x81, 0, 0, 0x82, 0, 2, 1, b'x', 0];
        let mut s = Storage::open(Cursor::new(bytes)).unwrap();
        s.write(2, b"y").unwrap();
        assert_eq!(s.read(1).unwrap(), Some(vec![]));
        assert_eq!(s.read(2).unwrap(), Some(b"y".to_vec()));
        s.rw.set_position(0);
        let mut reopened = Storage::open(s.rw).unwrap();
        assert_eq!(reopened.read(2).unwrap(), Some(b"y".to_vec()));
        assert_eq!(reopened.records().next().unwrap().next_offset_len, 2);
    }

    #[test]
    fn indexed_placement_matches_linear_first_fit() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        let mut expected = HashMap::new();
        let mut seed = 17u64;
        for step in 0..2_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let id = (seed >> 32) % 80;
            let len = (seed >> 48) as usize % 350;
            let required =
                RecordHeader::encoded_len(RecordId(id), len as u64).unwrap() + len as u64 + 1;
            let old = s.record_positions.get(&RecordId(id)).copied();
            let retain = old.filter(|&pos| {
                s.records()
                    .find(|r| r.stream_position > pos)
                    .is_none_or(|r| pos + required <= r.stream_position)
            });
            let position = retain.unwrap_or_else(|| {
                let mut start = 1;
                for record in s.records().filter(|r| r.header.id != RecordId(id)) {
                    if start + required <= record.stream_position {
                        break;
                    }
                    start = record.trailer_position().unwrap() + 1;
                }
                start
            });
            let data = vec![id as u8 + 1; len];
            s.set_zero_free_space(step % 2 == 0);
            s.write(id, &data).unwrap();
            assert_eq!(s.record_positions[&RecordId(id)], position, "write {step}");
            expected.insert(id, data);
            // Verify the serialized chain and payloads, then continue with the
            // index reconstructed from disk. Also exercise writes after vacuum.
            if step % 97 == 0 {
                if step % 194 == 0 {
                    s.vacuum().unwrap();
                }
                s.rw.set_position(0);
                s = Storage::open(s.rw).unwrap();
                assert_eq!(s.records().len(), expected.len());
                for (&id, data) in &expected {
                    assert_eq!(s.read(id).unwrap().as_deref(), Some(data.as_slice()));
                }
            }
        }
    }

    #[test]
    fn empty_payload_does_not_end_list() {
        let mut s = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
        s.write(1, b"").unwrap();
        s.write(2, b"").unwrap();
        check(&mut s, &[(1, b""), (2, b"")]);
    }
}
