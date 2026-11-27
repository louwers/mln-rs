//! Shared targets: textures of the frame's size that extensions declare for
//! the frames of a style ([`Router`]), which the draws of any layer, a
//! built-in's too, can draw into instead of the map, and which layers read
//! ([`PrepareCx::read_target`](crate::layer::PrepareCx::read_target)).
//!
//! Each frame draws a pass into each shared target that a layer draws into
//! or reads, cleared to transparent, with the draws of every layer routed
//! into it in the layers' order. The passes draw before the frame's: a
//! target's after the passes of the targets that its layers read and after
//! its layers' own targets ([`Draws::target`](crate::draw::Draws::target)),
//! and before the layers that read it, so that a layer reads a target as
//! every layer routed into it draws it in the frame, wherever they are among
//! the layers. Layers read the targets for their draws over the map, in the
//! map's frame, not in a drape.
//!
//! A route that doesn't fit is refused, with a warning, and the layer draws
//! on the map: a route into a target the frame doesn't have, of a layer
//! whose type doesn't route ([`Layer::ROUTES`]) or whose blend reads what is
//! behind it, or one that would make a target read what it draws. Over
//! terrain, a routed layer that drapes draws flat into its target instead,
//! since a drape draws into the terrain tiles' textures, and one that stands
//! on the ground stands on it in its target as on the map.
//!
//! [`Layer::ROUTES`]: crate::layer::Layer::ROUTES

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use mln_gfx::gpu::{DepthStencilAttachment, Gpu, GpuError, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::{Rgba, Size};
use mln_style::Style;
use mln_style::collections::VecMap;
use mln_style::hash::HashMap;

use crate::draw::context::Context;
use crate::draw::resources::{GpuTexture, Resources};
use crate::draw::store::SharedTexture;
use crate::draw::target::Target;
use crate::layer::render_layer::RenderLayer;
use crate::log_target;
use crate::render::paint_parameters::OpenPass;

/// An extension's part in the frames of a style: it declares the frames'
/// shared targets and routes layers' draws into them, from the style's
/// values, such as its extension members
/// ([`mln_style::member`]). An extension registers it with
/// [`RegistryBuilder::add_router`](crate::RegistryBuilder::add_router).
pub trait Router: Send + Sync {
    /// Declares the shared targets of the frames of `style` and routes its
    /// layers into them. The renderer asks again for each version of the
    /// style it draws.
    fn route(&self, style: &Style, routes: &mut Routes);
}

/// What the routers declare for the frames of a style: the shared targets,
/// and the layers routed into them.
#[derive(Default)]
pub struct Routes {
    targets: Vec<(Arc<str>, TextureChannelDataType)>,
    /// Each route by layer id, the last given for a layer.
    routes: VecMap<Box<str>, Arc<str>>,
}

impl Routes {
    /// Declares the shared target `name`, whose texture's channels are of
    /// `type_`, or of the type the backend draws into for it. A second
    /// declaration of a name changes nothing.
    pub fn target(&mut self, name: &str, type_: TextureChannelDataType) {
        if !self.targets.iter().any(|(declared, _)| **declared == *name) {
            self.targets.push((Arc::from(name), type_));
        }
    }

    /// Routes the draws of the layer `layer` into the shared target `target`
    /// instead of the map, in place of an earlier route of the layer.
    pub fn route(&mut self, layer: &str, target: &str) {
        self.routes.insert(Box::from(layer), Arc::from(target));
    }
}

/// The routers' routes of a version of the style, by layer index.
#[derive(Default)]
pub(crate) struct Routing {
    targets: Vec<(Arc<str>, TextureChannelDataType)>,
    /// Each routed layer's index, with its target's index, or the name of a
    /// target the routers didn't declare; in index order.
    routes: Vec<(usize, Result<usize, Arc<str>>)>,
}

impl Routing {
    /// What `routers` route of `style`, whose layers are at their index in
    /// `indexes`.
    pub(crate) fn of(routers: &[&'static dyn Router], style: &Style, indexes: &HashMap<String, usize>) -> Routing {
        let mut routes = Routes::default();
        for router in routers {
            router.route(style, &mut routes);
        }
        let Routes { targets, routes } = routes;
        let target =
            |name: &Arc<str>| targets.iter().position(|(declared, _)| declared == name).ok_or_else(|| Arc::clone(name));
        // In the style's order, which is the render layers'.
        let routes = style
            .get_layers()
            .filter_map(|layer| {
                let to = routes.get(layer.get_id())?;
                Some((*indexes.get(layer.get_id())?, target(to)))
            })
            .collect();
        Routing { targets, routes }
    }

    /// Whether the routers route nothing and declare no target.
    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty() && self.routes.is_empty()
    }

    /// The route of the layer at `index`, if the routers route it.
    fn route(&self, index: usize) -> Option<&Result<usize, Arc<str>>> {
        let at = self.routes.binary_search_by_key(&index, |(layer, _)| *layer).ok()?;
        Some(&self.routes[at].1)
    }

    /// Whether the routers route the layer at `index`.
    pub(crate) fn routes(&self, index: usize) -> bool {
        self.route(index).is_some()
    }
}

/// The format of the depth and stencil that the shared targets' passes draw
/// with, which the routed layers' tile clipping and depth tests read as they
/// would the frame's.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// Why a layer draws otherwise than its routes say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Warning {
    Undeclared,
    Unroutable,
    Backdrop,
    Cycle,
    UndeclaredRead,
}

