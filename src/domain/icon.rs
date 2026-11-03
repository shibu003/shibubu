//! ICONS-1101（owner 227「なんでこんな細いの選んだの？」・228・230「テンプレのもの探して持ってくるだけ」・231「無料で使っていいもの…tailwindはなし」）:
//! 製品の icon の名前の一覧。形は既製の 1 つの集まり（egui-phosphor 0.9.0 の Phosphor Bold・MIT OR Apache-2.0）で、名前 → glyph の表は
//! ui の `theme::icon_glyph` の 1 か所だけ。押せる icon は `theme::icon_button(ui, Icon, name)` だけが描く = font の記号（"⇩" "↶" 等）や手描きの線を
//! icon にする道を型で塞ぐ。domain に置くのは、ui を引けない層（`domain::terminal` の agent の表・tests/*.rs の domain だけの試験 crate）からも名指すため
//! （この module は std 以外に依存しない）。
//! 名前は spec の置き換えの表（Lucide の名前の形・`undo-2` 等の番号は落とす）・中身は同じ形の Phosphor（各 variant の doc の `→`）。
//! key の字（⌘ ⌥ ⌃ ⇧ ↵）は字のまま（icon ではない）。provider の印は同梱の brand の logo（`logo_png`・PROVIDER-ICONS-1101）が先・無ければ Mac の app の icon・
//! どちらも無い時は呼ぶ側が頭文字の丸（Icon にしない・Phosphor の logo の variant は provider に使わない）。
//! 足す時: 同じ意味の variant が無い時だけ 1 つ足し、`theme::icon_glyph` の腕と `Icon::ALL` に 1 行ずつ（Design System の Icons の組も同じ名前で上げる）。
//! 状態の印（追記 238・owner「empty,partial,stale disconnected, blocked,はアイコンいらない。errorは赤、readyは緑runnningはloadingのように丸の中を回る定番のやつに回る演出。loadingも。verifyも緑。」）
//! = `states::RegionState::icon` の 3 つ（Circle・Check・Loader）だけ。印の無い状態は語だけ。
//! 例: `if theme::icon_button(ui, Icon::X, "Dismiss").clicked() { … }`・agent の表の行に `logo: Some("codex")`
//! Wave: ICONS-1101

/// PROVIDER-ICONS-1101（owner 248・249）: 同梱の provider / agent の brand の logo（`AgentKind::logo` の key → 64 px の PNG と `mono`〈字の色で塗る形 = 白で焼いた〉）。
/// 出所 = @lobehub/icons-static-svg 1.95.1（MIT）の SVG を macOS の `sips` で 64 px に焼いた物（`assets/provider-logos/README.md`・NOTICE）。表に無い key は None。
/// 新しい fn の理由: 絵の bytes を持つ所が無い（`Icon` は Phosphor の字だけ・Mac の app の icon は実行時に app から読む）。純関数。
/// 例: `let (png, mono) = logo_png("kiro")?;`・描くのは `theme::kind_icon`（`logo` の引数）と `theme::provider_icon`。
/// Wave: PROVIDER-ICONS-1101
pub fn logo_png(key: &str) -> Option<(&'static [u8], bool)> {
    // (key, PNG, mono)。mono = 元の SVG が currentColor で塗る形（白で焼いた・描く時に字の色を掛ける）= bake-provider-logos.sh の 3 つ目の数が 1 以上の物
    const LOGOS: [(&str, &[u8], bool); 14] = [
        ("amp", include_bytes!("../../assets/provider-logos/amp.png"), false),
        ("claude", include_bytes!("../../assets/provider-logos/claude.png"), false),
        ("codex", include_bytes!("../../assets/provider-logos/codex.png"), false),
        ("copilot", include_bytes!("../../assets/provider-logos/copilot.png"), true),
        ("cursor-agent", include_bytes!("../../assets/provider-logos/cursor-agent.png"), true),
        ("gemini", include_bytes!("../../assets/provider-logos/gemini.png"), false),
        ("goose", include_bytes!("../../assets/provider-logos/goose.png"), true),
        ("grok", include_bytes!("../../assets/provider-logos/grok.png"), true),
        ("kimi", include_bytes!("../../assets/provider-logos/kimi.png"), false),
        ("kiro", include_bytes!("../../assets/provider-logos/kiro.png"), false),
        ("lmstudio", include_bytes!("../../assets/provider-logos/lmstudio.png"), true),
        ("ollama", include_bytes!("../../assets/provider-logos/ollama.png"), true),
        ("opencode", include_bytes!("../../assets/provider-logos/opencode.png"), true),
        ("qwen", include_bytes!("../../assets/provider-logos/qwen.png"), false),
    ];
    LOGOS.iter().find(|(k, ..)| *k == key).map(|&(_, png, mono)| (png, mono))
}

