//! ui の headless test が共有する物（ui_portal.rs・ui_build_progress.rs）: ctx・描いた形の読み出し・宣伝文の判定。
//! `crate::ui` は各 test crate が `#[path]` で取り込んだ src/ui。
#![allow(dead_code)]

pub mod readback; // W2c: 試験の側の oracle と L1〜L4 の判定（std と serde_json だけ）

use crate::ui::{motion, theme};
use eframe::egui::{self, epaint::ClippedShape, Color32, Context, FontFamily, FullOutput, Pos2, Rect, Shape};

pub const SCREEN: Rect = Rect { min: Pos2::ZERO, max: Pos2::new(1200.0, 800.0) };
/// shimmer の光が画面の中央を通る時刻（位相 0.5 = 1.2s 周期の 0.6s）。光が画面の外の時刻で測ると mesh 0 の偽の赤になる。
pub const SHINE_MID: f64 = 1.8;

/// 動きの段を明示した ctx（true = 試験だけの即時 `MotionMode::Cut`・1 frame で終点を読む／false = Full）。Reduce Motion の短い fade は
/// `motion::set_motion_mode(&c, MotionMode::Fade)` で測る（UI-6a）。見出しの font family（kangae-semibold）は既定の proportional で代用する（system font を読まない）。
pub fn ctx(reduce: bool) -> Context {
    let c = Context::default();
    motion::set_motion_mode(&c, if reduce { motion::MotionMode::Cut } else { motion::MotionMode::Full });
    let mut f = egui::FontDefinitions::default();
    let prop = f.families[&FontFamily::Proportional].clone();
    f.families.insert(FontFamily::Name(theme::SEMIBOLD.into()), prop);
    c.set_fonts(f);
    c.set_theme(egui::ThemePreference::Light);
    c
}

pub fn shapes(out: &FullOutput) -> Vec<&Shape> {
    fn walk<'a>(s: &'a Shape, v: &mut Vec<&'a Shape>) {
        match s {
            Shape::Vec(inner) => inner.iter().for_each(|s| walk(s, v)),
            s => v.push(s),
        }
    }
    let mut v = Vec::new();
    out.shapes.iter().for_each(|ClippedShape { shape, .. }| walk(shape, &mut v));
    v
}

/// 描いた文字（galley の text と描いた範囲・最初の section の色）。
pub fn texts(out: &FullOutput) -> Vec<(String, Rect, Color32)> {
    shapes(out)
        .into_iter()
        .filter_map(|s| match s {
            Shape::Text(t) => Some((t.galley.text().to_owned(), t.visual_bounding_rect(), t.galley.job.sections.first().map_or(t.fallback_color, |s| s.format.color))),
            _ => None,
        })
        .collect()
}

/// slot の中に描いた、色の付いた rect / 円（骨格の帯）と mesh（光）。
pub fn bars_in(out: &FullOutput, slot: Rect) -> (usize, usize) {
    let inside = |r: Rect| slot.shrink(0.5).intersects(r);
    let mut rects = 0;
    let mut meshes = 0;
    for s in shapes(out) {
        match s {
            Shape::Rect(r) if r.fill != Color32::TRANSPARENT && inside(r.rect) => rects += 1,
            Shape::Circle(c) if c.fill != Color32::TRANSPARENT && inside(Rect::from_center_size(c.center, egui::Vec2::splat(c.radius * 2.0))) => rects += 1,
            Shape::Mesh(m) if inside(m.calc_bounds()) => meshes += 1,
            _ => {}
        }
    }
    (rects, meshes)
}

// ---------------------------------------------------------------------------
// 宣伝文 0（scripts/check_ui_copy.py と同じ表 docs/ui-copy-rules.json を読む。表を 2 か所に書かない）
// ---------------------------------------------------------------------------

pub struct Rules {
    dekimasu: Vec<String>,
    trailing: Vec<String>,
    ad: Vec<String>,
    exclaim: Vec<String>,
    ends: Vec<char>,
    state_endings: Vec<String>,
    state_prefix: String,
    name_max: f64,
    next_max: f64,
}

