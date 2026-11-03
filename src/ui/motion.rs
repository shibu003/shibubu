//! DESIGN.md §15.2-1 の animation 部品。殻への当て方は docs/diagrams.md §16.1（View の切替・行の stagger・rail / sidebar・rail の返事・状態の色）。
//! - 時間は全部 `ctx.input().time`（秒）が基準。headless test は `RawInput::time` を進めれば決定的に測れる
//! - reduce motion（OS の「視差効果を減らす」/ env `KANGAE_REDUCE_MOTION=1`）= `MotionMode::Fade`: 位置と大きさは即時・不透明度と色だけ `REDUCE_FADE` まで（owner-addendum ①）。
//!   全部即時（全時間 0・repaint を頼まない）は試験だけの `MotionMode::Cut`
//! - animation 中だけ `request_repaint` し、終われば頼まない（idle の CPU を上げない）

use eframe::egui::{self, Color32, Context, Id, Ui, UiBuilder, Vec2};
use std::hash::Hash;

pub use egui::emath::easing::cubic_out as ease_out;

/// 画面の切替（fade + slide）。
pub const PAGE: f32 = 0.220;
/// rail / drawer の開閉。
pub const RAIL: f32 = 0.180;
/// list の行を順に出す時の 1 行ぶんのずれ。
pub const STAGGER: f32 = 0.030;
/// button の hover / press。
pub const HOVER: f32 = 0.100;
/// toast（下から slide）。
pub const TOAST: f32 = 0.240;
/// 数値の tween。
pub const NUMBER: f32 = 0.300;
/// 画面の切替で下から上がる量。
pub const SLIDE_PX: f32 = 8.0;
/// UI-6a: Reduce Motion（`MotionMode::Fade`）の時の cross-fade の長さ（FINAL の既定 120 = duration-tab / exit と同じ・owner-addendum ①「0 でなく短い fade」）。
/// 契約: `dur` の Fade の腕は `base.min(REDUCE_FADE)`（`HOVER` 100 より長い base だけを詰める）。値は 0 < 120 ≤ `PAGE`（門 `a1_reduce_fade`）。
/// 保証しない: timer（受け取りの 4 秒など・UI-6b の `HOLD_*` / `DELAY_*`）の長さ。timer は `dur` を通さない（通すと 120 に潰れる・門 `a6_timer_not_dur`）。
/// Wave: UI-6a
pub const REDUCE_FADE: f32 = 0.120;
/// UI-6a: 状態の色の cross-fade（`cross_fade`）の長さ（FINAL の duration-diff 160・M18）。本体で `cross_fade` が `PAGE` 220 からこれに替わる。
/// Wave: UI-6a
pub const DIFF: f32 = 0.160;
/// FX3（FX-011・UIUX B10・M03・DESIGN §3 / §9）: 帯の構成が変わった時（足す・動かす・名前替え・消す・戻す・やり直す）に、影響を受けた tab に塗る `highlight` の tint が消えるまでの長さ。
/// 例: `let tint = 1.0 - motion::progress(ctx, changed_at, motion::TINT);` → `theme::tab(.., tint)`（`progress` が 1 未満の間だけ repaint を頼む）。
/// 保証する: 色だけの fade なので `dur` を通さない（Reduce Motion の `Fade` でも 600ms・試験の `Cut` は呼ぶ側が 0 にする）。保証しない: 動いた tab の移動（`travel`・`PAGE`）と主役以外の fade（本体で `dur`）。
/// Wave: FX3
pub const TINT: f32 = 0.600;
// UI-3: hover の scale（`HOVER_SCALE` 1.015・`PRESS_SCALE` 0.985・`hover_scale`）は消した（呼ぶ所 0・SPEC §12「hover は色」= theme の button の塗り）。

