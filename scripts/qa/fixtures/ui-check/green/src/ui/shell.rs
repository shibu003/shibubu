pub enum View { Home, Runs, App(String) }
pub struct Tier { pub sidebar_collapsed: bool }
fn tier(w: f32) -> Tier { if w > 900.0 { Tier { sidebar_collapsed: false } } else { Tier { sidebar_collapsed: true } } }
fn toggle(i: &InputState) -> bool { i.consume_key(Modifiers { ctrl: true, mac_cmd: true, ..Default::default() }, Key::S) }
