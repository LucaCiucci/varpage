//! Compress a file as independent zstd frames in a varpage store.
//!
//! Run with `cargo run --release --example zstd_chunks -- input.bin output.varpage`.

use std::env;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::time::Instant;

use varpage::Storage;

const CHUNK_SIZE: usize = 4_096;

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let [input_path, output_path] = args.as_slice() else {
        eprintln!("Usage: cargo run --release --example zstd_chunks -- <input> <output>");
        std::process::exit(2);
    };

    let start = Instant::now();
    let mut input = BufReader::new(File::open(input_path)?);
    let mut compressor = zstd::bulk::Compressor::new(zstd::DEFAULT_COMPRESSION_LEVEL)?;
    let mut output_file = File::create(output_path)?;
    let mut store = Storage::new_empty(&mut output_file)?;
    let mut chunk = Vec::with_capacity(CHUNK_SIZE);
    let mut input_bytes = 0;
    let mut chunks = 0;

    loop {
        chunk.clear();
        let bytes_read = input
            .by_ref()
            .take(CHUNK_SIZE as u64)
            .read_to_end(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }

        let compressed = compressor.compress(&chunk)?;
        store.write(chunks, &compressed)?;
        input_bytes += bytes_read as u64;
        chunks += 1;
    }

    store.flush()?;
    let payload_size = store.compute_total_payload_size();
    drop(store);
    let archive_bytes = output_file.metadata()?.len();
    drop(output_file);
    let duration = start.elapsed();
    println!("Elapsed time: {:.2?}", duration);
    println!("Chunks: {chunks}");
    println!("Input: {input_bytes} bytes");
    println!("Compressed payloads: {payload_size} bytes");
    println!("Varpage file: {archive_bytes} bytes");
    if input_bytes != 0 {
        let percent = 100.0 * archive_bytes as f64 / input_bytes as f64;
        println!("Varpage file is {percent:.2}% of the input size");
    }
    Ok(())
}
