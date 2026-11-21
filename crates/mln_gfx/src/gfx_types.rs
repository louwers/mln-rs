/// Drawing mode type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DrawModeType {
    Points,
    Lines,
    LineLoop,
    LineStrip,
    Triangles,
    TriangleStrip,
    TriangleFan,
}

/// Attribute data types
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AttributeDataType {
    Byte,
    Byte2,
    Byte3,
    Byte4,
    UByte,
    UByte2,
    UByte3,
    UByte4,
    Short,
    Short2,
    Short3,
    Short4,
    UShort,
    UShort2,
    UShort3,
    UShort4,
    UShort8,
    Int,
    Int2,
    Int3,
    Int4,
    UInt,
    UInt2,
    UInt3,
    UInt4,
    Float,
    Float2,
    Float3,
    Float4,
    UByte4Normalized,
    Invalid = 255,
}

impl AttributeDataType {
    /// The bytes a value of the type takes in a vertex buffer.
    #[must_use]
    pub fn stride(self) -> usize {
        use AttributeDataType::{
            Byte, Byte2, Byte3, Byte4, Float, Float2, Float3, Float4, Int, Int2, Int3, Int4, Invalid, Short, Short2,
            Short3, Short4, UByte, UByte2, UByte3, UByte4, UByte4Normalized, UInt, UInt2, UInt3, UInt4, UShort,
            UShort2, UShort3, UShort4, UShort8,
        };
        match self {
            Byte | UByte => 1,
            Byte2 | UByte2 | Short | UShort => 2,
            Byte3 | UByte3 => 3,
            Byte4 | UByte4 | UByte4Normalized | Short2 | UShort2 | Int | UInt | Float => 4,
            Short3 | UShort3 => 6,
            Short4 | UShort4 | Int2 | UInt2 | Float2 => 8,
            Int3 | UInt3 | Float3 => 12,
            UShort8 | Int4 | UInt4 | Float4 => 16,
            Invalid => 0,
        }
    }
}
