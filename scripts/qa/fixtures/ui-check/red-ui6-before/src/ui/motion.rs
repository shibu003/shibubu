// yc/ui a18b14c の形: reduce は全部 0・MotionMode 無・slide_in は mode を見ない。
pub const PAGE: f32 = 0.220;
pub const SLIDE_PX: f32 = 8.0;
pub fn dur(ctx: &Context, base: f32) -> f32 {
    if reduce_motion(ctx) { 0.0 } else { base }
}
fn cross_fade(ctx: &Context) -> f32 { dur(ctx, PAGE) }
fn slide_in(ui: &mut Ui) {
    let dy = SLIDE_PX;
}
