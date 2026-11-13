/// Something that debug groups can be pushed to (`CommandEncoder`,
/// `RenderPass`, `UploadPass`).
pub trait DebugGroupTarget {
    fn push_debug_group(&self, name: &str);
    fn pop_debug_group(&self);
}
