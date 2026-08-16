//! Cube faces and the axis bookkeeping the mesher needs.

/// The six faces of a voxel, in a fixed order the renderer and mesher agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Face {
    NegX = 0,
    PosX = 1,
    NegY = 2,
    PosY = 3,
    NegZ = 4,
    PosZ = 5,
}

/// All faces, in discriminant order. Handy for `for face in Face::ALL`.
impl Face {
    pub const ALL: [Face; 6] = [
        Face::NegX,
        Face::PosX,
        Face::NegY,
        Face::PosY,
        Face::NegZ,
        Face::PosZ,
    ];

    /// Unit offset from a voxel to the neighbour across this face.
    pub const fn offset(self) -> [i32; 3] {
        match self {
            Face::NegX => [-1, 0, 0],
            Face::PosX => [1, 0, 0],
            Face::NegY => [0, -1, 0],
            Face::PosY => [0, 1, 0],
            Face::NegZ => [0, 0, -1],
            Face::PosZ => [0, 0, 1],
        }
    }

    /// The axis this face's normal runs along: 0 = x, 1 = y, 2 = z.
    pub const fn axis(self) -> usize {
        match self {
            Face::NegX | Face::PosX => 0,
            Face::NegY | Face::PosY => 1,
            Face::NegZ | Face::PosZ => 2,
        }
    }

    /// Whether the normal points along the positive direction of its axis.
    pub const fn is_positive(self) -> bool {
        matches!(self, Face::PosX | Face::PosY | Face::PosZ)
    }

    /// The face pointing the opposite way.
    pub const fn opposite(self) -> Face {
        match self {
            Face::NegX => Face::PosX,
            Face::PosX => Face::NegX,
            Face::NegY => Face::PosY,
            Face::PosY => Face::NegY,
            Face::NegZ => Face::PosZ,
            Face::PosZ => Face::NegZ,
        }
    }

    /// Outward unit normal as floats, for vertex data.
    pub const fn normal(self) -> [f32; 3] {
        match self {
            Face::NegX => [-1.0, 0.0, 0.0],
            Face::PosX => [1.0, 0.0, 0.0],
            Face::NegY => [0.0, -1.0, 0.0],
            Face::PosY => [0.0, 1.0, 0.0],
            Face::NegZ => [0.0, 0.0, -1.0],
            Face::PosZ => [0.0, 0.0, 1.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_is_an_involution() {
        for face in Face::ALL {
            assert_eq!(face.opposite().opposite(), face);
            assert_ne!(face.opposite(), face);
        }
    }

    #[test]
    fn opposite_negates_the_offset() {
        for face in Face::ALL {
            let a = face.offset();
            let b = face.opposite().offset();
            assert_eq!([-a[0], -a[1], -a[2]], b);
        }
    }

    #[test]
    fn offset_is_nonzero_only_on_its_own_axis() {
        for face in Face::ALL {
            for (axis, component) in face.offset().into_iter().enumerate() {
                if axis == face.axis() {
                    assert_ne!(component, 0);
                    assert_eq!(component > 0, face.is_positive());
                } else {
                    assert_eq!(component, 0);
                }
            }
        }
    }

    #[test]
    fn normal_matches_offset() {
        for face in Face::ALL {
            let offset = face.offset();
            let normal = face.normal();
            for axis in 0..3 {
                assert_eq!(normal[axis], offset[axis] as f32);
            }
        }
    }
}
