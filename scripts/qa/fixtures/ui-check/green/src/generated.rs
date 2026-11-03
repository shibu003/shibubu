// llm_line は Home（app.rs）だけが描く。Page は描かない。
pub fn draw_page(&mut self, ui: &mut Ui) {
    states::inline(ui, RegionState::Error, "x");
}
