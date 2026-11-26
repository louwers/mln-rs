use std::sync::Arc;

use mln_engine::Registry;
use mln_engine::camera::{BoundOptions, ProjectionMode};
use mln_engine::map::{Map, MapOptions};
use mln_engine::worker::worker_codec::{RemoteWorkers, ThreadWorkerInstances};
use mln_headless::HeadlessFrontend;
use mln_storage::{FileSource, FileSourceExt};
use mln_style::Edit;
use mln_style::bitmap::PremultipliedImage;
use mln_style::time::Instant;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

use crate::file_source::{Assets, proxy_loader};
use crate::metadata::{MapMode, TerrainSkirtLength, TestMetadata};
use crate::runner::{
    CameraOptions, ImplFactory, ImplOptions, MapError, QueryGeometry, ResourceOptions, TestRunnerImpl,
};
use crate::style_ops;

fn terrain_skirt_length(length: TerrainSkirtLength) -> mln_engine::map::TerrainSkirtLength {
    match length {
        TerrainSkirtLength::Auto => mln_engine::map::TerrainSkirtLength::Auto,
        TerrainSkirtLength::None => mln_engine::map::TerrainSkirtLength::None,
    }
}

fn map_mode(mode: MapMode) -> mln_engine::map::MapMode {
    match mode {
        MapMode::Continuous => mln_engine::map::MapMode::Continuous,
        MapMode::Static => mln_engine::map::MapMode::Static,
        MapMode::Tile => mln_engine::map::MapMode::Tile,
    }
}

pub(crate) struct Impl {
    pub(crate) frontend: HeadlessFrontend,
    /// The `ProxyFileSource` over the cache database at the resource
    /// options' cache path.
    pub(crate) file_source: Option<Arc<dyn FileSource>>,
    pub(crate) map: Map,
    /// The runner's queue, which the frontend runs while it waits.
    queue: Queue<()>,
}

impl Impl {
    /// A frontend, file source and map for a test with `metadata`, loading
    /// through a [`proxy_loader`].
    pub(crate) fn new(
        metadata: &TestMetadata,
        resource_options: &ResourceOptions,
        impl_options: ImplOptions,
        queue: &Queue<()>,
        pool: &Pool,
    ) -> Result<Impl, String> {
        let assets = (!resource_options.asset_path.is_empty())
            .then_some(Assets { path: resource_options.asset_path.as_str(), only: resource_options.assets_only });
        let resource_options = mln_storage::ResourceOptions {
            cache_path: resource_options.cache_path.clone(),
            ..mln_storage::ResourceOptions::default()
        };
        let registry = Registry::with_builtins();
        let mut frontend =
            HeadlessFrontend::new(metadata.size, metadata.pixel_ratio, queue, pool).map_err(|e| e.to_string())?;
        if impl_options.tile_worker_codec > 0 {
            let count = impl_options.tile_worker_codec;
            let instances = Arc::clone(&registry);
            let workers = RemoteWorkers::new(Arc::clone(&registry), move |replies| {
                Box::new(ThreadWorkerInstances::new(count, instances, &replies))
            });
            frontend.renderer().set_tile_workers(workers);
        }
        let file_source = proxy_loader(&resource_options, assets, pool);
        let mut map = Map::with_registry(
            Arc::clone(&file_source),
            MapOptions {
                map_mode: map_mode(metadata.map_mode),
                size: metadata.size,
                pixel_ratio: metadata.pixel_ratio,
                cross_source_collisions: metadata.cross_source_collisions,
                terrain: mln_engine::map::TerrainOptions {
                    skirt_length: terrain_skirt_length(metadata.terrain_skirt_length),
                    ..Default::default()
                },
                ..Default::default()
            },
            Arc::clone(registry.style()),
            pool,
        )
        .map_err(|e| e.to_string())?;
        frontend.attach(&mut map);
        if let Some(max_pitch) = metadata.max_pitch {
            map.set_bounds(&BoundOptions { max_pitch: Some(f64::from(max_pitch)), ..Default::default() })
                .map_err(|e| e.to_string())?;
        }
        Ok(Impl { frontend, file_source: Some(file_source), map, queue: queue.clone() })
    }

