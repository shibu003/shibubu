//! CHAT-FREE-1101（owner 198 通目の 6「出力される形が常に一緒なの困る…aiに考えさせること」）: chat の答えの Markdown を描く（段 1）。
//! 読むのは pulldown-cmark（parse だけ・表・チェックリスト・取り消し線）、描くのは egui（ここ）。答えの形は AI が決め、製品は描くだけ。
//! 描く物: 見出し・太字・斜体・取り消し・`code`・箇条書き（入れ子）・番号・チェックリスト（Home は押して付け外し）・罫線の表・
//! code block（等幅・言語・Copy）・引用・link（http / https / mailto だけ本人の browser で開く）・区切りの線・`<details><summary>` の折りたたみ。
//! 描けない物は黙って消さない: HTML の block は code block・行の中の HTML は字のまま・画像は alt の字の link。
//! 段 2（owner 201「AI の画用紙」）: ```html は WebView の入った build（app.rs が `set_canvas_host`）なら `Interactive view` の card（`Run` で閉じた WebView・
//! 同時に 1 つ・枠が全部見えている時だけ置く = `take_canvases`）、入っていない build は code block のまま。
//! 字は全部 `theme::chat_label` = 選べる・scroll をまたいで選べる（COPY-SCROLL-1101）。
//! 例: `if let Some(n) = markdown::show(ui, (&turn.at, i), &turn.text, ink, true) { intent = Some(Intent::CheckItem(i, n)) }`
//! ponytail: 毎 frame 文を読み直す（会話は数 KB で速い）。長い会話で重くなったら文の hash で `Block` を ctx の cache に置く。

