/// We can choose to constrain the map both horizontally or vertically, only
/// vertically e.g. while panning, or screen to the specified bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ConstrainMode {
    None,
    #[default]
    HeightOnly,
    WidthAndHeight,
    Screen,
}

/// Satisfies embedding platforms that requires the viewport coordinate
/// systems to be set according to its standards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ViewportMode {
    #[default]
    Default,
    FlippedY,
}
