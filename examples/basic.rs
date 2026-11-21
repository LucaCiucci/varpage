use std::io::Cursor;

use varpage::Storage;

mod common;
use common::dump_colored;

fn main() {
    let mut buf = vec![];

    let mut store = Storage::new_empty(Cursor::new(&mut buf)).unwrap();

    store.set_zero_free_space(true);
    store.write(0, b"Hello").unwrap();
    store.write(1, b"There").unwrap();
    store.write(2, b"!").unwrap();

    // Overwrite record 1
    store.write(1, b"foo").unwrap();

    // finish and get the buffer back
    dump_colored(&buf);

    // reopen the storage and read back the records
    let mut store = Storage::open(Cursor::new(&mut buf)).unwrap();
    for id in 0..3 {
        let record = store
            .read(id)
            .unwrap()
            .map(|r| String::from_utf8_lossy(&r).into_owned());
        println!("Record {id}: {record:?}");
    }
}