impl Warning {
    fn message(self, id: &str, name: &str) -> String {
        match self {
            Warning::Undeclared => format!("Layer \"{id}\" draws on the map: \"{name}\" is no shared target"),
            Warning::Unroutable => format!("Layer \"{id}\" draws on the map: its type draws on the map alone"),
            Warning::Backdrop => {
                format!("Layer \"{id}\" draws on the map: its blend reads what is behind it, which needs the map")
            }
            Warning::Cycle => {
                format!("Layer \"{id}\" draws on the map: drawing into \"{name}\" would make it read what it draws")
            }
            Warning::UndeclaredRead => format!("Layer \"{id}\" reads nothing: \"{name}\" is no shared target"),
        }
    }
}

/// The warnings of the layers that draw otherwise than their routes say: a
/// layer's warning logs once while it draws so in consecutive frames.
#[derive(Default)]
pub(crate) struct Warnings {
    /// What the last frame warned of.
    last: Vec<(Box<str>, Warning)>,
    /// What this frame warned of so far.
    this: Vec<(Box<str>, Warning)>,
}

impl Warnings {
    /// Warns that the layer `id` draws otherwise, for `warning` about the
    /// target `name`, unless the last frame or this one did. Returns whether
    /// it logged.
    fn warn(&mut self, id: &str, warning: Warning, name: &str) -> bool {
        let of = |held: &(Box<str>, Warning)| *held.0 == *id && held.1 == warning;
        if self.this.iter().any(of) {
            return false;
        }
        if let Some(index) = self.last.iter().position(of) {
            let held = self.last.swap_remove(index);
            self.this.push(held);
            return false;
        }
        self.this.push((Box::from(id), warning));
        log::warn!(target: log_target::RENDER, "{}", warning.message(id, name));
        true
    }

    /// Ends a frame's warnings.
    fn end_frame(&mut self) {
        std::mem::swap(&mut self.last, &mut self.this);
        self.this.clear();
    }
}

/// What the frame draws before its passes, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Run {
    /// The draws of the layer at this index into its own targets, and what
    /// it does with the frame's commands before the passes.
    Own(usize),
    /// The pass into the shared target at this index.
    Shared(usize),
}

/// A layer of the frame, among those the shared targets order.
struct FrameLayer {
    index: usize,
    /// The target it draws into, if its route stands.
    route: Option<usize>,
}

/// The renderer's shared targets, and the routes and reads of the frame's
/// layers.
#[derive(Default)]
pub struct SharedTargets {
    /// The targets the routing declares, each with its texture.
    targets: Vec<(Arc<str>, TextureChannelDataType, Target)>,
    /// The depth and stencil of the targets' passes, with its size.
    depth_stencil: Option<(Size, GpuTexture)>,
    warnings: Warnings,
    /// Whether the frame has targets or routes; without, the frame orders
    /// nothing.
    active: bool,
    /// The frame's layers that draw, in the layers' order.
    layers: Vec<FrameLayer>,
    /// The targets that the layer whose draws the frame makes read, while
    /// it makes them.
    reading: Option<Vec<usize>>,
    /// Each target that a layer read, with the layer's place in `layers`.
    reads: Vec<(usize, usize)>,
    /// Each target that a routed layer read, with the target it draws into.
    chains: Vec<(usize, usize)>,
    /// What the frame draws before its passes; empty where no layer draws
    /// into a shared target or reads one.
    order: Vec<Run>,
}

