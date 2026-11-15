use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{BoundOptions, Map, MapOptions, ProjectionMode};
use mln_renderer::tile::worker_codec::{SerializingTransport, ThreadWorkerInstances};
use mln_storage::{FileSource, FileSourceExt};
use mln_util::image::PremultipliedImage;

use crate::file_source::proxy_loader;
use crate::metadata::{MapMode, TestMetadata};
use crate::runner::{
    CameraOptions, ImplFactory, ImplOptions, MapError, QueryGeometry, ResourceOptions, TestRunnerImpl,
};
use crate::style_ops;

fn map_mode(mode: MapMode) -> mln_map::MapMode {
    match mode {
        MapMode::Continuous => mln_map::MapMode::Continuous,
        MapMode::Static => mln_map::MapMode::Static,
        MapMode::Tile => mln_map::MapMode::Tile,
    }
}

pub(crate) struct Impl {
    pub(crate) frontend: HeadlessFrontend,
    /// The `ProxyFileSource` over the cache database at the resource
    /// options' cache path.
    pub(crate) file_source: Option<Arc<dyn FileSource>>,
    pub(crate) map: Map,
    /// The runner's loop, which the map runs on.
    run_loop: RunLoop,
}

impl Impl {
    /// A frontend, file source and map for a test with `metadata`, loading
    /// through a [`proxy_loader`].
    pub(crate) fn new(
        metadata: &TestMetadata,
        resource_options: &ResourceOptions,
        impl_options: ImplOptions,
        run_loop: &RunLoop,
    ) -> Result<Impl, String> {
        let resource_options = mln_storage::ResourceOptions {
            cache_path: resource_options.cache_path.clone(),
            ..mln_storage::ResourceOptions::default()
        };
        let mut frontend =
            HeadlessFrontend::new(metadata.size, metadata.pixel_ratio, run_loop).map_err(|e| e.to_string())?;
        if impl_options.tile_worker_codec > 0 {
            let count = impl_options.tile_worker_codec;
            let transport = SerializingTransport::new(&run_loop.scheduler(), |replies| {
                Box::new(ThreadWorkerInstances::new(count, &replies))
            });
            frontend.renderer().set_tile_worker_transport(transport);
        }
        let file_source = proxy_loader(&resource_options);
        let mut map = Map::with_file_source(
            Arc::clone(&file_source),
            MapOptions {
                map_mode: map_mode(metadata.map_mode),
                size: metadata.size,
                pixel_ratio: metadata.pixel_ratio,
                cross_source_collisions: metadata.cross_source_collisions,
                ..Default::default()
            },
            run_loop,
        )
        .map_err(|e| e.to_string())?;
        frontend.attach(&map);
        if let Some(max_pitch) = metadata.max_pitch {
            map.set_bounds(&BoundOptions { max_pitch: Some(f64::from(max_pitch)), ..Default::default() })
                .map_err(|e| e.to_string())?;
        }
        Ok(Impl { frontend, file_source: Some(file_source), map, run_loop: run_loop.clone() })
    }

    /// Changes the map's tile settings.
    fn set_tile(&mut self, change: impl FnOnce(&mut mln_map::TileSettings)) {
        let mut tile = self.map.snapshot().tile;
        change(&mut tile);
        self.map.set_tile_settings(&tile);
    }
}

impl TestRunnerImpl for Impl {
    fn reset_context(&mut self, metadata: &TestMetadata) -> Result<(), MapError> {
        self.frontend.renderer().clear_data();
        self.frontend.set_size(metadata.size);
        self.map.set_size(metadata.size).map_err(|e| MapError::Exception(e.to_string()))?;
        self.map.set_projection_mode(
            &ProjectionMode::default()
                .with_axonometric(metadata.axonometric)
                .with_x_skew(metadata.x_skew)
                .with_y_skew(metadata.y_skew),
        );
        self.map.set_debug(mln_map::MapDebugOptions(metadata.debug.0));
        style_ops::load_json(self.map.style_mut(), &metadata.document_json);
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
        let camera = mln_map::CameraOptions {
            center: camera.center,
            center_altitude: camera.center_altitude,
            zoom: camera.zoom,
            bearing: camera.bearing,
            pitch: camera.pitch,
            roll: camera.roll,
            ..Default::default()
        };
        let bounds = self.map.lat_lng_bounds_for_camera(&camera).map_err(|e| MapError::Exception(e.to_string()))?;
        Ok(mln_renderer::util::tile_cover::tile_cover_bounds(&bounds, zoom)
            .into_iter()
            .map(|id| crate::runner::TileId { z: id.canonical.z, x: id.canonical.x, y: id.canonical.y, wrap: id.wrap })
            .collect())
    }

    fn collect_placed_symbol_data(&mut self, enable: bool) {
        self.frontend.renderer().collect_placed_symbol_data(enable);
    }

