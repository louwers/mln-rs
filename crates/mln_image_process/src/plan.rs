//! How an image-process layer's operations become the passes it draws, each
//! a sample of its input through a kernel, then pointwise operations.

use std::sync::Arc;

use mln_gfx::Size;
use mln_style::color::Color;
use mln_style::expression::{EvaluationContext, ValueConverter};
use mln_style::property::value::PropertyValue;

use crate::members::Operation;
use crate::program::ImageProcessUBO;

/// The most taps a blur takes each side of a texel, each between two
/// texels: a blur that reaches further runs on its input halved.
const MAX_TAPS: usize = 32;

/// The most pointwise operations a pass applies.
const MAX_OPS: usize = 8;

/// A blur's reach, in standard deviations.
const REACH: f32 = 3.0;

/// One pass of a frame's processing.
#[derive(Debug)]
pub(crate) struct Pass {
    /// The texels it draws: a scratch target's, or the frame's for the last
    /// pass.
    pub(crate) size: Size,
    /// The shared target its combines read.
    pub(crate) other: Option<Arc<str>>,
    pub(crate) block: ImageProcessUBO,
}

impl Pass {
    /// A copy of an input into `size` texels, which applies its operations.
    fn copy(size: Size) -> Pass {
        Pass { size, other: None, block: ImageProcessUBO::default() }
    }

    fn ops(&self) -> usize {
        self.block.texel[2] as usize
    }

    fn push(&mut self, kind: f32, parameter: f32, color: [f32; 4]) {
        let at = 2 * self.ops();
        self.block.ops[at] = [kind, parameter, 0.0, 0.0];
        self.block.ops[at + 1] = color;
        self.block.texel[2] += 1.0;
    }
}

/// `value` at `zoom`, or `default` where it is undefined.
fn at<T: Clone + ValueConverter>(value: &PropertyValue<T>, zoom: f32, default: T) -> T {
    match value {
        PropertyValue::Undefined => default,
        PropertyValue::Constant(value) => value.clone(),
        PropertyValue::Expression(expression) => expression.evaluate(&EvaluationContext::new(zoom), default),
    }
}

/// The passes that apply `operations` at `zoom` to a texture of `frame`'s
/// size, whose logical pixels are `logical`, the last drawing at `frame`'s
/// size. Scratch passes draw at logical pixels at most.
pub(crate) fn plan(operations: &[Operation], zoom: f32, frame: Size, logical: Size) -> Vec<Pass> {
    let mut passes: Vec<Pass> = Vec::new();
    let has_room = |pass: &Pass| pass.ops() < MAX_OPS;
    for operation in operations {
        match operation {
            Operation::Blur { size } => blur(&mut passes, at(size, zoom, 0.0).clamp(0.0, 64.0), frame, logical),
            Operation::Threshold { cutoff } => {
                let cutoff = at(cutoff, zoom, 128.0).clamp(0.0, 255.0) / 255.0;
                last(&mut passes, frame, has_room).push(1.0, cutoff, [0.0; 4]);
            }
            Operation::Color { value } => {
                let color = at(value, zoom, Color::white());
                last(&mut passes, frame, has_room).push(2.0, 0.0, color.to_float_array());
            }
            Operation::Opacity { value } => {
                let opacity = at(value, zoom, 1.0).clamp(0.0, 1.0);
                last(&mut passes, frame, has_room).push(3.0, opacity, [0.0; 4]);
            }
            Operation::Combine { target, mode } => {
                let free = |pass: &Pass| has_room(pass) && pass.other.as_ref().is_none_or(|other| other == target);
                let pass = last(&mut passes, frame, free);
                pass.other = Some(Arc::clone(target));
                pass.push(4.0, f32::from(*mode as u8), [0.0; 4]);
            }
        }
    }
    if passes.is_empty() {
        passes.push(Pass::copy(frame));
    }
    if let Some(pass) = passes.last_mut() {
        pass.size = frame;
    }
    for pass in &mut passes {
        let Size { width, height } = pass.size;
        pass.block.texel[0] = 1.0 / width.max(1) as f32;
        pass.block.texel[1] = 1.0 / height.max(1) as f32;
    }
    passes
}

