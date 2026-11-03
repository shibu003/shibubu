//! Home（§6.1）: 1 つの欄（主領域の縦の中央・G83）・作れなかった 1 行・`Try` と例 3 つの chip（Project と session の一覧は左の sidebar・SESSIONS-LEFT-1101）。
//! KPI も card も置かない（§0-6 / §3）。

use crate::domain::icon::Icon;
use super::build_progress::{self, Build};
use super::header;
use super::shell::{AppRow, Intent, PageIntent, SettingsSection, Shell, Tier, View};
use super::skeleton;
use super::states::{self, RegionState};
use super::theme::{self, Type, Variant};
use eframe::egui::{self, Rect, Vec2};
use std::time::Duration;

/// 欄の縦の中心 = 主領域の見えている高さのこの割合（owner 10-27「createするbarを画面の真ん中もしかしたら下ぐらいにおいて」）。
/// HOME-CHATBAR-1101 から縦の中心に置くのは card の文の場所（上の印と帯は上へ伸びる = 帯が出ても欄は動かない）。
pub const FIELD_AT: f32 = 0.52;
/// Home の card の幅の上限（`Tier::input_w` より狭くはしない・列〈gutter の内側〉より広くはしない）。
/// HOME-CHATBAR-1101: 以下の数は owner 11-01 15:4x の見本（3360×2100 = 1680×1050 pt）を画素で測った値（pt）: card の幅 688・角 16.5・上の帯 36・
/// 帯の下から添付の絵の上まで 89・下の行 40（中心は下端から 27）・送る丸 28・印の icon の高さ 22・印と card の間 20・card の左から印まで 10。
pub(super) const FIELD_W: f32 = 688.0;
const CARD_RADIUS: u8 = 16;
/// 知らせる事（`llm` の 1 行と次の一手）が在る時だけ card の頭に出す帯。
const BAND_H: f32 = 36.0;
/// 文の場所の高さの上限（3 行と上下の余白）。溢れたら中で scroll（`header::command_field`）。HOME-CHATBAR-1101 追記 222（owner「chatbarにスペース余ってるから縮めて。詰めて」）:
/// 空の時は 1 行分（`TEXT_MIN_H`）で、打った行に合わせてこの高さまで伸びる（`text_h`）。
const TEXT_H: f32 = 88.0;
const TEXT_MIN_H: f32 = 44.0;
/// card の中の下の行（左 `+` と folder・右 Provider · Model · Effort と送る丸）と、その下の余白（追記 222 で 40 → 32・8 → 4）。
const ROW_H: f32 = 32.0;
const ROW_PAD: f32 = 4.0;
/// 帯と添えた物が無く文が空の時の chat bar の高さ（Project の Chat の tab が初めの frame に置く高さ・次の frame からは測った値 = `agent_rail::chat_column`）。
pub(super) const BAR_H: f32 = TEXT_MIN_H + ROW_H + ROW_PAD + 2.0 * theme::HAIRLINE;
/// 送る丸と `+` の丸の直径（owner 188「+もっと大きく」= `+` は送る丸と同じ当たり 32・数は仮で owner が画面で決める）。
const SEND_D: f32 = 32.0;
/// `+` の icon の大きさ（owner 188 の「+もっと大きく」= 字 ≈ 20pt・ICONS-1101 で手描きの十字から `Icon::Plus`）。
const PLUS_ICON: f32 = 20.0;
/// card の上の印（icon と「Shibubu」）: 字の大きさ・icon の高さ（owner 188「アイコンももっと大きく」= 前の 22 の約 2 倍・字はそのまま）・行の高さ・card との間・card の左からの inset。
const BRAND_SIZE: f32 = 22.0;
const BRAND_ICON: f32 = 36.0;
const BRAND_ROW_H: f32 = 40.0;
const BRAND_GAP: f32 = 18.0;
const BRAND_INSET: f32 = 10.0;
/// 印の絵（LOOK-1101 の透明の版・icon は 2 つだけ = 黒の背景は .app の icns）。
const BRAND_PNG: &[u8] = include_bytes!("../../assets/icon/shibubu-icon-transparent-1024.png");

/// `Try` の chip（label, 一言）。契約（RQ2 から・RQ-009・型と中身は変えない）: chip は今のまま `Generate(一言)`（`EXAMPLES[0]` は主デモの一言・デモの 27 手と app.rs の試験が使う）。
/// RQ-009 の empty state の `Start with a sample` は `Intent::TrySample(index)` = model を呼ばず名前の表で作り、外の app と file に書かない例だけを出す
/// （今は 1 = `Inventory tracker`・2 = `Research notes`。0 は TextEdit と Excalidraw を使うので sample にしない）。作った Project の行に `Sample`。
/// 保証しない: 例の中身が利用者の目的に合う事（初回の成功体験のための見本）。
/// Wave: RQ2
pub const EXAMPLES: [(&str, &str); 3] = [("Dev environment for a repo", "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw."), ("Inventory tracker", "Track my inventory"), ("Research notes", "Keep research notes")];

/// L2（109 通目 15・111 通目 = handoff 03 / 05 と prototype の conversation-layout）: 会話。人の一言 = 右寄せの吹き出し
/// （`you_bubble`・幅の 85% まで・添えた物の名前も中に）、Shibubu = 12px の名前の行と 13px の本文（囲わない・答えの印〈済み・途中・止めた・失敗〉は文の頭）、行の間 16・時刻は hover。
/// 最後の Shibubu の選択肢 = 押すとその文が次の一言（`reply_options`・28 高の枠のある角丸）。
/// 案の card = `agent_rail::Card::plan` → `draw_card`（CHAT-011・上下の hairline だけで枠で囲まない・中身と灰の決まりは `Card::plan` の doc）。
/// `wait` = 今の待ちの理由の 1 行（AI が案を読んでいる・作っている・CLI を確かめている・止めている = `Build now` を灰にしてその理由・CHAT-011 反証 Y6）。
/// 契約（owner 181 × CHAT-ONE-1101）: 会話の描き方はこの 1 つの関数。Home（`shell.talk`）と Project の Chat の tab（`Generated::chat` = 保存した列・`agent_rail::chat_column`）が呼ぶ。
/// owner 191（追記 5）→ SESSIONS-LEFT-1101: `New chat` は会話と一緒に流れない左の sidebar の一番上（`new_chat_button`）。
/// CHAT-FREE-1101（owner 198 の 6）: Shibubu の答えは AI が形を決めた Markdown（`ui::markdown::show`）。引数 `checks` = その中のチェック（`- [ ]`）を押せる
/// （Home の会話 = `Shell::talk` の位置で `Intent::CheckItem(turn, item)`・Project の会話は読むだけ = false）。
/// 例: `if let Some(i) = home::talk_thread(ui, &shell.talk, wait, true) { .. }`。
pub fn talk_thread(ui: &mut egui::Ui, turns: &[crate::domain::chat::Turn], wait: Option<&str>, checks: bool) -> Option<Intent> {
    use crate::domain::chat::{Mark, Who, TALK_WAIT};
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    let width = ui.available_width();
    // COPY-SCROLL-1101: ⌘A = 会話の全部・引いて枠の上下へ出たら流す（選びは scroll で切れない = `theme::chat_text`）。Home も Project の Chat の tab もここ
    theme::chat_keys(ui, || turns.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join("\n"));
    let selecting = theme::selecting_text(ui);
    theme::edge_scroll(ui, selecting);
    for (n, turn) in turns.iter().enumerate() {
        ui.add_space(theme::SP_4);
        let said = match turn.who {
            Who::You => ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| you_bubble(ui, &turn.text, &turn.attachments, width * 0.85)).inner,
            Who::Shibubu => {
                theme::label(ui, Type::Small, "Shibubu", t.text_tertiary);
                ui.add_space(theme::SP_1);
                let ink = if turn.text == TALK_WAIT { t.text_tertiary } else { t.text_primary };
                // G86: Project の答えの印（Home の会話は印なし = 今の 1 つの Label のまま）
                let state = match turn.mark {
                    Mark::None => None,
                    Mark::Verified => Some(RegionState::Verified),
                    Mark::Partial => Some(RegionState::Partial),
                    Mark::Stopped => Some(RegionState::Blocked),
                    Mark::Failed => Some(RegionState::Error),
                };
                // CHAT-FREE-1101: 答えの形は AI が決める（表・code・チェックリスト等）。チェックを押すと文の [ ] / [x] を替える（次の一言で AI も読む・押せるのは
                // `checks` = Home の会話だけ・Project の会話は読むだけ）
                let shown = match state {
                    None => ui.scope(|ui| super::markdown::show(ui, (&turn.at, n), &turn.text, ink, checks)),
                    Some(s) => ui.horizontal_top(|ui| {
                        states::inline(ui, s, "");
                        super::markdown::show(ui, (&turn.at, n), &turn.text, ink, checks)
                    }),
                };
                if let Some(item) = shown.inner {
                    intent = Some(Intent::CheckItem(n, item));
                }
                shown.response
            }
        };
        if !turn.at.is_empty() {
            said.on_hover_text(&turn.at);
        }
    }
    let last = turns.iter().rev().find(|turn| turn.who == Who::Shibubu);
    if let Some(last) = last.filter(|l| !l.choices.is_empty()) {
        ui.add_space(theme::SP_2);
        if let Some(choice) = reply_options(ui, &last.choices) {
            intent = Some(Intent::Talk(choice));
        }
    }
    // CHAT-011: 案の card は提案の card と同じ型（`agent_rail::Card`）で、描くのも同じ `draw_card`
    if let Some(card) = super::agent_rail::Card::plan(turns, wait) {
        ui.add_space(theme::SP_2);
        theme::rule(ui);
        if let Some(i) = super::agent_rail::draw_card(ui, &card) {
            intent = Some(i);
        }
    }
    intent
}

/// owner 181（「helloとうった内容と返信そして三択の内容に囲いがないから背景の白と同化してみずらい」）: 会話の部品の (面, 枠)。
/// 面 = `surface_3`（背景より一段濃い・比は Light 1.09 / Dark 1.47 で弱い）・枠 = `border_input`（token の中で Light でも Dark でも `bg_app` と 3:1 以上の唯一の線の色 = 見分けは枠が持つ）。
/// 呼ぶ側: `you_bubble`（面と枠）・`reply_options`（枠・hover は面）・`card`（枠）。
fn talk_colors(t: &theme::Tokens) -> (egui::Color32, egui::Color32) {
    (t.surface_3, t.border_input)
}

/// owner 191（HOME-CHATBAR 追記 5「newchatはボタンとして周りに白入れて」）: Home の会話の `New chat`。枠 1px の角丸（色 `text_primary` = Dark で白）・
/// 地は透明。SESSIONS-LEFT-1101 から左の sidebar の一番上に幅いっぱいで置く（どの View でも出ている・scroll しない・前は会話の間の header の右端）。
/// 押した = true（呼ぶ側が `Intent::NewChat`）。AX = Button `New chat`。
pub fn new_chat_button(ui: &mut egui::Ui) -> bool {
    let t = theme::tokens(ui.ctx());
    let label = Type::Body.text(crate::domain::chat::NEW_CHAT).color(t.text_primary);
    ui.add(egui::Button::new(label).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, t.text_primary)).corner_radius(egui::CornerRadius::same(theme::RADIUS_MD)).min_size(Vec2::new(ui.available_width(), theme::BUTTON_H)))
        .clicked()
}

