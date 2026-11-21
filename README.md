# varpage

Compact storage for variable-size records on seekable streams.

A crate for storing variable-size records by ID in a file or other seekable
stream.

Use it when you need to replace individual chunks whose sizes can change, such
as compressed pages or cached blobs. If a replacement fits in its current
space, varpage keeps it there. Otherwise it finds a gap or appends the record
and updates the affected links. You can reopen a store, read by ID, or compact
it with [`Storage::vacuum`].

## Quick start

```rust
# use std::io::Cursor;
# use varpage::Storage;
let mut store = Storage::new_empty(Cursor::new(Vec::new())).unwrap();
store.write(7, b"hello").unwrap();
store.write(7, b"a larger replacement").unwrap();

assert_eq!(store.read(7).unwrap().as_deref(), Some(&b"a larger replacement"[..]));
```

Use a `File` for disk storage. [`Storage::open`] reopens a store from the stream's
current position.

## Goals and non-goals

- **Simple and compact.** The format uses variable-length integers for record
  metadata and reuses gaps left by resized records.
- **Local updates.** A write changes the record and any links that need updating.
  Opening the store rebuilds its indexes in memory.
- **Failure recovery is out of scope.** A crash, interruption, or write error
  can leave the store inconsistent. Use varpage for data you can rebuild or
  restore. Transactions and recovery are future possibilities.