/// UI-6a: 動きの段（owner-addendum ①: Reduce Motion は 0 でなく短い fade・位置と大きさは動かさない）。
/// - `Full` = 今まで通り（滑る・幅が寄る・色と不透明度が寄る）
/// - `Fade` = OS の Reduce Motion / env `KANGAE_REDUCE_MOTION=1`。位置と大きさ（`SLIDE_PX`・rail と sidebar の幅）は最初の frame から終わりの値で、
///   不透明度と色だけが `dur` の時間（`REDUCE_FADE` まで）で寄る。repaint は fade の間だけ頼む
/// - `Cut` = 試験だけの「全部即時」（今の reduce = 全時間 0 の振る舞い）。製品の設定にも env にも出さない（`tests/common::ctx(true)` の 23 か所が 1 frame で終点を読む）
/// 呼ぶ側の例: `match motion::motion_mode(ctx) { MotionMode::Full => slide, MotionMode::Fade | MotionMode::Cut => no_travel }`。
/// 新しくした理由: 前の private な `ReduceMotion(bool)` は 2 値で Fade と Cut を分けられない（UI-6a 本体で置き換えて消した・門 `a1_reduce_fade` がこの名と 3 つの腕を読む）。
/// 保証しない: OS の設定を替えた瞬間に効く事（`reduce_motion` の読み直しは 1 秒に 1 回まで）。
/// Wave: UI-6a
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionMode {
    Full,
    Fade,
    #[cfg_attr(not(test), allow(dead_code))] // 試験だけが上書きで作る
    Cut,
}

/// 段の記録（ctx の temp data に 1 つ）。`over` = `set_motion_mode` の上書き・`fixed` = env `KANGAE_REDUCE_MOTION` が決めた（OS を読み直さない）・
/// `on` と `read_at` = env か OS から解いた Reduce Motion と、OS を最後に読んだ ctx の時刻・`applied` = egui の Style に写した段。
#[derive(Clone, Copy)]
struct Motion {
    over: Option<MotionMode>,
    fixed: bool,
    on: bool,
    read_at: f64,
    applied: Option<MotionMode>,
}

/// OS の設定を読み直す間（秒）。UI の thread は `OS_REDUCE`（裏の thread が読んだ値）を見るだけで process を待たない。
const REREAD: f64 = 1.0;
/// G97: 裏の thread が `defaults read` で OS の設定を読み直す間（前は UI の thread が毎秒 process を待っていた・AUDIT2 D）。
const OS_REREAD: std::time::Duration = std::time::Duration::from_secs(2);
static OS_REDUCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static OS_WATCH: std::sync::Once = std::sync::Once::new();

fn motion_key() -> Id {
    Id::new("kangae.motion.mode")
}

/// reduce motion が効いているか（skeleton の shimmer を止める所などの bool の読み手）。
/// 契約（UI-6a）: `motion_mode(ctx) != MotionMode::Full` の写し（live = `motion_mode` の読み直しに従う）。
/// YC_WAVES の口の `NSWorkspace.accessibilityDisplayShouldReduceMotion`（objc2-app-kit を Cargo.toml に名指す）は採らない: operator の承認の無い crate を足さない。
/// Wave: UI-6a
pub fn reduce_motion(ctx: &Context) -> bool {
    motion_mode(ctx) != MotionMode::Full
}

/// 判定を上書きする（test・設定画面から）。
/// 契約（UI-6a）: `set_motion_mode(ctx, if on { MotionMode::Fade } else { MotionMode::Full })` と同じ。即時（Cut）が要る試験は `set_motion_mode(ctx, MotionMode::Cut)`（`tests/common::ctx(true)`）。
/// Wave: UI-6a
pub fn set_reduce_motion(ctx: &Context, on: bool) {
    set_motion_mode(ctx, if on { MotionMode::Fade } else { MotionMode::Full });
}

