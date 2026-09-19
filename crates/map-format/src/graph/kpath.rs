//! K-shortest-path candidate library.
//!
//! Precomputed candidates for common origin–destination pairs. Three properties
//! matter for the simulator and are enforced here:
//!
//! 1. **Exact endpoints.** A path's first and last samples are the true
//!    endpoints used when it was generated, not the rounded OD key cell. The
//!    simulator's last-mile attach contract compares them against the actual
//!    query points, so rounding them would shift every attachment.
//! 2. **Registered generation parameters.** The parameter set id records how
//!    the candidates were produced, so on-line generation can be checked
//!    against it and the path-choice distribution stays comparable.
//! 3. **Precomputed path-size factors.** `PS_j` depends only on the candidate
//!    set, never on the individual, so it is stored rather than recomputed.

use crate::bytes::{Reader, Writer};
use crate::error::{MapError, Result};

/// Generation parameters shared by library and on-line candidates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KPathParams {
    /// Identifier of this parameter combination.
    pub param_set_id: u32,
    /// Number of candidates generated per OD.
    pub k: u8,
    /// Edge penalty multiplier used when searching for the next candidate.
    pub penalty_mu: f32,
    /// Reject a candidate overlapping an accepted one by more than this ratio.
    pub max_overlap_ratio: f32,
    /// Identifier of the path-size formula in use.
    pub ps_formula: u8,
    /// Whether duplicate node sequences are dropped.
    pub dedup: bool,
    /// OD key quantisation cell size in metres.
    pub key_cell_m: f32,
}

impl Default for KPathParams {
    fn default() -> Self {
        Self {
            param_set_id: 1,
            k: 5,
            penalty_mu: 1.6,
            max_overlap_ratio: 0.8,
            ps_formula: 1,
            dedup: true,
            key_cell_m: 25.0,
        }
    }
}

impl KPathParams {
    /// Serialised size in bytes.
    pub const SIZE: usize = 20;

    /// Serialises the parameter block.
    #[allow(clippy::wrong_self_convention)]
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_u32(&mut buf, 0, self.param_set_id);
        let _ = crate::bytes::put_u8(&mut buf, 4, self.k);
        let _ = crate::bytes::put_f32(&mut buf, 5, self.penalty_mu);
        let _ = crate::bytes::put_f32(&mut buf, 9, self.max_overlap_ratio);
        let _ = crate::bytes::put_u8(&mut buf, 13, self.ps_formula);
        let _ = crate::bytes::put_u8(&mut buf, 14, self.dedup as u8);
        let _ = crate::bytes::put_f32(&mut buf, 15, self.key_cell_m);
        buf
    }

    /// Parses the parameter block.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            param_set_id: crate::bytes::get_u32(buf, 0)?,
            k: crate::bytes::get_u8(buf, 4)?,
            penalty_mu: crate::bytes::get_f32(buf, 5)?,
            max_overlap_ratio: crate::bytes::get_f32(buf, 9)?,
            ps_formula: crate::bytes::get_u8(buf, 13)?,
            dedup: crate::bytes::get_u8(buf, 14)? != 0,
            key_cell_m: crate::bytes::get_f32(buf, 15)?,
        })
    }

    /// Quantises a position into an OD key cell.
    pub fn key_of(&self, x: f64, y: f64) -> [i32; 2] {
        let cell = self.key_cell_m.max(1e-3) as f64;
        [(x / cell).floor() as i32, (y / cell).floor() as i32]
    }
}

/// One candidate path.
#[derive(Debug, Clone, PartialEq)]
pub struct KPath {
    /// Total cost in equivalent metres, including the attach segments.
    pub total_cost_equiv_m: f32,
    /// Geometric length in metres.
    pub length_m: f32,
    /// Precomputed path-size factor penalising overlap between candidates.
    pub path_size: f32,
    /// Range of this path's samples inside the library's node table.
    pub node_range: (u32, u32),
}

impl KPath {
    /// Serialised size in bytes.
    pub const SIZE: usize = 20;

