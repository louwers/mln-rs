/// Graphic primitive type enumeration
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PrimitiveType {
    Point,
    Line,
    Triangle,
}

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
