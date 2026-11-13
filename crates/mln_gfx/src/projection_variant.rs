/// Which projection prelude a shader is compiled against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ProjectionVariant {
    #[default]
    Mercator,
    Globe,
}
