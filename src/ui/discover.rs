//! PV1（図 §50・spec §3-A・§5-1・§5-2）: 迷いの一言（作る物の形が決まっていない game）に、Home の欄の下で成果物の候補 3 つ（OutcomeChoice）→
//! 1 つ選ぶ → 道具の行（CapabilityMatch・入っている → web → 入れる の順・発行元と出所の domain）→ 公式の頁・`Check`・`Start in <道具>`。
//! 何も入れない・走査しない（`Check` を押した時に表の 1 つの .app だけを読む = 見える opt-in・PV-007）。型の決まった部品だけ（model の HTML でない）。
//! 中身は `project::discover::{discover, act}` が表から作る（ui は表を引かない = ui だけを取り込む試験の crate でも compile できる）。

use super::shell::Intent;
use super::theme::{self, Type};
use eframe::egui;

/// 成果物の候補 1 件（OutcomeChoice・spec §5-1 の 4 欄 + 使う道具 + 押すと送る一言）。
/// Wave: PV1
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub title: &'static str,
    /// 作る物の範囲
    pub scope: &'static str,
    /// 出来上がりの例
    pub example: &'static str,
    /// 手間の目安
    pub effort: &'static str,
    /// 要る物の目安（道具・account・素材）
    pub needs: &'static str,
    /// 使う道具（`project::acquire::CATALOG` の名前）
    pub tool: &'static str,
    /// `Start in <tool>` で送る一言（既在の `Intent::Generate`。道具の名前を含むので候補をもう一度出さない）
    pub start: &'static str,
}

/// 道具の今（CapabilityMatch・spec §5-2）。spec の 5 つのうち Requires sign-in は使う物が無い（Godot の web 版は account が要らない）ので持たない。
/// Wave: PV1
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// `Check` でこの Mac に表の bundle id の .app を読めた（入れ直さない）
    Installed,
    /// 入れずに使える公式の web 版
    WebOption,
    /// 入れる道（入れるのは Project の 1 問の `Install & continue` = PV0。ここでは入れない）
    RequiresInstall,
    /// 同じ名前の別の物が在る・読めない
    Unavailable,
}

impl Match {
    /// 行の頭の語（spec §5-2 の語のまま）。
    pub const fn label(self) -> &'static str {
        match self {
            Match::Installed => "Installed",
            Match::WebOption => "Web option",
            Match::RequiresInstall => "Requires install",
            Match::Unavailable => "Unavailable",
        }
    }
}

/// 選んだ候補の道具の 1 行。
/// Wave: PV1
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub state: Match,
    /// 道具の名前（`Godot`・`Godot in the browser`）
    pub name: String,
    /// 確かめた事実の 1 行（Publisher · Source · Version · License · Price・確かめていない欄は Unknown）か、使えない理由
    pub line: String,
    /// 公式の頁（文字 = 行き先の host 入り, https の URL）
    pub link: Option<(String, String)>,
    /// `Check this Mac for <名前>` を出す（まだ読んでいない時だけ）
    pub check: bool,
}

/// Home の欄の下の発見の今（`Shell::discovery`・作るのは `project::discover::discover`・手を当てるのは `project::discover::act`）。
/// Wave: PV1
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// 送った一言（欄にも残す）
    pub goal: String,
    /// 頭の 1 行（`Discover (2 of 6) · …`）
    pub line: String,
    /// 3 つまでの候補
    pub outcomes: Vec<Outcome>,
    /// 選んだ候補（None = 候補を出している）
    pub chosen: Option<usize>,
    /// 選んだ候補の道具の行（入っている → web → 入れる の順）
    pub tools: Vec<Tool>,
}

/// 発見の手（`Intent::Discover`・app.rs が `project::discover::act` に渡す）。
/// Wave: PV1
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverAct {
    /// 候補を 1 つ選ぶ（道具の行を出す・Mac は読まない）
    Choose(usize),
    /// 選んだ候補の道具の .app だけを読む（見える opt-in）
    Check,
    /// 候補へ戻る
    Back,
    /// 閉じる（欄の文は残る）
    Close,
}

