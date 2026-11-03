// Keep の button が無い（"Keep research notes" の例文だけ）。
fn approval(ctx: &Context) { egui::Window::new("Approve this run").show(ctx, |_| {}); }
fn confirm(ui: &mut Ui) { theme::button_compact(ui, "Delete", Variant::Danger); }
const EXAMPLES: [(&str, &str); 1] = [("Research notes", "Keep research notes")];
fn home(ui: &mut Ui, g: &Generated) { if let Some(line) = g.llm_line() { states::inline_wrap(ui, RegionState::Partial, line); } }
