//! Counts calls and bytes at the backing stream's write interface.

use std::{
    cell::Cell,
    io::{self, Read, Seek, SeekFrom, Write},
};

use criterion::{
    Throughput,
    measurement::{Measurement, ValueFormatter},
};

thread_local! {
    static WRITE_TOTALS: Cell<WriteTotals> = const { Cell::new(WriteTotals { calls: 0, bytes: 0 }) };
}

#[derive(Clone, Copy)]
struct WriteTotals {
    calls: u64,
    bytes: u64,
}

pub(super) struct CountWrites<T>(pub(super) T);

impl<T: Read> Read for CountWrites<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl<T: Seek> Seek for CountWrites<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

impl<T: Write> Write for CountWrites<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.0.write(buf)?;
        WRITE_TOTALS.with(|totals| {
            let old = totals.get();
            totals.set(WriteTotals {
                calls: old.calls + 1,
                bytes: old.bytes + written as u64,
            });
        });
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

pub(super) enum WriteMeasurement {
    Calls,
    Bytes,
}

impl WriteMeasurement {
    fn total(&self) -> u64 {
        WRITE_TOTALS.with(|totals| match self {
            Self::Calls => totals.get().calls,
            Self::Bytes => totals.get().bytes,
        })
    }

    fn unit(&self) -> &'static str {
        match self {
            Self::Calls => "write calls",
            Self::Bytes => "written bytes",
        }
    }
}

impl Measurement for WriteMeasurement {
    type Intermediate = u64;
    type Value = u64;

    fn start(&self) -> u64 {
        self.total()
    }

    fn end(&self, start: u64) -> u64 {
        self.total() - start
    }

    fn add(&self, a: &u64, b: &u64) -> u64 {
        a + b
    }

    fn zero(&self) -> u64 {
        0
    }

    fn to_f64(&self, value: &u64) -> f64 {
        *value as f64
    }

    fn formatter(&self) -> &dyn ValueFormatter {
        self
    }
}

impl ValueFormatter for WriteMeasurement {
    fn scale_values(&self, _: f64, _: &mut [f64]) -> &'static str {
        self.unit()
    }

    fn scale_throughputs(&self, _: f64, _: &Throughput, _: &mut [f64]) -> &'static str {
        // These benchmarks report counts per iteration and register no throughput.
        unreachable!("write-count benchmarks do not set throughput")
    }

    fn scale_for_machines(&self, _: &mut [f64]) -> &'static str {
        self.unit()
    }
}
