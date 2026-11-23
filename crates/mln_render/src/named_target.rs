//! Named render targets: textures as large as the frame that the style
//! declares ([`NamedTargets`]), which layers draw into instead of the map
//! ([`Destination`]) and other layers read
//! ([`UpdateCx::target`](crate::layer::UpdateCx::target)).
//!
//! Every registry holds the two style keys of these values: the root's
//! `org.maplibre:targets`, `[{"id": <name>, "precision": "standard" |
//! "high"}, ...]`, and a layer's `org.maplibre:destination-target`, the name
//! of the target it draws into.
//!
//! Each frame draws one pass into each named target that a layer draws into
//! or reads, cleared to transparent, with the draws of every layer routed
//! into it, in the layers' order. A pass runs after the passes of the
//! targets its layers read, and after their own targets
//! ([`Draws::target`](crate::draw::Draws::target)), so that a layer reads a
//! target as every layer routed into it draws it in the frame, wherever they
//! are among the layers. A layer whose route would make a target read what
//! it draws draws on the map instead, as does a layer of 3D geometry, or one
//! whose destination the style doesn't declare.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use feature::{Value, ValueObject};
use mln_gfx::gpu::{Gpu, GpuError, TextureFormat, TextureId};
use mln_gfx::types::TextureChannelDataType;
use mln_style::conversion::{Convertible, Error};
use mln_style::expression::Functions;
use mln_style::style_key::StyleKey;
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::encode::Draws;
use crate::gfx::context::Resources;
use crate::gfx::resources::GpuTexture;
use crate::handles::{Target, Texture};
use crate::layer::LendAt;
use crate::paint_parameters::OpenPass;

/// A render target that the style declares: as large as the frame, in
/// device pixels, cleared to transparent in each frame that draws into it or
/// reads it, with RGBA channels of `channel`, or of unsigned bytes for half
/// floats that the device can't draw into
/// ([`Caps::half_float_render_targets`](mln_gfx::gpu::Caps::half_float_render_targets)).
///
/// On every backend, its texel at `(x, y)` is what the layers drawn into it
/// drew at the fragment coordinate `(x, y)` of the frame, so a draw of the
/// frame samples what lies under its fragment at the fragment's coordinate
/// over [`Frame::target_size`](crate::layer::Frame::target_size).
#[derive(Clone, Debug, PartialEq)]
pub struct NamedTarget {
    /// The name that layers draw into it and read it by.
    pub name: Arc<str>,
    /// The type of its channels.
    pub channel: TextureChannelDataType,
}

/// The style's named targets, in the order it declares them: the value of
/// the style's root `org.maplibre:targets`. Of two targets of one name, the
/// first is the target.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NamedTargets(pub Vec<NamedTarget>);

/// Where a layer draws: into the named target of this name instead of the
/// map. The value of a layer's `org.maplibre:destination-target`.
#[derive(Clone, Debug, PartialEq)]
pub struct Destination(pub Arc<str>);

/// The style's root `org.maplibre:targets`, which every registry holds.
pub(crate) static TARGETS: StyleKey<NamedTargets> =
    StyleKey::root("org.maplibre:targets", parse_targets, targets_json);

/// A layer's `org.maplibre:destination-target`, on a layer of any type,
/// which every registry holds.
pub(crate) static DESTINATION: StyleKey<Destination> =
    StyleKey::layer("org.maplibre:destination-target", parse_destination, |to| Value::String(to.0.to_string()));

fn name(value: &Convertible<'_>) -> Result<Arc<str>, Error> {
    value.to_string().map(Into::into).ok_or_else(|| Error::new("expected a target's id"))
}

fn parse_targets(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<NamedTargets, Error> {
    if !value.is_array() {
        return Err(Error::new("expected an array of targets"));
    }
    let mut targets: Vec<NamedTarget> = Vec::new();
    for i in 0..value.array_length() {
        let target = value.array_member(i);
        let at = |message: String| Error::new(format!("[{i}]: {message}"));
        let unknown = |member: &str, _: &Convertible<'_>| {
            (!["id", "precision"].contains(&member)).then(|| at(format!("no member \"{member}\"")))
        };
        if let Some(error) = target.each_member(unknown) {
            return Err(error);
        }
        let name = target.object_member("id").ok_or_else(|| at("expected an id".to_owned()))?;
        let name = self::name(&name).map_err(|e| at(e.message))?;
        if targets.iter().any(|target| target.name == name) {
            return Err(at(format!("a second target \"{name}\"")));
        }
        let channel = match target.object_member("precision").map(|precision| precision.to_string()) {
            None => TextureChannelDataType::UnsignedByte,
            Some(Some(precision)) if precision == "standard" => TextureChannelDataType::UnsignedByte,
            Some(Some(precision)) if precision == "high" => TextureChannelDataType::HalfFloat,
            Some(_) => return Err(at("expected a precision of \"standard\" or \"high\"".to_owned())),
        };
        targets.push(NamedTarget { name, channel });
    }
    Ok(NamedTargets(targets))
}