    /// Changes the map's tile settings.
    fn set_tile(&mut self, change: impl FnOnce(&mut mln_engine::map::TileSettings)) {
        let mut tile = self.map.snapshot().tile;
        change(&mut tile);
        self.map.set_tile_settings(&tile);
    }
}

impl TestRunnerImpl for Impl {
    fn reset_context(&mut self, metadata: &TestMetadata) -> Result<(), MapError> {
        self.frontend.clear_data();
        self.frontend.set_size(metadata.size);
        self.map.set_size(metadata.size).map_err(|e| MapError::Exception(e.to_string()))?;
        self.map.set_projection_mode(
            &ProjectionMode::default()
                .with_axonometric(metadata.axonometric)
                .with_x_skew(metadata.x_skew)
                .with_y_skew(metadata.y_skew),
        );
        // A test that unclamps the center leaves the map to the next one.
        self.map.set_center_clamped_to_ground(true);
        self.map.load_style_json(&metadata.document_json);
        // C++ parses a style as it loads: the operations after it see it.
        self.map.process_events();
        Ok(())
    }

    fn jump_to_default_camera(&mut self) -> Result<(), MapError> {
        let camera = style_ops::default_camera(self.map.style());
        self.map.jump_to(&camera);
        Ok(())
    }

    fn render(&mut self) -> Result<PremultipliedImage, String> {
        let result = self.frontend.render(&mut self.map);
        // No operation reads the map's events.
        self.map.drain_events();
        result.map(|result| result.image).map_err(|e| e.to_string())
    }

    fn dump_debug_logs(&mut self) {
        self.map.dump_debug_logs();
        self.frontend.renderer().dump_debug_logs();
    }

    // query: tile mode.
    fn default_camera(&mut self) -> Result<CameraOptions, MapError> {
        let camera = style_ops::default_camera(self.map.style());
        Ok(CameraOptions {
            center: camera.center,
            center_altitude: camera.center_altitude,
            zoom: camera.zoom,
            bearing: camera.bearing,
            pitch: camera.pitch,
            roll: camera.roll,
        })
    }

    fn tile_cover_for_camera(
        &mut self,
        camera: &CameraOptions,
        zoom: u8,
    ) -> Result<Vec<crate::runner::TileId>, MapError> {
        let camera = mln_engine::camera::CameraOptions {
            center: camera.center,
            center_altitude: camera.center_altitude,
            zoom: camera.zoom,
            bearing: camera.bearing,
            pitch: camera.pitch,
            roll: camera.roll,
            ..Default::default()
        };
        let bounds = self.map.lat_lng_bounds_for_camera(&camera).map_err(|e| MapError::Exception(e.to_string()))?;
        Ok(mln_engine::tile_store::tile_cover::tile_cover_bounds(&bounds, zoom)
            .into_iter()
            .map(|id| crate::runner::TileId { z: id.canonical.z, x: id.canonical.x, y: id.canonical.y, wrap: id.wrap })
            .collect())
    }

    fn collect_placed_symbol_data(&mut self, enable: bool) {
        self.frontend.renderer().collect_placed_symbol_data(enable);
    }

    fn get_placed_symbols_data(&mut self) -> Vec<crate::runner::PlacedSymbol> {
        let to_box = |b: &mln_engine::placement::Aabb| [b.min.x, b.min.y, b.max.x, b.max.y];
        self.frontend
            .renderer()
            .placed_symbols_data()
            .iter()
            .map(|data| crate::runner::PlacedSymbol {
                key: data.key.clone(),
                text_collision_box: data.text_collision_box.as_ref().map(to_box),
                icon_collision_box: data.icon_collision_box.as_ref().map(to_box),
                text_placed: data.text_placed,
                icon_placed: data.icon_placed,
                intersects_tile_border: data.intersects_tile_border,
                viewport_padding: data.viewport_padding,
                layer: data.layer.clone(),
            })
            .collect()
    }