/// 候補か道具の行を描き、押した手を返す。`Start in <tool>` は `Intent::Generate(outcome.start, [])`（添えた物は home.rs が乗せる）・link は egui が既定の browser で開く。
/// AX の名前: `Choose <題>`・`Check this Mac for <名前>`・link の文字・`Start in <tool>`・`Back`・`Close`（どれも 1 つずつ）。
/// 例: `if let Some(d) = &shell.discovery { if let Some(i) = discover::draw(ui, d) { intent = Some(i) } }`（home::draw の Try の行の後）
/// Wave: PV1
pub fn draw(ui: &mut egui::Ui, d: &Discovery) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    let mut button = |ui: &mut egui::Ui, text: &str, ax: &str, then: Intent| {
        if theme::ax_name(theme::button_compact(ui, text, theme::Variant::Ghost), ax).clicked() {
            intent = Some(then);
        }
    };
    ui.horizontal(|ui| {
        theme::label(ui, Type::Helper, &d.line, t.text_secondary);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| button(ui, "Close", "Close", Intent::Discover(DiscoverAct::Close)));
    });
    ui.add_space(theme::SP_2);
    theme::rule(ui);
    match d.chosen.and_then(|i| d.outcomes.get(i).map(|o| (i, o))) {
        // 候補 3 つ（題・範囲と例・手間と要る物と道具）。card にしない = 行と hairline（§0-6）
        None => {
            for (i, o) in d.outcomes.iter().enumerate() {
                ui.add_space(theme::SP_2);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 120.0).max(120.0));
                        theme::label(ui, Type::BodyStrong, o.title, t.text_primary);
                        theme::label(ui, Type::Helper, format!("{} · e.g. {}", o.scope, o.example), t.text_secondary);
                        theme::label(ui, Type::Helper, format!("{} · {} · Uses {}", o.effort, o.needs, o.tool), t.text_tertiary);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| button(ui, "Choose", &format!("Choose {}", o.title), Intent::Discover(DiscoverAct::Choose(i))));
                });
                ui.add_space(theme::SP_2);
                theme::rule(ui);
            }
        }
        // 道具の行（入っている → web → 入れる）・公式の頁の link・Check・Start・Back
        Some((_, o)) => {
            for tool in &d.tools {
                ui.add_space(theme::SP_2);
                theme::label(ui, Type::BodyStrong, format!("{} · {}", tool.state.label(), tool.name), t.text_primary);
                theme::label(ui, Type::Helper, &tool.line, t.text_secondary);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::SP_3;
                    if let Some((text, url)) = &tool.link {
                        ui.hyperlink_to(text, url);
                    }
                    if tool.check {
                        button(ui, "Check", &format!("Check this Mac for {}", tool.name), Intent::Discover(DiscoverAct::Check));
                    }
                });
                ui.add_space(theme::SP_2);
                theme::rule(ui);
            }
            ui.add_space(theme::SP_3);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = theme::SP_2;
                let start = format!("Start in {}", o.tool);
                button(ui, &start, &start, Intent::Generate(o.start.to_owned(), Vec::new()));
                button(ui, "Back", "Back", Intent::Discover(DiscoverAct::Back));
            });
        }
    }
    intent
}

#[cfg(test)]
mod pv1_tests {
    use super::*;
    use egui::accesskit::{Action, ActionRequest, Node, NodeId, Role};

    fn sample(chosen: Option<usize>) -> Discovery {
        let o = |title, tool| Outcome { title, scope: "One screen", example: "Dodge blocks", effort: "About an hour", needs: "A game engine · no account", tool, start: "Make it in Godot" };
        let tools = vec![
            Tool { state: Match::WebOption, name: "Godot in the browser".into(), line: "Publisher: Godot Engine · Source: editor.godotengine.org · No install".into(), link: Some(("Open Godot in the browser (editor.godotengine.org)".into(), "https://editor.godotengine.org/releases/latest/".into())), check: false },
            Tool { state: Match::RequiresInstall, name: "Godot".into(), line: "Publisher: Godot Engine · Source: godotengine.org · Version: Unknown until you install".into(), link: Some(("Open official page (godotengine.org)".into(), "https://godotengine.org/".into())), check: true },
        ];
        Discovery { goal: "I want to make a small game".into(), line: "Discover (2 of 6) · Pick what to make".into(), outcomes: vec![o("Arcade", "Godot"), o("Platformer", "Godot"), o("Story", "Godot")], chosen, tools: if chosen.is_some() { tools } else { vec![] } }
    }

