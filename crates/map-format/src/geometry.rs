//! Axis-aligned bounding boxes and Morton (Z-order) codes.
//!
//! OMF uses Morton codes in two places with different widths:
//! * [`morton_encode_2d`] — 64-bit codes for quadtree skeleton nodes, where the
//!   number of significant bits encodes the node depth (linear quadtree);
//! * [`morton_encode_chunk`] — 32-bit codes for chunk identifiers inside one
//!   `(layer, level)` grid.

/// Axis-aligned bounding box in the map's local metre plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    /// Minimum x (metres).
    pub min_x: f64,
    /// Minimum y (metres).
    pub min_y: f64,
    /// Maximum x (metres).
    pub max_x: f64,
    /// Maximum y (metres).
    pub max_y: f64,
}

impl Aabb {
    /// Creates a box from two corners in any order.
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self {
            min_x: x0.min(x1),
            min_y: y0.min(y1),
            max_x: x0.max(x1),
            max_y: y0.max(y1),
        }
    }

    /// Creates a box from its centre and full side lengths.
    pub fn from_center_size(cx: f64, cy: f64, size_x: f64, size_y: f64) -> Self {
        Self {
            min_x: cx - size_x * 0.5,
            min_y: cy - size_y * 0.5,
            max_x: cx + size_x * 0.5,
            max_y: cy + size_y * 0.5,
        }
    }

    /// Width along x (metres).
    #[inline]
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    /// Height along y (metres).
    #[inline]
    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    /// Centre point.
    #[inline]
    pub fn center(&self) -> (f64, f64) {
        (
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
        )
    }

    /// Point containment test (inclusive on the upper edges).
    #[inline]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// True when the two boxes share any area or touch.
    #[inline]
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min_x <= other.max_x
            && other.min_x <= self.max_x
            && self.min_y <= other.max_y
            && other.min_y <= self.max_y
    }

    /// Smallest box containing both inputs.
    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// Box grown by `margin` on every side.
    pub fn expanded(&self, margin: f64) -> Aabb {
        Aabb {
            min_x: self.min_x - margin,
            min_y: self.min_y - margin,
            max_x: self.max_x + margin,
            max_y: self.max_y + margin,
        }
    }

    /// Box intersected with `other`, or `None` when they are disjoint.
    pub fn intersection(&self, other: &Aabb) -> Option<Aabb> {
        let out = Aabb {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        };
        (out.min_x <= out.max_x && out.min_y <= out.max_y).then_some(out)
    }
}

/// Interleaves the bits of two 32-bit coordinates into a 64-bit Morton code.
///
/// Bit `2i` of the result comes from `x`, bit `2i + 1` from `y`.
pub fn morton_encode_2d(x: u32, y: u32) -> u64 {
    let mut out = 0u64;
    for i in 0..32 {
        out |= (((x >> i) & 1) as u64) << (2 * i);
        out |= (((y >> i) & 1) as u64) << (2 * i + 1);
    }
    out
}

/// Inverse of [`morton_encode_2d`].
pub fn morton_decode_2d(code: u64) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    for i in 0..32 {
        x |= (((code >> (2 * i)) & 1) as u32) << i;
        y |= (((code >> (2 * i + 1)) & 1) as u32) << i;
    }
    (x, y)
}

/// Depth of a linear-quadtree node: the root (code 0) is depth 0, a node whose
/// cells span `2^d × 2^d` of the finest grid is depth `d`.
pub fn morton_depth(code: u64) -> u8 {
    ((64 - code.leading_zeros()) / 2) as u8
}

/// Parent of a linear-quadtree node, or `None` for the root.
pub fn morton_parent(code: u64) -> Option<u64> {
    (code != 0).then_some(code >> 2)
}

/// Linear-quadtree node key: `1 << (2 * depth) | interleave(x, y)`.
///
/// A bare interleaved code cannot carry the depth: the node at the origin has
/// code 0 at *every* depth, so depth-3 and depth-5 nodes over the same corner
/// would collide. Reserving one bit above the coordinate bits makes the depth
/// recoverable from the leading one bit while keeping the coordinate bits
/// spatially ordered, which is the standard linear-quadtree key.
pub fn node_key(x: u32, y: u32, depth: u8) -> u64 {
    let depth = depth.min(31) as u32;
    (1u64 << (2 * depth)) | morton_encode_2d(x, y)
}

/// Depth of a [`node_key`].
pub fn node_key_depth(key: u64) -> u8 {
    if key == 0 {
        return 0;
    }
    ((64 - key.leading_zeros() - 1) / 2) as u8
}

/// Coordinates of a [`node_key`], masked to its depth.
pub fn node_key_xy(key: u64) -> (u32, u32) {
    let depth = node_key_depth(key) as u32;
    if depth == 0 {
        return (0, 0);
    }
    let mask = (1u64 << (2 * depth)) - 1;
    morton_decode_2d(key & mask)
}

/// Parent key of a [`node_key`], or `None` for the root.
pub fn node_key_parent(key: u64) -> Option<u64> {
    let depth = node_key_depth(key);
    if depth == 0 {
        return None;
    }
    let (x, y) = node_key_xy(key);
    Some(node_key(x / 2, y / 2, depth - 1))
}

/// Child key in the given quadrant: 0 = low/low, 1 = high-x, 2 = high-y, 3 = both.
pub fn node_key_child(key: u64, quadrant: u8) -> u64 {
    let depth = node_key_depth(key);
    let (x, y) = node_key_xy(key);
    let (dx, dy) = match quadrant % 4 {
        0 => (0, 0),
        1 => (1, 0),
        2 => (0, 1),
        _ => (1, 1),
    };
    node_key(x * 2 + dx, y * 2 + dy, depth + 1)
}

/// Interleaves two 16-bit chunk indices into a 32-bit Morton chunk id.
pub fn morton_encode_chunk(x: u16, y: u16) -> u32 {
    let mut out = 0u32;
    for i in 0..16 {
        out |= (((x >> i) & 1) as u32) << (2 * i);
        out |= (((y >> i) & 1) as u32) << (2 * i + 1);
    }
    out
}

/// Inverse of [`morton_encode_chunk`].
pub fn morton_decode_chunk(code: u32) -> (u16, u16) {
    let mut x = 0u16;
    let mut y = 0u16;
    for i in 0..16 {
        x |= (((code >> (2 * i)) & 1) as u16) << i;
        y |= (((code >> (2 * i + 1)) & 1) as u16) << i;
    }
    (x, y)
}