use super::theme::{self, Type, Variant};
use eframe::egui::{
    self,
    text::{CCursor, LayoutJob, TextFormat},
    Color32, CornerRadius, FontFamily, FontId, Margin, Stroke,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use std::ops::Range;

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Style {
    strong: bool,
    em: bool,
    strike: bool,
    code: bool,
}

#[derive(Debug)]
struct Span {
    text: String,
    style: Style,
    link: Option<String>,
}

#[derive(Debug)]
enum Block {
    Para(Vec<Span>),
    Heading(Type, Vec<Span>),
    Code { lang: String, text: String },
    Quote(Vec<Block>),
    /// `start` = 番号の頭（None = 点）
    List { start: Option<u64>, items: Vec<Item> },
    Table { head: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
    Rule,
    Fold { summary: String, open: bool, body: Vec<Block> },
}

#[derive(Debug)]
struct Item {
    /// チェックリストの行 = (付いているか, 文の頭からの番号〈`toggle` が数えるのと同じ〉)。番号 None = HTML の中なので押せない
    check: Option<(bool, Option<usize>)>,
    body: Vec<Block>,
}

enum Kind {
    Root,
    Quote,
    Item(Option<(bool, Option<usize>)>),
    /// `<details>`（summary・open）。Start / End の event が無い = 閉じる前に別の End が来たら先に閉じる
    Fold(String, bool),
}

enum Line {
    Para,
    Heading(Type),
    Cell,
    /// 詰めた箇条書きの行の字（Paragraph の event が無い）。Start / End が無い
    Loose,
}

enum Inline {
    Strong,
    Em,
    Strike,
    Link(String),
    /// 読まない tag（脚注など・今の Options では出ない）。End と数を合わせるだけ
    Plain,
}

enum Frame {
    Blocks(Kind, Vec<Block>),
    List(Option<u64>, Vec<Item>),
    Table(Vec<Vec<Span>>, Vec<Vec<Vec<Span>>>),
    Row(bool, Vec<Vec<Span>>),
    Spans(Line, Vec<Span>),
    Code(String, String),
    Html(String),
    Inline(Inline),
}

/// event の列 → `Block` の木。End は tag を見ず、いちばん上の枠を閉じる（event は必ず入れ子 = 版ごとの TagEnd の形に依らない）。
struct Build {
    stack: Vec<Frame>,
    /// 次のチェックの番号。None = HTML の中の読み直し（`toggle` は HTML の中を数えない = 押せない）
    checks: Option<usize>,
}

fn parse(text: &str, checks: Option<usize>) -> Vec<Block> {
    let mut b = Build { stack: vec![Frame::Blocks(Kind::Root, Vec::new())], checks };
    for ev in Parser::new_ext(text, options()) {
        b.event(ev);
    }
    while b.stack.len() > 1 {
        b.close();
    }
    match b.stack.pop() {
        Some(Frame::Blocks(_, v)) => v,
        _ => Vec::new(),
    }
}

impl Build {
    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(_) => {
                // 自分の Start / End を持たない枠（詰めた行・<details>）は、外の End が来たら先に閉じる
                while matches!(self.stack.last(), Some(Frame::Spans(Line::Loose, _) | Frame::Blocks(Kind::Fold(..), _))) && self.stack.len() > 1 {
                    self.close();
                }
                self.close();
            }
            Event::Text(s) => self.text(&s, |_| {}),
            Event::Code(s) => self.text(&s, |st| st.code = true),
            Event::InlineHtml(s) => {
                let br = s.trim().to_ascii_lowercase().starts_with("<br");
                self.text(if br { "\n" } else { &*s }, |_| {})
            }
            Event::Html(s) => self.text(&s, |_| {}),
            Event::SoftBreak | Event::HardBreak => self.text("\n", |_| {}),
            Event::Rule => {
                self.settle();
                self.block(Block::Rule);
            }
            Event::TaskListMarker(on) => {
                let n = self.checks.as_mut().map(|c| {
                    *c += 1;
                    *c - 1
                });
                if let Some(Frame::Blocks(Kind::Item(c), _)) = self.stack.iter_mut().rev().find(|f| matches!(f, Frame::Blocks(Kind::Item(_), _))) {
                    *c = Some((on, n));
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        let frame = match tag {
            Tag::Paragraph => Frame::Spans(Line::Para, Vec::new()),
            Tag::Heading { level, .. } => Frame::Spans(Line::Heading(if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) { Type::Group } else { Type::BodyStrong }), Vec::new()),
            Tag::BlockQuote(_) => Frame::Blocks(Kind::Quote, Vec::new()),
            Tag::CodeBlock(kind) => Frame::Code(
                match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
                    CodeBlockKind::Indented => String::new(),
                },
                String::new(),
            ),
            Tag::HtmlBlock => Frame::Html(String::new()),
            Tag::List(start) => Frame::List(start, Vec::new()),
            Tag::Item => {
                self.settle();
                self.stack.push(Frame::Blocks(Kind::Item(None), Vec::new()));
                return self.stack.push(Frame::Spans(Line::Loose, Vec::new()));
            }
            Tag::Table(_) => Frame::Table(Vec::new(), Vec::new()),
            Tag::TableHead => return self.stack.push(Frame::Row(true, Vec::new())),
            Tag::TableRow => return self.stack.push(Frame::Row(false, Vec::new())),
            Tag::TableCell => return self.stack.push(Frame::Spans(Line::Cell, Vec::new())),
            Tag::Emphasis => return self.stack.push(Frame::Inline(Inline::Em)),
            Tag::Strong => return self.stack.push(Frame::Inline(Inline::Strong)),
            Tag::Strikethrough => return self.stack.push(Frame::Inline(Inline::Strike)),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => return self.stack.push(Frame::Inline(Inline::Link(dest_url.to_string()))),
            _ => return self.stack.push(Frame::Inline(Inline::Plain)),
        };
        self.settle();
        self.stack.push(frame);
    }

    /// block を始める前に、詰めた行の字を段落にして閉じる（空なら捨てる）。
    fn settle(&mut self) {
        if matches!(self.stack.last(), Some(Frame::Spans(Line::Loose, _))) {
            self.close();
        }
    }

    fn text(&mut self, s: &str, with: impl Fn(&mut Style)) {
        let mut style = Style::default();
        let mut link = None;
        for f in self.stack.iter().rev() {
            match f {
                Frame::Inline(Inline::Strong) => style.strong = true,
                Frame::Inline(Inline::Em) => style.em = true,
                Frame::Inline(Inline::Strike) => style.strike = true,
                Frame::Inline(Inline::Link(u)) => link = link.or_else(|| Some(u.clone())),
                Frame::Inline(Inline::Plain) => {}
                _ => break,
            }
        }
        with(&mut style);
        if !matches!(self.stack.iter().rev().find(|f| !matches!(f, Frame::Inline(_))), Some(Frame::Spans(..) | Frame::Code(..) | Frame::Html(_))) {
            self.stack.push(Frame::Spans(Line::Loose, Vec::new()));
        }
        match self.stack.iter_mut().rev().find(|f| !matches!(f, Frame::Inline(_))) {
            Some(Frame::Code(_, t) | Frame::Html(t)) => t.push_str(s),
            Some(Frame::Spans(_, spans)) => match spans.last_mut().filter(|l| l.style == style && l.link == link) {
                Some(last) => last.text.push_str(s),
                None => spans.push(Span { text: s.to_owned(), style, link }),
            },
            _ => {}
        }
    }

    /// 閉じた block を、いちばん近い block の列（Root・引用・項目・折りたたみ）に足す。
    fn block(&mut self, b: Block) {
        if let Some(Frame::Blocks(_, v)) = self.stack.iter_mut().rev().find(|f| matches!(f, Frame::Blocks(..))) {
            v.push(b);
        }
    }

    fn close(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let Some(top) = self.stack.pop() else { return };
        match top {
            Frame::Blocks(Kind::Root, v) => v.into_iter().for_each(|b| self.block(b)),
            Frame::Blocks(Kind::Quote, body) => self.block(Block::Quote(body)),
            Frame::Blocks(Kind::Fold(summary, open), body) => self.block(Block::Fold { summary, open, body }),
            Frame::Blocks(Kind::Item(check), body) => match self.stack.last_mut() {
                Some(Frame::List(_, items)) => items.push(Item { check, body }),
                _ => body.into_iter().for_each(|b| self.block(b)),
            },
            Frame::List(start, items) => self.block(Block::List { start, items }),
            Frame::Table(head, rows) => self.block(Block::Table { head, rows }),
            Frame::Row(head, cells) => {
                if let Some(Frame::Table(h, rows)) = self.stack.last_mut() {
                    if head {
                        *h = cells;
                    } else {
                        rows.push(cells);
                    }
                }
            }
            Frame::Spans(Line::Cell, spans) => {
                if let Some(Frame::Row(_, cells)) = self.stack.last_mut() {
                    cells.push(spans);
                }
            }
            Frame::Spans(Line::Para | Line::Loose, spans) => {
                if spans.iter().any(|s| !s.text.trim().is_empty()) {
                    self.block(Block::Para(spans));
                }
            }
            Frame::Spans(Line::Heading(ty), spans) => self.block(Block::Heading(ty, spans)),
            Frame::Code(lang, text) => self.block(Block::Code { lang, text }),
            Frame::Html(html) => self.html(&html),
            Frame::Inline(_) => {}
        }
    }

    /// HTML の block。`<details>`・`<summary>…</summary>`・`</details>` だけ読む（AI の長い log・考えた過程を畳む）。間の字は Markdown として読み直す。
    /// どれも無い HTML は code block のまま見せる（黙って消さない・描くのは段 2 の WebView の card）。
    fn html(&mut self, html: &str) {
        const TAGS: [&str; 3] = ["<details", "<summary", "</details"];
        let mut rest = html;
        let mut any = false;
        loop {
            // 小文字にしても byte の位置は同じ（ASCII だけが変わる）
            let lower = rest.to_ascii_lowercase();
            let Some((at, tag)) = TAGS.iter().filter_map(|t| lower.find(*t).map(|i| (i, *t))).min() else { break };
            any = true;
            self.nested(&rest[..at]);
            let after = &rest[at..];
            let end = after.find('>').map_or(after.len(), |i| i + 1);
            rest = &after[end..];
            match tag {
                "<details" => {
                    let open = after[..end].to_ascii_lowercase().contains(" open");
                    self.stack.push(Frame::Blocks(Kind::Fold(String::new(), open), Vec::new()));
                }
                "<summary" => {
                    let close = rest.to_ascii_lowercase().find("</summary").unwrap_or(rest.len());
                    let summary = strip_tags(&rest[..close]);
                    rest = &rest[close..];
                    rest = match rest.find('>') {
                        Some(i) => &rest[i + 1..],
                        None => "",
                    };
                    match self.stack.last_mut() {
                        Some(Frame::Blocks(Kind::Fold(s, _), _)) if s.is_empty() => *s = summary,
                        _ => self.block(Block::Para(vec![Span { text: summary, style: Style { strong: true, ..Style::default() }, link: None }])),
                    }
                }
                _ => {
                    if matches!(self.stack.last(), Some(Frame::Blocks(Kind::Fold(..), _))) {
                        self.close();
                    }
                }
            }
        }
        if any {
            self.nested(rest);
        } else if !html.trim().is_empty() {
            // 段 2: 文の中の生の HTML は code block のまま（動かすのは AI が ```html で囲んだ物だけ = `RAW_HTML` は card にしない）
            self.block(Block::Code { lang: RAW_HTML.into(), text: html.trim_end().to_owned() });
        }
    }

    /// HTML の block の中の字を Markdown として読み、今の列に足す（チェックは押せない = `toggle` は HTML の中を数えない）。
    fn nested(&mut self, text: &str) {
        if !text.trim().is_empty() {
            parse(text, None).into_iter().for_each(|b| self.block(b));
        }
    }
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// 文の `n` 番目のチェック（`- [ ]` / `- [x]`）を逆にした文（Home の会話で押した時・次の一言で AI も読む）。無ければ None。
/// 数え方は `show` の返す番号と同じ（文の頭から・HTML の block の中は数えない）。例: `markdown::toggle("- [ ] a\n- [x] b", 1) == Some("- [ ] a\n- [ ] b")`
pub fn toggle(text: &str, n: usize) -> Option<String> {
    let (_, range) = Parser::new_ext(text, options()).into_offset_iter().filter(|(e, _)| matches!(e, Event::TaskListMarker(_))).nth(n)?;
    let open = range.start + text.get(range.start..)?.find('[')?;
    let mark = match text.as_bytes().get(open + 1)? {
        b' ' => "x",
        b'x' | b'X' => " ",
        _ => return None,
    };
    let mut out = text.to_owned();
    out.replace_range(open + 1..open + 2, mark);
    Some(out)
}

/// chat の 1 つの答え（Shibubu の turn）を描く。`salt` = 会話の中で一意（turn の時刻と位置）= 折りたたみ・表・code の scroll の id。
/// `can_check` = チェックを押せる（Home の会話。Project の会話は読むだけ）。返り値 = 押したチェックの番号（`toggle` に渡す）。
pub fn show(ui: &mut egui::Ui, salt: impl std::hash::Hash, text: &str, ink: Color32, can_check: bool) -> Option<usize> {
    let blocks = parse(text, Some(0));
    let mut d = Draw { t: theme::tokens(ui.ctx()), base: egui::Id::new(("kangae.markdown", salt)), n: 0, can_check, hit: None };
    ui.vertical(|ui| d.blocks(ui, &blocks, ink));
    d.hit
}

/// 段 2: 文の中の生の HTML の block の code block の見出し（```html の card と分ける）。
const RAW_HTML: &str = "Raw HTML";

/// 段 2: ```html の card の枠の高さ（chat の列の中で図・表・画面の下書きが 1 枚見える高さ）。
const CANVAS_H: f32 = 280.0;

/// 段 2: この frame に置かれた `Run` の card（枠が全部見えている時だけ）。app.rs が frame の終わりに `take_canvases` で取り、閉じた WebView をこの矩形に合わせる。
/// AI-COMPOSE-1101: Page の AI が書いた部品（renderer の `canvas`）も同じ形で置く（`part` = その node の key と前に残した値・chat の card は None）。
/// 新しくした理由: markdown は描くだけで WebView（surface）を知らない = ui から host へ渡す物が要る。
#[cfg_attr(not(feature = "webview"), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct CanvasSlot {
    pub key: u64,
    pub html: String,
    pub rect: egui::Rect,
    pub part: Option<(String, serde_json::Map<String, serde_json::Value>)>,
}

fn slot_id() -> egui::Id {
    egui::Id::new("kangae.markdown.canvas.slot")
}

fn live_id() -> egui::Id {
    egui::Id::new("kangae.markdown.canvas.live")
}

fn host_id() -> egui::Id {
    egui::Id::new("kangae.markdown.canvas.host")
}

/// 段 2: ```html を card にできる build か（app.rs が feature `webview` の build で毎 frame `true`）。立っていなければ code block のまま（黙って消さない）。
#[cfg_attr(not(feature = "webview"), allow(dead_code))]
pub fn set_canvas_host(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(host_id(), on));
}

