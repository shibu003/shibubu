// 緑の形（ui-owner-check の fixture）。実物の theme.rs の名前だけを写した小さな物。
pub struct Tokens { pub text_primary: Color32, pub surface_3: Color32, pub surface_press: Color32, pub working: Color32 }
pub const LIGHT: Tokens = Tokens {
    text_primary: rgb(0x202731),
    surface_3: rgb(0xEAEDF1),
    surface_press: rgb(0xDDE1E7),
    working: rgb(0x2359CE),
    ansi: [rgb(0x0969DA)],
};
pub const DARK: Tokens = Tokens {
    text_primary: rgb(0xF1F5F9),
    surface_3: rgb(0x2C3540),
    surface_press: rgb(0x38424F),
    working: rgb(0x91B5FF),
    ansi: [rgb(0x58A6FF)],
};
pub const BUTTON_H: f32 = 28.0;
pub const BUTTON_COMPACT_H: f32 = 24.0;
pub const BUTTON_PAD_X: f32 = 8.0;
pub const BUTTON_COMPACT_PAD_X: f32 = 6.0;
pub const COMMAND_H: f32 = 32.0;
impl Type {
    pub const fn size(self) -> f32 {
        match self { Type::Body => 14.0, Type::Helper => 12.0 }
    }
}
fn framed_id() -> Id { Id::new("kangae.theme.framed") }
pub fn framed(ctx: &Context) -> Framed { ctx.data_mut(|d| d.insert_temp(framed_id(), true)); Framed }
// 枠は範囲（Settings・Connections・Runs）の中だけ。button の描画 fn は枠の token をここからだけ読む
fn frame_of(ui: &Ui, enabled: bool) -> Stroke {
    if !ui.ctx().data(|d| d.get_temp::<bool>(framed_id())).unwrap_or(false) { return Stroke::NONE; }
    Stroke::new(1.0, if enabled { t.border_strong } else { t.border })
}
pub fn button_sized(ui: &mut Ui, variant: Variant) {
    let fill = if response.is_pointer_button_down_on() { t.surface_press } else if hovered { t.surface_3 } else { TRANSPARENT };
    ui.painter().rect(rect, fill, frame_of(ui, enabled));
}
pub fn icon_button(ui: &mut Ui) {
    let fill = if response.is_pointer_button_down_on() { t.surface_press } else if hovered { t.surface_3 } else { TRANSPARENT };
    ui.painter().rect(rect, fill, Stroke::NONE);
}
pub fn chip(ui: &mut Ui, label: &str) {
    button_compact(ui, label, Variant::Ghost)
}
pub fn native_buttons(ui: &mut Ui) {
    ui.style_mut().visuals.widgets.inactive.bg_stroke = frame_of(ui, true);
}