/// UI-6a: 今の動きの段。上書きが無ければ env `KANGAE_REDUCE_MOTION`（`detect` と同じ読み・1 / true → Fade・0 / false → Full）→ OS の設定（1 秒に 1 回まで読み直す = live）。
/// 解いた段は egui の `Style::animation_time` と `scroll_animation` にも写す（Fade と Cut は 0 / 無し = egui 自身の開閉と scroll も travel しない・段が替わった時だけ書く）。
/// 呼ぶ側の例: `if motion::motion_mode(ui.ctx()) == MotionMode::Full { dy = SLIDE_PX * (1.0 - p) }`。保証しない: `Cut` を返す事（上書きした時だけ）・OS を替えた瞬間に効く事（`REREAD` ＋ `OS_REREAD` = 3 秒まで遅れる）。
/// Wave: UI-6a
pub fn motion_mode(ctx: &Context) -> MotionMode {
    let now = ctx.input(|i| i.time);
    let mut m = ctx.data(|d| d.get_temp::<Motion>(motion_key())).unwrap_or_else(|| {
        let env = std::env::var("KANGAE_REDUCE_MOTION").ok();
        Motion { over: None, fixed: env_choice(env.as_deref()).is_some(), on: detect(env.as_deref(), os_reduce_motion_live), read_at: now, applied: None }
    });
    // 上書きも env も無い間だけ OS を読み直す（時刻が戻った試験でも 1 秒を数え直す）
    if m.over.is_none() && !m.fixed && (now - m.read_at).abs() >= REREAD {
        (m.on, m.read_at) = (os_reduce_motion_live(), now);
    }
    let mode = m.over.unwrap_or(if m.on { MotionMode::Fade } else { MotionMode::Full });
    if m.applied != Some(mode) {
        let (time, scroll) = match mode {
            MotionMode::Full => (egui::Style::default().animation_time, egui::style::ScrollAnimation::default()),
            MotionMode::Fade | MotionMode::Cut => (0.0, egui::style::ScrollAnimation::none()),
        };
        ctx.all_styles_mut(|s| (s.animation_time, s.scroll_animation) = (time, scroll));
        m.applied = Some(mode);
    }
    ctx.data_mut(|d| d.insert_temp(motion_key(), m));
    mode
}

/// UI-6a: 動きの段を上書きする（試験・設定画面）。上書きした段は OS の読み直しで戻らない。
/// 呼ぶ側の例（試験）: `motion::set_motion_mode(&ctx, MotionMode::Cut)`（1 frame で終点）・`MotionMode::Fade`（Reduce Motion の短い fade を測る）。
/// Wave: UI-6a
pub fn set_motion_mode(ctx: &Context, mode: MotionMode) {
    ctx.data_mut(|d| {
        let m = d.get_temp::<Motion>(motion_key()).unwrap_or(Motion { over: None, fixed: false, on: false, read_at: 0.0, applied: None });
        d.insert_temp(motion_key(), Motion { over: Some(mode), ..m });
    });
}

/// RQ1（RQ-002・`model::UiSettings::reduce_motion` が None = 「OS に従う」）: `set_motion_mode` / `set_reduce_motion` の上書きを外し、env `KANGAE_REDUCE_MOTION` →
/// OS の設定の読みへ戻す（次の `motion_mode` で OS を読み直す）。呼ぶ側の例（app.rs の Settings の当て直し）:
/// `match store.ui.reduce_motion { Some(on) => motion::set_reduce_motion(ctx, on), None => motion::clear_motion_mode(ctx) }`。
/// 保証する: 上書きが無い時は何も変えない・env が決めた段（`fixed`）はそのまま。保証しない: OS を替えた瞬間に効く事（`REREAD` の 1 秒まで遅れる）。
/// Wave: RQ1
pub fn clear_motion_mode(ctx: &Context) {
    // 記録ごと外す → 次の `motion_mode` が env → OS を初めからと同じ順で読む（env の段も読み直しで戻る）
    ctx.data_mut(|d| {
        if d.get_temp::<Motion>(motion_key()).is_some_and(|m| m.over.is_some()) {
            d.remove::<Motion>(motion_key());
        }
    });
}

/// env が `1`/`true` なら on・`0`/`false` なら off（OS を読まない）・それ以外は OS の設定。
pub fn detect(env: Option<&str>, os: impl FnOnce() -> bool) -> bool {
    env_choice(env).unwrap_or_else(os)
}

/// env の値が決める段（`1`/`true` → Some(true)・`0`/`false` → Some(false)・他 → None = OS に任せる）。
fn env_choice(env: Option<&str>) -> Option<bool> {
    match env.map(str::trim) {
        Some("1") | Some("true") => Some(true),
        Some("0") | Some("false") => Some(false),
        _ => None,
    }
}

/// G97: OS の「視差効果を減らす」の最後に読めた値。最初に呼んだ thread で 1 回だけ読み（起動の最初の frame から正しい段）、
/// 以後は裏の thread が `OS_REDUCE` を `OS_REREAD` ごとに読み直す（UI の thread は process を待たない）。
fn os_reduce_motion_live() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    OS_WATCH.call_once(|| {
        OS_REDUCE.store(os_reduce_motion(), Relaxed);
        let _ = std::thread::Builder::new().name("reduce-motion".into()).spawn(|| loop {
            std::thread::sleep(OS_REREAD);
            OS_REDUCE.store(os_reduce_motion(), Relaxed);
        });
    });
    OS_REDUCE.load(Relaxed)
}

