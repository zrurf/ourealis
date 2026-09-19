//! Unit tests for chunk geometry and quantisation (included from `src/raster.rs`).

use super::*;
use crate::codec::id;

fn grid() -> ChunkGrid {
    ChunkGrid::new(Aabb::new(0.0, 0.0, 1000.0, 500.0), 0.5, 256, 3)
}

fn desc(dtype: DType) -> LayerDesc {
    LayerDesc::new(
        LayerId::ELEVATION,
        crate::layer::LayerKind::Raster,
        1,
        dtype,
        id::ZSTD,
    )
}

#[test]
fn level_geometry_is_consistent() {
    let grid = grid();
    assert_eq!(grid.cell_size_m(0), 0.5);
    assert_eq!(grid.cell_size_m(1), 1.0);
    assert_eq!(grid.cell_dims(0), (2000, 1000));
    assert_eq!(grid.chunk_dims(0), (8, 4));
    assert_eq!(grid.chunk_dims(1), (4, 2));
}

#[test]
fn chunk_lookup_roundtrips_through_morton() {
    let grid = grid();
    let (x, y) = (321.0, 187.0);
    let id = grid.chunk_id_at(x, y, 0).unwrap();
    let bounds = grid.chunk_bounds(0, id).unwrap();
    assert!(bounds.contains(x, y));
}

#[test]
fn outside_positions_have_no_chunk() {
    let grid = grid();
    assert!(grid.chunk_id_at(-1.0, 10.0, 0).is_none());
    assert!(grid.chunk_id_at(10.0, 10_000.0, 0).is_none());
}

#[test]
fn bbox_query_covers_the_region() {
    let grid = grid();
    let bbox = Aabb::new(0.0, 0.0, 300.0, 300.0);
    let ids = grid.chunks_in_bbox(&bbox, 0);
    // One chunk spans 0.5 m/cell * 256 cells = 128 m, so a 300 m box touches
    // chunk indices 0..=2 on both axes.
    assert_eq!(ids.len(), 9);
    for id in ids {
        assert!(grid.chunk_bounds(0, id).unwrap().intersects(&bbox));
    }
    assert!(
        grid.chunks_in_bbox(&Aabb::new(-500.0, -500.0, -100.0, -100.0), 0)
            .is_empty()
    );
}

#[test]
fn bit_layers_roundtrip() {
    let desc = LayerDesc::new(
        LayerId::HARD_FORBIDDEN,
        crate::layer::LayerKind::Bitmap,
        1,
        DType::Bit,
        id::RLE,
    );
    let mut chunk = RasterChunk::zeros(8, 2, 1);
    chunk.set(0, 0, 0, 1.0);
    chunk.set(7, 1, 0, 1.0);
    let packed = pack(&desc, &chunk).unwrap();
    assert_eq!(packed.len(), 2);
    let shape = ChunkShape::new(8, 2, 1, DType::Bit);
    let unpacked = unpack(&desc, &shape, &packed).unwrap();
    assert_eq!(unpacked.data, chunk.data);
}

#[test]
fn bit_rows_are_padded_to_a_byte_boundary() {
    let desc = LayerDesc::new(
        LayerId::HARD_FORBIDDEN,
        crate::layer::LayerKind::Bitmap,
        1,
        DType::Bit,
        id::RLE,
    );
    // width 3 is not a multiple of 8, so each row occupies its own byte rather
    // than letting bits of the next row continue the previous byte.
    let shape = ChunkShape::new(3, 2, 1, DType::Bit);
    assert_eq!(shape.row_bytes(), 1);
    assert_eq!(shape.total_bytes(), 2);

    let mut chunk = RasterChunk::zeros(3, 2, 1);
    chunk.set(0, 0, 0, 1.0);
    chunk.set(2, 0, 0, 1.0);
    chunk.set(1, 1, 0, 1.0);
    let packed = pack(&desc, &chunk).unwrap();
    assert_eq!(packed, vec![0b0000_0101, 0b0000_0010]);
    let unpacked = unpack(&desc, &shape, &packed).unwrap();
    assert_eq!(unpacked.data, chunk.data);

    // A payload shorter than `row_bytes * height` is truncated even though the
    // continuous bit count would fit in one byte.
    assert!(matches!(
        unpack(&desc, &shape, &packed[..1]),
        Err(MapError::Truncated { .. })
    ));
}