/// owner 181: 打った文 = 右寄せは呼ぶ側・角丸の吹き出し（面と枠 = `talk_colors`・中は折り返す本文・添えた物の名前は下に 1 行）。
/// owner 190（HOME-CHATBAR 追記 4「改行はいいけどなぜ左揃えなの？」）: 吹き出しは右のまま、中の文字は左揃え（Messages・Claude と同じ）・幅は文の長さまで縮める（最大 `max_w`）。
/// 前は Label が呼ぶ側の右寄せの列の揃えを継ぎ、折り返した 2 行目が右に寄っていた。
/// 返す = 吹き出しの Response（呼ぶ側が時刻の hover を付ける）。例: `ui.with_layout(Layout::top_down(Align::Max), |ui| home::you_bubble(ui, text, &names, w * 0.85))`。
fn you_bubble(ui: &mut egui::Ui, text: &str, attachments: &[String], max_w: f32) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let (fill, edge) = talk_colors(&t);
    let body = Type::Body.text(text).color(t.text_primary);
    let names = (!attachments.is_empty()).then(|| Type::Helper.text(attachments.join(" · ")).color(t.text_secondary));
    // 中の幅 = 一番長い行（`max_w` で折り返した後）。その幅の左揃えの列に置く = 右寄せの枠の中で文は左から
    let w = [Some(&body), names.as_ref()]
        .into_iter()
        .flatten()
        .map(|r| egui::WidgetText::from(r.clone()).into_galley(ui, Some(egui::TextWrapMode::Wrap), max_w, egui::TextStyle::Body).size().x)
        .fold(0.0_f32, f32::max);
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(theme::HAIRLINE, edge))
        .corner_radius(egui::CornerRadius::same(theme::RADIUS_LG))
        .inner_margin(egui::Margin::symmetric(theme::SP_3 as i8, theme::SP_2 as i8))
        .show(ui, |ui| {
            ui.set_max_width((w.ceil() + 1.0).min(max_w));
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                theme::chat_text(ui, body);
                if let Some(names) = names {
                    ui.add(egui::Label::new(names).halign(egui::Align::Min).wrap());
                }
            });
        })
        .response
}

/// owner 181: 答えの選択肢 = 枠のある角丸の候補（押せると分かる形・休み透明・hover 面 `surface_3`・押している間 `surface_press`・枠 = `talk_colors`）。
/// AX は Button・名前は候補の文。押した候補の文を返す（呼ぶ側が次の一言 `Intent::Talk` にする）。CHAT-ONE-1101 からも呼べる形。
/// owner 181 がこの候補に枠を求めた（UI-SWEEP-1101 から押せる物は全部枠か面を持つ = ui-owner-check の d2_btn_framed）。
pub fn reply_options(ui: &mut egui::Ui, choices: &[String]) -> Option<String> {
    let t = theme::tokens(ui.ctx());
    let (_, edge) = talk_colors(&t);
    let radius = egui::CornerRadius::same(theme::RADIUS_LG);
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(theme::SP_2);
        let enabled = ui.is_enabled();
        // 長い候補は列の幅で折り返す（列より広い chip にしない）
        let wrap = (ui.available_width() - 2.0 * theme::SP_3).max(theme::SP_6);
        for choice in choices {
            let galley = ui.painter().layout(choice.clone(), Type::Body.font(), t.text_primary, wrap);
            let size = Vec2::new(galley.size().x + 2.0 * theme::SP_3, (galley.size().y + 2.0 * theme::SP_1).max(theme::BUTTON_H));
            let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, choice));
            if ui.is_rect_visible(rect) {
                let fill = if r.is_pointer_button_down_on() { t.surface_press } else if r.hovered() { t.surface_3 } else { egui::Color32::TRANSPARENT };
                ui.painter().rect(rect, radius, fill, egui::Stroke::new(theme::HAIRLINE, edge), egui::StrokeKind::Inside);
                ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.text_primary);
                theme::focus_ring(ui, &r, rect, radius);
            }
            if r.clicked() {
                picked = Some(choice.clone());
            }
        }
    });
    picked
}

/// `note` は入力欄の下に出す 1 行（作れなかった理由と次の一手・開けなかった app の数）。
/// 契約（UI-4a から・署名は `draw(ui, note, apps, current)` = `prompt` を外した）: Home は見出しも textarea も `Create` の button も描かない
/// （一言は 1 つの欄〈G83 から Home では Home が描く〉・placeholder `What do you want to create?`・欄の右端は `↵ Send`〈L2・111 通目〉= 作るのは会話の案の `Build now` だけ）。
/// 欄の下に `note`・例の行（頭に `Try` の label〈copy.en.md:72・G14 C2〉と chip 3 つ = `Generate(例)`）。Project の一覧は左の sidebar（SESSIONS-LEFT-1101）。
/// 契約（G83・`current` を `shell` に替え `busy`・`llm` を足した）: `shell` は 1 つの欄（`header::command_field`）に渡す今の View と欄の文。
/// `busy` = AI が読んでいる・CLI を確かめている 1 行（`model_busy_line` か `Checking for AI on this Mac`・Some の間は欄を灰）・`llm` = 直前に model の答えを
/// 使えなかった理由（D-e）。2 つは app.rs が Home の後に描いていた行で、Home が描く。
/// 契約（G83・owner 10-27 で決定 1 を Home だけ替えた）: 上から 1 つの欄・`note`・Try の chip・`busy` と `llm`（Continue は SESSIONS-LEFT-1101 で sidebar へ）。欄の縦の中心は主領域の見えている高さ
/// （`ui.clip_rect()` = draw_app_region の ScrollArea の viewport）の `FIELD_AT`（scroll が 0 の時。中身が長ければ主領域ごと scroll し、欄は header の下へ隠れる）・
/// 横は列の中央・幅 = `min(列, max(FIELD_W, Tier::input_w))`。header は Home で欄を描かない（窓に 1 つ）。
/// 契約（G83b）: 欄の下の中身（`note`・Try の行・`busy` と `llm`）は欄と同じ左端と幅（Home の中身全部が 1 本の中央の列）。
/// 契約（I30・owner 271）: 会話の間（会話か password の card が在る）は card と下の段を主領域の底に固定し、会話はその上の ScrollArea（stick_to_bottom・
/// 行が増えた frame は底へ）。主領域は送らない = 会話 1 通でも 10 通でも欄の下端は同じ所。空の Home は今まで通り `FIELD_AT`。
/// 例: `home::draw(ui, note, &rows, &mut self.shell, busy_line, self.generated.llm_line())`。保証しない: Home の欄が 4 行まで下へ伸びる事（SPEC §2・後の Wave）・`Choose folder…` の footer（UI-7）。
/// 契約（FX2・`busy: Option<&str>` を `build: Option<&Build>` に替えた = 生成の 1 行を文だけでなく段・待ちの理由・経過・`Stop`・終わった 1 行と次の一手で渡すため）:
/// `build` = `Generated::model_progress()`（無ければ CLI を確かめている間の `Build::Running { waiting: Some("Checking for AI on this Mac"), .. }`）。
/// 欄の下（`note` と Try の行の後・`llm` の前）に `build_progress::show` で描き、押した手は `Intent::Build(a)`。欄を灰にして送らないのは
/// `Running` と `Stopping` の間だけ（`Ended` の後は送れる = Try again も欄の Create も同じ道）。すぐ終わる処理では出さない: `Running` の経過が
/// 骨格の 120ms（`skeleton::SHOW_AFTER`）未満の間は行も骨格も描かず、120ms の時に frame を頼む（欄は灰のまま）。
/// 例（FX2 の本体）: `let b = self.generated.model_progress().or(checking); home::draw(ui, note, &rows, &mut self.shell, b.as_ref(), self.generated.llm_line())`
/// Wave: UI-4a, M2, FX2
/// 契約（RQ2 → SESSIONS-LEFT-1101・owner 198 の 1・5）: Home の `Continue`（All projects · {n}・Search · ⌘K・Name / Recent・行の `•••`・Archived）は消した。
/// Project と session は左の sidebar（`sidebar::draw` の Sessions と Your projects・検索の欄・見出しと行の hover で出る `⌄ ••• ＋`・行の右 click の menu・見出しの `•••` の `Show archived · {n}`）。
/// 壊れて隔離した data の `note`（`Generated::DAMAGED_NOTE` など `Settings › Data & Recovery` で終わる 1 行）の下に `Restore…`（`Intent::Go(View::Settings(SettingsSection::Data))`）。
/// Wave: UI-4a, M2, RQ2, RQ3
/// G99: `llm` は (1 行, 次の一手の button の名前)。button が在る時は隣に `Check again` も（`Intent::AiStep` / `Intent::RecheckAi`）。
/// 契約（HOME-CHATBAR-1101・署名は変えない・owner 11-01 15:4x の見本の形と 180 通目）: 欄は角丸の card（`card`）= 上から 帯（`llm` の 1 行と次の一手・在る時だけ）・
/// 文（3 行・Enter で送る・⇧Enter で改行）・添えた物（`attach_row`）・下の行（左 `+`〈Attach の menu〉・右 送る丸〈AX は今の右端の語 `Send`〉）。
/// card の上に印（Shibubu の icon と「Shibubu」・左寄せ）= owner の一言が会話に 1 つも無い間だけ（送ったら消え、`New chat` で空に戻ると出る）。
/// card の中の下の行に Provider · Model · Effort の今の値（owner 188・それぞれ押すとその場で dropdown・`choices`）。その下は今のまま（note・Try）。
pub fn draw(ui: &mut egui::Ui, note: Option<&str>, apps: &[AppRow], shell: &mut Shell, build: Option<&Build>, llm: Option<(&str, Option<&'static str>)>) -> Option<Intent> {
    let busy = matches!(build, Some(Build::Running { .. } | Build::Stopping { .. }));
    // CHAT-011 反証 Y6: 案の Build now の灰の理由はその時の待ち（CLI を確かめている = その 1 行・AI が読んでいる・止めている）
    let wait = match build {
        Some(Build::Running { waiting: Some(why), .. }) => Some(*why),
        Some(Build::Running { .. }) => Some(crate::domain::chat::WAIT_READING),
        Some(Build::Stopping { .. }) => Some(crate::domain::chat::WAIT_STOPPING),
        _ => None,
    };
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    // 欄の上の余白: 中身は draw_app_region の上余白 SP_7 の下から始まる（scroll に依らない式 = 中身と一緒に scroll する）。
    // G89: 見えている高さが無限（clip の無い Ui）・NaN なら 0 として読む（巨大な余白と rect を骨格へ渡さない）
    let view_h = Some(ui.clip_rect().height()).filter(|h| h.is_finite()).unwrap_or(0.0);
    // L2: 会話が在る間は prototype の conversation-layout（上に会話・その下に欄・Try は出さない = `New chat` で戻る）
    // PASSWORD-1101: password manager の card は会話の下（会話が空でも = 会話の形で描く）
    let vault = super::agent_rail::Card::password(shell);
    let talking = !shell.talk.is_empty() || vault.is_some();
    // HOME-CHATBAR-1101: card の上の印は owner の一言が会話に無い間だけ
    let brand = !shell.talk.iter().any(|turn| turn.who == crate::domain::chat::Who::You);
    if !talking {
        // card の文の中心を主領域の FIELD_AT に（その上の印・card の枠・帯の分を引く）
        let above = if brand { BRAND_ROW_H + BRAND_GAP } else { 0.0 } + theme::HAIRLINE + if llm.is_some() { BAND_H } else { 0.0 };
        ui.add_space((view_h * FIELD_AT - above - TEXT_MIN_H / 2.0 - theme::SP_7).max(0.0));
    }
    // owner 191（追記 5）: 会話の間は列の幅いっぱい（左右の余白は app.rs の `draw_region` の gutter だけ）・chat bar の card も同じ幅
    let w = if talking { ui.available_width() } else { ui.available_width().min(FIELD_W.max(Tier::current(ui.ctx()).input_w)) };
    let region = ui.available_rect_before_wrap();
    // I30（owner 271「chatbarが動かないように下に固定。今上にありすぎる」）: 会話の間は card と下の段（note・生成の行）を主領域の底に固定し、会話（と password の
    // card）はその上の ScrollArea（stick_to_bottom）= Project の Chat の列（`agent_rail::chat_column`）と同じ形。底の段の高さは前の pass で測った値（変わった pass は
    // 捨てて測り直す・egui の request_discard）。使う高さ = 見えている高さから draw_region の上下の余白（SP_7・SP_9）と列の後の 1 つの間を引いた物 = 主領域は送らない。
    let mut low = None;
    if talking {
        let low_id = egui::Id::new("kangae.home.low-h");
        let low_h = ui.ctx().data(|d| d.get_temp::<f32>(low_id)).unwrap_or(BAR_H);
        let top = ui.cursor().top();
        let bottom = top + (view_h - theme::SP_7 - theme::SP_9 - ui.spacing().item_spacing.y).max(low_h);
        let (left, right) = (region.center().x - w / 2.0, region.center().x + w / 2.0);
        let column = Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, (bottom - low_h - theme::SP_4).max(top)));
        let mut talk = ui.new_child(egui::UiBuilder::new().max_rect(column).layout(egui::Layout::top_down(egui::Align::Min)));
        egui::ScrollArea::vertical().id_salt("kangae-home-talk").max_height(column.height()).auto_shrink([false, false]).stick_to_bottom(true).show(&mut talk, |ui| {
            ui.set_width(w);
            if let Some(i) = talk_thread(ui, &shell.talk, wait, true) {
                intent = Some(i);
            }
            if let Some(i) = vault.as_ref().and_then(|c| super::agent_rail::draw_card(ui, c)) {
                intent = Some(i);
            }
            // L2: 会話に行が増えた frame だけ底まで送る（上へ戻して読んでいる間に送った時も新しい行を出す・打っている間と読んでいる間は動かさない）
            let seen = egui::Id::new("kangae.home.talk.len");
            if ui.ctx().data(|d| d.get_temp::<usize>(seen)) != Some(shell.talk.len()) {
                ui.ctx().data_mut(|d| d.insert_temp(seen, shell.talk.len()));
                ui.scroll_to_cursor(Some(egui::Align::Max));
            }
        });
        ui.advance_cursor_after_rect(Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom)));
        let at = Rect::from_min_max(egui::pos2(region.left(), bottom - low_h), egui::pos2(region.right(), bottom));
        low = Some((ui.new_child(egui::UiBuilder::new().max_rect(at).layout(egui::Layout::top_down(egui::Align::Min))), low_id, low_h));
    }
    let ui = match low.as_mut() {
        Some((bottom, _, _)) => bottom,
        None => ui,
    };
    let mut field = Rect::NOTHING;
    ui.horizontal_top(|ui| {
        ui.add_space((region.center().x - w / 2.0 - ui.cursor().left()).max(0.0));
        ui.vertical(|ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 0.0;
            if brand {
                brand_row(ui);
                ui.add_space(BRAND_GAP);
            }
            let (i, rect) = chat_bar(ui, shell, apps, busy, llm, false);
            field = rect;
            if i.is_some() {
                intent = i;
            }
        });
    });
    // G83b: 欄の下の中身は欄と同じ左端と幅（Home の中身全部が 1 本の中央の列・owner の写真で Try は列の左端・Continue は列の全幅だった）
    ui.horizontal_top(|ui| {
        ui.add_space((field.left() - ui.cursor().left()).max(0.0));
        ui.vertical(|ui| {
            ui.set_width(w);
            ui.add_space(theme::SP_3);
            if let Some(note) = note {
                theme::label(ui, Type::Helper, note, t.danger);
                // RQ3: 壊れて隔離した data の 1 行（`Settings › Data & Recovery` で終わる note）は、その場から戻す所へ
                if note.ends_with("Settings › Data & Recovery") && theme::button_compact(ui, "Restore…", Variant::Ghost).clicked() {
                    intent = Some(Intent::Go(View::Settings(SettingsSection::Data)));
                }
                ui.add_space(theme::SP_3);
            }
            if !talking {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::SP_2;
                    theme::label(ui, Type::Helper, "Try", t.text_tertiary);
                    for (label, example) in EXAMPLES {
                        // L2（111 通目・prototype の data-suggest）: 例は欄に入れるだけ（作らない・送るのは利用者の Enter）
                        if theme::chip(ui, label).on_hover_text(example).clicked() {
                            (shell.command, shell.focus_command) = (example.to_owned(), true);
                        }
                    }
                });
            }
            // PV1: 迷いの一言の成果物の候補と道具の行（何も入れない・Try の行の後）
            if let Some(d) = &shell.discovery {
                ui.add_space(theme::SP_3);
                if let Some(mut i) = super::discover::draw(ui, d) {
                    // G104: `Start in <tool>` も Try の例と同じく添えた物を乗せる
                    if let Intent::Generate(_, a) = &mut i {
                        *a = std::mem::take(&mut shell.attach);
                    }
                    intent = Some(i);
                }
            }
            // FX2: 生成の段・待ちの理由・経過・Stop、止めている間、終わった 1 行と次の一手（build_progress の部品）。すぐ終わる処理（120ms 未満）は出さない
            if let Some(b) = build {
                match b {
                    Build::Running { elapsed, .. } if elapsed.as_secs_f64() < skeleton::SHOW_AFTER => {
                        ui.ctx().request_repaint_after(Duration::from_secs_f64(skeleton::SHOW_AFTER) - *elapsed);
                    }
                    _ => {
                        if let Some(a) = build_progress::show(ui, "home.build", b, |_| {}) {
                            intent = Some(Intent::Build(a));
                        }
                    }
                }
            }
        });
    });
    // I30: 底の段の高さが前の pass と違えば覚えて、この pass は捨てて描き直す（card が底から外れた形を見せない）
    if let Some((bottom, id, h)) = &low {
        let got = bottom.min_rect().height();
        if (got - h).abs() > 0.5 {
            bottom.ctx().data_mut(|d| d.insert_temp(*id, got));
            bottom.ctx().request_discard("home chat bar size");
            bottom.ctx().request_repaint();
        }
    }
    intent
}