thread_local! {
    /// この thread で `defaults` を起こした回数（試験が UI の thread で数える・`#[cfg(test)]` にすると interfaces.py がこの後ろを読まない）
    #[cfg_attr(not(test), allow(dead_code))]
    static OS_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// macOS の「視差効果を減らす」。key が無い（一度も触っていない）/ 読めない時は off。process を起こして待つので UI の thread からは呼ばない（`os_reduce_motion_live`）。
#[cfg(target_os = "macos")]
fn os_reduce_motion() -> bool {
    OS_READS.with(|c| c.set(c.get() + 1));
    std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false)
}
#[cfg(not(target_os = "macos"))]
fn os_reduce_motion() -> bool {
    false
}

/// 時間の定数を今の段に合わせた物。描く側は定数を直に使わずこれを通す。
/// 契約（UI-6a = 答え ①）: Full は `base`・Fade（Reduce Motion）は `base.min(REDUCE_FADE)`（0 にしない）・Cut（試験だけ）は 0。
/// 不透明度と色の寄せの時間（位置と大きさは `travel`・`slide_in` が Fade で動かさない）。引数は const（数の literal を渡さない・門 `a6_duration_token`）。
/// 保証しない: timer の長さ（`HOLD_*` / `DELAY_*` を渡さない・門 `a6_timer_not_dur`）。
/// Wave: UI-6a
pub fn dur(ctx: &Context, base: f32) -> f32 {
    match motion_mode(ctx) {
        MotionMode::Full => base,
        MotionMode::Fade => base.min(REDUCE_FADE),
        MotionMode::Cut => 0.0,
    }
}

/// list の `index` 行目を出し始めるまでの遅れ（秒）。`fade_slide_in` の `t0` に足す。
/// 契約（UI-6a）: Fade と Cut では 0（遅れは並びの時間で動きではない・FINAL の cut）。`dur(STAGGER)` が Fade で 30 に戻らないよう段を直に見る。
/// Wave: UI-6a
pub fn stagger(ctx: &Context, index: usize) -> f32 {
    match motion_mode(ctx) {
        MotionMode::Full => STAGGER * index as f32,
        MotionMode::Fade | MotionMode::Cut => 0.0,
    }
}

fn progress_at(now: f64, t0: f64, duration: f32) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    (((now - t0) / duration as f64).clamp(0.0, 1.0)) as f32
}

/// `t0`（ctx の time・秒）から `duration` の進み 0..=1（線形）。1 未満の間だけ repaint を頼む（`t0` が未来でも頼む）。
pub fn progress(ctx: &Context, t0: f64, duration: f32) -> f32 {
    let p = progress_at(ctx.input(|i| i.time), t0, duration);
    if p < 1.0 {
        ctx.request_repaint();
    }
    p
}

/// 中身を fade（opacity 0→1）しながら 8px 下から上げて出す（画面の切替・行の stagger）。`t0` は出し始める時刻（`ui.input(|i| i.time)` + `stagger`）。
/// 中身の id は `id` で固定する（位置がずれても widget の状態が切れない）。
/// 契約（UI-6a）: Fade では上げない（`dy` = 0・rect の y は最初の frame から終わりの値）・不透明度だけ `dur(PAGE)`（= `REDUCE_FADE`）で 0 → 1。
/// 中の `slide_in` が `motion_mode(` を見る（門 `a1_reduce_fade` の「位置を動かす fn は段を見る」）。`toast_in` も同じ（受け取りへの作り直しは UI-6b）。
/// Wave: UI-6a
pub fn fade_slide_in<R>(ui: &mut Ui, id: impl Hash, t0: f64, add: impl FnOnce(&mut Ui) -> R) -> R {
    slide_in(ui, id, t0, PAGE, add)
}