impl SharedTargets {
    /// Begins the frame's routes of `routing`, with the targets of `size`
    /// device pixels. A target of a name and channel type that the last
    /// frame had keeps its texture.
    pub(crate) fn begin_frame(&mut self, res: &mut Resources<'_>, routing: &Routing, size: Size) {
        self.warnings.end_frame();
        self.layers.clear();
        self.reads.clear();
        self.chains.clear();
        self.order.clear();
        self.active = !routing.is_empty();
        let same = self.targets.len() == routing.targets.len()
            && self
                .targets
                .iter()
                .zip(&routing.targets)
                .all(|((name, type_, _), declared)| (name, type_) == (&declared.0, &declared.1));
        if !same {
            let mut kept = std::mem::take(&mut self.targets);
            for (name, type_) in &routing.targets {
                let target = match kept.iter().position(|(held, held_type, _)| (held, held_type) == (name, type_)) {
                    Some(index) => kept.swap_remove(index).2,
                    None => res.target(size, *type_, Rgba::TRANSPARENT),
                };
                self.targets.push((Arc::clone(name), *type_, target));
            }
            if self.targets.is_empty() {
                self.depth_stencil = None;
            }
        }
        for (_, _, target) in &self.targets {
            target.set_size(res, size);
        }
    }

    /// Whether the frame has targets or routes.
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// Drops the targets' textures, which go with the renderer's GPU
    /// context: the next frame makes them again.
    pub(crate) fn lose(&mut self) {
        self.targets.clear();
        self.depth_stencil = None;
    }

    /// The target the frame's layer at `index`, `layer`, draws into, as
    /// `routing` routes it, if the route stands: a route of a type that
    /// doesn't route, of a layer whose blend reads its backdrop or into a
    /// target the routing doesn't declare draws on the map, with a warning.
    pub(crate) fn route(&mut self, routing: &Routing, index: usize, layer: &dyn RenderLayer) -> Option<usize> {
        let route = routing.route(index)?;
        let refused = match route {
            Err(name) => Some((Warning::Undeclared, &**name)),
            Ok(_) if !layer.routes() => Some((Warning::Unroutable, "")),
            Ok(_) if layer.reads_backdrop() => Some((Warning::Backdrop, "")),
            Ok(_) => None,
        };
        if let Some((warning, name)) = refused {
            self.warnings.warn(layer.id(), warning, name);
            return None;
        }
        route.as_ref().ok().copied()
    }

    /// Begins the frame's draws of its layer at `index`, which draws into
    /// the target `route`, if any, in a frame that [`is_active`](Self::is_active).
    pub(crate) fn begin_layer(&mut self, index: usize, route: Option<usize>) {
        self.layers.push(FrameLayer { index, route });
        self.reading = Some(Vec::new());
    }

