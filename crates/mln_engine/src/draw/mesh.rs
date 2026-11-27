//! Index buffers drawn in segments: a segment's 16-bit indices address at
//! most [`MAX_SEGMENT_VERTICES`] vertices from its first.

use geometry::Point;

use crate::draw::segment::{MAX_SEGMENT_VERTICES, SegmentBase};

/// Adds the quads that circles and heatmap points are drawn as, one for
/// each of `points`, and indexes their triangles. `vertex` makes each
/// corner from the point doubled, with the corner's right and top sides in
/// the low bits.
pub fn add_point_quads<V: Copy>(
    vertices: &mut Vec<V>,
    triangles: &mut Indexed<'_>,
    points: impl IntoIterator<Item = Point<i16>>,
    vertex: impl Fn([i16; 2]) -> V,
) {
    for p in points {
        let (x, y) = (p.x.wrapping_mul(2), p.y.wrapping_mul(2));
        let first = vertices.len();
        vertices.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]].map(&vertex));
        triangles.add(vertices, first, [[0, 1, 2], [0, 3, 2]]);
    }
}

/// The indexes of one kind of primitive, such as triangles or lines, over a
/// vertex buffer, and the segments that draw them.
///
/// Where a vertex buffer has per-vertex buffers beside it, a mesh larger
/// than a segment, whose split copies vertices, must extend them after.
pub struct Indexed<'a> {
    pub indexes: &'a mut Vec<u16>,
    pub segments: &'a mut Vec<SegmentBase>,
}

