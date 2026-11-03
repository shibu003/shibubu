// 赤: 一言の欄が 2 つ。どちらも COMMAND_WORDS に無い名前（変数名でなく構築の数を読む）。
fn draw(shell: &mut Shell, text: &mut String) {
    ui.add(egui::TextEdit::singleline(&mut shell.input));
    ui.add(egui::TextEdit::multiline(text));
}
