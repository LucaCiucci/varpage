use std::{
    hint::black_box,
    io::{Cursor, Read, Seek, Write},
};

use criterion::{BenchmarkId, Criterion, measurement::Measurement};
use varpage::Storage;

mod write_measurement;
use write_measurement::{CountWrites, WriteMeasurement};

const RECORD_COUNTS: [u64; 2] = [1_000, 100_000];
const FILLER: [u8; 128] = [0x11; 128];
const PAGE: [u8; 4096] = [0x22; 4096];
const SMALL_PAGE: [u8; 2048] = [0x33; 2048];

fn fixture(records: u64, is_page: impl Fn(u64) -> bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut storage = Storage::new_empty(Cursor::new(&mut bytes)).unwrap();
    for id in 0..records {
        let payload: &[u8] = if is_page(id) { &PAGE } else { &FILLER };
        storage.write(id, payload).unwrap();
    }
    bytes
}

fn write_benches<M: Measurement, T: Read + Write + Seek>(
    criterion: &mut Criterion<M>,
    metric: &str,
    stream: impl Fn(Vec<u8>) -> T,
) {
    let mut group = criterion.benchmark_group(metric);
    for records in RECORD_COUNTS {
        let id = records / 2;
        group.bench_function(BenchmarkId::new("replace_4k", records), |b| {
            let mut storage =
                Storage::open(stream(fixture(records, |record| record == id))).unwrap();
            b.iter(|| storage.write(black_box(id), black_box(&PAGE)).unwrap());
        });

        group.bench_function(BenchmarkId::new("shuffled_replace_4k", records), |b| {
            const PAGE_STRIDE: usize = 8;
            const SEED: u64 = 42;
            // Spread the full-size pages throughout the store. Shuffle once so
            // all metrics see the same order and RNG work stays outside timing.
            let mut ids: Vec<_> = (0..records).step_by(PAGE_STRIDE).collect();
            fastrand::Rng::with_seed(SEED).shuffle(&mut ids);
            let mut ids = ids.into_iter().cycle();
            let bytes = fixture(records, |record| record.is_multiple_of(PAGE_STRIDE as u64));
            let mut storage = Storage::open(stream(bytes)).unwrap();
            b.iter(|| {
                storage
                    .write(black_box(ids.next().unwrap()), black_box(&PAGE))
                    .unwrap()
            });
        });

        group.bench_function(BenchmarkId::new("shrink_regrow_zeroed", records), |b| {
            let mut storage =
                Storage::open(stream(fixture(records, |record| record == id))).unwrap();
            storage.set_zero_free_space(true);
            // Each iteration restores the starting layout so subsequent writes
            // have the same gap and link widths.
            b.iter(|| {
                storage
                    .write(black_box(id), black_box(&SMALL_PAGE))
                    .unwrap();
                storage.write(black_box(id), black_box(&PAGE)).unwrap();
            });
        });
    }
    group.finish();
}

fn main() {
    let mut wall_time = Criterion::default().configure_from_args();
    write_benches(&mut wall_time, "wall_time", Cursor::new);
    wall_time.final_summary();

    for (name, measurement) in [
        ("write_calls", WriteMeasurement::Calls),
        ("written_bytes", WriteMeasurement::Bytes),
    ] {
        let mut criterion = Criterion::default()
            .with_measurement(measurement)
            .configure_from_args()
            .without_plots();
        write_benches(&mut criterion, name, |bytes| {
            CountWrites(Cursor::new(bytes))
        });
        criterion.final_summary();
    }
}