/// 段 2: ```html と Page の `canvas` を閉じた WebView にできる build か（renderer の `canvas` の腕も読む）。
pub fn canvas_host(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(host_id())).unwrap_or(false)
}

/// 段 2: `Run` を押した card の key（同時に 1 つ・None = どれも動かしていない）。
pub fn live_canvas(ctx: &egui::Context) -> Option<u64> {
    ctx.data(|d| d.get_temp::<u64>(live_id()))
}

/// 段 2: 動かす card を替える（card の `Run` / `Stop`・host が頼みを 1 回受けた後と、開けなかった時に None）。
pub fn set_live_canvas(ctx: &egui::Context, key: Option<u64>) {
    ctx.data_mut(|d| match key {
        Some(k) => d.insert_temp(live_id(), k),
        None => d.remove::<u64>(live_id()),
    });
}

/// 段 2: この frame に置かれた card を全部取る（取ったら空 = 次の frame に置かれなければ host は隠す）。chat の card は多くて 1 つ・Page の部品は見えている数だけ。
#[cfg_attr(not(feature = "webview"), allow(dead_code))]
pub fn take_canvases(ctx: &egui::Context) -> Vec<CanvasSlot> {
    ctx.data_mut(|d| {
        let slots = d.get_temp::<Vec<CanvasSlot>>(slot_id()).unwrap_or_default();
        d.remove::<Vec<CanvasSlot>>(slot_id());
        slots
    })
}