fn targets_json(targets: &NamedTargets) -> Value {
    let target = |target: &NamedTarget| {
        let mut json = ValueObject::new();
        json.insert("id".to_owned(), Value::String(target.name.to_string()));
        if target.channel == TextureChannelDataType::HalfFloat {
            json.insert("precision".to_owned(), Value::String("high".to_owned()));
        }
        Value::Object(json)
    };
    Value::Array(targets.0.iter().map(target).collect())
}

fn parse_destination(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<Destination, Error> {
    name(value).map(Destination)
}

/// The depth and stencil format of the targets' passes where the frame's
/// can't serve them.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// Why a layer draws otherwise than its style says, which the renderer
/// warns of when it starts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Warning {
    Undeclared,
    Unread,
    Volume,
    Cycle,
    Backdrop,
    Lend,
}

/// What the frame draws before its passes, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Run {
    /// The draws of the layer at this index into its own targets.
    Own(usize),
    /// The pass into the named target at this index.
    Named(usize),
}

/// The renderer's named targets, and the routes of the frame's layers into
/// them.
#[derive(Default)]
pub(crate) struct Routes {
    /// The style's targets, each with its texture.
    targets: Vec<(NamedTarget, Target)>,
    /// The depth and stencil of the targets' passes, where the frame's can't
    /// serve them, with its size.
    depth_stencil: Option<(Size, GpuTexture)>,
    /// The layers that the last frame warned of, each with why.
    warned: Vec<(Box<str>, Warning)>,
    /// The layers that this frame warned of so far, each with why.
    warning: Vec<(Box<str>, Warning)>,
    /// The targets that the layer being updated asked to read.
    asked: Vec<usize>,
    /// The first name that the layer being updated asked to read and the
    /// style doesn't declare.
    missed: Option<Box<str>>,
    /// The frame's layers that draw, by index among the renderer's, in the
    /// layers' order. The routes name a layer by its place among them.
    layers: Vec<usize>,
    /// Each layer routed into a target, with the target, in the layers'
    /// order.
    routed: Vec<(usize, usize)>,
    /// Each target a layer reads, with the layer.
    reads: Vec<(usize, usize)>,
    /// Each target that a routed layer reads, with the target it draws into.
    chains: Vec<(usize, usize)>,
    /// What the frame draws before its passes; empty where no layer draws
    /// into a named target or reads one.
    order: Vec<Run>,
}

impl Routes {
    /// Makes the targets those that the style declares, `declared`. A target
    /// of a name and channel that it declared before keeps its texture.
    pub(crate) fn sync(&mut self, declared: Option<&NamedTargets>, res: &mut Resources) {
        let declared = declared.map_or(&[][..], |targets| &targets.0[..]);
        if self.targets.len() == declared.len() && self.targets.iter().zip(declared).all(|((held, _), new)| held == new)
        {
            return;
        }
        let mut kept = std::mem::take(&mut self.targets);
        for target in declared {
            let texture = match kept.iter().position(|(held, _)| held == target) {
                Some(index) => kept.swap_remove(index).1,
                None => res.target(Size::new(1, 1), target.channel, Color::new(0.0, 0.0, 0.0, 0.0)),
            };
            self.targets.push((target.clone(), texture));
        }
        if self.targets.is_empty() {
            self.depth_stencil = None;
        }
    }

    /// Drops the targets' textures, which went with the renderer's GPU
    /// context: the next update makes them again.
    pub(crate) fn lose(&mut self) {
        self.targets.clear();
        self.depth_stencil = None;
    }

    /// Whether `destination` names a target the style declares, for a layer
    /// that isn't of 3D geometry (`volume`).
    pub(crate) fn routes(&self, destination: Option<&Destination>, volume: bool) -> bool {
        !volume && destination.is_some_and(|to| self.index(&to.0).is_some())
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.targets.iter().position(|(target, _)| *target.name == *name)
    }

    /// Begins the frame's routes, with the targets `size` device pixels.
    pub(crate) fn begin_frame(&mut self, size: Size, res: &mut Resources) {
        std::mem::swap(&mut self.warned, &mut self.warning);
        self.warning.clear();
        self.layers.clear();
        self.routed.clear();
        self.reads.clear();
        self.chains.clear();
        self.order.clear();
        for (_, target) in &self.targets {
            res.resize(target, size);
        }
    }

