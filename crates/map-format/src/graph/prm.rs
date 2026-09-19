//! PRM waypoint graph.
//!
//! The probabilistic roadmap models how people actually cross open areas:
//! desire lines across a plaza, cut corners over a lawn. The graph is built in
//! preprocessing — sampling at runtime would be both wasteful and
//! non-reproducible — and each batch carries the seed that produced it, so a
//! rebuilt file yields exactly the same corridors.
//!
//! Nodes are stored with the `region interface` flag rather than in a separate
//! table; interface links, which couple an interface node to a fine-grid cell,
//! form their own section.

use crate::bytes::{Reader, Writer};
use crate::error::{MapError, Result};

/// Waypoint node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrmNode {
    /// Position in local metres; `z` is the terrain elevation at that point.
    pub position: [f32; 3],
    /// Node flags, see [`PrmNode::INTERFACE`] and friends.
    pub flags: u16,
}

impl PrmNode {
    /// Region interface node: stitches the PRM graph to the fine grid.
    pub const INTERFACE: u16 = 1 << 0;
    /// Node coincides with a Z-axis connector endpoint.
    pub const CONNECTOR_ENDPOINT: u16 = 1 << 1;
    /// Node lies in a direction-constrained area.
    pub const DIRECTION_CONSTRAINED: u16 = 1 << 2;

    /// Creates a plain waypoint.
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            position: [x, y, z],
            flags: 0,
        }
    }

    /// Serialised size in bytes.
    pub const SIZE: usize = 14;

    /// True when the node is a region interface node.
    #[inline]
    pub fn is_interface(&self) -> bool {
        self.flags & Self::INTERFACE != 0
    }

    /// Serialises the node.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_f32(&mut buf, 0, self.position[0]);
        let _ = crate::bytes::put_f32(&mut buf, 4, self.position[1]);
        let _ = crate::bytes::put_f32(&mut buf, 8, self.position[2]);
        let _ = crate::bytes::put_u16(&mut buf, 12, self.flags);
        buf
    }

    /// Parses a node.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            position: [
                crate::bytes::get_f32(buf, 0)?,
                crate::bytes::get_f32(buf, 4)?,
                crate::bytes::get_f32(buf, 8)?,
            ],
            flags: crate::bytes::get_u16(buf, 12)?,
        })
    }
}

/// Directed graph edge in the CSR edge array.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrmEdge {
    /// Target node index.
    pub to: u32,
    /// Cost in equivalent metres (preference-weighted distance).
    pub cost_equiv_m: f32,
    /// Geometric length in metres.
    pub len_m: f32,
    /// Compass index of the edge direction (16 directions), for direction cost.
    pub dir: u8,
    /// Edge flags, reserved.
    pub flags: u8,
}

impl PrmEdge {
    /// Serialised size in bytes.
    pub const SIZE: usize = 14;

    /// Serialises the edge.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_u32(&mut buf, 0, self.to);
        let _ = crate::bytes::put_f32(&mut buf, 4, self.cost_equiv_m);
        let _ = crate::bytes::put_f32(&mut buf, 8, self.len_m);
        let _ = crate::bytes::put_u8(&mut buf, 12, self.dir);
        let _ = crate::bytes::put_u8(&mut buf, 13, self.flags);
        buf
    }

    /// Parses an edge.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            to: crate::bytes::get_u32(buf, 0)?,
            cost_equiv_m: crate::bytes::get_f32(buf, 4)?,
            len_m: crate::bytes::get_f32(buf, 8)?,
            dir: crate::bytes::get_u8(buf, 12)?,
            flags: crate::bytes::get_u8(buf, 13)?,
        })
    }
}

/// Coupling between an interface node and a fine-grid cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterfaceLink {
    /// PRM node index of the interface node.
    pub prm_node: u32,
    /// Fine-grid cell coordinates at level 0.
    pub grid_cell: [u32; 2],
    /// Cost of the connecting edge in equivalent metres.
    pub cost_equiv_m: f32,
    /// Geometric length of the connecting edge in metres.
    pub len_m: f32,
}

impl InterfaceLink {
    /// Serialised size in bytes.
    pub const SIZE: usize = 20;