/// 段 2 / AI-COMPOSE-1101: この frame の card を 1 つ置く（chat の `Run` の card・renderer の `canvas`）。
pub fn push_canvas(ctx: &egui::Context, slot: CanvasSlot) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<CanvasSlot>>(slot_id()).push(slot));
}

struct Draw {
    t: theme::Tokens,
    base: egui::Id,
    n: usize,
    can_check: bool,
    hit: Option<usize>,
}

impl Draw {
    fn id(&mut self) -> egui::Id {
        self.n += 1;
        self.base.with(self.n)
    }

    fn blocks(&mut self, ui: &mut egui::Ui, blocks: &[Block], ink: Color32) {
        for (i, b) in blocks.iter().enumerate() {
            if i > 0 {
                ui.add_space(if matches!(b, Block::Heading(..)) { theme::SP_3 } else { theme::SP_2 });
            }
            self.block(ui, b, ink);
        }
    }

    fn block(&mut self, ui: &mut egui::Ui, b: &Block, ink: Color32) {
        let t = self.t;
        match b {
            Block::Para(spans) => self.line(ui, spans, Type::Body, ink, true),
            Block::Heading(ty, spans) => self.line(ui, spans, *ty, ink, true),
            Block::Code { lang, text } if lang.eq_ignore_ascii_case("html") && canvas_host(ui.ctx()) => self.canvas(ui, text),
            Block::Code { lang, text } => self.code(ui, lang, text),
            Block::Quote(body) => {
                let r = ui.horizontal_top(|ui| {
                    ui.add_space(theme::SP_3);
                    ui.vertical(|ui| self.blocks(ui, body, t.text_secondary));
                });
                let rect = r.response.rect;
                ui.painter().vline(rect.left() + 1.0, rect.y_range(), Stroke::new(2.0, t.border_strong));
            }
            Block::List { start, items } => {
                for (k, item) in items.iter().enumerate() {
                    if k > 0 {
                        ui.add_space(theme::SP_1);
                    }
                    ui.horizontal_top(|ui| {
                        match item.check {
                            Some((on, n)) => {
                                let mut v = on;
                                let r = ui.add_enabled(self.can_check && n.is_some(), egui::Checkbox::without_text(&mut v));
                                let r = theme::ax_name(r, &plain(&item.body));
                                if r.clicked() {
                                    self.hit = n;
                                }
                            }
                            None => {
                                let mark = start.map_or_else(|| "•".to_owned(), |s| format!("{}.", s + k as u64));
                                theme::label(ui, Type::Body, mark, t.text_secondary);
                            }
                        }
                        ui.vertical(|ui| self.blocks(ui, &item.body, ink));
                    });
                }
            }
            Block::Table { head, rows } => self.table(ui, head, rows, ink),
            Block::Rule => theme::rule(ui),
            Block::Fold { summary, open, body } => {
                let id = self.id();
                let title = if summary.is_empty() { "Details" } else { summary.as_str() };
                egui::CollapsingHeader::new(Type::BodyStrong.text(title).color(ink)).id_salt(id).default_open(*open).show(ui, |ui| self.blocks(ui, body, ink));
            }
        }
    }