/// 知らせの 1 行を下から slide + fade で出す（toast・240ms）。
pub fn toast_in<R>(ui: &mut Ui, id: impl Hash, t0: f64, add: impl FnOnce(&mut Ui) -> R) -> R {
    slide_in(ui, id, t0, TOAST, add)
}

fn slide_in<R>(ui: &mut Ui, id: impl Hash, t0: f64, base: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = ease_out(progress(ui.ctx(), t0, dur(ui.ctx(), base)));
    // Fade・Cut は位置を動かさない（不透明度だけ寄せる = 答え ①）
    let dy = if motion_mode(ui.ctx()) == MotionMode::Full { SLIDE_PX * (1.0 - p) } else { 0.0 };
    let rect = ui.available_rect_before_wrap().translate(Vec2::new(0.0, dy));
    let mut child = ui.new_child(UiBuilder::new().id_salt(id).max_rect(rect).layout(*ui.layout()));
    child.multiply_opacity(p);
    let r = add(&mut child);
    ui.advance_cursor_after_rect(child.min_rect().translate(Vec2::new(0.0, -dy)));
    r
}

#[derive(Clone, Copy, PartialEq)]
struct Tween<T> {
    from: T,
    to: T,
    t0: f64,
}

/// 目標が変わった瞬間に「今見えている値」から新しい目標へ ease-out で寄せる。止まっている間は repaint を頼まない。
fn tween<T>(ctx: &Context, id: Id, target: T, duration: f32, lerp: fn(T, T, f32) -> T) -> T
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    let now = ctx.input(|i| i.time);
    let at = |tw: Tween<T>| lerp(tw.from, tw.to, ease_out(progress_at(now, tw.t0, duration)));
    let tw = match ctx.data(|d| d.get_temp::<Tween<T>>(id)) {
        // 初めて見た値は動かさない（t0 を遠い過去に置く = 進み 1）
        None => Tween { from: target, to: target, t0: f64::NEG_INFINITY },
        Some(tw) if tw.to == target => tw,
        Some(tw) => Tween { from: at(tw), to: target, t0: now },
    };
    ctx.data_mut(|d| d.insert_temp(id, tw));
    lerp(tw.from, tw.to, ease_out(progress(ctx, tw.t0, duration)))
}

/// 値を `target` へ `base`（の reduce motion 後の時間）で ease-out で寄せる。開閉（rail = 0 / 1・sidebar の幅）に使う。
/// 目標が変わった瞬間から寄せ始め、初めて見た値は動かさない。
/// 契約（UI-6a）: 「薄れる量」（不透明度・色の混ぜ・数の表示）の寄せ = `dur` の時間（Fade は `REDUCE_FADE` まで）。
/// 位置と大きさ（rail と sidebar の幅）は `travel`（Fade で幅が 120ms かけて寄ると travel になる）。
/// Wave: UI-6a
pub fn animate(ctx: &Context, id: Id, target: f32, base: f32) -> f32 {
    tween(ctx, id, target, dur(ctx, base), |a, b, t| a + (b - a) * t)
}

/// UI-6a: 「動く量」（位置・幅・大きさ）を `target` へ寄せる。Full は `animate` と同じ（`base` で ease-out）、Fade と Cut は即時に `target`（rect が動かない）。
/// 呼ぶ側の例（sidebar・rail の開閉）: `let w = motion::travel(ctx, id, open_w, motion::RAIL); let a = motion::animate(ctx, id.with("fade"), open as u8 as f32, motion::RAIL);`
/// （幅は Fade で即時・中身の不透明度は `REDUCE_FADE` で寄る = Done 9）。保証しない: 呼ぶ側が幅と不透明度を別の id で寄せる事。
/// Wave: UI-6a
pub fn travel(ctx: &Context, id: Id, target: f32, base: f32) -> f32 {
    if motion_mode(ctx) == MotionMode::Full {
        return animate(ctx, id, target, base);
    }
    // 即時: 寄せ終わった記録として目標を覚える（Full に戻った最初の frame で目標が替わっても今の値から滑る = Done 9・古い from からは滑らない）
    ctx.data_mut(|d| d.insert_temp(id, Tween { from: target, to: target, t0: f64::NEG_INFINITY }));
    target
}

