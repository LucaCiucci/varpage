use smallvec::SmallVec;
//use zigzag::{ZigZagDecode, ZigZagEncode};

pub fn variable_unsigned_integer_len(value: u64) -> std::io::Result<u64> {
    let mut bytes = SmallVec::<[u8; 16]>::new();
    write_variable_unsigned_integer(u128::from(value), &mut bytes)?;
    Ok(bytes.len() as u64)
}

pub fn write_variable_unsigned_integer(
    mut value: u128,
    w: &mut impl std::io::Write,
) -> std::io::Result<()> {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            // write the byte as is, the MSB is 0
            w.write_all(&[byte])?;
            break;
        } else {
            // write the byte with MSB set
            w.write_all(&[byte | 0x80])?;
        }
    }
    Ok(())
}

pub fn read_variable_unsigned_integer(r: &mut impl std::io::Read) -> std::io::Result<u128> {
    let mut result: u128 = 0;
    let mut shift = 0;
    loop {
        let mut buf = [0u8; 1];
        r.read_exact(&mut buf)?;
        let byte = buf[0];
        let bits = (byte & 0x7f) as u128;
        if shift >= 128 || (shift == 126 && bits > 3) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "variable integer overflow",
            ));
        }
        result |= bits << shift;
        if (byte & 0x80) == 0 {
            break;
        }
        shift += 7;
    }
    Ok(result)
}

#[cfg(false)]
fn encode(value: i128) -> u128 {
    // first, we try to fit the value in the smallest possible number of bytes, then
    // place it back into a u128 for writing
    const I8_MIN: i128 = i8::MIN as i128;
    const I8_MAX: i128 = i8::MAX as i128;
    const I16_MIN: i128 = i16::MIN as i128;
    const I16_MAX: i128 = i16::MAX as i128;
    const I32_MIN: i128 = i32::MIN as i128;
    const I32_MAX: i128 = i32::MAX as i128;
    const I64_MIN: i128 = i64::MIN as i128;
    const I64_MAX: i128 = i64::MAX as i128;
    match value {
        I8_MIN..=I8_MAX => (value as i8).zigzag_encode() as u128,
        I16_MIN..=I16_MAX => (value as i16).zigzag_encode() as u128,
        I32_MIN..=I32_MAX => (value as i32).zigzag_encode() as u128,
        I64_MIN..=I64_MAX => (value as i64).zigzag_encode() as u128,
        _ => value.zigzag_encode(),
    }
}

#[cfg(false)]
fn decode(value: u128) -> i128 {
    // now we have to understand what was the original size. So if, for example,
    // the value has 8 bits or less, we interpret it as i8, if it has 16 bits or less,
    // we interpret it as i16, and so on.
    // We need to use signed bounds to match the write function's logic
    if value <= u8::MAX as u128 {
        (value as u8).zigzag_decode() as i128
    } else if value <= u16::MAX as u128 {
        (value as u16).zigzag_decode() as i128
    } else if value <= u32::MAX as u128 {
        (value as u32).zigzag_decode() as i128
    } else if value <= u64::MAX as u128 {
        (value as u64).zigzag_decode() as i128
    } else {
        value.zigzag_decode() as i128
    }
}

/// Write a variable-length integer to the given writer
///
/// The tag is written using a variable-length encoding where each byte
/// contains 7 bits of the value, and the most significant bit (MSB)
/// indicates whether there are more bytes to read (1) or if this is the
/// last byte (0).
#[cfg(false)]
pub fn write_variable_signed_integer(
    value: i128,
    w: &mut impl std::io::Write,
) -> std::io::Result<()> {
    let value = encode(value);
    write_variable_unsigned_integer(value, w)
}