    /// Samples of this path inside `nodes`.
    pub fn points<'a>(&self, nodes: &'a [[f32; 3]]) -> &'a [[f32; 3]] {
        let start = self.node_range.0 as usize;
        let end = start + self.node_range.1 as usize;
        nodes.get(start..end).unwrap_or(&[])
    }

    /// First sample, which is the exact path start used at generation time.
    pub fn start_point<'a>(&self, nodes: &'a [[f32; 3]]) -> Option<&'a [f32; 3]> {
        self.points(nodes).first()
    }

    /// Last sample, which is the exact path end used at generation time.
    pub fn end_point<'a>(&self, nodes: &'a [[f32; 3]]) -> Option<&'a [f32; 3]> {
        self.points(nodes).last()
    }

    fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_f32(&mut buf, 0, self.total_cost_equiv_m);
        let _ = crate::bytes::put_f32(&mut buf, 4, self.length_m);
        let _ = crate::bytes::put_f32(&mut buf, 8, self.path_size);
        let _ = crate::bytes::put_u32(&mut buf, 12, self.node_range.0);
        let _ = crate::bytes::put_u32(&mut buf, 16, self.node_range.1);
        buf
    }

    fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            total_cost_equiv_m: crate::bytes::get_f32(buf, 0)?,
            length_m: crate::bytes::get_f32(buf, 4)?,
            path_size: crate::bytes::get_f32(buf, 8)?,
            node_range: (
                crate::bytes::get_u32(buf, 12)?,
                crate::bytes::get_u32(buf, 16)?,
            ),
        })
    }
}

/// OD index entry mapping a quantised origin–destination pair to a path set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OdEntry {
    /// Quantised origin cell.
    pub start_key: [i32; 2],
    /// Quantised destination cell.
    pub goal_key: [i32; 2],
    /// Index of the path set.
    pub set_index: u32,
}

impl OdEntry {
    /// Serialised size in bytes.
    pub const SIZE: usize = 20;

    /// Sort key for binary search.
    #[inline]
    pub fn sort_key(&self) -> ([i32; 2], [i32; 2]) {
        (self.start_key, self.goal_key)
    }

    // `to_*` taking `&self` is intentional here: every record type in the file
    // exposes the same signature so call sites read uniformly.
    #[allow(clippy::wrong_self_convention)]
    fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_i32(&mut buf, 0, self.start_key[0]);
        let _ = crate::bytes::put_i32(&mut buf, 4, self.start_key[1]);
        let _ = crate::bytes::put_i32(&mut buf, 8, self.goal_key[0]);
        let _ = crate::bytes::put_i32(&mut buf, 12, self.goal_key[1]);
        let _ = crate::bytes::put_u32(&mut buf, 16, self.set_index);
        buf
    }

    fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            start_key: [
                crate::bytes::get_i32(buf, 0)?,
                crate::bytes::get_i32(buf, 4)?,
            ],
            goal_key: [
                crate::bytes::get_i32(buf, 8)?,
                crate::bytes::get_i32(buf, 12)?,
            ],
            set_index: crate::bytes::get_u32(buf, 16)?,
        })
    }
}

/// Candidate library: parameters, OD index, path sets and the sample table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KPathLibrary {
    /// Generation parameters of every set in this library.
    pub params: KPathParams,
    /// OD index, sorted by key.
    pub od_index: Vec<OdEntry>,
    /// Path sets referenced by the OD index.
    pub sets: Vec<Vec<KPath>>,
    /// Shared sample point table.
    pub nodes: Vec<[f32; 3]>,
}

impl KPathLibrary {
    /// Looks up the candidate set for a quantised OD pair.
    pub fn find(&self, start_key: [i32; 2], goal_key: [i32; 2]) -> Option<&[KPath]> {
        let key = (start_key, goal_key);
        self.od_index
            .binary_search_by_key(&key, OdEntry::sort_key)
            .ok()
            .and_then(|index| self.sets.get(self.od_index[index].set_index as usize))
            .map(|set| set.as_slice())
    }

    /// Inserts a path set, replacing an existing entry for the same OD pair.
    pub fn insert_set(&mut self, entry: OdEntry, paths: Vec<KPath>) {
        if let Ok(index) = self
            .od_index
            .binary_search_by_key(&entry.sort_key(), OdEntry::sort_key)
        {
            self.sets[self.od_index[index].set_index as usize] = paths;
            return;
        }
        let set_index = self.sets.len() as u32;
        self.sets.push(paths);
        let entry = OdEntry { set_index, ..entry };
        match self
            .od_index
            .binary_search_by_key(&entry.sort_key(), OdEntry::sort_key)
        {
            Ok(_) => {}
            Err(index) => self.od_index.insert(index, entry),
        }
    }

    /// Appends samples to the shared node table, returning their range.
    pub fn push_nodes(&mut self, points: &[[f32; 3]]) -> (u32, u32) {
        let start = self.nodes.len() as u32;
        self.nodes.extend_from_slice(points);
        (start, points.len() as u32)
    }

    /// Serialises the library into a graph-layer payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut od = Writer::with_capacity(self.od_index.len() * OdEntry::SIZE);
        for entry in &self.od_index {
            od.write_bytes(&entry.to_bytes());
        }

