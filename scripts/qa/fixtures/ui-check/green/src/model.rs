// 窓全体の設定。畳んだ状態は rail_pinned の隣（derive が save / load の両方を通す）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiSettings {
    #[serde(default)] pub rail_pinned: bool,
    #[serde(default)] pub sidebar_collapsed: bool,
}