#[test]
fn oversized_grids_are_rejected_instead_of_aliasing() {
    let grid = ChunkGrid::new(Aabb::new(0.0, 0.0, 1.0e9, 1.0e9), 1.0, 1, 1);
    assert!(grid.validate().is_err());
    assert!(grid.chunk_index_at(1.0, 1.0, 0).is_none());
    assert!(
        grid.chunks_in_bbox(&Aabb::new(0.0, 0.0, 1000.0, 1000.0), 0)
            .is_empty()
    );
}

#[test]
fn valid_grids_pass_validation() {
    assert!(grid().validate().is_ok());
    let degenerate = ChunkGrid::new(Aabb::new(0.0, 0.0, 10.0, 10.0), 0.0, 256, 1);
    assert!(degenerate.validate().is_err());
}

#[test]
fn quantised_i16_roundtrip_respects_precision() {
    let desc = desc(DType::I16).with_quantisation(0.1, -100.0);
    let mut chunk = RasterChunk::zeros(4, 1, 1);
    for (i, value) in [-99.9f32, -50.0, 0.0, 123.4].iter().enumerate() {
        chunk.set(i as u32, 0, 0, *value);
    }
    let packed = pack(&desc, &chunk).unwrap();
    assert_eq!(packed.len(), 8);
    let shape = ChunkShape::new(4, 1, 1, DType::I16);
    let unpacked = unpack(&desc, &shape, &packed).unwrap();
    for (a, b) in unpacked.data.iter().zip(chunk.data.iter()) {
        assert!((a - b).abs() <= 0.05, "{a} vs {b}");
    }
}

#[test]
fn u8_quantisation_clamps() {
    let desc = desc(DType::U8).with_quantisation(1.0, 0.0);
    let mut chunk = RasterChunk::zeros(3, 1, 1);
    chunk.set(0, 0, 0, -5.0);
    chunk.set(1, 0, 0, 300.0);
    chunk.set(2, 0, 0, 42.0);
    let packed = pack(&desc, &chunk).unwrap();
    assert_eq!(packed, vec![0, 255, 42]);
}

#[test]
fn f16_roundtrip_is_close() {
    let desc = desc(DType::F16);
    let mut chunk = RasterChunk::zeros(3, 1, 1);
    chunk.set(0, 0, 0, 0.5);
    chunk.set(1, 0, 0, -2.25);
    chunk.set(2, 0, 0, 100.0);
    let packed = pack(&desc, &chunk).unwrap();
    assert_eq!(packed.len(), 6);
    let shape = ChunkShape::new(3, 1, 1, DType::F16);
    let unpacked = unpack(&desc, &shape, &packed).unwrap();
    for (a, b) in unpacked.data.iter().zip(chunk.data.iter()) {
        assert!((a - b).abs() <= 0.05 * b.abs().max(1.0));
    }
}

#[test]
fn channel_continuous_indexing() {
    let mut chunk = RasterChunk::zeros(2, 2, 3);
    chunk.set(1, 1, 2, 7.0);
    assert_eq!(chunk.index(1, 1, 2), 3 * 3 + 2);
    assert_eq!(chunk.get(1, 1, 2), 7.0);
    assert_eq!(chunk.cell(1, 1), &[0.0, 0.0, 7.0]);
    assert_eq!(chunk.get(5, 5, 5), 0.0);
}

#[test]
fn mis_sized_chunk_data_is_rejected() {
    assert!(RasterChunk::from_vec(2, 2, 1, vec![0.0; 3]).is_err());
    assert!(RasterChunk::from_vec(2, 2, 1, vec![0.0; 4]).is_ok());
}

#[test]
fn cell_centres_follow_the_grid() {
    let grid = grid();
    let (x, y) = grid.cell_center(0, 0, 0);
    assert!((x - 0.25).abs() < 1e-9);
    assert!((y - 0.25).abs() < 1e-9);
    let (x1, _) = grid.cell_center(0, 0, 1);
    assert!((x1 - 0.5).abs() < 1e-9);
}
