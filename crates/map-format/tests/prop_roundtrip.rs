//! Property tests: arbitrary chunk contents and metadata must survive a full
//! encode → decode cycle unchanged.

use ourealis_map_format::codec::{self, ChunkShape, CodecContext, id};
use ourealis_map_format::layer::DType;
use proptest::prelude::*;

fn any_dtype() -> impl Strategy<Value = DType> {
    prop_oneof![
        Just(DType::F32),
        Just(DType::I32),
        Just(DType::I16),
        Just(DType::U8),
        Just(DType::F16),
    ]
}

fn any_codec() -> impl Strategy<Value = u8> {
    prop_oneof![
        Just(id::RAW),
        Just(id::ZSTD),
        Just(id::LZ4),
        Just(id::DELTA_VERTICAL),
        Just(id::SPARSE),
        Just(id::RLE),
        Just(id::QUANTISED_ZSTD),
        Just(id::PYRAMID),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every codec is a lossless transform of the stored bytes it is given.
    #[test]
    fn codecs_are_lossless(
        codec_id in any_codec(),
        dtype in any_dtype(),
        width in 1u32..24,
        height in 1u32..24,
        channels in 1u8..3,
        seed in any::<u32>(),
    ) {
        let shape = ChunkShape::new(width, height, channels, dtype);
        // Run-length coding only applies to one-byte element types.
        if codec_id == id::RLE && dtype.element_size() != 1 {
            return Ok(());
        }
        // Channel delta needs at least two channels.
        if codec_id == id::DELTA_CHANNEL && channels < 2 {
            return Ok(());
        }
        let mut state = seed | 1;
        let data: Vec<u8> = (0..shape.total_bytes())
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 17) as u8
            })
            .collect();

        let ctx = CodecContext::none();
        let stored = codec::encode(codec_id, &shape, &data, &ctx)?;
        let restored = codec::decode(codec_id, &shape, &stored, &ctx)?;
        prop_assert_eq!(restored, data);
    }

    /// Chunk payloads survive packing and unpacking within the quantisation
    /// step of their declared precision.
    #[test]
    fn raster_pack_unpack_respects_quantisation(
        scale in 0.001f32..2.0,
        values in prop::collection::vec(-500.0f32..500.0, 1..64),
    ) {
        use ourealis_map_format::layer::{LayerDesc, LayerId, LayerKind};
        use ourealis_map_format::raster::{RasterChunk, pack, unpack};

        let desc = LayerDesc::new(
            LayerId::ELEVATION,
            LayerKind::Raster,
            1,
            DType::I16,
            id::ZSTD,
        )
        .with_quantisation(scale, 0.0);
        // Values are drawn inside the i16 range the layer can represent, so the
        // property under test is quantisation error rather than saturation.
        let scale = scale.clamp(1e-3, 2.0);
        let width = values.len() as u32;
        let mut chunk = RasterChunk::zeros(width, 1, 1);
        for (index, value) in values.iter().enumerate() {
            let bounded = value.clamp(-30_000.0 * scale, 30_000.0 * scale);
            chunk.set(index as u32, 0, 0, bounded);
        }
        let packed = pack(&desc, &chunk)?;
        let shape = ChunkShape::new(width, 1, 1, DType::I16);
        let restored = unpack(&desc, &shape, &packed)?;
        for (before, after) in chunk.data.iter().zip(restored.data.iter()) {
            prop_assert!((before - after).abs() <= scale.max(1e-3) * 1.01);
        }
    }
}