/// 製品の icon（大きさ 16・太さは Bold の 1 つ・色は呼ぶ側の字の色）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// `•••` の menu（Page options・行の More actions）→ DOTS_THREE
    Ellipsis,
    /// 閉じる・外す・知らせの ×（`×` `x` `✕`）→ X
    X,
    /// 戻す（`↶`・⌘Z）→ ARROW_U_UP_LEFT
    Undo,
    /// やり直す（`↷`・⇧⌘Z）→ ARROW_U_UP_RIGHT
    Redo,
    /// 済んだ・選んでいる（`✓`）→ CHECK
    Check,
    /// 読み直す（`↻`）→ ARROWS_CLOCKWISE
    Refresh,
    /// 確定（`↵` の icon の形・key の字の ↵ は字のまま）→ ARROW_ELBOW_DOWN_LEFT
    CornerDownLeft,
    /// 進む（`→`）→ ARROW_RIGHT
    ArrowRight,
    /// 戻る（`←`・web の Back）→ ARROW_LEFT
    ArrowLeft,
    /// 前へ（`‹`）→ CARET_LEFT
    ChevronLeft,
    /// 次へ（`›`）→ CARET_RIGHT
    ChevronRight,
    /// 上へ開く（`⌃`）→ CARET_UP
    ChevronUp,
    /// menu を開く（`⌄`・Project の ⌄）→ CARET_DOWN
    ChevronDown,
    /// 足す（`＋` `+`）→ PLUS
    Plus,
    /// 減らす（`−`・文字を小さく）→ MINUS
    Minus,
    /// 送る丸の中・上へ（`↑`）→ ARROW_UP
    ArrowUp,
    /// 下へ（`↓`）→ ARROW_DOWN
    ArrowDown,
    /// folder を選ぶ（`📁`）→ FOLDER
    Folder,
    /// AI の提案（`✦`）→ SPARKLE
    Sparkles,
    /// Settings（`⚙`）→ GEAR
    Settings,
    /// 分ける（`◫`・Split）→ SQUARE_SPLIT_HORIZONTAL（追記 243 で COLUMNS から替えた = 縦の棒 2 本は ⏸ に見えた）
    Split,
    /// 左の sidebar（`◧`）→ SIDEBAR_SIMPLE
    PanelLeft,
    /// 右の sidebar / chat（`◨`）→ SIDEBAR_SIMPLE を左右に返した形（Phosphor に右の sidebar が無い）
    PanelRight,
    /// 下の panel を開く（`panel-raise`）→ ARROW_LINE_UP
    PanelBottomOpen,
    /// 下の panel を畳む（`panel-lower`）→ ARROW_LINE_DOWN
    PanelBottomClose,
    /// Home（`⌂`）→ HOUSE
    House,
    /// Connections（`⇄`）→ PLUG
    Plug,
    /// Runs・履歴（`↳`）→ CLOCK_COUNTER_CLOCKWISE
    History,
    /// app・Project の面（`▣`）・app の icon が取れない時の代わり → APP_WINDOW
    AppWindow,
    /// 動いている（`▶`）→ PLAY
    Play,
    /// 探す（手描きの虫眼鏡）→ MAGNIFYING_GLASS
    Search,
    /// Terminal（`>_`）→ TERMINAL_WINDOW
    SquareTerminal,
    /// chat の吹き出し → CHAT_CIRCLE
    MessageSquare,
    /// 添付の写真（`▣`）→ IMAGE
    Image,
    /// 添付の file（`▤`）→ FILE
    File,
    /// 状態の印: Error（赤）と Ready（緑）の丸（`●`）→ CIRCLE
    Circle,
    /// 状態の印: Running と Loading の回る印（定番の spinner・`states::paint_mark` が回す・追記 238）→ CIRCLE_NOTCH
    Loader,
    /// Connections の GitHub の相手 → GITHUB_LOGO（PROVIDER-ICONS-1101: provider / agent の印は同梱の brand の logo = `logo_png`・OpenAI と Google の Phosphor の logo は外した）
    GithubLogo,
    /// Connections の行の相手（TITLEBAR-ICONS-1101 の 3）: Web data（`web`）→ GLOBE
    Globe,
    /// 同: Screen control（`gui` = 画面を押して打つ）→ CURSOR_CLICK
    MousePointerClick,
    /// 同: OS の許可の Screen Recording → MONITOR
    Monitor,
    /// 同: OS の許可の Accessibility → PERSON_ARMS_SPREAD
    Accessibility,
    /// 同: OS の許可の Microphone → MICROPHONE
    Mic,
}

impl Icon {
    /// 全部の icon（Design System の Icons の組・試験が一巡する）。variant を足したらここにも 1 つ
    pub const ALL: [Icon; 43] = [
        Icon::Ellipsis,
        Icon::X,
        Icon::Undo,
        Icon::Redo,
        Icon::Check,
        Icon::Refresh,
        Icon::CornerDownLeft,
        Icon::ArrowRight,
        Icon::ArrowLeft,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ChevronUp,
        Icon::ChevronDown,
        Icon::Plus,
        Icon::Minus,
        Icon::ArrowUp,
        Icon::ArrowDown,
        Icon::Folder,
        Icon::Sparkles,
        Icon::Settings,
        Icon::Split,
        Icon::PanelLeft,
        Icon::PanelRight,
        Icon::PanelBottomOpen,
        Icon::PanelBottomClose,
        Icon::House,
        Icon::Plug,
        Icon::History,
        Icon::AppWindow,
        Icon::Play,
        Icon::Search,
        Icon::SquareTerminal,
        Icon::MessageSquare,
        Icon::Image,
        Icon::File,
        Icon::Circle,
        Icon::Loader,
        Icon::GithubLogo,
        Icon::Globe,
        Icon::MousePointerClick,
        Icon::Monitor,
        Icon::Accessibility,
        Icon::Mic,
    ];
}

#[cfg(test)]
mod tests {
    use super::Icon;

    /// ICONS-1101: `ALL` に同じ icon が 2 度無い（足し忘れは ui の `icons1101_every_icon_has_its_own_glyph` が glyph の数で見る）
    #[test]
    fn icons1101_all_lists_each_icon_once() {
        let mut seen = std::collections::HashSet::new();
        for i in Icon::ALL {
            assert!(seen.insert(i), "{i:?} が ALL に 2 度");
        }
    }
}
