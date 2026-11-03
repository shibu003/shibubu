pub const PAGE: f32 = 0.220;
pub const SLIDE_PX: f32 = 8.0;
pub const REDUCE_FADE: f32 = 0.120;
pub const HOLD_RECEIPT: f32 = 4.0;
pub enum MotionMode { Full, Fade, Cut }
pub fn dur(ctx: &Context, base: f32) -> f32 {
    match motion_mode(ctx) {
        MotionMode::Full => base,
        MotionMode::Fade => base.min(REDUCE_FADE),
        MotionMode::Cut => 0.0,
    }
}
fn cross_fade(ctx: &Context) -> f32 { dur(ctx, PAGE) }
fn slide_in(ui: &mut Ui) {
    let dy = if motion_mode(ui.ctx()) == MotionMode::Full { SLIDE_PX } else { 0.0 };
}
