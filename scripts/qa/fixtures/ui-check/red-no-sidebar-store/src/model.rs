// 赤: UiSettings に畳みの欄が無い（畳んでも再起動で戻る）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiSettings {
    #[serde(default)] pub rail_pinned: bool,
}