/// The last of `passes` where it `fits` the next pointwise operation, or
/// else a new copy of what it draws, or of the source, of `frame`'s size.
fn last(passes: &mut Vec<Pass>, frame: Size, fits: impl Fn(&Pass) -> bool) -> &mut Pass {
    if !passes.last().is_some_and(fits) {
        let input = passes.last().map_or(frame, |pass| pass.size);
        passes.push(Pass::copy(input));
    }
    let last = passes.len() - 1;
    &mut passes[last]
}

/// Adds the passes of a blur of `sigma` logical pixels: halving copies of
/// its input until its taps reach far enough, then a pass along each axis at
/// logical pixels at most.
fn blur(passes: &mut Vec<Pass>, sigma: f32, frame: Size, logical: Size) {
    if sigma <= 0.0 {
        return;
    }
    let mut input = passes.last().map_or(frame, |pass| pass.size);
    let texels = |input: Size| sigma * input.width as f32 / logical.width.max(1) as f32;
    while (REACH * texels(input)).ceil() as usize > 2 * MAX_TAPS && input.width > 1 && input.height > 1 {
        input = Size::new(input.width.div_ceil(2), input.height.div_ceil(2));
        passes.push(Pass::copy(input));
    }
    let output = Size::new(input.width.min(logical.width), input.height.min(logical.height));
    let mut along_x = Pass::copy(output);
    taps(&mut along_x.block, texels(input), [1.0 / input.width.max(1) as f32, 0.0]);
    passes.push(along_x);
    let mut along_y = Pass::copy(output);
    let texels = sigma * output.height as f32 / logical.height.max(1) as f32;
    taps(&mut along_y.block, texels, [0.0, 1.0 / output.height.max(1) as f32]);
    passes.push(along_y);
}

