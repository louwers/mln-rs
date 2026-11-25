/// A sprite that a style names: its id, and the URL its images and index
/// load from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sprite {
    pub id: String,
    pub sprite_url: String,
}

impl Sprite {
    pub fn new(id: impl Into<String>, sprite_url: impl Into<String>) -> Self {
        Sprite { id: id.into(), sprite_url: sprite_url.into() }
    }
}
