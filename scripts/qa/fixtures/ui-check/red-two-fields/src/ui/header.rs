// 赤: 一言の欄が 2 つ（header と別の面）。
fn draw(shell: &mut Shell) {
    ui.add(egui::TextEdit::singleline(&mut shell.command));
    ui.add(egui::TextEdit::singleline(&mut shell.rail_input));
}
