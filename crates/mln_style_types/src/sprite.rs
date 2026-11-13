// Cut point (added by storage, see image.rs): `mln_style::sprite`
// re-exports it.

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