    fn get_placed_symbols_data(&mut self) -> Vec<crate::runner::PlacedSymbol> {
        let to_box = |b: &mln_util::grid_index::BBox| [b.min.x, b.min.y, b.max.x, b.max.y];
        self.frontend
            .renderer()
            .get_placed_symbols_data()
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
        let camera = mln_map::CameraOptions {
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
        style_ops::load_json(self.map.style_mut(), json);
        self.map.process_events();
        Ok(())
    }

    fn load_style_url(&mut self, url: &str) -> Result<(), MapError> {
        style_ops::load_url(self.map.style_mut(), url);
        self.map.process_events();
        Ok(())
    }

    fn set_layer_property(&mut self, layer: &str, name: &str, value: &serde_json::Value) -> Result<(), MapError> {
        style_ops::set_layer_property(self.map.style_mut(), layer, name, value)
    }

    fn set_layer_zoom_range(&mut self, layer: &str, min: f32, max: f32) -> Result<(), MapError> {
        style_ops::set_layer_zoom_range(self.map.style_mut(), layer, min, max)
    }

    fn add_layer(&mut self, json: &str) -> Result<(), MapError> {
        style_ops::add_layer(self.map.style_mut(), json)
    }

    fn remove_layer(&mut self, id: &str) -> Result<bool, MapError> {
        Ok(style_ops::remove_layer(self.map.style_mut(), id))
    }

    fn set_layer_filter(&mut self, layer: &str, filter: &serde_json::Value) -> Result<(), MapError> {
        style_ops::set_layer_filter(self.map.style_mut(), layer, filter)
    }

    fn set_global_state_property(&mut self, name: &str, value: &feature::Value) -> Result<(), MapError> {
        style_ops::set_global_state_property(self.map.style_mut(), name, value)
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
        let _ = self.frontend.renderer().get_feature_state(source_id, source_layer, feature_id);
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
                let mut error = mln_conversion::Error::default();
                match mln_style::conversion::convert::<mln_style::filter::Filter>(
                    &mln_conversion::Convertible::from(json),
                    &mut error,
                ) {
                    Some(filter) => Some(filter),
                    None => return Err(MapError::Conversion(error.message)),
                }
            }
            None => None,
        };
        let options = mln_renderer::query::RenderedQueryOptions::new(options.layer_ids.clone(), filter);
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
        self.set_tile(|tile| tile.lod_pitch_threshold = mln_math::rad2deg(value));
        Ok(())
    }

    fn set_tile_lod_zoom_shift(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(|tile| tile.lod_zoom_shift = value);
        Ok(())
    }

    fn set_tile_lod_mode(&mut self, mode: crate::parser::TileLodMode) -> Result<(), MapError> {
        self.set_tile(|tile| {
            tile.lod_mode = match mode {
                crate::parser::TileLodMode::Default => mln_map::TileLodMode::Default,
                crate::parser::TileLodMode::Distance => mln_map::TileLodMode::Distance,
            };
        });
        Ok(())
    }

    // query: the remaining runner operations.
    fn set_light(&mut self, json: &str) -> Result<(), MapError> {
        style_ops::set_light(self.map.style_mut(), json)
    }

    fn add_source(&mut self, id: &str, json: &str) -> Result<(), MapError> {
        style_ops::add_source(self.map.style_mut(), id, json)
    }

    fn remove_source(&mut self, id: &str) -> Result<bool, MapError> {
        Ok(style_ops::remove_source(self.map.style_mut(), id))
    }

    fn add_image(&mut self, name: &str, image_path: &str, pixel_ratio: f32, sdf: bool) -> Result<(), MapError> {
        // `ctx.getFileSource().request(Resource::image("mapbox://render-tests/" +
        // imagePath), ...)`, running the run loop until it completes.
        let Some(file_source) = &self.file_source else {
            return Err(MapError::LoadFailed);
        };
        let maybe_image: Rc<std::cell::RefCell<Option<Vec<u8>>>> = Rc::default();
        let request_completed = Rc::new(Cell::new(false));
        let _req = {
            let maybe_image = Rc::clone(&maybe_image);
            let request_completed = Rc::clone(&request_completed);
            file_source.request(
                &self.run_loop.scheduler(),
                &mln_storage::Resource::image(&format!("mapbox://render-tests/{image_path}")),
                Box::new(move |response: mln_storage::Response| {
                    if let Some(data) = &response.data {
                        *maybe_image.borrow_mut() = Some(data.to_vec());
                    }
                    request_completed.set(true);
                }),
            )
        };

        while !request_completed.get() {
            self.run_loop.run_once();
            self.map.process_events();
        }

        let Some(bytes) = maybe_image.borrow_mut().take() else {
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
        self.map.style_mut().add_image(Arc::new(image));
        Ok(())
    }

    fn remove_image(&mut self, name: &str) -> Result<(), MapError> {
        self.map.style_mut().remove_image(name);
        Ok(())
    }

    fn sleep(&mut self, duration: std::time::Duration) -> Result<(), MapError> {
        // util::Timer sleepTimer; sleepTimer.start(duration, zero, ...);
        // while (sleeping) RunLoop::Get()->runOnce();
        let sleeping = Rc::new(Cell::new(true));
        let sleep_timer = mln_actor::Timer::new(&self.run_loop);
        {
            let sleeping = Rc::clone(&sleeping);
            sleep_timer.start(duration, std::time::Duration::ZERO, move || sleeping.set(false));
        }
        while sleeping.get() {
            self.run_loop.run_once();
            self.map.process_events();
        }
        Ok(())
    }
}

/// The factory `TestRunner::run` uses for `maps[key]`, on the runner's loop.
pub fn factory() -> ImplFactory {
    Box::new(
        |metadata: &TestMetadata,
         resource_options: &ResourceOptions,
         impl_options: &ImplOptions,
         run_loop: &RunLoop| { Ok(Box::new(Impl::new(metadata, resource_options, *impl_options, run_loop)?)) },
    )
}
