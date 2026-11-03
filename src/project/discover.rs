//! PV1（図 §50・spec §6 の discover）: 迷いの一言（game を作りたい・道具を名指ししない）に成果物の候補 3 つ（`GAME_OUTCOMES`）→
//! 選んだ候補の道具の行（入っている → web → 入れる・表の事実・版は読めた時だけ）。何も入れない・Mac を読むのは `Check` を押した時の .app 1 つだけ。
//! 描くのは `ui::discover::draw`、ここは表（`acquire::CATALOG`）から中身を作って手を当てる。acquire.rs と分ける理由: ui の型を使う
//! （acquire.rs は std と serde_json だけ = acquire だけを `#[path]` で引く反証の crate が compile できる）。

use super::acquire::{entry_for, host, readback, Entry, Installed, Stage, CATALOG};
use crate::ui::discover::{DiscoverAct, Discovery, Match, Outcome, Tool};
use std::path::Path;

/// PV1: game の迷いの一言に出す成果物の候補（軸 = 大きさと遊び方の違う 3 つ・どれも表の Godot で作れる・送る一言は道具の名前を含む）。
/// 要る物は確かめた事だけ（容量は書かない = 測っていない）。
/// Wave: PV1
pub static GAME_OUTCOMES: [Outcome; 3] = [
    Outcome {
        title: "One-screen arcade game",
        scope: "One screen · a score · game over",
        example: "Dodge falling blocks for 60 seconds",
        effort: "About an hour",
        needs: "A game engine · no account",
        tool: "Godot",
        start: "Make a one-screen arcade game in Godot: dodge falling blocks for 60 seconds, with a score and game over",
    },
    Outcome {
        title: "Small platformer",
        scope: "3 short levels · run, jump, collect",
        example: "A cat that jumps across rooftops",
        effort: "A weekend",
        needs: "A game engine · simple shapes for art",
        tool: "Godot",
        start: "Make a small platformer in Godot with 3 short levels where you run, jump and collect stars",
    },
    Outcome {
        title: "Story you play by choosing",
        scope: "Text and choices · 3 endings",
        example: "A mystery in an old house",
        effort: "An evening",
        needs: "A game engine · your own writing",
        tool: "Godot",
        start: "Make a short choose-your-path story game in Godot with 3 endings",
    },
];

/// PV1: 一言が作る事（make・build・create・want）と game を言い、表の app の名前を言っていない時だけ成果物の候補（`GAME_OUTCOMES`）。
/// それ以外は None（今まで通り一言から作る）。27 手の文・Home の例は None（試験が docs/demo/full.md と rehearse.py の行で守る）。
/// 例: `outcomes_for("I want to make a small game")` → Some(3 件)・`outcomes_for("Make a 2D game in Godot")` → None
/// Wave: PV1
pub fn outcomes_for(goal: &str) -> Option<&'static [Outcome]> {
    let lower = goal.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let says = |any: &[&str]| words.iter().any(|w| any.contains(w));
    // 表の名前・別名を言った一言は名指し = 今まで通り作る（その app が無ければ PV0 の 1 問）
    let named = CATALOG.iter().any(|e| std::iter::once(e.app_name).chain(e.aliases.iter().copied()).any(|n| lower.contains(&n.to_lowercase())));
    // ponytail: 語の表 1 つ（game の目的だけ）。目的を足す時は候補の表と一緒に足す
    (says(&["game", "games"]) && says(&["make", "making", "build", "create", "want"]) && !named).then_some(&GAME_OUTCOMES[..])
}

/// 候補を出している時の頭の 1 行。
fn pick_line() -> String {
    format!("{} · Pick what to make · Nothing is installed", Stage::Discover.row())
}

/// PV1: 迷いの一言の発見の今（候補を出している・道具の行は無い・Mac は読まない）。None = 候補を出さない一言。
/// Wave: PV1
pub fn discover(goal: &str) -> Option<Discovery> {
    let outcomes = outcomes_for(goal)?;
    Some(Discovery { goal: goal.to_owned(), line: pick_line(), outcomes: outcomes.to_vec(), chosen: None, tools: Vec::new() })
}