    /// What the update of a frame's layer reads the targets through.
    pub(crate) fn reads(&mut self) -> TargetReads<'_> {
        self.asked.clear();
        self.missed = None;
        TargetReads { targets: &self.targets, asked: &mut self.asked, missed: &mut self.missed }
    }

    /// Routes the frame's layer at `index`, `id`, whose style draws it into
    /// `destination`, once its update read the targets it asked for:
    /// returns the index of the target it draws into, `None` for the map. A
    /// layer of 3D geometry (`volume`) draws on the map. Warns of a target
    /// it asked for that the style doesn't declare.
    pub(crate) fn route(
        &mut self,
        index: usize,
        id: &str,
        destination: Option<&Destination>,
        volume: bool,
    ) -> Option<usize> {
        if let Some(name) = self.missed.take() {
            self.warn(id, Warning::Unread, || format!("\"{name}\" is no target the style declares"));
        }
        // Without targets, a layer reads none, and draws on the map.
        if self.targets.is_empty() && destination.is_none() {
            return None;
        }
        let place = self.layers.len();
        self.layers.push(index);
        let asked = std::mem::take(&mut self.asked);
        self.reads.extend(asked.iter().map(|&target| (target, place)));
        let route = destination.and_then(|destination| self.destination(id, destination, volume, &asked));
        if let Some(to) = route {
            self.routed.push((place, to));
            self.chains.extend(asked.iter().map(|&from| (from, to)));
        }
        self.asked = asked;
        route
    }

    /// The target that the layer `id`, which reads `asked`, draws into as
    /// `destination` says, if it may.
    fn destination(&mut self, id: &str, destination: &Destination, volume: bool, asked: &[usize]) -> Option<usize> {
        let Destination(name) = destination;
        let Some(to) = self.index(name) else {
            self.warn(id, Warning::Undeclared, || {
                format!("its destination \"{name}\" is no target the style declares")
            });
            return None;
        };
        if volume {
            self.warn(id, Warning::Volume, || "a layer of 3D geometry draws on the map alone".to_owned());
            return None;
        }
        if asked.contains(&to) || self.reaches(to, asked) {
            self.warn(id, Warning::Cycle, || format!("drawing into \"{name}\" would make it read what it draws"));
            return None;
        }
        Some(to)
    }

    /// Whether what draws into the target `from` reaches one of `targets`.
    fn reaches(&self, from: usize, targets: &[usize]) -> bool {
        let mut seen = vec![false; self.targets.len()];
        let mut next = vec![from];
        while let Some(target) = next.pop() {
            for &(_, to) in self.chains.iter().filter(|(chained, _)| *chained == target) {
                if targets.contains(&to) {
                    return true;
                }
                if !std::mem::replace(&mut seen[to], true) {
                    next.push(to);
                }
            }
        }
        false
    }

    /// Warns of what the routed layer `id` drew otherwise than its style
    /// says, `draws`: a composite refused, or commands asked to be lent in
    /// the pass.
    pub(crate) fn check_routed(&mut self, id: &str, draws: &Draws) {
        if draws.refused_backdrop() {
            self.warn(id, Warning::Backdrop, || "a blend that reads what is behind it needs the map".to_owned());
        }
        if draws.lends(LendAt::Pass) {
            self.warn(id, Warning::Lend, || "a named target's pass lends no commands".to_owned());
        }
    }

    /// Warns that the layer `id` draws otherwise than its style says, as
    /// `why` says, unless it did so in the last frame too: a layer's warning
    /// logs once while it draws so in consecutive frames.
    fn warn(&mut self, id: &str, warning: Warning, why: impl FnOnce() -> String) {
        let of = |held: &(Box<str>, Warning)| *held.0 == *id && held.1 == warning;
        if self.warning.iter().any(of) {
            return;
        }
        if let Some(index) = self.warned.iter().position(of) {
            let held = self.warned.swap_remove(index);
            self.warning.push(held);
            return;
        }
        self.warning.push((id.into(), warning));
        let does = match warning {
            Warning::Undeclared | Warning::Volume | Warning::Cycle => "draws on the map",
            Warning::Unread => "reads nothing of a target it asked for",
            Warning::Backdrop => "draws into its target without its layer blend",
            Warning::Lend => "draws into its target without the commands it asked to be lent in the pass",
        };
        Log::warning(Event::Render, format!("Layer \"{id}\" {does}: {}", why()));
    }

    /// Orders what the frame draws before its passes, once each of its
    /// layers that draw was routed: a target's pass after the runs that draw
    /// what its layers read, and the runs of a layer that reads a target after
    /// its pass; otherwise in the layers' order, the layers' own targets
    /// first.
    pub(crate) fn finish_frame(&mut self) {
        self.order.clear();
        if self.routed.is_empty() && self.reads.is_empty() {
            return;
        }
        // The nodes are the layers' own runs, by place, then the targets'.
        let layers = self.layers.len();
        let mut edges: Vec<(usize, usize)> =
            self.reads.iter().map(|&(target, layer)| (layers + target, layer)).collect();
        edges.extend(self.routed.iter().map(|&(layer, target)| (layer, layers + target)));
        edges.sort_unstable();
        let mut used = vec![false; self.targets.len()];
        for target in self.reads.iter().map(|&(target, _)| target).chain(self.routed.iter().map(|&(_, to)| to)) {
            used[target] = true;
        }
        let mut waits = vec![0usize; layers + self.targets.len()];
        for &(_, to) in &edges {
            waits[to] += 1;
        }
        let present = |node: usize| node < layers || used[node - layers];
        let mut ready: BinaryHeap<Reverse<usize>> =
            (0..waits.len()).filter(|&node| present(node) && waits[node] == 0).map(Reverse).collect();
        while let Some(Reverse(node)) = ready.pop() {
            self.order.push(if node < layers { Run::Own(self.layers[node]) } else { Run::Named(node - layers) });
            let after = edges.partition_point(|&(from, _)| from < node);
            for &(_, to) in edges[after..].iter().take_while(|&&(from, _)| from == node) {
                waits[to] -= 1;
                if waits[to] == 0 {
                    ready.push(Reverse(to));
                }
            }
        }
        debug_assert_eq!(
            self.order.len(),
            layers + used.iter().filter(|&&used| used).count(),
            "a cycle among the frame's routes"
        );
    }

    /// What the frame draws before its passes, in order; empty where no
    /// layer draws into a named target or reads one, and the layers draw
    /// into their own targets in their order.
    pub(crate) fn order(&self) -> &[Run] {
        &self.order
    }

    /// The frame's layers routed into the target at `target`, by index, in
    /// the layers' order.
    pub(crate) fn writers(&self, target: usize) -> impl Iterator<Item = usize> + '_ {
        self.routed.iter().filter(move |&&(_, to)| to == target).map(|&(place, _)| self.layers[place])
    }

    /// The pass into the target at `target`, cleared to transparent, with
    /// `frame_depth`, the frame's depth and stencil and their format, where
    /// a pass can draw into them with any color, and otherwise with a depth
    /// and stencil of the targets' own, cleared.
    pub(crate) fn pass(
        &mut self,
        gpu: &mut dyn Gpu,
        res: &mut Resources,
        target: usize,
        frame_depth: Option<(TextureId, TextureFormat)>,
    ) -> Result<OpenPass, GpuError> {
        let (_, texture) = &self.targets[target];
        let size = res.size(texture.texture());
        let depth_stencil = match (frame_depth, &self.depth_stencil) {
            (Some(depth_stencil), _) => depth_stencil,
            (None, Some((made, own))) if *made == size => (own.id(), DEPTH_STENCIL),
            (None, _) => {
                self.depth_stencil = None;
                let own = res.device().attachment(gpu, "named targets' depth and stencil", size, DEPTH_STENCIL)?;
                let id = own.id();
                self.depth_stencil = Some((size, own));
                (id, DEPTH_STENCIL)
            }
        };
        let (_, texture) = &self.targets[target];
        let mut pass = res.target_pass(gpu, texture, Some(depth_stencil))?;
        pass.label = "named target";
        Ok(pass)
    }
}