impl Indexed<'_> {
    /// Indexes `primitives`, each `N` indices into a mesh whose vertices are
    /// `vertices[first..]`, which follow every vertex the segments draw, and
    /// returns where the mesh's first vertex is in its segment: 0 for a mesh
    /// spread over several, whose first segment starts at it, and for a mesh
    /// without primitives, which no segment draws.
    ///
    /// A mesh that fits in a segment goes in the last one when that ends at
    /// `first` and has room, or else in a new one. A larger mesh is spread
    /// over several: the first draws the mesh's leading vertices in place,
    /// and each later one draws copies, appended to `vertices`, of the
    /// vertices its primitives use. A mesh without primitives adds no
    /// segment.
    pub fn add<V: Copy, const N: usize>(
        &mut self,
        vertices: &mut Vec<V>,
        first: usize,
        primitives: impl IntoIterator<Item = [u32; N]>,
    ) -> usize {
        self.add_keyed(vertices, first, 0.0, primitives)
    }

    /// [`Indexed::add`] into segments of one sort key, which a draw orders
    /// by: a mesh continues the last segment only if it has `sort_key`.
    pub fn add_keyed<V: Copy, const N: usize>(
        &mut self,
        vertices: &mut Vec<V>,
        first: usize,
        sort_key: f32,
        primitives: impl IntoIterator<Item = [u32; N]>,
    ) -> usize {
        let mut primitives = primitives.into_iter().peekable();
        if primitives.peek().is_none() {
            return 0;
        }
        let count = vertices.len() - first;
        if count > MAX_SEGMENT_VERTICES {
            let primitives: Vec<[u32; N]> = primitives.collect();
            for source in self.add_split(first, count, sort_key, &primitives) {
                vertices.push(vertices[source]);
            }
            return 0;
        }

        let index_offset = self.indexes.len();
        self.indexes.reserve(N * primitives.size_hint().0);
        let segment = match self.segments.last_mut() {
            Some(last)
                if last.vertex_offset + last.vertex_length == first
                    && last.vertex_length + count <= MAX_SEGMENT_VERTICES
                    && last.sort_key.total_cmp(&sort_key).is_eq() =>
            {
                last
            }
            _ => self.segments.push_mut(SegmentBase { sort_key, ..SegmentBase::new(first, index_offset) }),
        };
        let base = segment.vertex_length;
        for primitive in primitives {
            self.indexes.extend_from_slice(&primitive.map(|i| (base as u32 + i) as u16));
        }
        segment.vertex_length += count;
        segment.index_length += self.indexes.len() - index_offset;
        base
    }

    /// Spreads `primitives` of the `count` vertices from `first`, which end
    /// the vertex buffer, over segments, and returns the vertices that the
    /// segments after the first draw copies of, to append in order.
    fn add_split<const N: usize>(
        &mut self,
        first: usize,
        count: usize,
        sort_key: f32,
        primitives: &[[u32; N]],
    ) -> Vec<usize> {
        let end = first + count;
        let mut copies = Vec::new();
        // Where each mesh vertex was last put in the vertex buffer, and the
        // segment being filled; it draws the vertices in that range.
        let mut placed: Vec<usize> = (first..end).collect();
        let mut segment =
            SegmentBase { sort_key, ..SegmentBase::with_lengths(first, self.indexes.len(), MAX_SEGMENT_VERTICES, 0) };
        let drawn = |segment: &SegmentBase, at: usize| {
            (segment.vertex_offset..segment.vertex_offset + segment.vertex_length).contains(&at)
        };
        for primitive in primitives {
            let missing = primitive.iter().filter(|&&i| !drawn(&segment, placed[i as usize])).count();
            if segment.vertex_length + missing > MAX_SEGMENT_VERTICES {
                let next = SegmentBase { sort_key, ..SegmentBase::new(end + copies.len(), self.indexes.len()) };
                let full = std::mem::replace(&mut segment, next);
                // The first may have drawn nothing in place.
                if full.index_length > 0 {
                    self.segments.push(full);
                }
            }
            for &i in primitive {
                let i = i as usize;
                if !drawn(&segment, placed[i]) {
                    copies.push(first + i);
                    placed[i] = end + copies.len() - 1;
                    segment.vertex_length += 1;
                }
                self.indexes.push((placed[i] - segment.vertex_offset) as u16);
            }
            segment.index_length += N;
        }
        self.segments.push(segment);
        copies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The primitives a draw of every segment fetches, as vertex values.
    fn drawn<const N: usize>(vertices: &[u32], indexes: &[u16], segments: &[SegmentBase]) -> Vec<[u32; N]> {
        segments
            .iter()
            .flat_map(|segment| {
                assert!(segment.vertex_length <= MAX_SEGMENT_VERTICES);
                assert!(segment.index_length > 0);
                assert!(segment.vertex_offset + segment.vertex_length <= vertices.len());
                indexes[segment.index_offset..segment.index_offset + segment.index_length]
                    .as_chunks::<N>()
                    .0
                    .iter()
                    .map(|primitive| {
                        primitive.map(|i| {
                            assert!(usize::from(i) < segment.vertex_length);
                            vertices[segment.vertex_offset + usize::from(i)]
                        })
                    })
            })
            .collect()
    }

    /// Adds a mesh of `count` vertices, numbered on from `vertices.len()`,
    /// and returns the primitives it should draw.
    fn add_mesh<const N: usize>(
        vertices: &mut Vec<u32>,
        indexed: &mut Indexed<'_>,
        count: u32,
        primitives: &[[u32; N]],
    ) -> Vec<[u32; N]> {
        let first = vertices.len();
        vertices.extend(first as u32..first as u32 + count);
        indexed.add(vertices, first, primitives.iter().copied());
        primitives.iter().map(|p| p.map(|i| first as u32 + i)).collect()
    }

    /// A strip of triangles over `count` vertices.
    fn strip(count: u32) -> Vec<[u32; 3]> {
        (2..count).map(|i| [i - 2, i - 1, i]).collect()
    }

    #[test]
    fn meshes_share_a_segment_while_they_fit() {
        let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        let mut indexed = Indexed { indexes: &mut indexes, segments: &mut segments };
        let mut expected = add_mesh(&mut vertices, &mut indexed, 40_000, &strip(40_000));
        expected.extend(add_mesh(&mut vertices, &mut indexed, 20_000, &strip(20_000)));
        expected.extend(add_mesh(&mut vertices, &mut indexed, 10_000, &strip(10_000)));
        assert_eq!(segments.len(), 2);
        assert_eq!(drawn::<3>(&vertices, &indexes, &segments), expected);
    }

    #[test]
    fn a_mesh_continues_a_segment_only_where_it_ends() {
        let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        let mut indexed = Indexed { indexes: &mut indexes, segments: &mut segments };
        let mut expected = add_mesh(&mut vertices, &mut indexed, 3, &[[0, 1, 2]]);
        // Vertices that the segments don't draw, such as another index
        // buffer's.
        vertices.extend([100, 101]);
        expected.extend(add_mesh(&mut vertices, &mut indexed, 3, &[[0, 1, 2]]));
        assert_eq!(segments.len(), 2);
        assert_eq!(drawn::<3>(&vertices, &indexes, &segments), expected);
    }

    #[test]
    fn a_mesh_continues_a_segment_only_of_its_sort_key() {
        let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        let mut indexed = Indexed { indexes: &mut indexes, segments: &mut segments };
        let mut offsets = Vec::new();
        for sort_key in [1.0, 1.0, 2.0] {
            let first = vertices.len();
            vertices.extend([0u32; 3]);
            offsets.push(indexed.add_keyed(&mut vertices, first, sort_key, [[0, 1, 2]]));
        }
        assert_eq!(offsets, [0, 3, 0]);
        let keys: Vec<(f32, usize)> = segments.iter().map(|s| (s.sort_key, s.vertex_length)).collect();
        assert_eq!(keys, [(1.0, 6), (2.0, 3)]);
    }

    #[test]
    fn a_mesh_without_primitives_adds_no_segment() {
        let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        let mut indexed = Indexed { indexes: &mut indexes, segments: &mut segments };
        add_mesh::<2>(&mut vertices, &mut indexed, 3, &[]);
        assert_eq!(segments, []);
    }

    #[test]
    fn a_mesh_larger_than_a_segment_is_spread_over_several() {
        for count in [70_000, 200_000] {
            let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
            let mut indexed = Indexed { indexes: &mut indexes, segments: &mut segments };
            let mut expected = add_mesh(&mut vertices, &mut indexed, 3, &[[0, 1, 2]]);
            // A strip, and triangles that reach back to its start.
            let mut primitives = strip(count);
            primitives.extend((1..count).step_by(9_999).map(|i| [0, i - 1, i]));
            expected.extend(add_mesh(&mut vertices, &mut indexed, count, &primitives));
            expected.extend(add_mesh(&mut vertices, &mut indexed, 3, &[[0, 1, 2]]));
            assert_eq!(drawn::<3>(&vertices, &indexes, &segments), expected);
            // The first of the large mesh's segments draws its vertices in
            // place.
            assert_eq!((segments[1].vertex_offset, segments[1].vertex_length), (3, MAX_SEGMENT_VERTICES));
        }
    }

    #[test]
    fn a_large_mesh_whose_first_primitive_reaches_past_a_segment_starts_with_copies() {
        let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        let mut indexed = Indexed { indexes: &mut indexes, segments: &mut segments };
        let expected = add_mesh(&mut vertices, &mut indexed, 70_000, &[[0, 69_998, 69_999], [0, 1, 2]]);
        assert_eq!(drawn::<3>(&vertices, &indexes, &segments), expected);
    }

    #[test]
    fn lines_of_a_large_mesh_copy_the_vertices_they_use() {
        let (mut vertices, mut indexes, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        let mut indexed = Indexed { indexes: &mut indexes, segments: &mut segments };
        let ring: Vec<[u32; 2]> = (0..100_000).map(|i| [i, (i + 1) % 100_000]).collect();
        let expected = add_mesh(&mut vertices, &mut indexed, 100_000, &ring);
        assert_eq!(drawn::<2>(&vertices, &indexes, &segments), expected);
    }
}