/// PV1: 表の 1 件の道具の行（入っている → web → 入れる の順・spec §6 の「在る物を強く優先」）。`checked` = None はまだ読んでいない（ここでは読まない）・
/// Some は `readback` の答え（None = 無い・Ok = 表の bundle id の .app・Err = 別の物か読めない）。版は読めた時だけ（それ以外は Unknown）。
/// Wave: PV1
pub fn tools(entry: &Entry, checked: Option<Option<Result<Installed, String>>>) -> Vec<Tool> {
    let page = |url: &str, label: String| Some((format!("{label} ({})", host(url)), url.to_owned()));
    let web = entry.web.map(|url| Tool {
        state: Match::WebOption,
        name: format!("{} in the browser", entry.app_name),
        line: format!("Publisher: {} · Source: {} · No install · Price: {}", entry.publisher, host(url), entry.price),
        link: page(url, format!("Open {} in the browser", entry.app_name)),
        check: false,
    });
    let facts = format!("Publisher: {} · Source: {} · Version: Unknown until you install · License: {} · Price: {}", entry.publisher, host(entry.official_page), entry.license, entry.price);
    let official = page(entry.official_page, "Open official page".into());
    let tool = |state, line, link, check| Tool { state, name: entry.app_name.to_owned(), line, link, check };
    let local = match checked {
        Some(Some(Ok(i))) => tool(Match::Installed, format!("Found {} in Applications · Version {} · Nothing to install", entry.app_file, i.version), None, false),
        Some(Some(Err(why))) => tool(Match::Unavailable, why, official, false),
        Some(None) => tool(Match::RequiresInstall, format!("{facts} · Not in Applications"), official, false),
        None => tool(Match::RequiresInstall, facts, official, true),
    };
    // 在る物を強く優先（spec §6）: 入っていれば先頭・それ以外は入れずに済む web 版を先に
    let mut out: Vec<Tool> = web.into_iter().collect();
    if local.state == Match::Installed {
        out.insert(0, local);
    } else {
        out.push(local);
    }
    out
}

/// PV1: 発見の手を当てる（Choose = 道具の行を読まずに作る・Check = 選んだ候補の道具の .app 1 つだけ `readback`・Back = 候補へ）。false = 閉じる（Close）。
/// 範囲の外の Choose・選んでいない時の Check は何もしない。`apps_dir` は製品なら /Applications（`Homebrew::find().apps_dir`）。
/// Wave: PV1
pub fn act(d: &mut Discovery, a: DiscoverAct, apps_dir: &Path) -> bool {
    match a {
        DiscoverAct::Choose(i) => {
            if let Some((title, tool)) = d.outcomes.get(i).map(|o| (o.title, o.tool)) {
                if let Some(e) = entry_for(tool) {
                    d.line = format!("{} · {title} uses {tool} · Nothing is installed", Stage::Discover.row());
                    d.chosen = Some(i);
                    d.tools = tools(e, None);
                }
            }
        }
        DiscoverAct::Check => {
            if let Some(e) = d.chosen.and_then(|i| d.outcomes.get(i)).and_then(|o| entry_for(o.tool)) {
                d.tools = tools(e, Some(readback(e, apps_dir)));
            }
        }
        DiscoverAct::Back => {
            d.chosen = None;
            d.tools.clear();
            d.line = pick_line();
        }
        DiscoverAct::Close => return false,
    }
    true
}

#[cfg(test)]
mod pv1_tests {
    use crate::project::acquire::pv0_tests::temp;
    use super::*;

    /// 候補を出すのは「作る」と game を言って app を名指ししない一言だけ。27 手の文（docs/demo/full.md と rehearse.py の各行）と Home の例は出さない・
    /// 候補の一言（`start`）を送っても候補をもう一度出さない（道具の名前を含む）。
    /// 負の対照: 作る語の条件を外す → board game の行で赤／表の名前の条件を外す → in Godot と start で赤。
    #[test]
    fn pv1_outcomes_only_for_a_vague_game_goal() {
        for g in ["I want to make a small game", "help me build a game", "Create games with my kid"] {
            assert_eq!(outcomes_for(g).map(<[Outcome]>::len), Some(3), "{g}");
        }
        for g in ["Make a 2D game in Godot", "make a godot engine game", "Track my board game collection", "Keep research notes", ""] {
            assert!(outcomes_for(g).is_none(), "{g}");
        }
        for (_, ex) in crate::ui::home::EXAMPLES {
            assert!(outcomes_for(ex).is_none(), "{ex}");
        }
        for line in include_str!("../../docs/demo/full.md").lines().chain(include_str!("../../scripts/qa/bg/rehearse.py").lines()) {
            assert!(outcomes_for(line).is_none(), "27 手の行で候補を出す: {line}");
        }
        for o in &GAME_OUTCOMES {
            assert!(entry_for(o.tool).is_some(), "表に無い道具: {}", o.tool);
            assert!(outcomes_for(o.start).is_none(), "start をもう一度候補にする: {}", o.start);
        }
    }

