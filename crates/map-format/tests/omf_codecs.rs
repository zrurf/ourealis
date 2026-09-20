//! Codec round-trip tests: every implemented codec must reproduce the stored
//! bytes exactly, and unsupported ids must degrade instead of panicking.

use ourealis_map_format::codec::{self, ChunkShape, CodecContext, id};
use ourealis_map_format::layer::DType;

fn payload(shape: &ChunkShape, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    (0..shape.total_bytes())
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 16) as u8
        })
        .collect()
}

fn smooth_payload(shape: &ChunkShape) -> Vec<u8> {
    // A spatially smooth i16 field, which is what the delta codecs target.
    let cells = shape.width as usize * shape.height as usize;
    let mut out = Vec::with_capacity(shape.total_bytes());
    for cell in 0..cells {
        let x = (cell % shape.width as usize) as i32;
        let y = (cell / shape.width as usize) as i32;
        let value = ((x * 3 + y * 5) % 400) as i16 - 200;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn round_trip(codec_id: u8, shape: &ChunkShape, data: &[u8], ctx: &CodecContext<'_>) -> Vec<u8> {
    let stored = codec::encode(codec_id, shape, data, ctx).expect("encode");
    let restored = codec::decode(codec_id, shape, &stored, ctx).expect("decode");
    assert_eq!(
        restored,
        data,
        "codec {} did not round-trip",
        codec::name(codec_id)
    );
    stored
}

#[test]
fn raw_and_entropy_codecs_round_trip() {
    let shape = ChunkShape::new(32, 8, 1, DType::U8);
    let data = payload(&shape, 7);
    let raw = round_trip(id::RAW, &shape, &data, &CodecContext::none());
    assert_eq!(raw.len(), data.len());
    round_trip(id::ZSTD, &shape, &data, &CodecContext::none());
    round_trip(id::LZ4, &shape, &data, &CodecContext::none());
    round_trip(id::QUANTISED_ZSTD, &shape, &data, &CodecContext::none());
}

#[test]
fn delta_codecs_round_trip_and_shrink_smooth_data() {
    let shape = ChunkShape::new(64, 64, 1, DType::I16);
    let data = smooth_payload(&shape);
    let vertical = round_trip(id::DELTA_VERTICAL, &shape, &data, &CodecContext::none());
    let plain = round_trip(id::ZSTD, &shape, &data, &CodecContext::none());
    assert!(
        vertical.len() < plain.len(),
        "vertical delta should beat plain zstd on a smooth field: {} vs {}",
        vertical.len(),
        plain.len()
    );

    let multi = ChunkShape::new(32, 32, 3, DType::I16);
    let data = smooth_payload(&ChunkShape::new(32, 32, 1, DType::I16));
    let mut channels = Vec::new();
    for _ in 0..3 {
        channels.extend_from_slice(&data);
    }
    round_trip(id::DELTA_CHANNEL, &multi, &channels, &CodecContext::none());
}

#[test]
fn channel_delta_rejects_single_channel_shapes() {
    let shape = ChunkShape::new(8, 8, 1, DType::U8);
    let data = payload(&shape, 3);
    assert!(codec::encode(id::DELTA_CHANNEL, &shape, &data, &CodecContext::none()).is_err());
}

#[test]
fn run_length_and_sparse_round_trip() {
    let shape = ChunkShape::new(64, 4, 1, DType::U8);
    let mut data = vec![7u8; shape.total_bytes()];
    for (index, byte) in data.iter_mut().enumerate() {
        if index % 64 == 0 {
            *byte = 3;
        }
    }
    let stored = round_trip(id::RLE, &shape, &data, &CodecContext::none());
    assert!(
        stored.len() < data.len() / 8,
        "run-length should compress hard"
    );

    let mut sparse = vec![0u8; shape.total_bytes()];
    sparse[5] = 9;
    sparse[100] = 4;
    let stored = round_trip(id::SPARSE, &shape, &sparse, &CodecContext::none());
    assert!(stored.len() < data.len() / 4);

    let restored = codec::decode(id::SPARSE, &shape, &stored, &CodecContext::none()).unwrap();
    let mut expected = vec![0u8; shape.total_bytes()];
    expected[5] = 9;
    expected[100] = 4;
    assert_eq!(restored, expected);
}

#[test]
fn bit_packed_shapes_round_trip() {
    let shape = ChunkShape::new(64, 4, 1, DType::Bit);
    assert_eq!(shape.row_bytes(), 8);
    assert_eq!(shape.total_bytes(), 32);
    let data = payload(&shape, 11);
    round_trip(id::RAW, &shape, &data, &CodecContext::none());
    round_trip(id::RLE, &shape, &data, &CodecContext::none());
    assert!(codec::encode(id::SPARSE, &shape, &data, &CodecContext::none()).is_err());
}

#[test]
fn channel_delta_rejects_bit_packed_shapes() {
    // A bit-packed chunk pads every row to a byte boundary, so its cells are not
    // `channels * element_size` apart. Both directions must refuse such a shape
    // instead of walking past the payload.
    let shape = ChunkShape::new(64, 4, 2, DType::Bit);
    let data = payload(&ChunkShape::new(64, 4, 2, DType::U8), 7);
    assert!(
        codec::encode(id::DELTA_CHANNEL, &shape, &data, &CodecContext::none()).is_err(),
        "encoding a bit-packed shape must fail"
    );
    assert!(
        codec::decode(id::DELTA_CHANNEL, &shape, &data, &CodecContext::none()).is_err(),
        "decoding a bit-packed shape must fail"
    );
}

#[test]
fn pyramid_uses_the_parent_chunk_as_dictionary() {
    let shape = ChunkShape::new(32, 32, 1, DType::I16);
    let data = smooth_payload(&shape);
    let parent = smooth_payload(&ChunkShape::new(32, 32, 1, DType::I16));
    let ctx = CodecContext {
        dict: None,
        parent: Some(&parent),
    };
    round_trip(id::PYRAMID, &shape, &data, &ctx);
    round_trip(id::PYRAMID, &shape, &data, &CodecContext::none());
}

#[test]
fn dictionary_compression_round_trips() {
    let shape = ChunkShape::new(32, 32, 1, DType::U8);
    let data = payload(&shape, 5);
    let dict = payload(&ChunkShape::new(256, 1, 1, DType::U8), 5);
    let ctx = CodecContext::with_dict(&dict);
    round_trip(id::ZSTD, &shape, &data, &ctx);
    round_trip(id::QUANTISED_ZSTD, &shape, &data, &ctx);
}

#[test]
fn unknown_codec_is_reported_not_fatal() {
    let shape = ChunkShape::new(8, 8, 1, DType::U8);
    let data = payload(&shape, 1);
    assert!(!codec::is_supported(0x7E));
    match codec::encode(0x7E, &shape, &data, &CodecContext::none()) {
        Err(ourealis_map_format::MapError::UnsupportedCodec { codec: c, .. }) => {
            assert_eq!(c, 0x7E)
        }
        other => panic!("unexpected: {other:?}"),
    }
    assert!(codec::decode(0x7E, &shape, &data, &CodecContext::none()).is_err());
    assert_eq!(codec::name(0x7E), "unknown");
}

#[test]
fn payload_length_mismatch_is_rejected() {
    let shape = ChunkShape::new(8, 8, 1, DType::U8);
    let short = vec![0u8; 10];
    assert!(codec::encode(id::ZSTD, &shape, &short, &CodecContext::none()).is_err());
}

#[test]
fn truncated_stored_data_is_rejected() {
    let shape = ChunkShape::new(16, 16, 1, DType::U8);
    let data = payload(&shape, 9);
    let stored = codec::encode(id::ZSTD, &shape, &data, &CodecContext::none()).unwrap();
    let truncated = &stored[..stored.len() / 2];
    assert!(codec::decode(id::ZSTD, &shape, truncated, &CodecContext::none()).is_err());
}

#[test]
fn every_supported_codec_is_named() {
    for codec_id in [
        id::RAW,
        id::ZSTD,
        id::LZ4,
        id::DELTA_VERTICAL,
        id::DELTA_CHANNEL,
        id::SPARSE,
        id::RLE,
        id::QUANTISED_ZSTD,
        id::PYRAMID,
    ] {
        assert!(codec::is_supported(codec_id));
        assert_ne!(codec::name(codec_id), "unknown");
    }
}

#[test]
fn run_length_runs_cannot_expand_past_the_expected_length() {
    use ourealis_map_format::codec::sparse::rle_decode;

    // Three bytes describe one run of 65535 copies; the expected payload is 16
    // bytes, so the decoder must reject it instead of resizing to the run.
    let body = [0xAAu8, 0xFF, 0xFF];
    let stored = codec::entropy::zstd_compress(&body, None).unwrap();
    assert!(rle_decode(&stored, 16).is_err());

    // Exactly the expected length is fine.
    let exact_body = [0xAAu8, 0x10, 0x00];
    let exact = codec::entropy::zstd_compress(&exact_body, None).unwrap();
    assert_eq!(rle_decode(&exact, 16).unwrap(), vec![0xAA; 16]);

    // The same bomb through the codec entry point must fail, not allocate.
    let shape = ChunkShape::new(16, 1, 1, DType::U8);
    assert!(codec::decode(id::RLE, &shape, &stored, &CodecContext::none()).is_err());
}

#[test]
fn zstd_bounded_rejects_expansion_beyond_the_bound() {
    use ourealis_map_format::codec::entropy::zstd_decompress_bounded;

    let payload = vec![0u8; 1 << 20];
    let stored = codec::entropy::zstd_compress(&payload, None).unwrap();
    // A one-megabyte frame from a few dozen bytes must be stopped at the bound
    // rather than materialised first.
    assert!(zstd_decompress_bounded(&stored, 1024).is_err());
    assert_eq!(
        zstd_decompress_bounded(&stored, payload.len()).unwrap(),
        payload
    );
}

#[test]
fn sparse_shapes_are_capped_before_decompressing() {
    use ourealis_map_format::MapError;
    use ourealis_map_format::codec::sparse;

    // 65535 x 65535 cells x 4 channels is ~17 GiB of samples; the shape comes
    // from an untrusted header and must not drive an allocation.
    let shape = ChunkShape::new(u16::MAX as u32, u16::MAX as u32, 4, DType::U8);
    assert!(shape.total_bytes() > ourealis_map_format::codec::MAX_RAW_CHUNK_BYTES);
    let stored = codec::entropy::zstd_compress(&[0u8; 8], None).unwrap();
    match sparse::decode(&stored, &shape) {
        Err(MapError::Invalid(message)) => {
            assert!(
                message.contains("above the"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected the shape cap to reject the shape, got {other:?}"),
    }
    assert!(codec::decode(id::SPARSE, &shape, &stored, &CodecContext::none()).is_err());
}
