//! Unit tests for the little-endian primitives (included from `src/bytes.rs`).

use super::*;

#[test]
fn scalar_roundtrip_at_offset() {
    let mut buf = [0u8; 32];
    put_u8(&mut buf, 0, 0xAB).unwrap();
    put_u16(&mut buf, 1, 0xBEEF).unwrap();
    put_u32(&mut buf, 3, 0xDEAD_BEEF).unwrap();
    put_u64(&mut buf, 7, 0x0123_4567_89AB_CDEF).unwrap();
    put_f32(&mut buf, 15, -1.5).unwrap();
    put_f64(&mut buf, 19, 1.0e-9).unwrap();

    assert_eq!(get_u8(&buf, 0).unwrap(), 0xAB);
    assert_eq!(get_u16(&buf, 1).unwrap(), 0xBEEF);
    assert_eq!(get_u32(&buf, 3).unwrap(), 0xDEAD_BEEF);
    assert_eq!(get_u64(&buf, 7).unwrap(), 0x0123_4567_89AB_CDEF);
    assert_eq!(get_f32(&buf, 15).unwrap(), -1.5);
    assert_eq!(get_f64(&buf, 19).unwrap(), 1.0e-9);
}

#[test]
fn little_endian_byte_order_is_explicit() {
    let mut buf = [0u8; 4];
    put_u32(&mut buf, 0, 0x0102_0304).unwrap();
    assert_eq!(buf, [0x04, 0x03, 0x02, 0x01]);
}

#[test]
fn out_of_range_reads_report_truncation() {
    let buf = [0u8; 2];
    let err = get_u32(&buf, 0).unwrap_err();
    match err {
        MapError::Truncated {
            offset,
            needed,
            available,
        } => {
            assert_eq!(offset, 0);
            assert_eq!(needed, 4);
            assert_eq!(available, 2);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn out_of_range_writes_report_truncation() {
    let mut buf = [0u8; 3];
    assert!(put_u32(&mut buf, 0, 1).is_err());
    assert!(put_u32(&mut buf, 1, 1).is_err());
}

#[test]
fn unsigned_and_signed_views_agree() {
    let mut buf = [0u8; 8];
    put_i16(&mut buf, 0, -2).unwrap();
    put_i32(&mut buf, 2, -70_000).unwrap();
    put_i8(&mut buf, 6, -1).unwrap();
    assert_eq!(get_i16(&buf, 0).unwrap(), -2);
    assert_eq!(get_i32(&buf, 2).unwrap(), -70_000);
    assert_eq!(get_i8(&buf, 6).unwrap(), -1);
    assert_eq!(get_u16(&buf, 0).unwrap(), 0xFFFE);
    assert_eq!(get_u8(&buf, 6).unwrap(), 0xFF);
}

#[test]
fn reader_sequence_and_blobs() {
    let mut w = Writer::new();
    w.write_u8(1).write_u16(2).write_u32(3).write_u64(4);
    w.write_string("campus");
    w.write_blob(&[9, 8, 7]);
    let bytes = w.into_vec();

    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_u8().unwrap(), 1);
    assert_eq!(r.read_u16().unwrap(), 2);
    assert_eq!(r.read_u32().unwrap(), 3);
    assert_eq!(r.read_u64().unwrap(), 4);
    assert_eq!(r.read_string().unwrap(), "campus");
    assert_eq!(r.read_blob().unwrap(), &[9, 8, 7]);
    assert!(r.is_empty());
    assert_eq!(r.position(), bytes.len());
}

#[test]
fn reader_reports_overrun() {
    let bytes = [1u8, 2, 3];
    let mut r = Reader::new(&bytes);
    r.read_u16().unwrap();
    assert!(r.read_u16().is_err());
    assert_eq!(r.remaining(), 1);
}

#[test]
fn vector_reads() {
    let mut w = Writer::new();
    w.write_f32(0.5).write_f32(-0.25).write_f32(2.0);
    w.write_u32(7).write_u32(9);
    let bytes = w.into_vec();

    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_f32_vec(3).unwrap(), vec![0.5, -0.25, 2.0]);
    assert_eq!(r.read_u32_vec(2).unwrap(), vec![7, 9]);
}

#[test]
fn padding_to_alignment() {
    let mut buf = vec![1u8; 5];
    pad_to_alignment(&mut buf, 8);
    assert_eq!(buf.len(), 8);
    assert_eq!(&buf[5..], &[0, 0, 0]);

    let mut aligned = vec![1u8; 8];
    pad_to_alignment(&mut aligned, 8);
    assert_eq!(aligned.len(), 8);

    let mut untouched = vec![1u8; 3];
    pad_to_alignment(&mut untouched, 0);
    assert_eq!(untouched.len(), 3);
}

#[test]
fn writer_len_tracks_position() {
    let mut w = Writer::with_capacity(16);
    assert!(w.is_empty());
    w.write_u64(0);
    assert_eq!(w.len(), 8);
    assert!(!w.is_empty());
    assert_eq!(w.as_slice().len(), 8);
}