/// G104 → HOME-ATTACH-1101（owner 180 通目「+を押すと…add file or photoというものを出せるように・file, link, videoの表示はいらない」）: 欄の左の `+` の menu は
/// `Add files or photos` の 1 行だけ（`Intent::Attach` = file・写真・zip・folder を 1 つの窓で）。Provider・Model・Effort は `+` から外した（chat の下の行 = HOME-CHATBAR-1101）。
/// 今の AI が何でも押せる（owner 185 通目「AI が何であっても Shibubu が受け取り chat bar に長方形として置く」）= 読めない AI・AI 無しは送る時に 1 行（`chat::recheck`）。
/// owner 188（「+もっと大きく」）: `+` は送る丸と同じ直径 `SEND_D` の丸（休み = 枠だけ・hover = 面 `surface_3`）に、`Icon::Plus`（`PLUS_ICON`）を描く。AX は `Attach` のまま。
fn attach_menu(ui: &mut egui::Ui, rect: Rect) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        let round = egui::CornerRadius::same((SEND_D / 2.0) as u8);
        let button = egui::Button::new("").min_size(Vec2::splat(SEND_D)).corner_radius(round).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::new(theme::HAIRLINE, talk_colors(&t).1));
        let menu = egui::menu::menu_custom_button(ui, button, |ui| {
            if ui.button(ADD_FILES).clicked() {
                intent = Some(Intent::Attach);
                ui.close_menu();
            }
        });
        let at = menu.response.rect;
        let (c, p) = (at.center(), ui.painter());
        if menu.response.hovered() {
            p.circle_filled(c, SEND_D / 2.0 - theme::HAIRLINE, t.surface_3);
        }
        theme::paint_icon(p, Icon::Plus, c, PLUS_ICON, t.text_primary, 0.0);
        theme::ax_name(menu.response, "Attach").on_hover_text(ADD_FILES);
    });
    intent
}

/// HOME-ATTACH-1101: `+` の menu の 1 行（見本 ref-1700 の 1 行目と同じ語）。
const ADD_FILES: &str = "Add files or photos";
/// HOME-ATTACH-1101: 添付の長方形の高さ（見本 ref-chatbar-crop.png の欄の下の角丸の絵 ≈ 36pt）。写真は同じ辺の正方形の縮小絵。
const TILE: f32 = 36.0;
/// 写真でない長方形の名前の幅の上限（長い名前は … で切り、全文は hover）。
const TILE_TEXT_W: f32 = 140.0;
/// 長方形の右上の × の押せる四方（見た目の丸は 2pt 内側）。
const TILE_X: f32 = 18.0;
/// 貼った文の読むだけの中身の高さの上限（それより長い文は中で scroll）。
const PEEK_H: f32 = 240.0;

/// G104 → HOME-ATTACH-1101（owner 175 通目「upload or d&dしたfileや300行以上のpateした文章やfileはchatを打つしたに長方形として小さくまとまるように」）:
/// 添えた物は欄の下に小さな角丸の長方形で横に並ぶ（溢れたら横に scroll）。写真 = `TILE` 四方の縮小絵（読めるまで・読めない形は種類の印）・
/// file（folder・zip も）= 印と名前（`TILE_TEXT_W` で省略）と大きさ・貼った文 = 印と `Pasted text` と行の数。各長方形の右上の × で外す（AX `Remove {名前}`）。
/// 長方形の AX の名前は種類と名前（`Photo shot.png`・`File notes.md`・`Pasted text · 412 lines`）。貼った文は押すと下に読むだけの中身が開く（もう 1 回で閉じる・AX expanded）。
/// 添えられなかった 1 行は今のまま。外した後は欄に focus を戻す（H04）。
fn attach_row(ui: &mut egui::Ui, shell: &mut Shell) {
    use crate::domain::chat::Attachment;
    let t = theme::tokens(ui.ctx());
    if !shell.attach.is_empty() {
        let peek_id = egui::Id::new("kangae.attach.peek");
        // 開いている貼った文（並びが変わって貼った文でなくなった index は閉じた扱い）
        let peek = ui.data(|d| d.get_temp::<usize>(peek_id)).filter(|&i| matches!(shell.attach.get(i), Some(Attachment::Pasted { .. })));
        let (mut remove, mut toggle) = (None, None);
        // 上下の余白は focus の輪（長方形の外 2pt）が scroll の枠で切れない分
        egui::ScrollArea::horizontal().id_salt("kangae.attach.row").show(ui, |ui| {
            ui.add_space(theme::SP_1);
            ui.horizontal(|ui| {
                ui.add_space(theme::SP_1);
                ui.spacing_mut().item_spacing.x = theme::SP_2;
                for (i, a) in shell.attach.iter().enumerate() {
                    let (open, gone) = attach_tile(ui, a, peek == Some(i));
                    if open {
                        toggle = Some(i);
                    }
                    if gone {
                        remove = Some(i);
                    }
                }
            });
            ui.add_space(theme::SP_1);
        });
        if let Some(i) = toggle {
            ui.data_mut(|d| if peek == Some(i) { d.remove::<usize>(peek_id) } else { d.insert_temp(peek_id, i) });
        }
        if let Some(Attachment::Pasted { text, lines }) = peek.filter(|_| toggle.is_none()).and_then(|i| shell.attach.get(i)) {
            ui.add_space(theme::SP_1);
            egui::Frame::new().fill(t.surface_2).corner_radius(egui::CornerRadius::same(theme::RADIUS_MD)).inner_margin(egui::Margin::same(theme::SP_2 as i8)).show(ui, |ui| {
                // 行を見える分だけ描く（大きな貼り付けでも毎 frame 全部を組まない）
                egui::ScrollArea::both().id_salt("kangae.attach.peek.text").max_height(PEEK_H).auto_shrink([false, true]).show_rows(ui, Type::Mono.line_height(), *lines, |ui, rows| {
                    for line in text.lines().skip(rows.start).take(rows.len()) {
                        ui.add(egui::Label::new(Type::Mono.text(line).color(t.text_secondary)).extend());
                    }
                });
            });
        }
        if let Some(i) = remove {
            shell.attach.remove(i);
            ui.data_mut(|d| d.remove::<usize>(peek_id));
            (shell.attach_line, shell.focus_command) = (None, true);
        }
        ui.add_space(theme::SP_2);
    }
    if let Some(line) = &shell.attach_line {
        theme::label(ui, Type::Helper, line, t.danger);
        ui.add_space(theme::SP_2);
    }
}