    /// Serialises the link.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let _ = crate::bytes::put_u32(&mut buf, 0, self.prm_node);
        let _ = crate::bytes::put_u32(&mut buf, 4, self.grid_cell[0]);
        let _ = crate::bytes::put_u32(&mut buf, 8, self.grid_cell[1]);
        let _ = crate::bytes::put_f32(&mut buf, 12, self.cost_equiv_m);
        let _ = crate::bytes::put_f32(&mut buf, 16, self.len_m);
        buf
    }

    /// Parses a link.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(MapError::Truncated {
                offset: 0,
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        Ok(Self {
            prm_node: crate::bytes::get_u32(buf, 0)?,
            grid_cell: [
                crate::bytes::get_u32(buf, 4)?,
                crate::bytes::get_u32(buf, 8)?,
            ],
            cost_equiv_m: crate::bytes::get_f32(buf, 12)?,
            len_m: crate::bytes::get_f32(buf, 16)?,
        })
    }
}

/// One PRM batch: a complete roadmap built from a single seed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrmGraph {
    /// Batch index, matching the position in the `PRM_SEEDS` list.
    pub batch: u16,
    /// Seed this batch was sampled with.
    pub seed: u64,
    /// Waypoints.
    pub nodes: Vec<PrmNode>,
    /// CSR offsets: node `i` owns edges `offsets[i]..offsets[i + 1]`.
    pub offsets: Vec<u32>,
    /// All edges.
    pub edges: Vec<PrmEdge>,
    /// Interface links to the fine grid.
    pub interfaces: Vec<InterfaceLink>,
}

impl PrmGraph {
    /// Node count.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Edge count.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Edges leaving `node`.
    pub fn neighbours(&self, node: usize) -> &[PrmEdge] {
        match (self.offsets.get(node), self.offsets.get(node + 1)) {
            (Some(start), Some(end)) => &self.edges[*start as usize..*end as usize],
            _ => &[],
        }
    }

    /// Verifies the CSR arrays are consistent with the node and edge counts.
    pub fn validate(&self) -> Result<()> {
        if self.offsets.len() != self.nodes.len() + 1 {
            return Err(MapError::invalid(format!(
                "PRM offsets length {} does not match {} node(s)",
                self.offsets.len(),
                self.nodes.len()
            )));
        }
        if self.offsets.last().copied().unwrap_or(0) as usize != self.edges.len() {
            return Err(MapError::invalid("PRM offsets do not cover the edge array"));
        }
        if self.offsets.windows(2).any(|w| w[0] > w[1]) {
            return Err(MapError::invalid("PRM offsets are not monotonic"));
        }
        if let Some(edge) = self
            .edges
            .iter()
            .find(|e| e.to as usize >= self.nodes.len())
        {
            return Err(MapError::invalid(format!(
                "PRM edge points at node {}, only {} exist",
                edge.to,
                self.nodes.len()
            )));
        }
        Ok(())
    }

    /// Rebuilds the CSR offsets from a sorted adjacency list.
    ///
    /// Edges whose source node is out of range are dropped, and the two passes
    /// apply the same filter so that the placement pass cannot index past the
    /// counted range.
    pub fn build_offsets(nodes: usize, edges: &[(u32, PrmEdge)]) -> (Vec<u32>, Vec<PrmEdge>) {
        let mut offsets = vec![0u32; nodes + 1];
        let mut kept = 0usize;
        for (from, _) in edges {
            if (*from as usize) < nodes {
                offsets[*from as usize + 1] += 1;
                kept += 1;
            }
        }
        for index in 0..nodes {
            offsets[index + 1] += offsets[index];
        }
        let mut sorted = vec![PrmEdge::default_edge(); kept];
        let mut cursor = offsets.clone();
        for (from, edge) in edges {
            let from = *from as usize;
            if from >= nodes {
                continue;
            }
            let at = cursor[from] as usize;
            sorted[at] = *edge;
            cursor[from] += 1;
        }
        (offsets, sorted)
    }

    /// Serialises the batch into a graph-layer payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut nodes = Writer::with_capacity(self.nodes.len() * PrmNode::SIZE);
        for node in &self.nodes {
            nodes.write_bytes(&node.to_bytes());
        }
        let mut offsets = Writer::with_capacity(self.offsets.len() * 4);
        for offset in &self.offsets {
            offsets.write_u32(*offset);
        }
        let mut edges = Writer::with_capacity(self.edges.len() * PrmEdge::SIZE);
        for edge in &self.edges {
            edges.write_bytes(&edge.to_bytes());
        }
        let mut interfaces = Writer::with_capacity(self.interfaces.len() * InterfaceLink::SIZE);
        for link in &self.interfaces {
            interfaces.write_bytes(&link.to_bytes());
        }