/// What a layer's update reads the style's named targets through.
pub(crate) struct TargetReads<'a> {
    targets: &'a [(NamedTarget, Target)],
    /// The targets the layer asked to read, by index.
    asked: &'a mut Vec<usize>,
    /// The first name it asked for that the style doesn't declare.
    missed: &'a mut Option<Box<str>>,
}

impl TargetReads<'_> {
    /// The texture of the target `name`, which the layer reads in the frame.
    pub(crate) fn read(&mut self, name: &str) -> Option<Texture> {
        let Some(index) = self.targets.iter().position(|(target, _)| *target.name == *name) else {
            self.missed.get_or_insert_with(|| name.into());
            return None;
        };
        if !self.asked.contains(&index) {
            self.asked.push(index);
        }
        Some(self.targets[index].1.texture().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(names: &[&str]) -> NamedTargets {
        NamedTargets(
            names
                .iter()
                .map(|&name| NamedTarget { name: name.into(), channel: TextureChannelDataType::HalfFloat })
                .collect(),
        )
    }

    /// Routes with the targets `names`, as a frame begins them.
    fn routes(names: &[&str]) -> (Routes, Resources) {
        let mut res = Resources::detached();
        let mut routes = Routes::default();
        for (target, texture) in targets(names).0.into_iter().map(|target| {
            let texture = res.target(Size::new(1, 1), target.channel, Color::black());
            (target, texture)
        }) {
            routes.targets.push((target, texture));
        }
        routes.begin_frame(Size::new(4, 4), &mut res);
        (routes, res)
    }

    /// Routes the frame's layer `index`, which reads `reads` and draws into
    /// `to`.
    fn layer(routes: &mut Routes, index: usize, reads: &[&str], to: Option<&str>) -> Option<usize> {
        let mut read = routes.reads();
        for name in reads {
            read.read(name);
        }
        routes.route(index, &format!("layer {index}"), to.map(|to| Destination(to.into())).as_ref(), false)
    }

    #[test]
    fn a_target_draws_after_its_writers_and_before_its_readers_wherever_they_are() {
        let (mut routes, _res) = routes(&["a", "b"]);
        // A reader of `b` below the layers that draw `b` from `a`, and
        // `a`'s writer at the top.
        assert_eq!(None, layer(&mut routes, 0, &["b"], None));
        assert_eq!(Some(1), layer(&mut routes, 1, &["a"], Some("b")));
        assert_eq!(Some(0), layer(&mut routes, 2, &[], Some("a")));
        routes.finish_frame();
        assert_eq!(routes.order(), [Run::Own(2), Run::Named(0), Run::Own(1), Run::Named(1), Run::Own(0)]);
        assert_eq!(vec![1], routes.writers(1).collect::<Vec<_>>());
    }

    #[test]
    fn the_layers_drawn_into_a_target_draw_in_one_pass_in_their_order() {
        let (mut routes, _res) = routes(&["a"]);
        for index in 0..5 {
            assert_eq!(Some(0), layer(&mut routes, index, &[], Some("a")));
        }
        assert_eq!(None, layer(&mut routes, 5, &["a"], None));
        routes.finish_frame();
        let passes = routes.order().iter().filter(|run| matches!(run, Run::Named(0))).count();
        assert_eq!(1, passes);
        assert_eq!(vec![0, 1, 2, 3, 4], routes.writers(0).collect::<Vec<_>>());
    }

    #[test]
    fn a_route_that_would_read_what_it_draws_draws_on_the_map() {
        let (mut routes, _res) = routes(&["a", "b"]);
        assert_eq!(None, layer(&mut routes, 0, &["a"], Some("a")));
        assert_eq!(Some(1), layer(&mut routes, 1, &["a"], Some("b")));
        assert_eq!(None, layer(&mut routes, 2, &["b"], Some("a")));
        routes.finish_frame();
        let named: Vec<usize> = routes
            .order()
            .iter()
            .filter_map(|run| match run {
                Run::Named(target) => Some(*target),
                Run::Own(_) => None,
            })
            .collect();
        assert_eq!(named, [0, 1]);
    }

    #[test]
    fn a_frame_without_routes_or_reads_draws_the_layers_targets_in_their_order() {
        let (mut routes, _res) = routes(&["a"]);
        assert_eq!(None, layer(&mut routes, 0, &[], None));
        assert_eq!(None, layer(&mut routes, 1, &[], Some("elsewhere")));
        routes.finish_frame();
        assert_eq!(routes.order(), []);
    }

    #[test]
    fn a_style_keeps_the_textures_of_the_targets_it_still_declares() {
        let mut res = Resources::detached();
        let mut routes = Routes::default();
        routes.sync(Some(&targets(&["a", "b"])), &mut res);
        let b = routes.targets[1].1.clone();
        routes.sync(Some(&targets(&["b", "c"])), &mut res);
        assert_eq!(b, routes.targets[0].1);
        assert_eq!(2, routes.targets.len());
        // The detached resources' device draws no half floats.
        routes.begin_frame(Size::new(4, 2), &mut res);
        assert_eq!(Size::new(4, 2), res.size(b.texture()));
        let format = res.stored_texture(&b.texture().0).expect("the texture").format();
        assert_eq!(TextureFormat::Rgba8Unorm, format);
        routes.sync(None, &mut res);
        assert_eq!(0, routes.targets.len());
    }
}
