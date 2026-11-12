// Ports include/mln/util/tile_server_options.hpp, src/mln/util/tile_server_options.cpp
// and include/mln/util/default_style.hpp (+ default_style.cpp).
//
// (MapboxConfiguration and MapTilerConfiguration added by glfw.)

/// `util::DefaultStyle`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DefaultStyle {
    url: String,
    name: String,
    current_version: i32,
}

impl DefaultStyle {
    pub fn new(url: impl Into<String>, name: impl Into<String>, current_version: i32) -> Self {
        DefaultStyle { url: url.into(), name: name.into(), current_version }
    }

    #[must_use]
    pub fn get_url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub fn get_name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn get_current_version(&self) -> i32 {
        self.current_version
    }
}

/// `TileServerOptions`. `Clone` is `clone()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileServerOptions {
    base_url: String,
    version_prefix: Option<String>,
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

    default_styles: Vec<DefaultStyle>,
    default_style: String,
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
    pub fn with_version_prefix(mut self, version_prefix: Option<String>) -> Self {
        self.version_prefix = version_prefix;
        self
    }

    #[must_use]
    pub fn version_prefix(&self) -> &Option<String> {
        &self.version_prefix
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

    #[must_use]
    pub fn default_styles(&self) -> Vec<DefaultStyle> {
        self.default_styles.clone()
    }

    #[must_use]
    pub fn with_default_styles(mut self, styles: Vec<DefaultStyle>) -> Self {
        self.default_styles = styles;
        self
    }

    #[must_use]
    pub fn with_default_style(mut self, default_style: impl Into<String>) -> Self {
        self.default_style = default_style.into();
        self
    }

    #[must_use]
    pub fn default_style(&self) -> &str {
        &self.default_style
    }

    /// `DefaultConfiguration()`.
    #[must_use]
    pub fn default_configuration() -> Self {
        Self::maplibre_configuration()
    }

    /// `MapLibreConfiguration()`.
    #[must_use]
    pub fn maplibre_configuration() -> Self {
        // https://demotiles.maplibre.org/style.json
        let styles = vec![DefaultStyle::new("maplibre://maps/style", "Basic", 1)];

        TileServerOptions::new()
            .with_base_url("https://demotiles.maplibre.org")
            .with_uri_scheme_alias("maplibre")
            .with_api_key_parameter_name("")
            .with_source_template("/tiles/{domain}.json", "", None)
            .with_style_template("{path}.json", "maps", None)
            .with_sprites_template("/{path}/sprite{scale}.{format}", "", None)
            .with_glyphs_template("/font/{fontstack}/{start}-{end}.pbf", "fonts", None)
            .with_tile_template("/{path}", "tiles", None)
            .with_default_styles(styles)
            .with_default_style("Basic")
            .set_requires_api_key(false)
    }

    /// `MapboxConfiguration()`.
    #[must_use]
    pub fn mapbox_configuration() -> Self {
        let styles = vec![
            DefaultStyle::new("mapbox://styles/mapbox/streets-v11", "Streets", 11),
            DefaultStyle::new("mapbox://styles/mapbox/outdoors-v11", "Outdoors", 11),
            DefaultStyle::new("mapbox://styles/mapbox/light-v10", "Light", 10),
            DefaultStyle::new("mapbox://styles/mapbox/dark-v10", "Dark", 10),
            DefaultStyle::new("mapbox://styles/mapbox/satellite-v9", "Satellite", 9),
            DefaultStyle::new("mapbox://styles/mapbox/satellite-streets-v11", "Satellite Streets", 11),
        ];

        TileServerOptions::new()
            .with_base_url("https://api.mapbox.com")
            .with_uri_scheme_alias("mapbox")
            .with_api_key_parameter_name("access_token")
            .with_source_template("/{domain}.json", "", Some("/v4".to_owned()))
            .with_style_template("/styles/v1{path}", "styles", None)
            .with_sprites_template("/styles/v1{directory}{filename}/sprite{extension}", "sprites", None)
            .with_glyphs_template("/fonts/v1{path}", "fonts", None)
            .with_tile_template("{path}", "tiles", Some("/v4".to_owned()))
            .with_default_styles(styles)
            .with_default_style("Streets")
            .set_requires_api_key(true)
    }

    /// `MapTilerConfiguration()`.
    #[must_use]
    pub fn map_tiler_configuration() -> Self {
        let styles = vec![
            DefaultStyle::new("maptiler://maps/streets", "Streets", 1),
            DefaultStyle::new("maptiler://maps/outdoor", "Outdoor", 1),
            DefaultStyle::new("maptiler://maps/basic", "Basic", 1),
            DefaultStyle::new("maptiler://maps/bright", "Bright", 1),
            DefaultStyle::new("maptiler://maps/pastel", "Pastel", 1),
            DefaultStyle::new("maptiler://maps/hybrid", "Satellite Hybrid", 1),
            DefaultStyle::new("maptiler://maps/topo", "Satellite Topo", 1),
        ];

        TileServerOptions::new()
            .with_base_url("https://api.maptiler.com")
            .with_uri_scheme_alias("maptiler")
            .with_api_key_parameter_name("key")
            .with_source_template("/tiles{path}/tiles.json", "sources", None)
            .with_style_template("/maps{path}/style.json", "maps", None)
            .with_sprites_template("/maps{path}", "sprites", None)
            .with_glyphs_template("/fonts{path}", "fonts", None)
            .with_tile_template("{path}", "tiles", None)
            .with_default_styles(styles)
            .with_default_style("Streets")
            .set_requires_api_key(true)
    }
}
