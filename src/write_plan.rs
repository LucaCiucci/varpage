//! Plans a write from the current record and gap indexes without stream I/O.
//!
//! A replacement stays at its old position if it fits. Otherwise we search the
//! gap index in stream order, also considering the gap created by removing the
//! old record. Once we have a position, only the old and new predecessors can
//! need link updates.
//!
//! For example, if X grows and moves from `A -> X -> B` to `A -> B -> X`,
//! the plan updates A's link to B and B's link to X, writes X with an end marker,
//! and marks the old X bytes for optional clearing.

use std::{io, ops::Range};

use smallvec::SmallVec;

use crate::varint::variable_unsigned_integer_len;
use crate::{Record, RecordHeader, RecordId, Storage, position_overflow};

/// The metadata and local edits for one insert or replacement.
pub(super) struct WritePlan {
    /// The record to write, including its new outgoing link.
    pub record: Record,
    /// The previous position of this ID, if it already exists.
    pub old_position: Option<u64>,
    /// The replacement first-record offset and its encoded length, if changed.
    pub first_link: Option<(u64, u64)>,
    /// Updates to existing records' outgoing links.
    pub links: SmallVec<[LinkUpdate; 2]>,
    /// Absolute half-open ranges to clear if zeroing is enabled.
    pub released: SmallVec<[Range<u64>; 8]>,
    /// Right-hand endpoints of gaps that need refreshing after the write.
    pub gap_ends: SmallVec<[u64; 3]>,
}

pub(super) struct LinkUpdate {
    pub record_position: u64,
    pub trailer: u64,
    pub offset: u64,
    pub len: u64,
}

impl<T> Storage<T> {
    pub(super) fn plan_write(&self, id: RecordId, payload_len: u64) -> io::Result<WritePlan> {
        let old_position = self.record_positions.get(&id).copied();
        let header_len = RecordHeader::encoded_len(id, payload_len)?;
        let body_len = header_len
            .checked_add(payload_len)
            .ok_or_else(position_overflow)?;
        let required = body_len.checked_add(1).ok_or_else(position_overflow)?;
        let position = self.choose_position(old_position, required)?;
        let trailer = position
            .checked_add(body_len)
            .ok_or_else(position_overflow)?;
        let successor = self.successor(position, old_position);
        let offset = successor.map_or(0, |record| record.stream_position - trailer);
        let next_offset_len = variable_unsigned_integer_len(offset)?;
        let end = trailer
            .checked_add(next_offset_len)
            .ok_or_else(position_overflow)?;
        let record = Record {
            stream_position: position,
            header_len,
            next_offset_len,
            next_offset: offset,
            header: RecordHeader { id, payload_len },
        };

        let old_prev = old_position.and_then(|pos| self.predecessor(pos, None));
        let new_prev = self.predecessor(position, old_position);
        let mut links = SmallVec::new();
        let mut candidates = SmallVec::<[Range<u64>; 4]>::new();
        let mut live = SmallVec::<[Range<u64>; 4]>::new();
        live.push(position..end);

        if let Some(old) = old_position.map(|pos| &self.records[&pos]) {
            candidates.push(old.stream_position..old.end_position()?);
        }
        for prev in [old_prev, new_prev].into_iter().flatten() {
            let pos = prev.stream_position;
            if links
                .iter()
                .any(|link: &LinkUpdate| link.record_position == pos)
            {
                continue;
            }
            let next = self.successor(pos, old_position).map(|r| r.stream_position);
            let next = if position > pos {
                Some(next.map_or(position, |next| next.min(position)))
            } else {
                next
            };
            let trailer = prev.trailer_position()?;
            let offset = next.map_or(0, |next| next - trailer);
            if offset != prev.next_offset {
                let len = variable_unsigned_integer_len(offset)?;
                let end = trailer.checked_add(len).ok_or_else(position_overflow)?;
                candidates.push(trailer..prev.end_position()?);
                live.push(trailer..end);
                links.push(LinkUpdate {
                    record_position: pos,
                    trailer,
                    offset,
                    len,
                });
            }
        }

        let first = self
            .records
            .keys()
            .copied()
            .find(|pos| Some(*pos) != old_position)
            .map_or(position, |pos| pos.min(position));
        let first_link = if self.records.first_key_value().map(|(&pos, _)| pos) == Some(first) {
            None
        } else {
            let offset = first - self.first_stream_position;
            let len = variable_unsigned_integer_len(offset)?;
            candidates.push(
                self.first_stream_position..self.first_stream_position + self.storage_header_len,
            );
            live.push(self.first_stream_position..self.first_stream_position + len);
            Some((offset, len))
        };

        let mut gap_ends = SmallVec::new();
        if old_position == Some(position) {
            if self.records[&position].trailer_position()? != trailer
                && let Some(next) = successor
            {
                gap_ends.push(next.stream_position);
            }
        } else {
            gap_ends.push(position);
            if let Some(next) = successor {
                gap_ends.push(next.stream_position);
            }
            if let Some(next) = old_position.and_then(|pos| self.successor(pos, None)) {
                gap_ends.push(next.stream_position);
            }
        }
        gap_ends.sort_unstable();
        gap_ends.dedup();

        let released = if self.zero_free_space {
            subtract_live(&candidates, &mut live)
        } else {
            SmallVec::new()
        };
        Ok(WritePlan {
            record,
            old_position,
            first_link,
            links,
            released,
            gap_ends,
        })
    }

    fn choose_position(&self, old: Option<u64>, required: u64) -> io::Result<u64> {
        if let Some(pos) = old {
            let limit = self
                .successor(pos, None)
                .map_or(u64::MAX, |r| r.stream_position);
            if required <= limit - pos {
                return Ok(pos);
            }
        }

        let indexed = self.gaps.first_fit(required);
        let merged = if let Some(pos) = old {
            let start = self.gap_start(self.predecessor(pos, None))?;
            let limit = self
                .successor(pos, None)
                .map_or(u64::MAX, |r| r.stream_position);
            (required <= limit - start).then_some(start)
        } else {
            None
        };
        // Any indexed gap adjacent to the old record is contained in the
        // merged gap, so the merged candidate wins if either of those gaps fits.
        if let Some(pos) = indexed.into_iter().chain(merged).min() {
            return Ok(pos);
        }
        let last = self
            .records
            .values()
            .rev()
            .find(|record| Some(record.stream_position) != old);
        let start = self.gap_start(last)?;
        start.checked_add(required).ok_or_else(position_overflow)?;
        Ok(start)
    }
}

/// Subtract the newly written ranges from old bytes. Untouched records cannot
/// overlap any of these old ranges because the original layout was disjoint.
fn subtract_live(candidates: &[Range<u64>], live: &mut [Range<u64>]) -> SmallVec<[Range<u64>; 8]> {
    live.sort_unstable_by_key(|range| range.start);
    let mut released = SmallVec::new();
    for range in candidates {
        let mut start = range.start;
        for used in live.iter() {
            let end = range.end.min(used.start);
            if start < end {
                released.push(start..end);
            }
            start = start.max(used.end);
            if start >= range.end {
                break;
            }
        }
        if start < range.end {
            released.push(start..range.end);
        }
    }
    released
}