/// HOME-ATTACH-1101: 添付の長方形 1 つ（`attach_row` の中身）。返す = (貼った文を押した, × を押した)。`open` = 貼った文の中身が開いている。
fn attach_tile(ui: &mut egui::Ui, a: &crate::domain::chat::Attachment, open: bool) -> (bool, bool) {
    use crate::domain::chat::{AttachKind, Attachment};
    let t = theme::tokens(ui.ctx());
    let name = a.name();
    let (kind, size) = match a {
        Attachment::File { kind: AttachKind::Photo, bytes, .. } => ("Photo", crate::domain::size_line(*bytes)),
        Attachment::File { bytes, .. } => ("File", crate::domain::size_line(*bytes)),
        Attachment::Pasted { lines, .. } => ("Pasted text", format!("{lines} lines")),
    };
    let photo = match a {
        Attachment::File { path, kind: AttachKind::Photo, .. } => Some(thumb(ui.ctx(), path)),
        _ => None,
    };
    // 写真でない長方形: [印 | 名前 / 大きさ | × の席]
    let (title, sub) = match a {
        Attachment::Pasted { .. } => (kind.to_owned(), size.clone()),
        _ => (name.clone(), size.clone()),
    };
    let title = theme::truncated(ui, &title, Type::Small.font(), t.text_primary, TILE_TEXT_W);
    let sub = theme::truncated(ui, &sub, Type::Small.font(), t.text_tertiary, TILE_TEXT_W);
    let w = if photo.is_some() { TILE } else { theme::SP_2 + 16.0 + theme::SP_2 + title.size().x.max(sub.size().x) + TILE_X + theme::SP_1 };
    let pasted = matches!(a, Attachment::Pasted { .. });
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, TILE), if pasted { egui::Sense::click() } else { egui::Sense::hover() });
    let ax = if pasted { name.clone() } else { format!("{kind} {name}") };
    r.widget_info(|| egui::WidgetInfo::labeled(if pasted { egui::WidgetType::Button } else { egui::WidgetType::Label }, true, &ax));
    if pasted {
        r.ctx.accesskit_node_builder(r.id, |n| n.set_expanded(open));
    }
    let r = r.on_hover_text(format!("{name} · {size}"));
    let radius = egui::CornerRadius::same(theme::RADIUS_MD);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        match &photo {
            Some(Some(tex)) => {
                p.add(egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE).with_texture(tex.id(), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))));
            }
            _ => {
                let fill = if r.hovered() || open { t.surface_3 } else { t.surface_2 };
                p.rect_filled(rect, radius, fill);
                let mark = match a {
                    Attachment::File { kind: AttachKind::Photo, .. } => Icon::Image,
                    _ => Icon::File,
                };
                let mark_at = if photo.is_some() { rect.center() } else { egui::pos2(rect.left() + theme::SP_2 + 8.0, rect.center().y) };
                theme::paint_icon(p, mark, mark_at, theme::ICON_UI, t.text_secondary, 0.0);
                if photo.is_none() {
                    let x = rect.left() + theme::SP_2 + 16.0 + theme::SP_2;
                    let top = rect.center().y - (title.size().y + sub.size().y) / 2.0;
                    let title_h = title.size().y;
                    p.galley(egui::pos2(x, top), title, t.text_primary);
                    p.galley(egui::pos2(x, top + title_h), sub, t.text_tertiary);
                }
            }
        }
        p.rect_stroke(rect, radius, egui::Stroke::new(theme::HAIRLINE, t.border), egui::StrokeKind::Inside);
        theme::focus_ring(ui, &r, rect, radius);
    }
    // × は長方形の右上の内側（写真は絵の上に重ねる・scroll の枠で切れない）
    let x_rect = Rect::from_min_size(egui::pos2(rect.right() - TILE_X, rect.top()), Vec2::splat(TILE_X));
    let tip = format!("Remove {name}");
    let x = ui.interact(x_rect, r.id.with("remove"), egui::Sense::click());
    x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &tip));
    if ui.is_rect_visible(x_rect) {
        let fill = if x.hovered() { t.text_primary } else { t.surface_3 };
        let ink = if x.hovered() { t.bg_app } else { t.text_primary };
        ui.painter().circle_filled(x_rect.center(), TILE_X / 2.0 - 2.0, fill);
        theme::paint_icon(ui.painter(), Icon::X, x_rect.center(), 12.0, ink, 0.0);
        theme::focus_ring(ui, &x, x_rect, egui::CornerRadius::same((TILE_X / 2.0) as u8));
    }
    let gone = x.on_hover_text(&tip).clicked();
    (pasted && r.clicked() && !gone, gone)
}

/// HOME-ATTACH-1101: 写真の縮小絵（`TILE` の 2 倍の px 四方・中央で切り抜く）。初めて描く frame に裏の thread で読み（UI の thread で decode しない）、
/// 読めるまで・読めなかった時（GIF・WebP = decoder を入れていない・壊れた file）は None = 種類の印で描く。読んだ結果は path ごとに 1 回だけ覚える（毎 frame 読み直さない）。
fn thumb(ctx: &egui::Context, path: &std::path::Path) -> Option<egui::TextureHandle> {
    let p = path.to_path_buf();
    theme::texture_once(ctx, &format!("attach-thumb {}", path.display()), move || decode_thumb(&p))
}

/// HOME-ATTACH-1101: 写真を `TILE` の 2 倍の px 四方に縮めて中央で切り抜く（PNG・JPEG。他の形・壊れた file は None）。
fn decode_thumb(path: &std::path::Path) -> Option<egui::ColorImage> {
    let px = (TILE * 2.0) as u32;
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = img.resize_to_fill(px, px, image::imageops::FilterType::Triangle).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// HOME-CHATBAR-1101 × CHAT-ONE-1101（owner 184「chatbarは１つに統合して。そのchatbarをhome, app内で開けるという形」）: chat bar = この 1 つの部品。
/// Home（`draw`）と Project の Chat の tab（`agent_rail::chat_column`）が同じ物を呼ぶ。角丸の card（地 `surface_1` = 背景より一段明るい面・枠 = `talk_colors` の枠
/// 〈欄に focus の間 `focus_ring`〉・角 `CARD_RADIUS`）。上から 帯（`llm` が在る時だけ・`BAND_H`・地 `surface_3`・左に 1 行・右に次の一手と `Check again`）→
/// 文（`TEXT_H`・`header::command_field`）→ 添えた物（`attach_row`）→ 下の行（`ROW_H`・owner 188 = 左 `+`〈`attach_menu`〉と folder〈`folder_chip`〉・
/// 右 Provider · Model · Effort〈`choices`〉と送る丸〈`send_circle`〉）。Project で名前の欄（Rename）か ⌘K の list の間は、欄は header に在る（窓に 1 つ = d1_one_field）ので
/// 文の場所は何をする所かの 1 行。返す = 押した手と card の外形。例: `let (i, rect) = home::chat_bar(ui, &mut shell, &rows, busy, None, false);`
/// 追記 222（owner「右sidebarのchatbarの+ folder, provider, model が被ってるから2段にして…homeも下のchatbarも詰めて。ただしhome と下のchatbarは2段にしない」）:
/// `side` = 右の sidebar の中（下の行を 2 段 = 上 `+` と folder・下 Provider · Model · Effort と送る丸）。Home と下（false）は 1 行のまま・狭い時は Model の名前を … で縮める（`choices`）。
/// 3 か所とも詰めた: 文の場所は空なら 1 行分で行に合わせて伸びる（`text_h`）・下の行 `ROW_H` 32・`ROW_PAD` 4・左右の余白 1 段小さく。
pub fn chat_bar(ui: &mut egui::Ui, shell: &mut Shell, apps: &[AppRow], busy: bool, llm: Option<(&str, Option<&'static str>)>, side: bool) -> (Option<Intent>, Rect) {
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    let focused = ui.memory(|m| m.has_focus(egui::Id::new(header::FIELD_ID)));
    let frame = egui::Frame::new()
        .fill(t.surface_1)
        .stroke(egui::Stroke::new(theme::HAIRLINE, if focused { t.focus_ring } else { talk_colors(&t).1 }))
        .corner_radius(egui::CornerRadius::same(CARD_RADIUS));
    let inner_w = ui.available_width() - 2.0 * theme::HAIRLINE;
    let shown = frame.show(ui, |ui| {
        ui.set_width(inner_w);
        ui.spacing_mut().item_spacing.y = 0.0;
        if let Some((line, step)) = llm {
            let (_, band) = ui.allocate_space(Vec2::new(inner_w, BAND_H));
            let r = CARD_RADIUS.saturating_sub(theme::HAIRLINE as u8);
            ui.painter().rect_filled(band, egui::CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, t.surface_3);
            let mut row = ui.new_child(egui::UiBuilder::new().max_rect(band.shrink2(Vec2::new(theme::SP_3, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
            // 右から `Check again`・次の一手（見本の右の link の席）。左の 1 行は残りの幅で省略し、全文は hover
            if let Some(step) = step {
                if theme::button_compact(&mut row, "Check again", Variant::Ghost).clicked() {
                    intent = Some(Intent::RecheckAi);
                }
                if theme::button_compact(&mut row, step, Variant::Primary).clicked() {
                    intent = Some(Intent::AiStep(step));
                }
            }
            row.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                states::inline(ui, RegionState::Partial, line);
            })
            .response
            .on_hover_text(line);
        }
        let text_at = text_h(ui, shell, inner_w);
        let (_, text) = ui.allocate_space(Vec2::new(inner_w, text_at));
        let elsewhere = matches!(shell.view, View::Project { .. }) && (shell.renaming.is_some() || shell.palette.open);
        if elsewhere {
            let line = if shell.palette.open { "Type in the field at the top" } else { "Type the new name at the top · Enter saves, Esc keeps the old one" };
            let mut at = ui.new_child(egui::UiBuilder::new().max_rect(text.shrink2(Vec2::new(theme::SP_4, theme::SP_3))).layout(egui::Layout::left_to_right(egui::Align::Min)));
            theme::label(&mut at, Type::Helper, line, t.text_secondary);
        } else if let Some(i) = header::command_field(ui, text, shell, apps, busy) {
            intent = Some(i);
        }
        // HOME-ATTACH の持ち場（中身は変えない）: 文の下・下の行の上
        egui::Frame::new().inner_margin(egui::Margin::symmetric(theme::SP_2 as i8, 0)).show(ui, |ui| {
            ui.set_width(inner_w - 2.0 * theme::SP_2);
            attach_row(ui, shell);
        });
        // 追記 222: 右の sidebar だけ 2 段（`low` = 下の段）・Home と下は 1 行（`low` = `row`）
        let (_, row) = ui.allocate_space(Vec2::new(inner_w, ROW_H));
        let low = if side { ui.allocate_space(Vec2::new(inner_w, ROW_H)).1 } else { row };
        ui.add_space(ROW_PAD);
        let (plus, _) = bottom_row(row);
        let (_, send) = bottom_row(low);
        if let Some(i) = attach_menu(ui, plus) {
            intent = Some(i);
        }
        // owner 188（ref-1700 の並び）: `+` の右に folder・送る丸のすぐ左に右寄せで Provider · Model · Effort（1 行の時は folder の右の残り = 重ならない）
        let at = Rect::from_min_max(egui::pos2(plus.right() + theme::SP_1, row.top()), egui::pos2(if side { row.right() - theme::SP_2 } else { send.left() - theme::SP_2 }, row.bottom()));
        let mut folder_end = at.left();
        if at.width() > 0.0 {
            let mut mid = ui.new_child(egui::UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)));
            mid.set_clip_rect(at.intersect(ui.clip_rect()));
            if folder_chip(&mut mid, shell.folder.as_deref()) {
                intent = Some(Intent::Page(PageIntent::ChooseFolder));
            }
            folder_end = mid.min_rect().right();
        }
        let pick = Rect::from_min_max(egui::pos2(if side { low.left() + theme::SP_2 } else { folder_end + theme::SP_2 }, low.top()), egui::pos2(send.left() - theme::SP_2, low.bottom()));
        if pick.width() > 0.0 {
            let mut mid = ui.new_child(egui::UiBuilder::new().max_rect(pick).layout(egui::Layout::left_to_right(egui::Align::Center)));
            mid.set_clip_rect(pick.intersect(ui.clip_rect()));
            if let Some(i) = choices(&mut mid, shell) {
                intent = Some(i);
            }
        }
        if let Some(i) = send_circle(ui, send, shell, apps, busy) {
            intent = Some(i);
        }
    });
    (intent, shown.response.rect)
}

/// 文の場所の高さ（追記 222）: 今の文（空なら placeholder）を欄の幅 `w`（左右の余白 `SP_3` の内側）で折り返した高さ＋上下の余白を `TEXT_MIN_H..=TEXT_H` に丸める。
/// 例: `ui.allocate_space(Vec2::new(w, text_h(ui, shell, w)))`
fn text_h(ui: &egui::Ui, shell: &Shell, w: f32) -> f32 {
    let text = if shell.command.is_empty() { shell.field_hint() } else { shell.command.as_str() };
    let g = ui.fonts(|f| f.layout(text.to_owned(), Type::Nav.font(), egui::Color32::PLACEHOLDER, (w - 2.0 * theme::SP_3).max(1.0)));
    (g.size().y + theme::SP_3 + theme::SP_2).clamp(TEXT_MIN_H, TEXT_H)
}

/// HOME-CHATBAR-1101: card の下の行（`row`）の中の `+`（直径 `SEND_D` = 送る丸と同じ当たり・owner 188・左から `SP_1`）と送る丸（直径 `SEND_D`・右から `SP_2`・追記 222 で 1 段詰めた）。どちらも行の縦の中央。
/// 保証する: 2 つとも `row` の中・重ならない（行の幅が 2 つと inset の和より狭い時は `+` を優先し、丸は `row` の右端に寄せる）。
/// Wave: L1（`+` は欄の外に浮かない）, HOME-CHATBAR-1101
fn bottom_row(row: Rect) -> (Rect, Rect) {
    let plus = Rect::from_min_size(egui::pos2(row.left() + theme::SP_1, row.center().y - SEND_D / 2.0), Vec2::splat(SEND_D));
    let x = (row.right() - theme::SP_2 - SEND_D).max(plus.right() + theme::SP_1).min(row.right() - SEND_D);
    (plus, Rect::from_min_size(egui::pos2(x, row.center().y - SEND_D / 2.0), Vec2::splat(SEND_D)))
}

/// HOME-CHATBAR-1101: 送る丸（地 `text_primary` = Dark で白・中に `Icon::ArrowUp` を `bg_app` で）。AX = Button・名前は今の右端の語（`Send`・
/// 名前が同じ Project が在れば `Open “{name}”` = `Shell::enter_label` の ` · ` の前の語）・hover は全文。`busy` と ⌘K の list の間は押せない形（地 `surface_3`）。
/// 押した = `header::submit`（Enter と同じ道・空の欄なら何を打つかの 1 行）。
fn send_circle(ui: &mut egui::Ui, at: Rect, shell: &mut Shell, apps: &[AppRow], busy: bool) -> Option<Intent> {
    let t = theme::tokens(ui.ctx());
    let full = shell.enter_label(apps);
    let name = full.trim_start_matches("↵ ").split(" · ").next().unwrap_or_default().to_owned();
    let on = !busy && !shell.palette.open;
    let r = ui.interact(at, egui::Id::new("kangae.home.send"), if on { egui::Sense::click() } else { egui::Sense::hover() });
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, on, &name));
    if ui.is_rect_visible(at) {
        let fill = match (on, r.is_pointer_button_down_on(), r.hovered()) {
            (false, ..) => t.surface_3,
            (true, true, _) => t.text_tertiary,
            (true, false, true) => t.text_secondary,
            _ => t.text_primary,
        };
        let ink = if on { t.bg_app } else { t.text_tertiary };
        let p = ui.painter();
        p.circle_filled(at.center(), SEND_D / 2.0, fill);
        // ICONS-1101（owner 227「上矢印と下矢印のアイコンが辺だし…細い」）: 手描きの 3 本の線から Phosphor Bold の `ArrowUp`
        theme::paint_icon(p, Icon::ArrowUp, at.center(), 18.0, ink, 0.0);
        theme::focus_ring(ui, &r, at, egui::CornerRadius::same((SEND_D / 2.0) as u8));
    }
    let clicked = r.on_hover_text(full.as_str()).clicked();
    (on && clicked).then(|| header::submit(ui, shell, apps, false, true)).flatten()
}