    /// 1 段落（・見出し・表の 1 枡）。link の字の上は手の形と行き先の hover、押すと http / https / mailto だけ本人の browser で開く。
    fn line(&mut self, ui: &mut egui::Ui, spans: &[Span], base: Type, ink: Color32, wrap: bool) {
        let (job, links) = job(spans, base, ink, &self.t);
        let label = egui::Label::new(job);
        let (pos, galley, r) = theme::chat_label(ui, if wrap { label.wrap() } else { label.extend() });
        let Some(p) = r.hover_pos().filter(|_| !links.is_empty()) else { return };
        let rel = p - pos;
        // 字の行の上だけ（行の右の空きは link にしない）
        if !galley.rows.iter().any(|row| row.rect.contains(rel.to_pos2())) {
            return;
        }
        let c = galley.cursor_from_pos(rel).ccursor.index;
        let at = if rel.x < galley.pos_from_ccursor(CCursor::new(c)).min.x { c.saturating_sub(1) } else { c };
        let Some((_, url)) = links.iter().find(|(range, _)| range.contains(&at)) else { return };
        let web = ["http://", "https://", "mailto:"].iter().any(|s| url.to_ascii_lowercase().starts_with(s));
        if web {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if r.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
        r.on_hover_text_at_pointer(if web { url.clone() } else { format!("{url} · only web and mail links open") });
    }

    fn code(&mut self, ui: &mut egui::Ui, lang: &str, text: &str) {
        let t = self.t;
        let id = self.id();
        egui::Frame::new().fill(t.surface_2).corner_radius(CornerRadius::same(theme::RADIUS_SM)).inner_margin(Margin::same(theme::SP_2 as i8)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::label(ui, Type::Small, if lang.is_empty() { "Code" } else { lang }, t.text_tertiary);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let now = ui.input(|i| i.time);
                    let copied = ui.ctx().data(|d| d.get_temp::<f64>(id)).is_some_and(|at| now - at < 1.5);
                    if theme::button(ui, if copied { "Copied" } else { "Copy" }, Variant::Ghost).clicked() {
                        ui.ctx().copy_text(text.trim_end_matches('\n').to_owned());
                        ui.ctx().data_mut(|d| d.insert_temp(id, now));
                    }
                    if copied {
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                    }
                });
            });
            egui::ScrollArea::horizontal().id_salt(id).auto_shrink([false, true]).show(ui, |ui| {
                theme::chat_label(ui, egui::Label::new(Type::Mono.text(text.trim_end_matches('\n')).color(t.text_primary)).extend());
            });
        });
    }

    /// 段 2: ```html の card = `Interactive view` と `Show code` / `Run`（押すと `Stop`）・固定の高さの枠。`Run` の 1 つ（同時に 1 つ）は、枠が全部見えている
    /// frame だけ `CanvasSlot` を置く（WebView は egui で切れない・scroll で一部が隠れたら host が隠す）。それ以外の間は枠の真ん中に 1 行。`Show code` で中身の code block。
    fn canvas(&mut self, ui: &mut egui::Ui, text: &str) {
        let t = self.t;
        let id = self.id();
        let key = id.with(text).value();
        let ctx = ui.ctx().clone();
        let live = live_canvas(&ctx) == Some(key);
        let code_id = id.with("code");
        let mut code = ctx.data(|d| d.get_temp::<bool>(code_id)).unwrap_or(false);
        egui::Frame::new().stroke(theme::hairline(&t)).corner_radius(CornerRadius::same(theme::RADIUS_SM)).inner_margin(Margin::same(theme::SP_2 as i8)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::label(ui, Type::Small, "Interactive view", t.text_tertiary);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::button(ui, if live { "Stop" } else { "Run" }, Variant::Ghost).clicked() {
                        set_live_canvas(&ctx, (!live).then_some(key));
                    }
                    if theme::button(ui, if code { "Hide code" } else { "Show code" }, Variant::Ghost).clicked() {
                        code = !code;
                    }
                });
            });
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), CANVAS_H), egui::Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), t.surface_1);
            if live && ui.clip_rect().contains_rect(rect) {
                push_canvas(&ctx, CanvasSlot { key, html: text.to_owned(), rect, part: None });
            } else {
                let line = if live { "Scroll until all of it shows to use it" } else { "Press “Run” to try it here · it can't reach the internet or your files" };
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, line, Type::Small.font(), t.text_secondary);
            }
        });
        ctx.data_mut(|d| d.insert_temp(code_id, code));
        if code {
            ui.add_space(theme::SP_1);
            self.code(ui, "html", text);
        }
    }

    /// 罫線の表（hairline の枠・縞・見出しの行は太字）。幅が足りなければ横に scroll（枡の中は折り返さない）。
    /// ponytail: 列の右寄せ（`:--:` `--:`）は見ない（egui の Grid は枡の幅を前の frame で決めるので寄せが 1 frame ずれる）。数の列が読みにくければ寄せる
    fn table(&mut self, ui: &mut egui::Ui, head: &[Vec<Span>], rows: &[Vec<Vec<Span>>], ink: Color32) {
        let t = self.t;
        let id = self.id();
        egui::Frame::new().stroke(theme::hairline(&t)).corner_radius(CornerRadius::same(theme::RADIUS_SM)).inner_margin(Margin::same(theme::SP_2 as i8)).show(ui, |ui| {
            egui::ScrollArea::horizontal().id_salt(id).auto_shrink([true, true]).show(ui, |ui| {
                egui::Grid::new(id.with("grid")).striped(true).spacing([theme::SP_4, theme::SP_1]).show(ui, |ui| {
                    if !head.is_empty() {
                        for cell in head {
                            self.line(ui, cell, Type::BodyStrong, ink, false);
                        }
                        ui.end_row();
                    }
                    for row in rows {
                        for cell in row {
                            self.line(ui, cell, Type::Body, ink, false);
                        }
                        ui.end_row();
                    }
                });
            });
        });
    }
}