        let mut sets = Writer::new();
        sets.write_u32(self.sets.len() as u32);
        for set in &self.sets {
            sets.write_u32(set.len() as u32);
            for path in set {
                sets.write_bytes(&path.to_bytes());
            }
        }

        let mut nodes = Writer::with_capacity(self.nodes.len() * 12);
        nodes.write_u32(self.nodes.len() as u32);
        for point in &self.nodes {
            nodes.write_f32(point[0]);
            nodes.write_f32(point[1]);
            nodes.write_f32(point[2]);
        }

        let mut out = Writer::new();
        out.write_bytes(&super::write_section(
            super::SectionId::KPathParams,
            &self.params.to_bytes(),
        ));
        out.write_bytes(&super::write_section(
            super::SectionId::KPathOdIndex,
            od.as_slice(),
        ));
        out.write_bytes(&super::write_section(
            super::SectionId::KPathSets,
            sets.as_slice(),
        ));
        out.write_bytes(&super::write_section(
            super::SectionId::KPathNodes,
            nodes.as_slice(),
        ));
        out.into_vec()
    }

    /// Parses a library payload.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut params_raw = None;
        let mut od_raw = None;
        let mut sets_raw = None;
        let mut nodes_raw = None;
        let mut sections = super::SectionReader::new(bytes);
        while let Some((id, payload)) = sections.next_section()? {
            match super::SectionId::from_u16(id) {
                Some(super::SectionId::KPathParams) => params_raw = Some(payload),
                Some(super::SectionId::KPathOdIndex) => od_raw = Some(payload),
                Some(super::SectionId::KPathSets) => sets_raw = Some(payload),
                Some(super::SectionId::KPathNodes) => nodes_raw = Some(payload),
                _ => {}
            }
        }
        let params = KPathParams::from_bytes(params_raw.unwrap_or(&[]))?;

        let mut od_index = Vec::new();
        let mut od_reader = Reader::new(od_raw.unwrap_or(&[]));
        while od_reader.remaining() >= OdEntry::SIZE {
            od_index.push(OdEntry::from_bytes(od_reader.read_bytes(OdEntry::SIZE)?)?);
        }

        let mut sets = Vec::new();
        let mut sets_reader = Reader::new(sets_raw.unwrap_or(&[]));
        if sets_reader.remaining() >= 4 {
            let set_count = sets_reader.read_u32()? as usize;
            // Every set carries at least its 4-byte path count on the wire.
            let needed = set_count
                .checked_mul(4)
                .ok_or_else(|| MapError::invalid("path set count overflows"))?;
            if needed > sets_reader.remaining() {
                return Err(MapError::Truncated {
                    offset: sets_reader.position() as u64,
                    needed,
                    available: sets_reader.remaining(),
                });
            }
            sets.reserve(set_count);
            for _ in 0..set_count {
                let path_count = sets_reader.read_u32()? as usize;
                // Every path carries its fixed record on the wire.
                let needed = path_count
                    .checked_mul(KPath::SIZE)
                    .ok_or_else(|| MapError::invalid("path count overflows"))?;
                if needed > sets_reader.remaining() {
                    return Err(MapError::Truncated {
                        offset: sets_reader.position() as u64,
                        needed,
                        available: sets_reader.remaining(),
                    });
                }
                let mut set = Vec::with_capacity(path_count);
                for _ in 0..path_count {
                    set.push(KPath::from_bytes(sets_reader.read_bytes(KPath::SIZE)?)?);
                }
                sets.push(set);
            }
        }

        let mut nodes = Vec::new();
        let mut nodes_reader = Reader::new(nodes_raw.unwrap_or(&[]));
        if nodes_reader.remaining() >= 4 {
            let node_count = nodes_reader.read_u32()? as usize;
            // Every sample is three f32 values, 12 bytes on the wire.
            let needed = node_count
                .checked_mul(12)
                .ok_or_else(|| MapError::invalid("node count overflows"))?;
            if needed > nodes_reader.remaining() {
                return Err(MapError::Truncated {
                    offset: nodes_reader.position() as u64,
                    needed,
                    available: nodes_reader.remaining(),
                });
            }
            nodes.reserve(node_count);
            for _ in 0..node_count {
                nodes.push([
                    nodes_reader.read_f32()?,
                    nodes_reader.read_f32()?,
                    nodes_reader.read_f32()?,
                ]);
            }
        }

        if let Some(entry) = od_index.iter().find(|e| e.set_index as usize >= sets.len()) {
            return Err(MapError::invalid(format!(
                "OD entry references path set {}, only {} present",
                entry.set_index,
                sets.len()
            )));
        }

        Ok(Self {
            params,
            od_index,
            sets,
            nodes,
        })
    }
}