        let mut out = Writer::new();
        out.write_u16(self.batch);
        out.write_u16(0);
        out.write_u64(self.seed);
        out.write_u32(self.nodes.len() as u32);
        out.write_u32(self.edges.len() as u32);
        out.write_u32(self.interfaces.len() as u32);
        out.write_bytes(&super::write_section(
            super::SectionId::PrmNodes,
            nodes.as_slice(),
        ));
        out.write_bytes(&super::write_section(
            super::SectionId::PrmOffsets,
            offsets.as_slice(),
        ));
        out.write_bytes(&super::write_section(
            super::SectionId::PrmEdges,
            edges.as_slice(),
        ));
        out.write_bytes(&super::write_section(
            super::SectionId::PrmInterfaces,
            interfaces.as_slice(),
        ));
        out.into_vec()
    }

    /// Parses a batch payload.
    // `array_chunks` would express the fixed-size records more directly but is not stable yet.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let batch = reader.read_u16()?;
        let _reserved = reader.read_u16()?;
        let seed = reader.read_u64()?;
        let node_count = reader.read_u32()? as usize;
        let edge_count = reader.read_u32()? as usize;
        let interface_count = reader.read_u32()? as usize;
        let body = reader.read_bytes(reader.remaining())?;

        let mut nodes_raw = None;
        let mut offsets_raw = None;
        let mut edges_raw = None;
        let mut interfaces_raw = None;
        let mut sections = super::SectionReader::new(body);
        while let Some((id, payload)) = sections.next_section()? {
            match super::SectionId::from_u16(id) {
                Some(super::SectionId::PrmNodes) => nodes_raw = Some(payload),
                Some(super::SectionId::PrmOffsets) => offsets_raw = Some(payload),
                Some(super::SectionId::PrmEdges) => edges_raw = Some(payload),
                Some(super::SectionId::PrmInterfaces) => interfaces_raw = Some(payload),
                _ => {}
            }
        }

        let nodes_bytes = nodes_raw.unwrap_or(&[]);
        if nodes_bytes.len() < node_count * PrmNode::SIZE {
            return Err(MapError::invalid(
                "PRM node section is shorter than declared",
            ));
        }
        let mut nodes = Vec::with_capacity(node_count);
        for chunk in nodes_bytes.chunks_exact(PrmNode::SIZE).take(node_count) {
            nodes.push(PrmNode::from_bytes(chunk)?);
        }

        let offsets_bytes = offsets_raw.unwrap_or(&[]);
        let mut offsets = Vec::with_capacity(node_count + 1);
        let mut offsets_reader = Reader::new(offsets_bytes);
        while !offsets_reader.is_empty() {
            offsets.push(offsets_reader.read_u32()?);
        }
        if offsets.len() != node_count + 1 {
            return Err(MapError::invalid(format!(
                "PRM offset section holds {} value(s), expected {}",
                offsets.len(),
                node_count + 1
            )));
        }

        let edges_bytes = edges_raw.unwrap_or(&[]);
        if edges_bytes.len() < edge_count * PrmEdge::SIZE {
            return Err(MapError::invalid(
                "PRM edge section is shorter than declared",
            ));
        }
        let mut edges = Vec::with_capacity(edge_count);
        for chunk in edges_bytes.chunks_exact(PrmEdge::SIZE).take(edge_count) {
            edges.push(PrmEdge::from_bytes(chunk)?);
        }

        let interfaces_bytes = interfaces_raw.unwrap_or(&[]);
        if interfaces_bytes.len() < interface_count * InterfaceLink::SIZE {
            return Err(MapError::invalid(
                "PRM interface section is shorter than declared",
            ));
        }
        let mut interfaces = Vec::with_capacity(interface_count);
        for chunk in interfaces_bytes
            .chunks_exact(InterfaceLink::SIZE)
            .take(interface_count)
        {
            interfaces.push(InterfaceLink::from_bytes(chunk)?);
        }

        let graph = Self {
            batch,
            seed,
            nodes,
            offsets,
            edges,
            interfaces,
        };
        graph.validate()?;
        Ok(graph)
    }
}

impl PrmEdge {
    /// Zero-valued edge used to pre-size the sorted edge array.
    fn default_edge() -> Self {
        Self {
            to: 0,
            cost_equiv_m: 0.0,
            len_m: 0.0,
            dir: 0,
            flags: 0,
        }
    }
}
