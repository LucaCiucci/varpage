use std::io::Cursor;

use varpage::Storage;

mod common;
use common::dump_colored;

fn main() {
    let mut buf = vec![0u8; 0];
    let cursor = Cursor::new(&mut buf);

    let mut s = Storage::new_empty(cursor).unwrap();
    s.set_zero_free_space(true);
    s.write(1, b"A medium length string").unwrap();
    s.write(2, b"A shorted string").unwrap();
    s.write(1, b"Shortened").unwrap();
    s.write(3, b"Another record").unwrap();
    println!("  Before vacuuming:");
    dump_colored(s.get_ref().get_ref());
    println!();
    s.vacuum().unwrap();
    let end = s.end_position().unwrap();
    let payload_size = s.compute_total_payload_size();
    println!(
        "Total payload size: {:?} bytes, total storage size: {:?} bytes, ratio: {:?}",
        payload_size,
        end,
        payload_size as f64 / end as f64
    );
    drop(s);
    buf.truncate(usize::try_from(end).unwrap());

    println!("  After vacuuming:");
    dump_colored(&buf);
    std::fs::write("output.bin", &buf).unwrap();
    println!();

    println!("After reopening the storage and writing a longer string at record 1...");
    let mut s = Storage::open(Cursor::new(&mut buf)).unwrap();
    s.set_zero_free_space(true);
    s.write(1, b"This record becomes a much longer string")
        .unwrap();
    println!("  Before vacuuming:");
    dump_colored(s.get_ref().get_ref());
    s.vacuum().unwrap();
    println!("  After vacuuming:");
    dump_colored(s.get_ref().get_ref());
}