    /// 1 frame 描いて (AX の木の (id, role, label) の一覧, 押した手)。
    fn frame(ctx: &egui::Context, d: &Discovery, events: Vec<egui::Event>) -> (Vec<(NodeId, Role, String)>, Option<Intent>) {
        let mut got = None;
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))), events, ..Default::default() };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| got = draw(ui, d));
        });
        let nodes: Vec<(NodeId, Role, String)> = out.platform_output.accesskit_update.map(|u| u.nodes.iter().map(|(id, n): &(NodeId, Node)| (*id, n.role(), n.label().or(n.value()).unwrap_or_default().to_owned())).collect()).unwrap_or_default();
        (nodes, got)
    }

    fn press(ctx: &egui::Context, d: &Discovery, label: &str) -> Option<Intent> {
        let nodes = frame(ctx, d, vec![]).0;
        let target = nodes.iter().find(|(_, _, l)| l == label).map(|n| n.0).unwrap_or_else(|| panic!("{label} が AX に無い: {nodes:?}"));
        let ev = egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target, data: None });
        frame(ctx, d, vec![ev]).1.or_else(|| frame(ctx, d, vec![]).1)
    }

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        ctx
    }

    /// 候補は 3 つの button（`Choose <題>`）で、押すと `Discover(Choose(i))`・道具の行はまだ無い（Mac を読む button も無い）。
    /// 負の対照: Choose の index を 0 に固定 → Platformer で赤。
    #[test]
    fn pv1_the_choices_are_three_buttons_that_press_by_ax() {
        let (ctx, d) = (ctx(), sample(None));
        let nodes = frame(&ctx, &d, vec![]).0;
        let buttons: Vec<&str> = nodes.iter().filter(|n| n.1 == Role::Button).map(|n| n.2.as_str()).collect();
        assert!(buttons.contains(&"Choose Arcade") && buttons.contains(&"Choose Platformer") && buttons.contains(&"Choose Story") && buttons.contains(&"Close"), "{buttons:?}");
        assert!(!buttons.iter().any(|b| b.starts_with("Check") || b.starts_with("Start")), "選ぶ前に道具の手を出した: {buttons:?}");
        assert!(nodes.iter().any(|n| n.2.contains("About an hour")), "手間の目安が無い");
        assert_eq!(press(&ctx, &d, "Choose Platformer"), Some(Intent::Discover(DiscoverAct::Choose(1))));
        assert_eq!(press(&ctx, &d, "Close"), Some(Intent::Discover(DiscoverAct::Close)));
    }

    /// 選んだ後: 道具の行（語・名前・事実の 1 行）・公式の頁の link（文字に host）・`Check this Mac for Godot`・`Start in Godot`（= 候補の一言を送る）・`Back`。
    /// 負の対照: link を button にする → Role::Link が無く赤／Start が goal を送る → 赤。
    #[test]
    fn pv1_the_tool_rows_show_facts_and_press_by_ax() {
        let (ctx, d) = (ctx(), sample(Some(0)));
        let nodes = frame(&ctx, &d, vec![]).0;
        // AX の木の並びは描いた順でない（行の並びは project::discover の tools の試験が見る）= 集合で比べる
        let mut links: Vec<&str> = nodes.iter().filter(|n| n.1 == Role::Link).map(|n| n.2.as_str()).collect();
        links.sort_unstable();
        assert_eq!(links, ["Open Godot in the browser (editor.godotengine.org)", "Open official page (godotengine.org)"]);
        for want in ["Web option", "Requires install", "Publisher: Godot Engine", "Version: Unknown"] {
            assert!(nodes.iter().any(|n| n.2.contains(want)), "{want} が無い: {nodes:?}");
        }
        assert_eq!(press(&ctx, &d, "Check this Mac for Godot"), Some(Intent::Discover(DiscoverAct::Check)));
        assert_eq!(press(&ctx, &d, "Start in Godot"), Some(Intent::Generate("Make it in Godot".into(), vec![])));
        assert_eq!(press(&ctx, &d, "Back"), Some(Intent::Discover(DiscoverAct::Back)));
        // 読んだ後は Check を出さない
        let mut read = d.clone();
        read.tools[1].check = false;
        assert!(!frame(&ctx, &read, vec![]).0.iter().any(|n| n.2.starts_with("Check this Mac")));
    }
}
