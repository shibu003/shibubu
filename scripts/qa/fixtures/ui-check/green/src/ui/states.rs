// 緑の形（ui-owner-check の fixture）。Ready は描かない・同じ pass の同じ文は 1 回。
fn state_line(ui: &mut Ui, state: RegionState, detail: &str) {
    if state == RegionState::Ready { return; }
    let pass = ui.ctx().cumulative_pass_nr();
    ui.label(detail);
}