pub fn rules() -> Rules {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../docs/ui-copy-rules.json")).expect("語の表");
    let list = |k: &str| v[k].as_array().unwrap_or_else(|| panic!("{k}")).iter().map(|s| s.as_str().unwrap().to_owned()).collect::<Vec<_>>();
    Rules {
        dekimasu: list("dekimasu_endings"),
        trailing: list("dekimasu_trailing"),
        ad: list("ad_words"),
        exclaim: list("exclaim"),
        ends: list("sentence_ends").iter().flat_map(|s| s.chars()).collect(),
        state_endings: list("state_endings"),
        state_prefix: v["state_prefix"].as_str().unwrap().to_owned(),
        name_max: v["name_max_width"].as_f64().unwrap(),
        next_max: v["next_step_max_width"].as_f64().unwrap(),
    }
}

/// 字数（ASCII は半字・python の width と同じ）。
pub fn width(s: &str) -> f64 {
    s.trim().chars().map(|c| if c.is_ascii() { 0.5 } else { 1.0 }).sum()
}

fn dekimasu(r: &Rules, s: &str) -> bool {
    let s = s.trim();
    let s = r.trailing.iter().find_map(|t| s.strip_suffix(t.as_str())).unwrap_or(s);
    r.dekimasu.iter().any(|e| s.ends_with(e.as_str()))
}

/// 赤の規則 id（python の dekimasu・ad-word・exclaim・multi・long-name）。`name` = button / 状態語などの名前。
pub fn judge(r: &Rules, text: &str, name: bool) -> Vec<&'static str> {
    let mut hits = Vec::new();
    if dekimasu(r, text) {
        hits.push("dekimasu");
    }
    if r.ad.iter().any(|w| text.contains(w.as_str())) {
        hits.push("ad-word");
    }
    if r.exclaim.iter().any(|w| text.contains(w.as_str())) {
        hits.push("exclaim");
    }
    let parts: Vec<&str> = text.split(|c| r.ends.contains(&c)).map(str::trim).filter(|p| !p.is_empty()).collect();
    let fact = |p: &str| r.state_endings.iter().any(|e| p.ends_with(e.as_str())) || p.rsplit(|c: char| !c.is_alphanumeric()).next().is_some_and(|w| w.starts_with(r.state_prefix.as_str()));
    let allowed_two = parts.len() == 2 && fact(parts[0]) && width(parts[1]) <= r.next_max && !dekimasu(r, parts[1]);
    if parts.len() >= 2 && !allowed_two {
        hits.push("multi");
    }
    // 字幅の上限は日本語の名前の規則（check_ui_copy.py の judge と同じく、日本語の無い英語の名前には当てない・英語は表の EN をそのまま使う）
    let ja = text.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}'));
    if name && ja && width(text) > r.name_max {
        hits.push("long-name");
    }
    hits
}

/// 判定が赤くなれる事（表を読めていない・規則を写し損ねた時に、下の「0 件」が緑のまま嘘をつかない）。
#[test]
fn copy_judge_is_armed() {
    let r = rules();
    assert_eq!(judge(&r, "直接操作できます", false), ["dekimasu"]);
    assert_eq!(judge(&r, "まずここから", false), ["ad-word"]);
    assert_eq!(judge(&r, "接続しました！", false), ["exclaim"]);
    assert_eq!(judge(&r, "窓を重ねる。元の app が入力を受ける。", false), ["multi"]);
    assert_eq!(judge(&r, "元の app の窓をこの枠の中で直接操作する", true), ["long-name"]);
    assert!(judge(&r, "Run someone else's code in isolation", true).is_empty(), "英語の名前に日本語の字幅の上限を当てない（表の EN）");
    assert!(judge(&r, "まだデータがありません。「追加する」で 1 つ目を書く", false).is_empty(), "許す 2 文");
}