/// HOME-CHATBAR-1101（owner 11-01「chatbarの上にアイコンとShibubuとのせること」）: card の上の左寄せの印 = 透明の icon（`BRAND_PNG`・高さ `BRAND_ICON`・owner 188）と「Shibubu」（字 `BRAND_SIZE`・600）。
/// 絵が読めない時は語だけ。AX は語の 1 つ（icon は飾り = 名前を持たない）。
fn brand_row(ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), BRAND_ROW_H));
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(BRAND_INSET, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    row.spacing_mut().item_spacing.x = theme::SP_2;
    if let Some(tex) = brand_icon(ui.ctx()) {
        let size = tex.size_vec2();
        row.add(egui::Image::from_texture((tex.id(), Vec2::new(BRAND_ICON * size.x / size.y, BRAND_ICON))));
    }
    let font = egui::FontId::new(BRAND_SIZE, egui::FontFamily::Name(theme::SEMIBOLD.into()));
    row.add(egui::Label::new(egui::RichText::new("Shibubu").font(font).color(t.text_primary)).selectable(false));
}

/// 印の texture（ctx に 1 つ・初めて描く時に作る。読めなければ None を覚えて毎 frame 読み直さない）。TITLEBAR-ICONS-1101: header の題の帯の印も同じ物。
pub(super) fn brand_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("kangae.home.brand_icon");
    if let Some(cached) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) {
        return cached;
    }
    let tex = brand_image().map(|img| ctx.load_texture("shibubu-brand-icon", img.clone(), egui::TextureOptions::LINEAR));
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// 1024 の透明の PNG を 1/8（128）に縮め（premultiplied で平均 = 縁に黒い滲みを出さない）、絵の在る所（alpha > 0）だけに切る。process に 1 回。
fn brand_image() -> Option<&'static egui::ColorImage> {
    static IMAGE: std::sync::OnceLock<Option<egui::ColorImage>> = std::sync::OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let icon = eframe::icon_data::from_png_bytes(BRAND_PNG).ok()?;
            let (w, k) = (icon.width as usize, 8);
            let (sw, sh) = (w / k, icon.height as usize / k);
            let mut px = vec![egui::Color32::TRANSPARENT; sw * sh];
            for (i, out) in px.iter_mut().enumerate() {
                let (x, y) = (i % sw, i / sw);
                let mut acc = [0u32; 4];
                for (dy, dx) in (0..k).flat_map(|dy| (0..k).map(move |dx| (dy, dx))) {
                    let p = &icon.rgba[((y * k + dy) * w + x * k + dx) * 4..][..4];
                    let a = u32::from(p[3]);
                    for c in 0..3 {
                        acc[c] += u32::from(p[c]) * a;
                    }
                    acc[3] += a;
                }
                let n = (k * k) as u32;
                *out = egui::Color32::from_rgba_premultiplied((acc[0] / 255 / n) as u8, (acc[1] / 255 / n) as u8, (acc[2] / 255 / n) as u8, (acc[3] / n) as u8);
            }
            let on = |x: usize, y: usize| px[y * sw + x].a() > 0;
            let xs: Vec<usize> = (0..sw).filter(|&x| (0..sh).any(|y| on(x, y))).collect();
            let ys: Vec<usize> = (0..sh).filter(|&y| (0..sw).any(|x| on(x, y))).collect();
            let (x0, x1, y0, y1) = (*xs.first()?, *xs.last()?, *ys.first()?, *ys.last()?);
            let mut img = egui::ColorImage::new([x1 - x0 + 1, y1 - y0 + 1], egui::Color32::TRANSPARENT);
            for (y, x) in (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (y, x))) {
                img[(x - x0, y - y0)] = px[y * sw + x];
            }
            Some(img)
        })
        .as_ref()
}

/// CHAT-ONE-1101（owner 183「choose folderをchatごと、appごとに」・185「choosefolderはfolderアイコンで」・見本 ref-1715 の入力欄の下の「📁 yc-mvp」）:
/// chat bar の下の行の左の folder = `Icon::Folder`（ICONS-1101・NOTIFY-1101 の手描きの輪郭から）と選んだ folder の名前（`Shell::folder`・無ければ icon だけ・長い名前は 24 字で切る）。
/// AX = Button `Choose folder…`（folder の名前は説明）・hover は名前と `Choose folder…`。押した = true（呼ぶ側が `PageIntent::ChooseFolder`:
/// Home = この chat の folder〈AI が読む所・案で作った Project の folder になる〉・Project = その Project の folder〈TERM-ONE-1101 の terminal の cwd と同じ値〉）。
fn folder_chip(ui: &mut egui::Ui, folder: Option<&str>) -> bool {
    let t = theme::tokens(ui.ctx());
    let galley = folder.map(|n| {
        let shown: String = if n.chars().count() > 24 { n.chars().take(23).chain(['…']).collect() } else { n.to_owned() };
        ui.painter().layout_no_wrap(shown, Type::Small.font(), egui::Color32::PLACEHOLDER)
    });
    let w = theme::BUTTON_H + galley.as_ref().map_or(0.0, |g| g.size().x + theme::SP_2);
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, theme::BUTTON_H), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), CHOOSE_FOLDER));
    if let Some(n) = folder {
        ui.ctx().accesskit_node_builder(r.id, |b| b.set_description(n));
    }
    if ui.is_rect_visible(rect) {
        let radius = egui::CornerRadius::same(theme::RADIUS_MD);
        let fill = if r.is_pointer_button_down_on() { t.surface_press } else if r.hovered() { t.surface_3 } else { egui::Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, radius, fill);
        let ink = if r.hovered() { t.text_primary } else { t.text_secondary };
        let icon = Rect::from_min_size(rect.min, Vec2::splat(theme::BUTTON_H));
        theme::paint_icon(ui.painter(), Icon::Folder, icon.center(), theme::ICON_UI, ink, 0.0);
        if let Some(g) = galley {
            ui.painter().galley(egui::pos2(icon.right(), rect.center().y - g.size().y / 2.0), g, ink);
        }
        theme::focus_ring(ui, &r, rect, radius);
    }
    r.on_hover_text(folder.map_or_else(|| CHOOSE_FOLDER.to_owned(), |n| format!("{n} · {CHOOSE_FOLDER}"))).clicked()
}

/// folder の icon の AX の名前と hover の語（G99c の `Choose folder…` の button と同じ語）。
const CHOOSE_FOLDER: &str = "Choose folder…";