    /// Ends the frame's draws of the layer that [`begin_layer`](Self::begin_layer)
    /// began, `id`, and returns whether its route stands: one that would make
    /// its target read what it draws is refused, with a warning, and the
    /// layer draws on the map.
    pub(crate) fn end_layer(&mut self, id: &str) -> bool {
        let Some(read) = self.reading.take() else { return true };
        let place = self.layers.len() - 1;
        self.reads.extend(read.iter().map(|&target| (target, place)));
        let Some(to) = self.layers[place].route else { return true };
        if read.contains(&to) || self.reaches(to, &read) {
            let name = Arc::clone(&self.targets[to].0);
            self.warnings.warn(id, Warning::Cycle, &name);
            self.layers[place].route = None;
            return false;
        }
        self.chains.extend(read.iter().map(|&from| (from, to)));
        true
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

    /// The texture of the target `name`, which the layer `id` reads in the
    /// map's frame: for the layer whose draws the frame makes, which orders
    /// the target's pass before them. `None`, with a warning, for a name the
    /// frame has no target of.
    pub(crate) fn read(&mut self, id: &str, name: &str) -> Option<SharedTexture> {
        let Some(index) = self.index(name) else {
            self.warnings.warn(id, Warning::UndeclaredRead, name);
            return None;
        };
        if let Some(read) = &mut self.reading
            && !read.contains(&index)
        {
            read.push(index);
        }
        Some(self.targets[index].2.texture().clone())
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.targets.iter().position(|(declared, _, _)| **declared == *name)
    }

    /// Orders what the frame draws before its passes, once its layers'
    /// draws are made: a target's pass after the runs that draw what its
    /// layers read, and the runs of a layer that reads a target after its
    /// pass; otherwise in the layers' order, the layers' own targets first.
    pub(crate) fn finish_frame(&mut self) {
        self.order.clear();
        let routed = || self.layers.iter().enumerate().filter_map(|(place, layer)| Some((place, layer.route?)));
        if self.reads.is_empty() && routed().next().is_none() {
            return;
        }
        // The nodes are the layers' own runs, by place, then the targets'.
        // A layer reads each target once, and draws into one at most.
        let layers = self.layers.len();
        let mut edges: Vec<(usize, usize)> =
            self.reads.iter().map(|&(target, layer)| (layers + target, layer)).collect();
        edges.extend(routed().map(|(layer, target)| (layer, layers + target)));
        let mut used = vec![false; self.targets.len()];
        for &(from, to) in &edges {
            used[from.max(to) - layers] = true;
        }
        let mut waits = vec![0usize; layers + self.targets.len()];
        for &(_, to) in &edges {
            waits[to] += 1;
        }
        let present = |node: usize| node < layers || used[node - layers];
        let mut ready: BinaryHeap<Reverse<usize>> =
            (0..waits.len()).filter(|&node| present(node) && waits[node] == 0).map(Reverse).collect();
        while let Some(Reverse(node)) = ready.pop() {
            self.order.push(if node < layers { Run::Own(self.layers[node].index) } else { Run::Shared(node - layers) });
            for &(_, to) in edges.iter().filter(|&&(from, _)| from == node) {
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
    /// layer draws into a shared target or reads one, and the layers draw
    /// into their own targets in their order.
    pub(crate) fn order(&self) -> &[Run] {
        &self.order
    }

    /// The indexes of the frame's layers routed into the target at
    /// `target`, in the layers' order.
    pub(crate) fn writers(&self, target: usize) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.layers.iter().filter(move |layer| layer.route == Some(target)).map(|layer| layer.index)
    }

    /// The pass into the target at `target`, cleared to transparent, with a
    /// depth and stencil of the targets' own, cleared, made through `gpu`,
    /// the context's.
    pub(crate) fn pass(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        target: usize,
    ) -> Result<OpenPass, GpuError> {
        let (color, format, size) = self.targets[target].2.color_attachment(gpu, context)?;
        let depth_stencil = match &self.depth_stencil {
            Some((made, own)) if *made == size => own.id(),
            _ => {
                self.depth_stencil = None;
                let own = context.device().attachment(gpu, "shared targets' depth and stencil", size, DEPTH_STENCIL)?;
                let id = own.id();
                self.depth_stencil = Some((size, own));
                id
            }
        };
        Ok(OpenPass {
            label: "shared target",
            color: Some(color),
            depth_stencil: Some(DepthStencilAttachment {
                texture: depth_stencil,
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Clear(0),
            }),
            size,
            targets: TargetFormats { color: Some(format), depth_stencil: Some(DEPTH_STENCIL) },
        })
    }
}

#[cfg(test)]
mod tests {
    use mln_gfx::gpu::{Caps, ClipDepthRange};

    use super::*;

    fn context() -> Context {
        Context::new(
            Caps {
                buffer_offset_alignment: 256,
                max_texture_size: 4096,
                half_float_render_targets: false,
                clip_depth: ClipDepthRange::ZeroToOne,
                render_target_depth: None,
                clear_stencil_with_pass: false,
                cpu_rasterizer: false,
                max_line_width: 1.0,
                binds_vertex_buffers: false,
            },
            1.0,
        )
    }

    /// Shared targets of `names`, as a frame begins them.
    fn targets(context: &mut Context, names: &[&str]) -> SharedTargets {
        let type_ = TextureChannelDataType::UnsignedByte;
        let targets = names
            .iter()
            .map(|&name| (Arc::from(name), type_, Target::new(context, Size::new(4, 4), type_, Rgba::TRANSPARENT)))
            .collect();
        SharedTargets { targets, active: true, ..SharedTargets::default() }
    }

    /// Makes the draws of the frame's layer at `index`, which reads `reads`
    /// and is routed into `to`, and returns whether its route stands.
    fn layer(targets: &mut SharedTargets, index: usize, reads: &[&str], to: Option<usize>) -> bool {
        targets.begin_layer(index, to);
        let id = format!("layer {index}");
        for name in reads {
            targets.read(&id, name);
        }
        targets.end_layer(&id)
    }

    fn shared(order: &[Run]) -> Vec<usize> {
        order
            .iter()
            .filter_map(|run| match run {
                Run::Shared(target) => Some(*target),
                Run::Own(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_target_draws_after_its_writers_and_before_its_readers_wherever_they_are() {
        let mut context = context();
        let mut targets = targets(&mut context, &["a", "b"]);
        // A reader of `b` below the layer that draws `b` from `a`, and `a`'s
        // writer at the top.
        assert!(layer(&mut targets, 0, &["b"], None));
        assert!(layer(&mut targets, 1, &["a"], Some(1)));
        assert!(layer(&mut targets, 2, &[], Some(0)));
        targets.finish_frame();
        assert_eq!(targets.order(), [Run::Own(2), Run::Shared(0), Run::Own(1), Run::Shared(1), Run::Own(0)]);
        assert_eq!(vec![1], targets.writers(1).collect::<Vec<_>>());
    }

    #[test]
    fn the_layers_routed_into_a_target_draw_in_one_pass_in_their_order() {
        let mut context = context();
        let mut targets = targets(&mut context, &["a"]);
        for index in 0..5 {
            assert!(layer(&mut targets, index, &[], Some(0)));
        }
        assert!(layer(&mut targets, 5, &["a"], None));
        targets.finish_frame();
        assert_eq!(vec![0], shared(targets.order()));
        assert_eq!(vec![0, 1, 2, 3, 4], targets.writers(0).collect::<Vec<_>>());
    }

    #[test]
    fn a_route_that_would_make_a_target_read_what_it_draws_is_refused() {
        let mut context = context();
        let mut targets = targets(&mut context, &["a", "b"]);
        assert!(!layer(&mut targets, 0, &["a"], Some(0)), "a layer that reads its own target");
        assert!(layer(&mut targets, 1, &["a"], Some(1)));
        assert!(!layer(&mut targets, 2, &["b"], Some(0)), "a layer that draws `a` from `b`, which is drawn from `a`");
        targets.finish_frame();
        assert_eq!(vec![0, 1], shared(targets.order()));
        assert_eq!(0, targets.writers(0).count());
    }

    #[test]
    fn a_frame_without_routes_or_reads_orders_nothing() {
        let mut context = context();
        let mut targets = targets(&mut context, &["a"]);
        assert!(layer(&mut targets, 0, &[], None));
        assert!(layer(&mut targets, 1, &["elsewhere"], None));
        targets.finish_frame();
        assert_eq!(targets.order(), []);
    }

    #[test]
    fn a_layer_warns_once_while_consecutive_frames_draw_it_otherwise() {
        let mut warnings = Warnings::default();
        assert!(warnings.warn("a", Warning::Undeclared, "t"));
        assert!(!warnings.warn("a", Warning::Undeclared, "t"));
        assert!(warnings.warn("a", Warning::Cycle, "t"), "another warning of the layer");
        for _ in 0..3 {
            warnings.end_frame();
            assert!(!warnings.warn("a", Warning::Undeclared, "t"));
        }
        // A frame that draws the layer as routed ends the streak.
        warnings.end_frame();
        warnings.end_frame();
        assert!(warnings.warn("a", Warning::Undeclared, "t"));
    }
}
