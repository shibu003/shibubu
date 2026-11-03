// 消す確認は同じ場所の 1 行。Window は承認だけ。
fn approval(ctx: &Context) { egui::Window::new("Approve this run").show(ctx, |_| {}); }
fn confirm(ui: &mut Ui) { theme::button_compact(ui, "Delete", Variant::Danger); theme::button_compact(ui, "Keep", Variant::Ghost); }
// 素の menu は native_buttons の scope の中。menu の中の button は egui が既に枠を外している。
fn more(ui: &mut Ui) { ui.scope(|ui| { theme::native_buttons(ui); ui.menu_button("More", |ui| { ui.button("Inside"); }); }); }
// 例文の literal は button ではない（"Keep research notes" で a3 が立たない）。
const EXAMPLES: [(&str, &str); 1] = [("Research notes", "Keep research notes")];
fn home(ui: &mut Ui, g: &Generated) { if let Some(line) = g.llm_line() { states::inline_wrap(ui, RegionState::Partial, line); } }
fn strip(ctx: &Context) -> bool { ui::shell::execution_strip(ctx, 1) }