/// owner 180 通目（「providerやmodel, effort切り替えはchatの下に置くように今なんのproviderでmodel, effortを表示それぞれを押すとdropdown menuで選べる形」）× owner 188（card の中の下の行）:
/// 送る丸の左に右寄せの `{Provider} · {Model} · {Effort}` の今の値。それぞれ押すとその場で dropdown（今の値に印・押すと `UseAiAccount` /
/// `SetModel` / `SetEffort`）・選べない時は一覧の代わりに理由の 1 行（`Accepts::model_refusal` / `refusal(None)`）。
/// AX = Button `Provider: {値}`・`Model: {値}`・`Effort: {値}`（`+` の menu の段 `Provider` / `Model` / `Effort` と別の名前）。
/// 一覧は PROVIDER-1101 の形（選んだ AI ごと）: Provider = `Shell::providers` の行・Model = `Accepts::models()`・Effort = `Accepts::efforts`。
/// 行の Provider の語 = `Accepts::provider`（AI の名前）・空なら使っている行の語（` · ` の前）。
fn choices(ui: &mut egui::Ui, shell: &Shell) -> Option<Intent> {
    use crate::domain::chat::Effort;
    let t = theme::tokens(ui.ctx());
    let accepts = &shell.accepts;
    let mut intent = None;
    let row = shell.providers.iter().find(|p| p.2).map_or("Claude Code", |p| p.1.as_str());
    let provider = if accepts.provider.is_empty() { row.split(" · ").next().unwrap_or(row) } else { accepts.provider.as_str() };
    // PROVIDER-ICONS-1101: Provider の語の左と dropdown の行の左にその provider の印（account の無い行 = System default = Claude）
    let key_of = |acct: &Option<crate::model::AiAccount>| acct.as_ref().map_or("claude", |a| a.provider()).to_owned();
    let key = shell.providers.iter().find(|p| p.2).map_or_else(|| "claude".to_owned(), |p| key_of(&p.0));
    let current = shell.model.as_deref().filter(|m| accepts.model_ok(m));
    let model = current.and_then(|m| accepts.models().find(|(id, _)| *id == m)).map(|(_, label)| label);
    let effort = shell.effort.map(Effort::label);
    let reason = |ui: &mut egui::Ui, why: &str| {
        theme::label(ui, Type::Helper, why, t.text_tertiary);
    };
    let (effort_word, model_word) = (effort.unwrap_or("Default effort"), model.unwrap_or("Default model"));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        theme::native_buttons(ui);
        ui.spacing_mut().item_spacing.x = 0.0;
        // HOME-CHATBAR-1101 追記 222: 幅が足りない時は Model の名前を … で縮める（Effort と Provider と点 2 つの残り・全文は hover・AX の名前は全文）
        let wide = |ui: &egui::Ui, s: &str| ui.painter().layout_no_wrap(s.to_owned(), Type::Small.font(), egui::Color32::PLACEHOLDER).size().x;
        let pad = 2.0 * ui.spacing().button_padding.x;
        let model_max = (ui.available_width() - wide(ui, effort_word) - wide(ui, provider) - 2.0 * wide(ui, "·") - 3.0 * pad - theme::ICON_UI).max(theme::SP_6);
        let dot = |ui: &mut egui::Ui| {
            theme::label(ui, Type::Small, "·", t.text_tertiary);
        };
        let open = |ui: &mut egui::Ui, kind: &str, word: &str, max: f32, rows: &mut dyn FnMut(&mut egui::Ui)| {
            let shown = theme::truncated(ui, word, Type::Small.font(), t.text_secondary, max);
            let cut = shown.size().x + 0.5 < wide(ui, word);
            let r = ui.menu_button(shown, |ui| rows(ui));
            let r = theme::ax_name(r.response, &format!("{kind}: {word}"));
            if cut {
                let _ = r.on_hover_text(word);
            }
        };
        // 右から並べる（Effort・Model・Provider）
        open(ui, "Effort", effort_word, f32::INFINITY, &mut |ui| match accepts.refusal(None) {
            Some(why) => reason(ui, why.as_str()),
            None => {
                for (label, e) in std::iter::once(("Default", None)).chain(accepts.efforts.iter().map(|e| (e.label(), Some(*e)))) {
                    if ui.selectable_label(shell.effort == e, label).clicked() {
                        intent = Some(Intent::SetEffort(e));
                        ui.close_menu();
                    }
                }
            }
        });
        dot(ui);
        open(ui, "Model", model_word, model_max, &mut |ui| match accepts.model_refusal() {
            Some(why) => reason(ui, why.as_str()),
            None => {
                for (label, alias) in std::iter::once(("Default", None)).chain(accepts.models().map(|(a, l)| (l, Some(a)))) {
                    if ui.selectable_label(current == alias, label).clicked() {
                        intent = Some(Intent::SetModel(alias.map(str::to_owned)));
                        ui.close_menu();
                    }
                }
            }
        });
        dot(ui);
        // 行には Provider の名前（` · ` の後の請求の注は dropdown の行に）
        // ACCOUNTS-1101 2 段目（作る物 3）: 行 = 足した account も（`Claude · work`・`Claude 2`）。Project の中で替えたらその Project に覚える（app.rs の poll_llm）
        open(ui, "Provider", provider, f32::INFINITY, &mut |ui| {
            for (acct, label, on) in &shell.providers {
                if theme::provider_menu_item(ui, &key_of(acct), label, if *on { theme::icon_glyph(Icon::Check) } else { "" }).clicked() {
                    if !*on {
                        intent = Some(Intent::UseAiAccount(acct.clone()));
                    }
                    ui.close_menu();
                }
            }
            // AI が無い時はその理由（`Accepts::refusal` の AI 無しの語）。sign in がまだの行は `account_label` が「X · sign in first」と言う
            if let Some(why) = accepts.refusal(None).filter(|_| !accepts.ai) {
                reason(ui, why.as_str());
            }
        });
        // 右から並べているので Provider の語の後 = 語の左に印
        let (mark, _) = ui.allocate_exact_size(Vec2::splat(theme::ICON_UI), egui::Sense::hover());
        theme::provider_icon(ui, mark.center(), &key, provider, t.text_secondary);
    });
    intent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::chat::{Accepts, AttachKind, Attachment};
    use crate::ui::shell::View;
    use egui::accesskit::{Action, ActionRequest, Role};

    /// AI が入っていない時の帯の 1 行と次の一手（`llm_cli::NOT_INSTALLED` と `llm_cli::next_step` の写し）。ui は llm_cli を引けない
    /// （tests/ui_*.rs が `#[path]` で ui を取り込み、そこに llm_cli は無い = terminal_panel.rs の panel_spec と同じ決まり）。
    const NO_CLI: (&str, Option<&str>) = ("Claude Code isn't on this Mac", Some("Install Claude Code"));

    /// HOME-CHATBAR-1101 追記 222: 文の場所は空なら 1 行分（`TEXT_MIN_H`）・行が増えると伸び・`TEXT_H` で止まる。
    /// 負の対照: `text_h` を `TEXT_H` 固定に戻す → 1 つ目が赤・clamp の上限を外す → 3 つ目が赤。
    #[test]
    fn chatbar_222_text_area_grows_with_lines() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Dark);
        let mut shell = Shell::new(View::Home);
        let mut hs = Vec::new();
        for text in ["", "a\nb", "a\nb\nc\nd\ne\nf"] {
            shell.command = text.into();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| hs.push(text_h(ui, &shell, 600.0)));
            });
        }
        assert_eq!(hs[0], TEXT_MIN_H, "空の文の場所: {hs:?}");
        assert!(hs[1] > hs[0] && hs[1] < TEXT_H, "2 行で伸びない: {hs:?}");
        assert_eq!(hs[2], TEXT_H, "上限で止まらない: {hs:?}");
    }

    /// L1（109・8）× HOME-CHATBAR-1101: `+` と送る丸は card の下の行の中（外に浮かない）・重ならない・丸の直径は `SEND_D`（owner 188 で 32・仮）。
    /// 負の対照: `bottom_row` の `+` の x を `row.left() - SP_2` にする → 1 つ目が赤・丸の `.min(row.right() - SEND_D)` を外す → 狭い行で赤。
    #[test]
    fn l1_plus_and_send_sit_inside_the_card_row() {
        for row in [Rect::from_min_size(egui::pos2(80.0, 40.0), Vec2::new(686.0, ROW_H)), Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(60.0, ROW_H))] {
            let (plus, send) = bottom_row(row);
            for r in [plus, send] {
                assert!(row.contains_rect(r), "{r:?} not in {row:?}");
            }
            assert!(plus.right() <= send.left(), "重なった: {plus:?} {send:?}");
            assert_eq!(send.width(), SEND_D);
        }
    }

    fn turn(who: crate::domain::chat::Who, text: &str) -> crate::domain::chat::Turn {
        crate::domain::chat::Turn { who, text: text.into(), mark: crate::domain::chat::Mark::None, at: "t".into(), attachments: Vec::new(), choices: Vec::new(), proposal: None, preview: Vec::new(), sections: Vec::new(), needs: None }
    }

    /// AI が使える Home（Provider 2 行・Model は Fable と Sonnet・Effort あり）。
    fn ready() -> Shell {
        let mut shell = Shell::new(View::Home);
        let models = [("fable", "Fable"), ("sonnet", "Sonnet")].map(|(id, label)| (id.to_owned(), label.to_owned())).to_vec();
        shell.accepts = Accepts { ai: true, provider: "Claude Code".into(), photos: true, files: true, efforts: crate::domain::chat::Effort::ALL.to_vec(), models, ..Default::default() };
        shell.providers = vec![
            (None, "Claude Code".into(), true),
            (Some(crate::model::AiAccount::Key { id: "key-1".into(), last4: "a1b2".into() }), "API key ····a1b2 · billed per request by Anthropic".into(), false),
        ];
        shell
    }

    /// Home を `frames` の event で 1 frame ずつ描く（frame ごとに 1 束・最後に 2 frame 落ち着かせる）。返す = 最後の frame の AX の node・出た Intent。
    fn typed(shell: &mut Shell, frames: Vec<Vec<egui::Event>>) -> (Vec<egui::accesskit::Node>, Vec<Intent>) {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let (mut nodes, mut got) = (Vec::new(), Vec::new());
        for events in std::iter::once(Vec::new()).chain(frames).chain([Vec::new(), Vec::new()]) {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 900.0))), events, ..Default::default() };
            let o = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| got.extend(draw(ui, None, &[], shell, None, None)));
            });
            nodes = o.platform_output.accesskit_update.expect("AccessKit の木").nodes.into_iter().map(|(_, n)| n).collect();
        }
        (nodes, got)
    }

    fn key(modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// HOME-CHATBAR-1101: card の上の印（icon と「Shibubu」）は owner の一言が会話に無い間だけ。送ると消え、`New chat`（左の sidebar・app.rs が会話を空に）で戻る。
    /// Shibubu の答えだけの会話（owner の一言が無い）では残る。負の対照: `brand` を `true` にする → 2 つ目（送った後も印が在る）が赤。
    #[test]
    fn chatbar_brand_shows_until_the_owner_says_something() {
        use crate::domain::chat::Who;
        let brand = |names: &[String]| names.iter().filter(|n| *n == "Shibubu").count();
        let mut shell = ready();
        let (names, _) = home(&mut shell, &[]);
        assert_eq!(brand(&names), 1, "空の Home に印が無い: {names:?}");
        shell.talk.push(turn(Who::You, "Plan a trip"));
        let (names, _) = home(&mut shell, &[]);
        assert_eq!(brand(&names), 0, "送った後も印が残った: {names:?}");
        // owner 191: `New chat` は header の右端（`new_chat_button`）= ここは app.rs と同じく会話を空にする
        shell.talk.clear();
        let (names, _) = home(&mut shell, &[]);
        assert_eq!(brand(&names), 1, "New chat で空に戻っても印が出ない: {names:?}");
        assert!(brand_image().is_some_and(|i| i.size[0] > 0 && i.size[1] > 0), "印の絵が読めない");
    }

    /// HOME-CHATBAR-1101: Home の欄は複数行 = ⇧Enter で改行（送らない）・Enter で送る（`Talk`・欄は空に）・空の欄の Enter は何も送らず改行もしない。
    /// AX は 1 行の欄の role（TextInput = AXTextField）と今の名前（labels.py の `goal`）。
    /// 負の対照: Enter の判定の `modifiers.is_none()` を外す → ⇧Enter で送って赤・`return_key(⇧Enter)` を外す → 空の Enter が改行を入れて赤・
    /// role の上書きを外す → MultilineTextInput で赤。
    #[test]
    fn chatbar_enter_sends_and_shift_enter_breaks_the_line() {
        let mut shell = ready();
        shell.focus_command = true;
        let (nodes, got) = typed(&mut shell, vec![vec![], vec![key(egui::Modifiers::NONE)]]);
        assert!(got.is_empty() && shell.command.is_empty(), "空の Enter: {got:?} / {:?}", shell.command);
        let field = nodes.iter().find(|n| n.label() == Some(shell.view.field_name())).expect("欄の AX の名前が替わった");
        assert_eq!(field.role(), Role::TextInput, "欄の AX の role が 1 行の欄でない");

        shell.focus_command = true;
        let (_, got) = typed(&mut shell, vec![vec![], vec![egui::Event::Text("Plan".into())], vec![key(egui::Modifiers::SHIFT)], vec![egui::Event::Text("a trip".into())]]);
        assert!(got.is_empty(), "⇧Enter で送った: {got:?}");
        assert_eq!(shell.command, "Plan\na trip", "⇧Enter で改行しない");
        shell.focus_command = true;
        let (_, got) = typed(&mut shell, vec![vec![], vec![key(egui::Modifiers::NONE)]]);
        assert_eq!(got, [Intent::Talk("Plan\na trip".into())]);
        assert!(shell.command.is_empty(), "送った後に欄が空でない: {:?}", shell.command);
    }

    /// HOME-CHATBAR-1101: card の下の行の送る丸 = AX の Button `Send`（今の右端の語）・押すと Enter と同じ `Talk`・空の欄なら何を打つかの 1 行。
    /// 負の対照: `send_circle` の `header::submit` を None にする → 1 つ目が赤。
    #[test]
    fn chatbar_send_circle_sends_like_enter() {
        let mut shell = ready();
        shell.command = "Plan a trip".into();
        let (_, got) = home(&mut shell, &["Send"]);
        assert_eq!(got, [Intent::Talk("Plan a trip".into())]);
        assert!(shell.command.is_empty());
        let (_, got) = home(&mut shell, &["Send"]);
        assert_eq!(got, [Intent::Notice("Type what you want to create first".into())]);
    }

    /// owner 180: card の下の 1 行 = Provider · Model · Effort の今の値（AX `Provider: …` ほか）。押すと dropdown・行を押すと切り替え・選べない時は理由の 1 行。
    /// 負の対照: `choices` の Model の行の `intent = Some(Intent::SetModel(..))` を外す → 赤・Model の `model_refusal` の腕を外す → AI 無しで理由が無く赤。
    #[test]
    fn chatbar_choices_under_the_card_open_dropdowns() {
        use crate::domain::chat::Effort;
        let mut shell = ready();
        let (names, got) = home(&mut shell, &[]);
        assert!(got.is_empty());
        for want in ["Provider: Claude Code", "Model: Default model", "Effort: Default effort"] {
            assert!(names.iter().any(|n| n == want), "{want}: {names:?}");
        }
        let (names, got) = home(&mut shell, &["Model: Default model"]);
        assert!(got.is_empty() && names.iter().any(|n| n == "Fable") && !names.iter().any(|n| n == "Opus"), "受ける別名だけ: {names:?}");
        let (_, got) = home(&mut shell, &["Model: Default model", "Sonnet"]);
        assert_eq!(got, [Intent::SetModel(Some("sonnet".into()))]);
        let (_, got) = home(&mut shell, &["Effort: Default effort", "High"]);
        assert_eq!(got, [Intent::SetEffort(Some(Effort::High))]);
        let key = crate::model::AiAccount::Key { id: "key-1".into(), last4: "a1b2".into() };
        let (_, got) = home(&mut shell, &["Provider: Claude Code", "API key ····a1b2 · billed per request by Anthropic"]);
        assert_eq!(got, [Intent::UseAiAccount(Some(key))]);
        (shell.model, shell.effort) = (Some("sonnet".into()), Some(Effort::XHigh));
        let (names, _) = home(&mut shell, &[]);
        assert!(names.iter().any(|n| n == "Model: Sonnet") && names.iter().any(|n| n == "Effort: Extra high"), "今の値の語でない: {names:?}");

        let mut off = Shell::new(View::Home);
        let (names, _) = home(&mut off, &["Model: Default model"]);
        assert!(names.iter().any(|n| n == "Needs Claude Code on this Mac"), "AI 無しの Model に理由が無い: {names:?}");
    }

    /// HOME-CHATBAR-1101: 1680×1050 の Home で card の文の幅 = 688 − 枠 2・送る丸 `SEND_D`・`+` と丸は文より下の同じ高さ・印は文の左から 10 の上・
    /// 文の縦の位置は帯が在っても動かない（52% の位置そのものは app.rs の `fx4_home_field_center_stays_at_52_percent`）・横の中心は窓の中央（独立 review の指摘 = `add_space` の後のずれを測る）。
    /// 数の出所は `FIELD_W` の doc（見本の画素）。
    /// 負の対照: `draw` の `above` から帯の分を外す → 帯の在る時の中心が 36 ずれて赤。
    #[test]
    fn chatbar_card_has_the_sample_sizes() {
        let mut first = None;
        for band in [None, Some(NO_CLI)] {
            let mut shell = ready();
            let nodes = sized(&mut shell, Vec2::new(1680.0, 1050.0), egui::ThemePreference::Dark, band);
            let at = |name: &str| nodes.iter().find(|n| n.label() == Some(name) || n.value() == Some(name)).and_then(|n| n.bounds()).unwrap_or_else(|| panic!("{name} が無い"));
            let (field, send, plus, brand) = (at(shell.view.field_name()), at("Send"), at("Attach"), at("Shibubu"));
            assert!((field.width() - f64::from(FIELD_W - 2.0 * theme::HAIRLINE)).abs() < 0.5, "文の幅 {}", field.width());
            assert!((send.width() - f64::from(SEND_D)).abs() < 0.5 && (send.height() - f64::from(SEND_D)).abs() < 0.5, "丸 {send:?}");
            assert!(send.y0 > field.y1 && plus.y0 > field.y1 && ((send.y0 + send.y1) - (plus.y0 + plus.y1)).abs() < 1.0, "下の行 {plus:?} {send:?}");
            assert!(brand.y1 < field.y0 && brand.x0 - field.x0 > 0.0, "印 {brand:?} / 文 {field:?}");
            assert!(((field.x0 + field.x1) / 2.0 - 840.0).abs() < 1.0, "card が窓の中央に無い: {field:?}");
            let center = (field.y0 + field.y1) / 2.0;
            let was = *first.get_or_insert(center);
            assert!((center - was).abs() < 1.0, "帯が出て文が動いた: {was} → {center}");
        }
    }

    /// owner 181 の会話（打った文 1 つと三択の答え）。試験と写真の両方で使う。
    fn talk_sample() -> Vec<crate::domain::chat::Turn> {
        use crate::domain::chat::Who;
        let mut answer = turn(Who::Shibubu, "Hi! What would you like to make?");
        answer.choices = ["A page for my notes", "A trip plan", "A tracker"].map(String::from).to_vec();
        vec![turn(Who::You, "hello"), answer]
    }

    /// owner 181: 打った文の吹き出し・答えの候補（と card）は Light でも Dark でも背景から見分けられる = 面か枠が `bg_app` と 3:1 以上（theme の値で測る）。
    /// 描いた形も同じ色: 会話の在る Home の形の中に 面＋枠の吹き出しが 1 つ・透明＋枠の候補が 3 つ。
    /// 負の対照: `talk_colors` の枠を `t.bg_app` にする → 比 1.0 で赤・`you_bubble` の `.stroke(..)` を外す → 吹き出しが 0 で赤。
    #[test]
    fn owner181_talk_parts_stand_out_in_light_and_dark() {
        use crate::ui::theme::tests::contrast;
        for (pref, t) in [(egui::ThemePreference::Light, theme::LIGHT), (egui::ThemePreference::Dark, theme::DARK)] {
            let (fill, edge) = talk_colors(&t);
            let (f, e) = (contrast(fill, t.bg_app), contrast(edge, t.bg_app));
            assert!(fill != t.bg_app && f.max(e) >= 3.0, "{pref:?}: 面 {f:.2}・枠 {e:.2}（どちらも 3 未満）");
            let mut shell = ready();
            shell.talk = talk_sample();
            let (_, out, _) = sized_at(&mut shell, Vec2::new(1440.0, 900.0), pref, None, 1.0);
            let rects: Vec<_> = out.shapes.iter().filter_map(|c| match &c.shape { egui::Shape::Rect(r) => Some(r), _ => None }).collect();
            let edged = |inside: egui::Color32| rects.iter().filter(|r| r.fill == inside && r.stroke.color == edge && r.stroke.width >= theme::HAIRLINE).count();
            assert_eq!(edged(fill), 1, "{pref:?}: 面と枠の吹き出し");
            assert_eq!(edged(egui::Color32::TRANSPARENT), 3, "{pref:?}: 枠の候補 3 つ");
        }
    }

    /// `size` の窓・`theme` で Home を 4 frame 描く（`llm` = 帯の 1 行）。返す = 最後の frame の AX の node と FullOutput。
    fn sized(shell: &mut Shell, size: Vec2, theme_pref: egui::ThemePreference, llm: Option<(&str, Option<&'static str>)>) -> Vec<egui::accesskit::Node> {
        sized_at(shell, size, theme_pref, llm, 1.0).0
    }

    /// `sized` の倍率つき。返す = AX の node・最後の FullOutput（`textures_delta.set` は 4 frame 分を足した物）・tessellate する ctx。
    fn sized_at(shell: &mut Shell, size: Vec2, theme_pref: egui::ThemePreference, llm: Option<(&str, Option<&'static str>)>, ppp: f32) -> (Vec<egui::accesskit::Node>, egui::FullOutput, egui::Context) {
        let ctx = egui::Context::default();
        theme::install(&ctx, theme_pref);
        ctx.enable_accesskit();
        let mut last = None;
        let mut set = Vec::new();
        for _ in 0..4 {
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
            let mut o = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw(ui, None, &[], shell, None, llm);
                });
            });
            set.append(&mut o.textures_delta.set);
            last = Some(o);
        }
        let mut o = last.expect("frame");
        o.textures_delta.set = set;
        let nodes = o.platform_output.accesskit_update.take().expect("AccessKit の木").nodes.into_iter().map(|(_, n)| n).collect();
        (nodes, o, ctx)
    }

    /// HOME-CHATBAR-1101 の写真（試験の描画 = egui の tessellate の三角形を CPU で塗った物・実機の写真ではない）: 1680×1050 pt を 2 倍。
    /// `SHIBUBU_SHOT_DIR` に `home-empty.rgba`（AI が使える空の Home・Dark）と `home-band.rgba`（帯と添えた物が在る = 見本と同じ部品・Dark）・
    /// owner 181 の `talk-dark.rgba` / `talk-light.rgba`（`talk_sample` の会話）・各 `.json`（欄と送る丸の px の rect）を書く。PNG と見本との並べは docs/evidence/HOME-CHATBAR-1101/compose.py。
    #[test]
    #[ignore = "写真を書く（SHIBUBU_SHOT_DIR）"]
    fn chatbar_shot() {
        use std::collections::HashMap;
        let dir = std::path::PathBuf::from(std::env::var("SHIBUBU_SHOT_DIR").expect("SHIBUBU_SHOT_DIR"));
        let (dark, light) = (egui::ThemePreference::Dark, egui::ThemePreference::Light);
        let band = Some(NO_CLI);
        for (name, pref, llm, talk) in [("home-empty", dark, None, false), ("home-band", dark, band, false), ("talk-dark", dark, None, true), ("talk-light", light, None, true)] {
            let mut shell = ready();
            if llm.is_some() {
                shell.attach = vec![Attachment::File { path: "/tmp/shot/sketch.png".into(), kind: AttachKind::Photo, bytes: 3 }];
            }
            if talk {
                shell.talk = talk_sample();
            }
            let (ppp, size) = (2.0, Vec2::new(1680.0, 1050.0));
            let (nodes, out, ctx) = sized_at(&mut shell, size, pref, llm, ppp);
            let mut tex: HashMap<egui::TextureId, (usize, Vec<egui::Color32>)> = HashMap::new();
            for (id, delta) in &out.textures_delta.set {
                let (dim, px): ([usize; 2], Vec<egui::Color32>) = match &delta.image {
                    egui::ImageData::Color(c) => (c.size, c.pixels.clone()),
                    egui::ImageData::Font(f) => (f.size, f.srgba_pixels(None).collect()),
                };
                match delta.pos {
                    None => {
                        tex.insert(*id, (dim[0], px));
                    }
                    Some([x0, y0]) => {
                        let (tw, buf) = tex.get_mut(id).expect("部分の更新の前に全体");
                        for (i, c) in px.into_iter().enumerate() {
                            buf[(y0 + i / dim[0]) * *tw + x0 + i % dim[0]] = c;
                        }
                    }
                }
            }
            let (w, h) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
            let mut fb = vec![[0.0f32; 4]; w * h];
            let edge = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            for prim in ctx.tessellate(out.shapes, ppp) {
                let egui::epaint::Primitive::Mesh(mesh) = prim.primitive else { continue };
                let Some((tw, tpx)) = tex.get(&mesh.texture_id) else { continue };
                let th = tpx.len() / tw;
                let clip = prim.clip_rect;
                let (cx0, cy0) = ((clip.min.x * ppp).max(0.0) as usize, (clip.min.y * ppp).max(0.0) as usize);
                let (cx1, cy1) = (((clip.max.x * ppp).ceil() as usize).min(w), ((clip.max.y * ppp).ceil() as usize).min(h));
                for tri in mesh.indices.chunks_exact(3) {
                    let v = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
                    let p = v.map(|v| (v.pos.to_vec2() * ppp).to_pos2());
                    let area = edge(p[0], p[1], p[2]);
                    if area.abs() < 1e-6 {
                        continue;
                    }
                    let lo = |f: fn(f32, f32) -> f32, a: [f32; 3]| f(f(a[0], a[1]), a[2]);
                    let (xs, ys) = (p.map(|q| q.x), p.map(|q| q.y));
                    let (x0, x1) = ((lo(f32::min, xs).floor().max(0.0) as usize).max(cx0), (lo(f32::max, xs).ceil().max(0.0) as usize).min(cx1));
                    let (y0, y1) = ((lo(f32::min, ys).floor().max(0.0) as usize).max(cy0), (lo(f32::max, ys).ceil().max(0.0) as usize).min(cy1));
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let q = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                            let (b0, b1) = (edge(p[1], p[2], q) / area, edge(p[2], p[0], q) / area);
                            let b2 = 1.0 - b0 - b1;
                            if b0 < -1e-4 || b1 < -1e-4 || b2 < -1e-4 {
                                continue;
                            }
                            let uv = v[0].uv.to_vec2() * b0 + v[1].uv.to_vec2() * b1 + v[2].uv.to_vec2() * b2;
                            let texel = tpx[((uv.y * th as f32) as usize).min(th - 1) * tw + ((uv.x * *tw as f32) as usize).min(tw - 1)].to_array();
                            let dst = &mut fb[y * w + x];
                            let src: [f32; 4] = std::array::from_fn(|c| {
                                let vc = f32::from(v[0].color.to_array()[c]) * b0 + f32::from(v[1].color.to_array()[c]) * b1 + f32::from(v[2].color.to_array()[c]) * b2;
                                vc / 255.0 * f32::from(texel[c]) / 255.0
                            });
                            *dst = std::array::from_fn(|c| src[c] + dst[c] * (1.0 - src[3]));
                        }
                    }
                }
            }
            let bytes: Vec<u8> = fb.iter().flat_map(|c| [c[0], c[1], c[2], 1.0].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
            std::fs::write(dir.join(format!("{name}.rgba")), bytes).expect("写真を書けない");
            let px = |n: &str| nodes.iter().find(|x| x.label() == Some(n)).and_then(|x| x.bounds()).map(|b| [b.x0, b.y0, b.x1, b.y1].map(|v| v * f64::from(ppp))).expect(n);
            let json = serde_json::json!({ "size": [w, h], "field": px(shell.view.field_name()), "send": px("Send"), "attach": px("Attach") });
            std::fs::write(dir.join(format!("{name}.json")), json.to_string()).expect("json を書けない");
            eprintln!("{name}: {json}");
        }
    }

    /// Home を描いて、毎 frame `click` の名前の button を AX で押す（押すのは 1 回だけ）。返す = 最後の frame の AX の名前・出た Intent。
    fn home(shell: &mut Shell, click: &[&str]) -> (Vec<String>, Vec<Intent>) {
        let (nodes, got) = home_nodes(shell, click);
        // 押せない形の理由（`theme::label`）は AccessKit の value に名前を持つ
        (nodes.iter().filter_map(|n| n.label().or(n.value()).map(str::to_owned)).collect(), got)
    }

    /// `home` の AX の node そのもの（disabled・description・expanded を見る試験の分）。
    fn home_nodes(shell: &mut Shell, click: &[&str]) -> (Vec<egui::accesskit::Node>, Vec<Intent>) {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let (mut nodes, mut got, mut events, mut left) = (Vec::new(), Vec::new(), Vec::new(), click.to_vec());
        for _ in 0..(4 + 3 * click.len()) {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 900.0))), events: std::mem::take(&mut events), ..Default::default() };
            let o = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| got.extend(draw(ui, None, &[], shell, None, None)));
            });
            let tree = o.platform_output.accesskit_update.expect("AccessKit の木");
            if let Some(&want) = left.first() {
                if let Some((id, _)) = tree.nodes.iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some(want)) {
                    events = vec![egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target: *id, data: None })];
                    left.remove(0);
                }
            }
            nodes = tree.nodes.into_iter().map(|(_, n)| n).collect();
        }
        assert!(left.is_empty(), "押せなかった: {left:?} / {:?}", nodes.iter().filter_map(|n| n.label()).collect::<Vec<_>>());
        (nodes, got)
    }

    /// HOME-ATTACH-1101（owner 180 通目）: 欄の左の `Attach`（`+`）の menu は `Add files or photos` の 1 行だけ（Photo… / Video… / Files… / Link… と
    /// Provider・Model・Effort は無い）・押すと `Intent::Attach`。AI が無くても押せる（owner 185 通目・読めるかは送る時）。欄の AX の名前は変わらない。
    /// 負の対照: `Intent::Attach` を返さない → 赤・`ui.button` を AI の有無で押せない形に戻す → AI 無しの押しが赤。
    #[test]
    fn home_attach_the_plus_menu_is_one_add_files_row() {
        let mut shell = Shell::new(View::Home);
        shell.accepts = Accepts { ai: true, provider: "Claude Code".into(), photos: true, files: true, efforts: crate::domain::chat::Effort::ALL.to_vec(), ..Default::default() };
        let field = shell.view.field_name();
        let (nodes, got) = home_nodes(&mut shell, &["Attach"]);
        assert!(got.is_empty(), "{got:?}");
        let names: Vec<&str> = nodes.iter().filter_map(|n| n.label()).collect();
        assert!(names.contains(&ADD_FILES) && names.contains(&field), "{names:?}");
        for gone in ["Photo…", "Video…", "Files…", "Link…", "Effort", "Provider", "Model"] {
            assert!(!names.contains(&gone), "{gone} が `+` の menu に残った: {names:?}");
        }
        let (_, got) = home(&mut shell, &["Attach", ADD_FILES]);
        assert_eq!(got, [Intent::Attach]);

        let mut off = Shell::new(View::Home);
        let (_, got) = home(&mut off, &["Attach", ADD_FILES]);
        assert_eq!(got, [Intent::Attach], "AI が無くても添えられる");
    }

    /// HOME-ATTACH-1101（owner 175 通目）: 添えた物は欄の下の長方形 1 つずつ。AX = 種類と名前（`Photo shot.png`・`File notes.md`・`Pasted text · 300 lines`）と
    /// × の `Remove {名前}`。× で外した物は並びから消え、欄に focus が戻る。貼った文は押すと読むだけの中身が開き（expanded・本文の行）、もう 1 回で閉じる。
    /// 写真の縮小絵が読めない path（試験の偽の path）は種類の印で描く（長方形は在る）。
    /// 負の対照: attach_row の `shell.attach.remove(i)` を外す → 外した長方形が残って赤・attach_tile の `format!("{kind} {name}")` を名前だけにする → 赤。
    #[test]
    fn home_attach_tiles_name_their_kind_and_remove_and_peek() {
        let mut shell = Shell::new(View::Home);
        shell.accepts = Accepts { ai: true, photos: true, files: true, efforts: crate::domain::chat::Effort::ALL.to_vec(), ..Default::default() };
        let text = (1..=300).map(|i| format!("pasted line {i}\n")).collect::<String>();
        shell.attach = vec![
            Attachment::File { path: "/tmp/home-attach/shot.png".into(), kind: AttachKind::Photo, bytes: 2048 },
            Attachment::File { path: "/tmp/home-attach/notes.md".into(), kind: AttachKind::File, bytes: 3 },
            Attachment::Pasted { text, lines: 300 },
        ];
        let (names, _) = home(&mut shell, &[]);
        for want in ["Photo shot.png", "File notes.md", "Pasted text · 300 lines", "Remove shot.png", "Remove notes.md", "Remove Pasted text · 300 lines"] {
            assert!(names.iter().any(|n| n == want), "{want}: {names:?}");
        }
        assert!(!names.iter().any(|n| n == "pasted line 1"), "押す前に中身が開いた");
        let (nodes, _) = home_nodes(&mut shell, &["Pasted text · 300 lines"]);
        let tile = nodes.iter().find(|n| n.label() == Some("Pasted text · 300 lines")).expect("貼った文の長方形");
        assert_eq!(tile.is_expanded(), Some(true), "押しても開かない");
        assert!(nodes.iter().any(|n| n.label().or(n.value()) == Some("pasted line 1")), "中身の 1 行目が無い");
        let (names, _) = home(&mut shell, &["Pasted text · 300 lines", "Pasted text · 300 lines"]);
        assert!(!names.iter().any(|n| n == "pasted line 1"), "もう 1 回で閉じない");

        let (names, got) = home(&mut shell, &["Remove notes.md"]);
        assert!(got.is_empty(), "{got:?}");
        assert_eq!(shell.attach.iter().map(|a| a.name()).collect::<Vec<_>>(), ["shot.png", "Pasted text · 300 lines"], "× で外れない");
        assert!(!names.iter().any(|n| n == "File notes.md"), "{names:?}");
    }

    /// HOME-ATTACH-1101 の写真（受け入れ「Dark の Home に長方形が 3 つ〈写真・file・貼った文〉並んだ chat bar」）: Home を wgpu（Metal）で画面の外に描いて
    /// `docs/evidence/HOME-ATTACH-1101/` に PNG を書く（01 = 長方形 3 つ・02 = 貼った文を押して中身を開いた形）。.app は起こさない = 試験の写真で、実機の写真ではない。
    /// 写真の縮小絵は本物の PNG を `chat::attach` に通した物（裏の thread で読み終わるまで frame を回す）。回し方: `cargo test --bin shibubu home_attach_photo -- --ignored`
    #[test]
    #[ignore = "GPU で描いて docs/evidence に PNG を書く（写真を撮り直す時だけ回す）"]
    fn home_attach_photo_dark() {
        use eframe::{egui_wgpu, wgpu};
        let dir = std::env::temp_dir().join(format!("home-attach-photo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("sunset.png");
        image::RgbaImage::from_fn(512, 512, |x, y| {
            let (fx, fy) = (x as f32 / 511.0, y as f32 / 511.0);
            if ((fx - 0.5).powi(2) + (fy - 0.6).powi(2)).sqrt() < 0.16 {
                image::Rgba([255, 214, 120, 255])
            } else if fy > 0.68 {
                image::Rgba([(30.0 + 40.0 * fx) as u8, (50.0 + 30.0 * fy) as u8, 110, 255])
            } else {
                image::Rgba([(250.0 - 60.0 * fy) as u8, (110.0 + 70.0 * fy) as u8, (90.0 + 130.0 * fy) as u8, 255])
            }
        })
        .save(&shot)
        .unwrap();
        let notes = dir.join("meeting-notes.md");
        std::fs::write(&notes, "# Weekly sync\n- ship the attach row\n".repeat(120)).unwrap();
        let log = (1..=412).map(|i| format!("{i:>4}  17:{:02}:{:02}  INFO  GET /api/projects  200  {}ms\n", i / 60 % 60, i % 60, 40 + i % 70)).collect::<String>();
        let mut shell = Shell::new(View::Home);
        shell.accepts = Accepts { ai: true, provider: "Claude Code".into(), photos: true, files: true, efforts: crate::domain::chat::Effort::ALL.to_vec(), ..Default::default() };
        shell.command = "Summarize these for the team".into();
        shell.attach = vec![
            crate::domain::chat::attach(&shot).unwrap(),
            crate::domain::chat::attach(&notes).unwrap(),
            crate::domain::chat::paste(&log).unwrap().unwrap(),
        ];
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/evidence/HOME-ATTACH-1101");
        std::fs::create_dir_all(&out_dir).unwrap();

        let (w, h, ppp) = (960.0_f32, 560.0_f32, 2.0_f32);
        let (pw, ph) = ((w * ppp) as u32, (h * ppp) as u32);
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Dark);
        ctx.set_pixels_per_point(ppp);
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("GPU の adapter が無い");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).expect("GPU の device");
        // egui は gamma の空間で混ぜる = 出力は Unorm（Srgb にしない・egui_wgpu::Renderer::new の doc）
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        for (file, peek) in [("01-three-tiles-dark.png", None), ("02-pasted-text-open-dark.png", Some(2usize))] {
            let mut last = None;
            // 縮小絵の裏の読みと、出てくる動き（fade）が終わるまで回す
            for _ in 0..60 {
                if let Some(i) = peek {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("kangae.attach.peek"), i));
                }
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(w, h))), ..Default::default() };
                let out = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw(ui, None, &[], &mut shell, None, None);
                    });
                });
                for (id, delta) in &out.textures_delta.set {
                    renderer.update_texture(&device, &queue, *id, delta);
                }
                for id in &out.textures_delta.free {
                    renderer.free_texture(id);
                }
                last = Some(out);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let out = last.unwrap();
            let jobs = ctx.tessellate(out.shapes, out.pixels_per_point);
            let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [pw, ph], pixels_per_point: out.pixels_per_point };
            let size = wgpu::Extent3d { width: pw, height: ph, depth_or_array_layers: 1 };
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let cmds = renderer.update_buffers(&device, &queue, &mut enc, &jobs, &sd);
            {
                let mut pass = enc
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    })
                    .forget_lifetime();
                renderer.render(&mut pass, &jobs, &sd);
            }
            let row = (pw * 4).div_ceil(256) * 256;
            let buf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(row * ph), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(ph) } },
                size,
            );
            queue.submit(cmds.into_iter().chain([enc.finish()]));
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = device.poll(wgpu::Maintain::Wait);
            let data = buf.slice(..).get_mapped_range();
            let px: Vec<u8> = (0..ph).flat_map(|y| data[(y * row) as usize..(y * row + pw * 4) as usize].iter().copied()).collect();
            image::RgbaImage::from_raw(pw, ph, px).unwrap().save(out_dir.join(file)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

}
