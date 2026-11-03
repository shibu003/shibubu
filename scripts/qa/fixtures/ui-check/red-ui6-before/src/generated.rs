pub fn draw_page(&mut self, ui: &mut Ui) {
    if let Some(line) = self.llm_line() { states::inline_wrap(ui, RegionState::Partial, line); }
}