#[cfg(false)]
pub fn read_variable_signed_integer(r: &mut impl std::io::Read) -> std::io::Result<i128> {
    let result = read_variable_unsigned_integer(r)?;
    Ok(decode(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind, Read, Write};

    #[test]
    fn unsigned_known_encodings_and_u128_boundaries() {
        for (value, expected) in [
            (0, vec![0]),
            (1, vec![1]),
            (127, vec![0x7f]),
            (128, vec![0x80, 1]),
            (16_383, vec![0xff, 0x7f]),
            (16_384, vec![0x80, 0x80, 1]),
            (u128::MAX, [vec![0xff; 18], vec![3]].concat()),
        ] {
            let mut encoded = Vec::new();
            write_variable_unsigned_integer(value, &mut encoded).unwrap();
            assert_eq!(encoded, expected, "value {value}");
            assert_eq!(
                read_variable_unsigned_integer(&mut &encoded[..]).unwrap(),
                value
            );
        }
        for shift in (7..128).step_by(7) {
            let boundary = 1u128 << shift;
            for value in [boundary - 1, boundary, boundary + 1] {
                let mut bytes = Vec::new();
                write_variable_unsigned_integer(value, &mut bytes).unwrap();
                assert_eq!(
                    read_variable_unsigned_integer(&mut &bytes[..]).unwrap(),
                    value
                );
                if let Ok(value) = u64::try_from(value) {
                    assert_eq!(
                        variable_unsigned_integer_len(value).unwrap(),
                        bytes.len() as u64
                    );
                }
            }
        }
    }

    #[cfg(false)]
    #[test]
    fn signed_known_encodings_and_type_boundaries() {
        for (value, expected) in [(0, 0), (-1, 1), (1, 2), (-2, 3), (2, 4), (-64, 127)] {
            let mut bytes = Vec::new();
            write_variable_signed_integer(value, &mut bytes).unwrap();
            assert_eq!(bytes, [expected]);
            assert_eq!(
                read_variable_signed_integer(&mut &bytes[..]).unwrap(),
                value
            );
        }
        for boundary in [
            i8::MIN as i128,
            i8::MAX as i128,
            i16::MIN as i128,
            i16::MAX as i128,
            i32::MIN as i128,
            i32::MAX as i128,
            i64::MIN as i128,
            i64::MAX as i128,
        ] {
            for value in [boundary - 1, boundary, boundary + 1] {
                let mut bytes = Vec::new();
                write_variable_signed_integer(value, &mut bytes).unwrap();
                assert_eq!(
                    read_variable_signed_integer(&mut &bytes[..]).unwrap(),
                    value
                );
            }
        }
        for value in [i128::MIN, i128::MAX] {
            let mut bytes = Vec::new();
            write_variable_signed_integer(value, &mut bytes).unwrap();
            assert_eq!(
                read_variable_signed_integer(&mut &bytes[..]).unwrap(),
                value
            );
        }
    }

    #[test]
    fn rejects_truncated_and_overflowing_values() {
        for bytes in [&[][..], &[0x80][..], &[0xff; 18][..]] {
            assert_eq!(
                read_variable_unsigned_integer(&mut &bytes[..])
                    .unwrap_err()
                    .kind(),
                ErrorKind::UnexpectedEof
            );
        }
        let invalid_high_bits = [vec![0x80; 18], vec![4]].concat();
        let too_many_bytes = [vec![0x80; 19], vec![0]].concat();
        for bytes in [invalid_high_bits, too_many_bytes] {
            assert_eq!(
                read_variable_unsigned_integer(&mut &bytes[..])
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn reads_consecutive_values_and_propagates_io_errors() {
        let mut bytes = Vec::new();
        for value in [0, 128, u128::MAX] {
            write_variable_unsigned_integer(value, &mut bytes).unwrap();
        }
        let mut input = Cursor::new(bytes);
        for value in [0, 128, u128::MAX] {
            assert_eq!(read_variable_unsigned_integer(&mut input).unwrap(), value);
        }
        struct Fails;
        impl Read for Fails {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(ErrorKind::PermissionDenied.into())
            }
        }
        impl Write for Fails {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(ErrorKind::PermissionDenied.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            read_variable_unsigned_integer(&mut Fails)
                .unwrap_err()
                .kind(),
            ErrorKind::PermissionDenied
        );
        assert_eq!(
            write_variable_unsigned_integer(128, &mut Fails)
                .unwrap_err()
                .kind(),
            ErrorKind::PermissionDenied
        );
    }
}
