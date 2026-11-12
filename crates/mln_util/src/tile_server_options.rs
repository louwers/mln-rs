// Ports include/mln/util/tile_server_options.hpp and src/mln/util/tile_server_options.cpp.
//
// (MapboxConfiguration and MapTilerConfiguration added by glfw.)

/// `TileServerOptions`. `Clone` is `clone()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileServerOptions {
    base_url: String,
    uri_scheme_alias: String,

    source_template: String,
    source_domain_name: String,
    source_version_prefix: Option<String>,

    style_template: String,
    style_domain_name: String,
    style_version_prefix: Option<String>,

    sprites_template: String,
    sprites_domain_name: String,
    sprites_version_prefix: Option<String>,

    glyphs_template: String,
    glyphs_domain_name: String,
    glyphs_version_prefix: Option<String>,

    tile_template: String,
    tile_domain_name: String,
    tile_version_prefix: Option<String>,

    api_key_parameter_name: String,
    // C++ leaves `apiKeyRequired` uninitialized in a default-constructed
    // object; every configuration sets it.
    api_key_required: bool,
}

macro_rules! template_accessors {
    ($with:ident, $template:ident, $domain:ident, $prefix:ident) => {
        #[must_use]
        pub fn $with(
            mut self,
            template: impl Into<String>,
            domain_name: impl Into<String>,
            version_prefix: Option<String>,
        ) -> Self {
            self.$template = template.into();
            self.$domain = domain_name.into();
            self.$prefix = version_prefix;
            self
        }
        pub fn $template(&self) -> &str {
            &self.$template
        }
        pub fn $domain(&self) -> &str {
            &self.$domain
        }
        pub fn $prefix(&self) -> &Option<String> {
            &self.$prefix
        }
    };
}

impl TileServerOptions {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub fn with_uri_scheme_alias(mut self, alias: impl Into<String>) -> Self {
        self.uri_scheme_alias = alias.into();
        self
    }

    #[must_use]
    pub fn uri_scheme_alias(&self) -> &str {
        &self.uri_scheme_alias
    }

    template_accessors!(with_source_template, source_template, source_domain_name, source_version_prefix);
    template_accessors!(with_style_template, style_template, style_domain_name, style_version_prefix);
    template_accessors!(with_sprites_template, sprites_template, sprites_domain_name, sprites_version_prefix);
    template_accessors!(with_glyphs_template, glyphs_template, glyphs_domain_name, glyphs_version_prefix);
    template_accessors!(with_tile_template, tile_template, tile_domain_name, tile_version_prefix);

    #[must_use]
    pub fn with_api_key_parameter_name(mut self, name: impl Into<String>) -> Self {
        self.api_key_parameter_name = name.into();
        self
    }

    #[must_use]
    pub fn api_key_parameter_name(&self) -> &str {
        &self.api_key_parameter_name
    }

    #[must_use]
    pub fn set_requires_api_key(mut self, api_key_required: bool) -> Self {
        self.api_key_required = api_key_required;
        self
    }

    #[must_use]
    pub fn requires_api_key(&self) -> bool {
        self.api_key_required
    }

    /// `DefaultConfiguration()`.
    #[must_use]
    pub fn default_configuration() -> Self {
        Self::maplibre_configuration()
    }

    /// `MapLibreConfiguration()`.
    #[must_use]
    pub fn maplibre_configuration() -> Self {
        TileServerOptions::new()
            .with_base_url("https://demotiles.maplibre.org")
            .with_uri_scheme_alias("maplibre")
            .with_api_key_parameter_name("")
            .with_source_template("/tiles/{domain}.json", "", None)
            .with_style_template("{path}.json", "maps", None)
            .with_sprites_template("/{path}/sprite{scale}.{format}", "", None)
            .with_glyphs_template("/font/{fontstack}/{start}-{end}.pbf", "fonts", None)
            .with_tile_template("/{path}", "tiles", None)
            .set_requires_api_key(false)
    }

    /// `MapboxConfiguration()`.
    #[must_use]
    pub fn mapbox_configuration() -> Self {
        TileServerOptions::new()
            .with_base_url("https://api.mapbox.com")
            .with_uri_scheme_alias("mapbox")
            .with_api_key_parameter_name("access_token")
            .with_source_template("/{domain}.json", "", Some("/v4".to_owned()))
            .with_style_template("/styles/v1{path}", "styles", None)
            .with_sprites_template("/styles/v1{directory}{filename}/sprite{extension}", "sprites", None)
            .with_glyphs_template("/fonts/v1{path}", "fonts", None)
            .with_tile_template("{path}", "tiles", Some("/v4".to_owned()))
            .set_requires_api_key(true)
    }

    /// `MapTilerConfiguration()`.
    #[must_use]
    pub fn map_tiler_configuration() -> Self {
        TileServerOptions::new()
            .with_base_url("https://api.maptiler.com")
            .with_uri_scheme_alias("maptiler")
            .with_api_key_parameter_name("key")
            .with_source_template("/tiles{path}/tiles.json", "sources", None)
            .with_style_template("/maps{path}/style.json", "maps", None)
            .with_sprites_template("/maps{path}", "sprites", None)
            .with_glyphs_template("/fonts{path}", "fonts", None)
            .with_tile_template("{path}", "tiles", None)
            .set_requires_api_key(true)
    }
}