/// `key` が前の frame と違えば今の時刻、同じなら前に替わった時刻を返す（View の切替・知らせの 1 行の出し始め）。
/// 初めて見た key は遠い過去 = animation しない（起動直後の画面は殻の骨格の cross-fade が受け持つ）。
pub fn changed_at(ctx: &Context, id: Id, key: impl Hash) -> f64 {
    let key = Id::new(key);
    let now = ctx.input(|i| i.time);
    let at = match ctx.data(|d| d.get_temp::<(Id, f64)>(id)) {
        Some((k, at)) if k == key => at,
        Some(_) => now,
        None => f64::NEG_INFINITY,
    };
    ctx.data_mut(|d| d.insert_temp(id, (key, at)));
    at
}

/// `changed_at` が最後に返した時刻（key を知らない側が読む）。まだ無ければ遠い過去。
pub fn last_changed(ctx: &Context, id: Id) -> f64 {
    ctx.data(|d| d.get_temp::<(Id, f64)>(id)).map_or(f64::NEG_INFINITY, |(_, at)| at)
}

/// 数値の表示を目標へ 300ms で寄せる（件数・残り時間など）。表示する値を返す。
/// 目標が有限でない（0 除算の NaN・inf）間は寄せずにそのまま返し、tween の状態を消す。NaN は NaN と等しくないので
/// 寄せると毎 frame 動き直して repaint が止まらず、inf は inf × 0 で NaN を出す。有限に戻ったらその値から（動かさない）。
pub fn animated_number(ctx: &Context, id: Id, value: f32) -> f32 {
    if !value.is_finite() {
        ctx.data_mut(|d| d.remove::<Tween<f32>>(id));
        return value;
    }
    animate(ctx, id, value, NUMBER)
}

/// 状態の色の切替を cross-fade する。`color` が変わった時だけ前の色から寄せる。描く色を返す。
/// 契約（UI-6a）: 時間は `dur(ctx, DIFF)`（160・M18）。Fade は `REDUCE_FADE`（120）・Cut は即時。
/// Wave: UI-6a
pub fn cross_fade(ctx: &Context, id: Id, color: Color32) -> Color32 {
    tween(ctx, id, color, dur(ctx, DIFF), |a, b, t| a.lerp_to_gamma(b, t))
}

#[cfg(test)]
mod rq1_tests {
    use super::*;

    /// RQ1: Settings の Reduce motion を「OS に従う」へ戻すと、上書き（Cut は上書きでしか出ない）が外れて env → OS の読みに戻る。上書きが無い時は何も変えない。
    /// 負の対照: `clear_motion_mode` の `remove` を外す → Cut のまま = 赤。
    #[test]
    fn rq1_clear_motion_mode_goes_back_to_env_or_os() {
        let ctx = Context::default();
        let read = || if detect(std::env::var("KANGAE_REDUCE_MOTION").ok().as_deref(), os_reduce_motion_live) { MotionMode::Fade } else { MotionMode::Full };
        set_motion_mode(&ctx, MotionMode::Cut);
        assert_eq!(motion_mode(&ctx), MotionMode::Cut);
        clear_motion_mode(&ctx);
        assert_eq!(motion_mode(&ctx), read(), "上書きが外れない");
        clear_motion_mode(&ctx); // 上書きが無い時は記録を残す
        assert!(ctx.data(|d| d.get_temp::<Motion>(motion_key())).is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// G97（AUDIT2 D）: UI の thread は OS の設定を読む process（`defaults read`）を毎秒待たない（裏の thread が読み、UI は最後の値を見るだけ）。
    /// 負の対照: `motion_mode` の読み直しを `os_reduce_motion()` に戻す → 赤（1.5 秒ごとの frame のたびに読む）。
    #[test]
    fn g97_the_ui_thread_does_not_run_defaults_every_second() {
        assert!(std::env::var_os("KANGAE_REDUCE_MOTION").is_none(), "前提: env が段を決めると OS を読まない（試験が測らない）");
        let ctx = Context::default();
        let before = OS_READS.with(|c| c.get());
        for i in 0..5 {
            let _ = ctx.run(egui::RawInput { time: Some(i as f64 * 1.5), ..Default::default() }, |ctx| {
                motion_mode(ctx);
            });
        }
        let reads = OS_READS.with(|c| c.get()) - before;
        assert!(reads <= 1, "UI の thread で {reads} 回 defaults を待った");
    }
}