/// 項目の字（チェックの AX の名前）。
fn plain(body: &[Block]) -> String {
    body.iter()
        .find_map(|b| match b {
            Block::Para(spans) => Some(spans.iter().map(|s| s.text.as_str()).collect::<String>()),
            _ => None,
        })
        .unwrap_or_default()
}

/// 字の並び → 1 つの LayoutJob と link の字の範囲（char の位置・galley の cursor と同じ数え方）。
fn job(spans: &[Span], base: Type, ink: Color32, t: &theme::Tokens) -> (LayoutJob, Vec<(Range<usize>, String)>) {
    let mut job = LayoutJob::default();
    let mut links = Vec::new();
    let mut at = 0;
    let line = Stroke::new(1.0, ink);
    for s in spans {
        let font_id = if s.style.code {
            Type::Mono.font()
        } else if s.style.strong && base == Type::Body {
            FontId::new(base.size(), FontFamily::Name(theme::SEMIBOLD.into()))
        } else {
            base.font()
        };
        let n = s.text.chars().count();
        if let Some(u) = &s.link {
            links.push((at..at + n, u.clone()));
        }
        at += n;
        let format = TextFormat {
            font_id,
            line_height: Some(base.line_height()),
            color: ink,
            background: if s.style.code { t.surface_2 } else { Color32::TRANSPARENT },
            italics: s.style.em,
            underline: if s.link.is_some() { line } else { Stroke::NONE },
            strikethrough: if s.style.strike { line } else { Stroke::NONE },
            ..Default::default()
        };
        job.append(&s.text, 0.0, format);
    }
    (job, links)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 段 1 の形が Block になる・チェックの番号が `toggle` と同じ・HTML は code block のまま・<details> は畳む。
    #[test]
    fn markdown_blocks_and_checks() {
        let text = "## Plan\n\nSome **bold** and [a link](https://example.com).\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n- [ ] one\n- [x] two\n  - nested\n\n1. first\n2. second\n\n```rust\nfn main() {}\n```\n\n> quoted\n\n<div>raw</div>\n\n<details>\n<summary>Log</summary>\n\nlong log\n\n</details>\n";
        let b = parse(text, Some(0));
        assert!(matches!(&b[0], Block::Heading(Type::Group, s) if s[0].text == "Plan"), "{b:?}");
        assert!(matches!(&b[1], Block::Para(s) if s.iter().any(|x| x.style.strong) && s.iter().any(|x| x.link.as_deref() == Some("https://example.com"))), "{b:?}");
        assert!(matches!(&b[2], Block::Table { head, rows } if head.len() == 2 && rows.len() == 1), "{b:?}");
        let Block::List { start: None, items } = &b[3] else { panic!("{b:?}") };
        assert_eq!(items.iter().map(|i| i.check).collect::<Vec<_>>(), vec![Some((false, Some(0))), Some((true, Some(1)))]);
        assert!(matches!(items[1].body.last(), Some(Block::List { .. })), "入れ子の箇条書き: {items:?}");
        assert!(matches!(&b[4], Block::List { start: Some(1), items } if items.len() == 2), "{b:?}");
        assert!(matches!(&b[5], Block::Code { lang, text } if lang == "rust" && text.contains("fn main")), "{b:?}");
        assert!(matches!(&b[6], Block::Quote(q) if q.len() == 1), "{b:?}");
        assert!(matches!(&b[7], Block::Code { lang, text } if lang == RAW_HTML && text.contains("<div>")), "HTML を黙って消した: {b:?}");
        assert!(matches!(&b[8], Block::Fold { summary, open: false, body } if summary == "Log" && body.len() == 1), "{b:?}");
        assert_eq!(b.len(), 9, "{b:?}");
        let ticked = toggle(text, 0).expect("1 つ目のチェック");
        assert!(ticked.contains("- [x] one") && ticked.contains("- [x] two"), "{ticked}");
        assert!(toggle(&ticked, 1).is_some_and(|s| s.contains("- [ ] two")));
        assert_eq!(toggle(text, 2), None);
    }

    /// 段 2: ```html は WebView の入った build（host の印）だけ `Interactive view` の card・押す前は WebView の枠を置かない・`Run` で置く・
    /// 印が無い build と文の中の生の HTML は code block のまま（黙って消さない）。
    /// 負の対照（commit message・未実行）: `canvas` の `contains_rect` の条件を外す → scroll の外の card でも置いて 4 つ目で赤・block の腕の `canvas_host` を外す → 1 つ目で赤。
    #[test]
    fn chat_free_html_is_a_card_only_with_a_host() {
        let text = "```html\n<svg width=\"10\" height=\"10\"></svg>\n```\n\n<div>raw</div>\n";
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let run = |ctx: &egui::Context, host: bool, height: f32| -> Vec<String> {
            set_canvas_host(ctx, host);
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, height))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| show(ui, "t", text, Color32::WHITE, false));
                });
            });
            out.platform_output.accesskit_update.map(|u| u.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect()).unwrap_or_default()
        };
        let names = run(&ctx, false, 800.0);
        assert!(!names.iter().any(|l| l == "Interactive view") && names.iter().any(|l| l == "html"), "host の無い build で ```html を code block にしない: {names:?}");
        let names = run(&ctx, true, 800.0);
        assert!(names.iter().any(|l| l == "Interactive view") && names.iter().any(|l| l == "Run") && names.iter().any(|l| l == RAW_HTML), "card か生の HTML の code block が無い: {names:?}");
        assert!(take_canvases(&ctx).is_empty(), "押す前に WebView の枠を置いた");
        // Run（= 動かす key を立てる）: 枠が全部見えていれば置く・低い窓（枠の一部が scroll の外）では置かない
        let blocks = parse(text, Some(0));
        let Block::Code { text: html, .. } = &blocks[0] else { panic!("```html が code の block でない: {blocks:?}") };
        let key = egui::Id::new(("kangae.markdown", "t")).with(1usize).with(html.as_str()).value();
        set_live_canvas(&ctx, Some(key));
        run(&ctx, true, 800.0);
        assert!(matches!(&take_canvases(&ctx)[..], [s] if s.key == key && s.html.contains("<svg") && s.part.is_none()), "Run の card が枠を置かない");
        run(&ctx, true, 120.0);
        assert!(take_canvases(&ctx).is_empty(), "枠の一部が見えないのに WebView の枠を置いた");
    }
}
