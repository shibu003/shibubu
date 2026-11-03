// 一言の欄は title line に 1 つ。
fn draw(shell: &mut Shell) {
    ui.add(egui::TextEdit::singleline(&mut shell.command));
}
