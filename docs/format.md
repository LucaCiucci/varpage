# Storage format

A store starts at the stream position where it is created or opened. It begins
with a pointer to the first record, followed by records in increasing stream
position. Records can sit next to each other or have unused bytes between them.

```text
Store   [first offset] ... [record] ... [record]
Record  [ID] [payload length] [payload bytes] [next offset]
```

## Integers and offsets

IDs, payload lengths, and offsets are unsigned 64-bit values encoded as
unsigned LEB128. Each byte holds seven bits of the value, starting with the
least significant bits. The high bit is set when another byte follows.
For example, `127` encodes as `7f` and `128` as `80 01`. The writer uses the
shortest encoding, from one to ten bytes.

Both offsets count bytes from **the start of their own field**. Zero in the
first offset means the store is empty. Zero in a record's next offset marks
the last record. A nonzero offset must point at or beyond the end of its
encoded field. An offset of one puts the next record immediately after the
one-byte offset. Each live ID appears once.

## Example

Two adjacent records with IDs 7 and 9 hold `hi` and `!`, respectively.
All bytes below are hexadecimal.

```text
01 | 07 02 68 69 01 | 09 01 21 00
```

The opening `01` points one byte forward to record 7. Record 7's trailing
`01` points one byte forward to record 9. The final `00` ends the chain.
Here the store header takes one byte and each record adds three bytes of
metadata to its payload.

## Gaps and the logical end

Readers follow the links and skip unused bytes. A replacement can move into
a gap, and the bytes it leaves behind may still contain old data unless
free-space zeroing is enabled. The logical end is just after the last record's
offset, or after the first offset for an empty store. Vacuum packs the live
records together and returns the new logical end for callers that want to
truncate the backing stream.

The format has no version marker. Files produced by the earlier ZigZag
encoding are incompatible with this layout.
