fn usable_line_in(ui: &mut Ui, state: Option<RegionState>, line: &str) {
    match state {
        Some(s) => states::inline_wrap(ui, s, line),
        None => { ui.label(format!("{} {line}", RegionState::Ready.glyph())); }
    }
}