/// Writes into `block` the taps of a Gaussian of `sigma` texels along
/// `step`, one texel in uv: each tap samples between two texels, weighing
/// them as the Gaussian does.
fn taps(block: &mut ImageProcessUBO, sigma: f32, step: [f32; 2]) {
    let reach = ((REACH * sigma).ceil() as usize).clamp(1, 2 * MAX_TAPS);
    // The weight of a texel past the center, which weighs 1: its own
    // formula gives 0/0 for a sigma whose square underflows.
    let weight = |texel: usize| (-((texel * texel) as f32) / (2.0 * sigma * sigma)).exp();
    let total = 1.0 + 2.0 * (1..=reach).map(weight).sum::<f32>();
    let mut count = 0;
    for first in (1..=reach).step_by(2) {
        let (a, b) = (weight(first) / total, if first < reach { weight(first + 1) / total } else { 0.0 });
        // A narrow Gaussian's weights past the center can underflow to 0,
        // and those further out too.
        if a + b == 0.0 {
            break;
        }
        let offset = (first as f32 * a + (first + 1) as f32 * b) / (a + b);
        let tap = &mut block.taps[count / 2][2 * (count % 2)..][..2];
        tap.copy_from_slice(&[offset, a + b]);
        count += 1;
    }
    block.blur = [step[0], step[1], count as f32, 1.0 / total];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::members::Combine;

    fn constant(value: f32) -> PropertyValue<f32> {
        PropertyValue::Constant(value)
    }

    fn sizes(passes: &[Pass]) -> Vec<(u32, u32)> {
        passes.iter().map(|pass| (pass.size.width, pass.size.height)).collect()
    }

    const FRAME: Size = Size::new(200, 100);
    const LOGICAL: Size = Size::new(100, 50);

    #[test]
    fn pointwise_operations_draw_in_one_pass_at_the_frames_size() {
        let operations = [
            Operation::Threshold { cutoff: constant(51.0) },
            Operation::Opacity { value: constant(0.5) },
            Operation::Combine { target: "other".into(), mode: Combine::Max },
        ];
        let passes = plan(&operations, 0.0, FRAME, LOGICAL);
        assert_eq!(sizes(&passes), [(200, 100)]);
        let block = &passes[0].block;
        assert_eq!(block.texel, [1.0 / 200.0, 1.0 / 100.0, 3.0, 0.0]);
        assert_eq!(block.ops[0], [1.0, 0.2, 0.0, 0.0]);
        assert_eq!(block.ops[4], [4.0, 2.0, 0.0, 0.0]);
        assert_eq!(passes[0].other.as_deref(), Some("other"));
        assert_eq!(block.blur[2], 0.0, "a copy takes no taps");
    }

    #[test]
    fn a_blur_draws_along_each_axis_at_logical_pixels_and_the_operations_after_it_join_the_last() {
        let operations = [Operation::Blur { size: constant(2.0) }, Operation::Threshold { cutoff: constant(128.0) }];
        let passes = plan(&operations, 0.0, FRAME, LOGICAL);
        assert_eq!(sizes(&passes), [(100, 50), (200, 100)]);
        // Along x, from the frame's texels: 4 of them a logical pixel's 2.
        assert_eq!(passes[0].block.blur[..3], [1.0 / 200.0, 0.0, 6.0]);
        assert_eq!(passes[1].block.blur[..3], [0.0, 1.0 / 50.0, 3.0]);
        assert_eq!(passes[1].block.texel[2], 1.0);
        let block = &passes[1].block;
        let taps: f32 = (0..3).map(|tap| block.taps[tap / 2][2 * (tap % 2) + 1]).sum();
        assert!((block.blur[3] + 2.0 * taps - 1.0).abs() < 1e-5, "a blur's weights add up to 1");
    }

    #[test]
    fn a_blur_too_narrow_to_reach_a_neighbor_takes_no_taps() {
        // 0.04 logical pixels are 0.04 texels along y, whose neighbors'
        // weights underflow; at 1e-23 the square of the deviation does.
        for size in [0.04, 1e-23] {
            let passes = plan(&[Operation::Blur { size: constant(size) }], 0.0, FRAME, LOGICAL);
            assert_eq!(passes[1].block.blur[2..], [0.0, 1.0], "only the center, wholly, at {size}");
            for pass in &passes {
                assert!(pass.block.taps.as_flattened().iter().all(|value| value.is_finite()), "at {size}");
            }
        }
    }

    #[test]
    fn a_wide_blur_runs_on_its_input_halved() {
        let passes = plan(&[Operation::Blur { size: constant(40.0) }], 0.0, FRAME, LOGICAL);
        // 40 logical pixels are 80 of the frame's, then 40, then 20 texels:
        // 60 a side at three deviations.
        assert_eq!(sizes(&passes), [(100, 50), (50, 25), (50, 25), (200, 100)]);
        assert_eq!(passes[2].block.blur[2], 30.0);
    }

    #[test]
    fn a_second_target_to_combine_with_starts_a_pass() {
        let operations = [
            Operation::Combine { target: "a".into(), mode: Combine::Multiply },
            Operation::Combine { target: "a".into(), mode: Combine::Over },
            Operation::Combine { target: "b".into(), mode: Combine::Under },
        ];
        let passes = plan(&operations, 0.0, FRAME, LOGICAL);
        assert_eq!(sizes(&passes), [(200, 100), (200, 100)]);
        assert_eq!(passes[0].other.as_deref(), Some("a"));
        assert_eq!(passes[0].block.texel[2], 2.0);
        assert_eq!(passes[1].other.as_deref(), Some("b"));
    }

    #[test]
    fn no_operations_copy_the_source() {
        let passes = plan(&[Operation::Blur { size: constant(0.0) }], 0.0, FRAME, LOGICAL);
        assert_eq!(sizes(&passes), [(200, 100)]);
        let texel = [1.0 / 200.0, 0.01, 0.0, 0.0];
        assert_eq!(passes[0].block, ImageProcessUBO { texel, ..ImageProcessUBO::default() });
    }
}