    /// 道具の行の順は 入っている → web → 入れる・事実は表の物（Publisher・Source の host・License・Price）と、読めた時だけの版（それ以外は Unknown）。
    /// 負の対照: Installed の時に RequiresInstall も出す → 赤／版に表の外の値を書く → Unknown で赤。
    #[test]
    fn pv1_tools_list_installed_then_web_then_install() {
        let e = &CATALOG[0];
        let st = |c| tools(e, c).iter().map(|t| t.state).collect::<Vec<_>>();
        assert_eq!(st(None), [Match::WebOption, Match::RequiresInstall]);
        assert_eq!(st(Some(None)), [Match::WebOption, Match::RequiresInstall]);
        let i = Installed { bundle_id: "org.godotengine.godot".into(), version: "4.6".into(), path: "/Applications/Godot.app".into() };
        assert_eq!(st(Some(Some(Ok(i.clone())))), [Match::Installed, Match::WebOption]);
        assert_eq!(st(Some(Some(Err("Applications has a Godot.app, but it is com.example.other, not Godot".into())))), [Match::WebOption, Match::Unavailable]);
        let unread = tools(e, None);
        let install = &unread[1];
        for want in ["Publisher: Godot Engine", "Source: godotengine.org", "Version: Unknown", "License: MIT", "Price: Free"] {
            assert!(install.line.contains(want), "{want}: {install:?}");
        }
        assert!(install.check, "まだ読んでいないのに Check を出さない");
        assert_eq!(install.link, Some(("Open official page (godotengine.org)".into(), "https://godotengine.org/".into())));
        assert_eq!(unread[0].link, Some(("Open Godot in the browser (editor.godotengine.org)".into(), "https://editor.godotengine.org/releases/latest/".into())));
        assert!(!tools(e, Some(None))[1].check, "読んだ後に Check を出した");
        assert!(tools(e, Some(Some(Ok(i))))[0].line.contains("4.6"), "読めた版を出さない");
    }

    /// Mac を読むのは `Check` を押した時だけ・選んだ候補の道具の .app 1 つだけ（見える opt-in・PV-007）。範囲の外・選ぶ前の手は何もしない・Close で閉じる。
    /// 負の対照: Choose で readback する → Choose の直後の行が Installed で赤。
    #[test]
    fn pv1_check_reads_only_the_one_app_and_only_after_the_press() {
        let dir = temp("pv1-check");
        let apps = dir.join("apps");
        std::fs::create_dir_all(apps.join("Godot.app/Contents")).unwrap();
        std::fs::write(apps.join("Godot.app/Contents/Info.plist"), r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.godotengine.godot</string><key>CFBundleShortVersionString</key><string>4.6</string></dict></plist>"#).unwrap();
        let mut d = discover("I want to make a small game").expect("候補");
        assert_eq!((d.outcomes.len(), d.chosen, d.tools.len()), (3, None, 0));
        assert!(d.line.starts_with("Discover (2 of 6)"), "{}", d.line);
        let states = |d: &Discovery| d.tools.iter().map(|t| t.state).collect::<Vec<_>>();
        assert!(act(&mut d, DiscoverAct::Choose(1), &apps));
        assert_eq!(d.chosen, Some(1));
        assert_eq!(states(&d), [Match::WebOption, Match::RequiresInstall], "Choose で Mac を読んだ");
        assert!(act(&mut d, DiscoverAct::Check, &apps));
        assert_eq!(states(&d), [Match::Installed, Match::WebOption]);
        let before = d.clone();
        assert!(act(&mut d, DiscoverAct::Choose(7), &apps));
        assert_eq!(d, before, "範囲の外の Choose で変わった");
        assert!(act(&mut d, DiscoverAct::Back, &apps));
        assert_eq!((d.chosen, d.tools.len()), (None, 0));
        assert!(act(&mut d, DiscoverAct::Check, &apps));
        assert!(d.tools.is_empty(), "選ぶ前の Check で読んだ");
        assert!(!act(&mut d, DiscoverAct::Close, &apps));
        assert!(discover("Make a 2D game in Godot").is_none());
    }
}