    fn jump_to(&mut self, camera: &CameraOptions) -> Result<(), MapError> {
        let camera = mln_engine::camera::CameraOptions {
            center: camera.center,
            center_altitude: camera.center_altitude,
            zoom: camera.zoom,
            bearing: camera.bearing,
            pitch: camera.pitch,
            roll: camera.roll,
            ..Default::default()
        };
        self.map.jump_to(&camera);
        Ok(())
    }

    fn load_style_json(&mut self, json: &str) -> Result<(), MapError> {
        self.map.load_style_json(json);
        self.map.process_events();
        Ok(())
    }

    fn load_style_url(&mut self, url: &str) -> Result<(), MapError> {
        self.map.load_style_url(url);
        self.map.process_events();
        Ok(())
    }

    fn set_layer_property(&mut self, layer: &str, name: &str, value: &serde_json::Value) -> Result<(), MapError> {
        self.map.edit_style(|style| style_ops::set_layer_property(style, layer, name, value))
    }

    fn set_layer_zoom_range(&mut self, layer: &str, min: f32, max: f32) -> Result<(), MapError> {
        self.map.edit_style(|style| style_ops::set_layer_zoom_range(style, layer, min, max))
    }

    fn add_layer(&mut self, json: &str) -> Result<(), MapError> {
        self.map.edit_style(|style| style_ops::add_layer(style, json))
    }

    fn remove_layer(&mut self, id: &str) -> Result<bool, MapError> {
        self.map.edit_style(|style| Ok(style_ops::remove_layer(style, id)))
    }

    fn set_layer_filter(&mut self, layer: &str, filter: &serde_json::Value) -> Result<(), MapError> {
        self.map.edit_style(|style| style_ops::set_layer_filter(style, layer, filter))
    }

    fn set_global_state_property(&mut self, name: &str, value: &feature::Value) -> Result<(), MapError> {
        style_ops::set_global_state_property(&mut self.map, name, value)
    }

    fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer: Option<&str>,
        feature_id: &str,
        state: &feature::PropertyMap,
    ) -> Result<bool, MapError> {
        if self.frontend.render(&mut self.map).is_err() {
            return Ok(false);
        }
        self.frontend.renderer().set_feature_state(source_id, source_layer, feature_id, state);
        Ok(true)
    }

    fn get_feature_state(
        &mut self,
        source_id: &str,
        source_layer: Option<&str>,
        feature_id: &str,
    ) -> Result<bool, MapError> {
        if self.frontend.render(&mut self.map).is_err() {
            return Ok(false);
        }
        let _ = self.frontend.renderer().feature_state(source_id, source_layer, feature_id);
        Ok(true)
    }

    fn remove_feature_state(
        &mut self,
        source_id: &str,
        source_layer: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) -> Result<bool, MapError> {
        if self.frontend.render(&mut self.map).is_err() {
            return Ok(false);
        }
        self.frontend.renderer().remove_feature_state(source_id, source_layer, feature_id, state_key);
        Ok(true)
    }

    // query
    fn query_rendered_features(
        &mut self,
        geometry: &QueryGeometry,
        options: &crate::metadata::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, MapError> {
        // The parser's `convert<style::Filter>(filter)` (C++ asserts it
        // converts).
        let filter = match &options.filter {
            Some(json) => {
                let mut error = mln_style::conversion::Error::default();
                match mln_style::filter::convert_filter(
                    &mln_style::conversion::Convertible::from(json),
                    self.map.style().registry().functions(),
                    &mut error,
                ) {
                    Some(filter) => Some(filter),
                    None => return Err(MapError::Conversion(error.message)),
                }
            }
            None => None,
        };
        let options = mln_engine::query::RenderedQueryOptions::new(options.layer_ids.clone(), filter);
        let renderer = self.frontend.renderer();
        match geometry {
            QueryGeometry::Point(point) => renderer.query_rendered_features_at_point(point, &options),
            QueryGeometry::Box(screen_box) => {
                renderer.query_rendered_features_in_box(&screen_box.min, &screen_box.max, &options)
            }
        }
        .map_err(|e| MapError::Exception(e.to_string()))
    }

    // conformance: setTileLod* (Map::setTileLod*, glfw ported them).
    fn set_tile_lod_min_radius(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(|tile| tile.lod_min_radius = value);
        Ok(())
    }

    fn set_tile_lod_scale(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(|tile| tile.lod_scale = value);
        Ok(())
    }

    /// The runner gives the threshold in radians, as C++ takes it.
    fn set_tile_lod_pitch_threshold(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(|tile| tile.lod_pitch_threshold = mln_style::math::rad2deg(value));
        Ok(())
    }

    fn set_tile_lod_zoom_shift(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(|tile| tile.lod_zoom_shift = value);
        Ok(())
    }

    fn set_tile_lod_mode(&mut self, mode: crate::parser::TileLodMode) -> Result<(), MapError> {
        self.set_tile(|tile| {
            tile.lod_mode = match mode {
                crate::parser::TileLodMode::Default => mln_engine::tile_store::TileLodMode::Default,
                crate::parser::TileLodMode::Distance => mln_engine::tile_store::TileLodMode::Distance,
            };
        });
        Ok(())
    }

    // query: the remaining runner operations.
    fn set_light(&mut self, json: &str) -> Result<(), MapError> {
        self.map.edit_style(|style| style_ops::set_light(style, json))
    }

    fn set_terrain(&mut self, json: &str) -> Result<(), MapError> {
        self.map.edit_style(|style| style_ops::set_terrain(style, json))
    }

    fn set_center_clamped_to_ground(&mut self, clamped: bool) -> Result<(), MapError> {
        self.map.set_center_clamped_to_ground(clamped);
        Ok(())
    }

    fn add_source(&mut self, id: &str, json: &str) -> Result<(), MapError> {
        style_ops::add_source(&mut self.map, id, json)
    }

    fn remove_source(&mut self, id: &str) -> Result<bool, MapError> {
        Ok(style_ops::remove_source(&mut self.map, id))
    }

    fn add_image(&mut self, name: &str, image_path: &str, pixel_ratio: f32, sdf: bool) -> Result<(), MapError> {
        // `ctx.getFileSource().request(Resource::image("mapbox://render-tests/" +
        // imagePath), ...)`, running the queue until it completes.
        let Some(file_source) = &self.file_source else {
            return Err(MapError::LoadFailed);
        };
        let (sender, responses) = std::sync::mpsc::channel();
        let wake = self.frontend.waker();
        let _req = file_source.request(
            &mln_storage::Resource::image(&format!("mapbox://render-tests/{image_path}")),
            move |response: mln_storage::Response| {
                let _ = sender.send(response.data);
                wake();
            },
        );

        let data = loop {
            match responses.try_recv() {
                Ok(data) => break data,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.queue.run_once(&mut ());
                    self.map.process_events();
                    self.queue.park();
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break None,
            }
        };
        let Some(bytes) = data else {
            return Err(MapError::LoadFailed);
        };

        // C++ `decodeImage` and the `style::Image` constructor throw (not
        // caught by the runner).
        let image = mln_image::decode_image(&bytes).map_err(|e| MapError::Exception(e.to_string()))?;
        let image = mln_style::image::Impl::new(
            name.to_string(),
            image,
            pixel_ratio,
            sdf,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .map_err(|e| MapError::Exception(e.to_string()))?;
        self.map.edit_style(|style| style.apply(Edit::AddImage(Arc::new(image)))).unwrap();
        Ok(())
    }

    fn remove_image(&mut self, name: &str) -> Result<(), MapError> {
        self.map.edit_style(|style| style.apply(Edit::RemoveImage(name.to_owned()))).unwrap();
        Ok(())
    }

    fn sleep(&mut self, duration: std::time::Duration) -> Result<(), MapError> {
        // C++ runs the loop until a timer of `duration` fires.
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.queue.run_once(&mut ());
            self.map.process_events();
            self.queue.park_until(Some(deadline));
        }
        Ok(())
    }
}

/// The factory `TestRunner::run` uses for `maps[key]`, on the runner's
/// queue.
pub fn factory() -> ImplFactory {
    Box::new(
        |metadata: &TestMetadata,
         resource_options: &ResourceOptions,
         impl_options: &ImplOptions,
         queue: &Queue<()>,
         pool: &Pool| { Ok(Box::new(Impl::new(metadata, resource_options, *impl_options, queue, pool)?)) },
    )
}
