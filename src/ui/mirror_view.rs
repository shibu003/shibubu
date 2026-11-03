//! W16M-1: Page の Mirror の矩形（`SurfaceSpec{backend: Mirror}`）に、後ろの元の app の窓の映像を egui の texture で描く。
//! feature `mirror` の時だけ compile する（W15 の .app では切る）。入力の転送（W16M-2）: `mirror_inputs` が egui の入力を `MirrorInput` に直し、
//! `MirrorPages::relay` が面ごとの lease で `relay_human` に渡す（拒否は面の Err にせず 1 行）。
//! 呼ぶ所: `ui::renderer::components::source_surface` が node の `props.surface_ref` を `Cx::surfaces.specs`（Project の `surface_specs`）で引き、
//! backend が Mirror の時だけ `MirrorPages::show` を呼ぶ。`MirrorPages` は `Generated` が 1 つ持つ（同意は ui_state に保存）。
//! 図は docs/diagrams.md §19 の「W16M-1 の映像」。
//! Wave: W16M-1, W16M-1b, W16M-2, W16M-3

use crate::domain::project::{SurfaceBackendKind, SurfaceFrame, SurfaceSpec, SurfaceTarget};
use crate::surface::mirror::{consent_button_id, frame_log_path, frame_point_to_window, percentiles, relay_log_path, window_point_to_frame, ActivityHold, ConsentGate, ConsentRecord, FrameLog, FrameRecord, IoSurface, KeyMod, Launcher, MirrorError, MirrorInput, MirrorSurface, MirrorTarget, RelayLog, RelayRecord, FRAME_SCHEMA_V};
use crate::surface::{AppIdentity, Bridge, ControlLease, Dock, Frame, Host, Lifecycle, MirrorFrameMeta, Msg, PortalState};
use crate::ui::portal_frame::{slot_on_screen, PortalAction};
use crate::ui::states::RegionState;
use crate::ui::{motion, states, theme};
use eframe::egui::{self, accesskit, pos2, Color32, ColorImage, Event, PointerButton, Pos2, Rect, Sense, TextureHandle, TextureOptions, UiBuilder, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

/// G97: helper の読み手の thread から UI を起こす（`MirrorSurface::wake_with`）。MIRROR-SPEED-1101: 着いたらすぐ描き直す（前は 33ms にまとめた = 30 枚 / 秒が上限）。
/// 続けて着いた物は egui が次の 1 frame にまとめる・鳴るのは frame と出来事が着いた時だけ（背景で回り続けない = G97 の AUDIT2 D のまま）
struct Repaint(egui::Context);
impl Wake for Repaint {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }
}

/// MIRROR-SPEED-1101: wgpu の描画の口（app.rs が起動の 1 回 `set_gpu` で置く）。無い = 試験・wgpu でない起動 = IOSurface の frame は CPU で写して描く
static GPU: std::sync::OnceLock<eframe::egui_wgpu::RenderState> = std::sync::OnceLock::new();

/// MIRROR-SPEED-1101（owner 205「他に滑らかに普通のappみたいに表示させる方法ないの？」）: helper が渡した IOSurface を、写さずに GPU の texture として描く口を置く。
/// 例（app.rs の `open`）: `if let Some(rs) = &cc.wgpu_render_state { mirror_view::set_gpu(rs.clone()) }`
/// 保証する: 2 回目からは何もしない（窓は 1 つ）。保証しない: Metal が IOSurface の texture を作れる事（断ったら CPU で写す）。
/// Wave: MIRROR-SPEED-1101
pub fn set_gpu(rs: eframe::egui_wgpu::RenderState) {
    let _ = GPU.set(rs);
}

/// MIRROR-SPEED-1101: 面の映像の texture。`Handle` = egui が持つ texture（pipe の RGBA・IOSurface を CPU で写した物）・`Native` = IOSurface を
/// そのまま wgpu の texture にして egui に登録した物（写し 0 回・持ち主は `MirrorPages` の登録の表で、ここは借りた id）。
/// 新しい型の理由: `TextureHandle` は egui が作った texture しか持てず、wgpu の native texture（`TextureId::User`）を入れる所が無い。
enum Tex {
    Handle(TextureHandle),
    Native(egui::TextureId),
}

impl Tex {
    fn id(&self) -> egui::TextureId {
        match self {
            Tex::Handle(t) => t.id(),
            Tex::Native(id) => *id,
        }
    }
}

/// MIRROR-SPEED-1101: egui に登録した IOSurface の texture 1 つ（IOSurface の id ごと・SCK の pool の 3〜5 枚を使い回すので登録は最初の 1 回だけ）。
/// wgpu の texture と view を持つ（Metal の texture が IOSurface を retain する）・放す時は `free_texture`（native の texture は egui が数えない）
struct Native {
    id: egui::TextureId,
    used: f64,
    _texture: eframe::wgpu::Texture,
    _view: eframe::wgpu::TextureView,
}

/// MIRROR-SPEED-1101: IOSurface を wgpu の texture にして egui に登録した id（同じ IOSurface は登録済みの物）。
/// None = wgpu の口が無い（`set_gpu` の前・試験）か Metal が断った = 呼ぶ側は `IoSurface::to_rgba` で写す
#[cfg(target_os = "macos")]
fn native_texture(natives: &mut HashMap<u32, Native>, s: &IoSurface, now: f64) -> Option<egui::TextureId> {
    use eframe::wgpu::{self, hal};
    // 前提: 登録した Native の Metal の texture が IOSurface を retain する間は、その id を別の IOSurface が使わない（id で引けば同じ画素の入れ物・大きさも同じ）
    if let Some(n) = natives.get_mut(&s.id()) {
        n.used = now;
        return Some(n.id);
    }
    let rs = GPU.get()?;
    let (w, h) = s.size();
    // SAFETY: as_hal の device は wgpu の device の物で callback の中だけ使う（Metal の device から IOSurface の texture を作るだけ・壊さない）
    let raw = unsafe { rs.device.as_hal::<hal::api::Metal, _, _>(|d| d.and_then(|d| metal_texture(&d.raw_device().lock(), s.raw(), w, h))) }?;
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    // SAFETY: raw はこの device で作った 2D・BGRA8 sRGB・mip 1・層 1 の texture（下の desc と同じ）で、IOSurface の画素で初めから埋まっている
    let texture = unsafe {
        let hal_texture = hal::metal::Device::texture_from_raw(raw, format, metal::MTLTextureType::D2, 1, 1, hal::CopyExtent { width: w, height: h, depth: 1 });
        rs.device.create_texture_from_hal::<hal::api::Metal>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("mirror-iosurface"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let id = rs.renderer.write().register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear);
    natives.insert(s.id(), Native { id, used: now, _texture: texture, _view: view });
    Some(id)
}

#[cfg(not(target_os = "macos"))]
fn native_texture(_natives: &mut HashMap<u32, Native>, _s: &IoSurface, _now: f64) -> Option<egui::TextureId> {
    None
}

/// MIRROR-SPEED-1101: IOSurface（BGRA の 1 plane）をそのまま中身にする Metal の texture（`newTextureWithDescriptor:iosurface:plane:`・写さない）。断られたら None
#[cfg(target_os = "macos")]
fn metal_texture(device: &metal::DeviceRef, surface: *mut std::ffi::c_void, w: u32, h: u32) -> Option<metal::Texture> {
    use metal::foreign_types::{ForeignType, ForeignTypeRef};
    use std::ffi::{c_char, c_void};
    #[link(name = "objc")]
    extern "C" {
        fn objc_msgSend();
        fn sel_registerName(name: *const c_char) -> *mut c_void;
    }
    let desc = metal::TextureDescriptor::new();
    desc.set_texture_type(metal::MTLTextureType::D2);
    desc.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm_sRGB);
    desc.set_width(u64::from(w));
    desc.set_height(u64::from(h));
    desc.set_usage(metal::MTLTextureUsage::ShaderRead);
    // 理由: metal の crate はこの method を持たない。objc_msgSend を引数の型の C 関数として呼ぶ（surface/mod.rs の appkit と同じ形）。
    // new… は +1 で返る = from_ptr（retain しない）。storageMode は既定のまま（Intel の Managed は IOSurface に使える・Private だけ不可）
    // SAFETY: device と desc は生きている Metal の物・surface は use count を持つ IOSurfaceRef（呼ぶ側の IoSurface）
    unsafe {
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, usize) -> *mut c_void = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let t = send(device.as_ptr().cast(), sel_registerName(c"newTextureWithDescriptor:iosurface:plane:".as_ptr()), desc.as_ptr().cast(), surface, 0);
        (!t.is_null()).then(|| metal::Texture::from_ptr(t.cast()))
    }
}

/// MIRROR-SPEED-1101: 面が今描いていない・`NATIVE_KEEP_MS` 受け取っていない IOSurface の texture を放す（`end_frame`・大きさが替わった後の古い面・SCK の pool が替わった）
fn evict_natives(natives: &mut HashMap<u32, Native>, panes: &HashMap<String, Pane>, now: f64) {
    if natives.is_empty() {
        return;
    }
    let drawn: HashSet<egui::TextureId> = panes.values().filter_map(|p| match &p.texture { Some(Tex::Native(id)) => Some(*id), _ => None }).collect();
    natives.retain(|_, n| {
        let keep = drawn.contains(&n.id) || now - n.used < NATIVE_KEEP_MS;
        if let (false, Some(rs)) = (keep, GPU.get()) {
            rs.renderer.write().free_texture(&n.id);
        }
        keep
    });
}

/// MIRROR-SPEED-1101: 「撮った → 描いた」の ms を 1 分ためて、app.log（stderr）に中央値と 95% 点を 1 行（owner が次の見る用の build の画面のまま数字を読む）。
/// app.log = Finder / Dock から起こした .app では data の `app.log`（起動口 `scripts/release/build-mac.sh` が stderr の /dev/null を足し先に替える）・試験は `open --stderr` の file。
/// 面をまたいで 1 つ（Shibubu の Mirror 全体の遅れ）・GPU に載せた枚数と helper の今の枚数（stats の `fps`）も添える
#[derive(Default)]
struct Speed {
    ms: Vec<f64>,
    since: f64,
    gpu: usize,
    fps: Option<u64>,
}

impl Speed {
    fn add(&mut self, ms: f64, gpu: bool, now: f64) {
        if self.ms.is_empty() {
            self.since = now;
        }
        self.ms.push(ms);
        self.gpu += usize::from(gpu);
        if now - self.since < SPEED_LOG_MS {
            return;
        }
        let n = self.ms.len();
        if let Some((p50, p95)) = percentiles(&mut self.ms) {
            let fps = self.fps.map_or_else(|| "?".to_string(), |f| f.to_string());
            eprintln!(
                "MIRROR-SPEED-1101: Mirror の撮った → 描いた（{:.0} 秒・{n} 枚・GPU {} 枚・helper {fps} 枚/秒）: 中央値 {p50:.0} ms・95% 点 {p95:.0} ms",
                (now - self.since) / 1000.0,
                self.gpu
            );
        }
        self.ms.clear();
        self.gpu = 0;
    }
}

/// MIRROR-SPEED-1101: 描く大きさを helper に送り直すのは、替わってからこの間（ms）ごとまで（四隅を引く間に 1 px ごとに SCK を組み直さない）
const VIEW_GAP_MS: f64 = 200.0;
/// MIRROR-SPEED-1101: 登録した IOSurface の texture を、受け取らなくなってからこの間（ms）で放す（面が今描いている物は放さない）
const NATIVE_KEEP_MS: f64 = 2000.0;
/// MIRROR-SPEED-1101: 「撮った → 描いた」の 1 行を app.log に出す間（ms）
const SPEED_LOG_MS: f64 = 60_000.0;

/// 窓が無い・許可が無い時に、もう一度探すまでの間（毎 frame helper を起こさない）。
const RETRY_MS: f64 = 2000.0;
/// MIRROR-QUIET-1101（owner 208）: 探し始めてこの間（ms）を過ぎても窓が無ければ、探し直しを `RETRY_SLOW_MS` ごとに落とす
const RETRY_FAST_FOR_MS: f64 = 20_000.0;
const RETRY_SLOW_MS: f64 = 10_000.0;

/// MIRROR-QUIET-1101: 探し始めて `searching_ms` 経った面の、次に探すまでの間（20 秒までは 2 秒・その後 10 秒）。
/// app の起動の知らせ（bridge の app_launched / app_activated / app_unhidden）と Page が見えた時は、面の `tried_ms` を -∞ にして数え直す（すぐ探す）。
/// Wave: MIRROR-QUIET-1101
fn search_gap(searching_ms: f64) -> f64 {
    if searching_ms < RETRY_FAST_FOR_MS {
        RETRY_MS
    } else {
        RETRY_SLOW_MS
    }
}
/// W16M-2: 入力 1 つの拒否の 1 行を出しておく間。
const LINE_MS: f64 = 6000.0;
/// G82: 他の面の窓探しの答えを待つ上限。超えたら待たずに自分で探す（helper の `list` が混んで答えない間に「探しています」が続かない）。
const WAIT_OTHER_MS: f64 = 10_000.0;
/// W16M-2: 押した点と離した点がこれ以上（point）離れたら click ではなく drag。
const DRAG_MIN: f32 = 4.0;
/// G87: app を起こしてから窓が出るのを `Starting {App}…` で待つ上限。過ぎたら窓が無い（`NoWindow` と `Open {App}`）。
/// G90: 起こした app が前面を取ったら Shibubu に戻す見張りも同じ間（`open -g` でも自分で前に出る app・実機 05 の REAPER は約 3 秒・混んだ Mac では何十秒）。
/// この間に人が自分でその app を前に出しても 1 回戻る（Dock から開いた等）
/// ponytail: 固定の 60 秒（8 GB の Intel で REAPER の評価版の起動は何十秒）。app ごとに測った値が要るなら launcher の答えに足す
const LAUNCH_WAIT_MS: f64 = 60_000.0;
/// G90: Shibubu が前でない時に起こした app を見張る間、前面を読む間隔（`front_app` は lsappinfo を 2 回起こす = 毎 frame は読まない）
const FRONT_POLL_MS: f64 = 500.0;

/// G91（改）/ MIRROR-HYBRID-1101: 本物が前に居る間に、人の静けさ（`idle`）と前面を読む間（Shibubu に event が来ないので repaint を頼んで読む）。
/// G91 改の「hover 0.3 秒で前に出す・枠の外へ 0.3 秒離れたら戻す（AWAY_S）」は MIRROR-HYBRID-1101 でやめた（戻すのは被さり・scroll・移動・枠の外の click・
/// 何もしない・Page の切り替え）。追記 235 で前に出す方だけ `DWELL_S` で戻した（枠の外へ離れたら戻すは戻さない）
const DIRECT_POLL: Duration = Duration::from_millis(100);
/// MIRROR-HYBRID-1101 追記 235 の 2（owner「mirrorの中で操作するとdelayがある これ限りなく0にして」）: pointer が映像の上でこの秒止まったら、押す前に本物を前へ
/// （最初の押しから本物に直に届く）。10-27 の決まりの例外（Shibubu が前でない・Shibubu の欄に打っている・button を押している間）は出さない
const DWELL_S: f64 = 0.15;
/// 追記 235 の 2 の例外「Shibubu の欄に打っている」= 別の欄が keyboard の focus を持ち、この秒の内に打った（focus が残っているだけでは打っていない =
/// chat の欄は送った後も focus を持つので、focus だけで見ると止まった pointer で一度も出せない）
const TYPING_S: f64 = 1.0;
/// MIRROR-HYBRID-1101: 本物を触っている間に人が何もしない（どの app の keyboard / mouse も）この秒で、本物を Shibubu の下へ戻す（Mirror の絵のまま）
const IDLE_S: f64 = 10.0;
/// MIRROR-HYBRID-1101: Page の scroll・Shibubu の窓の移動で本物を下へ戻した後、枠がこの秒動かなければ（本物の位置を合わせ直した後）本物に戻す
const SETTLE_S: f64 = 0.3;
/// MIRROR-HYBRID-1101 追記 214（owner「触ると光るんだけど何も起きない」）: 前に出す手（bridge の raise + activate・本体の activate）を打ってから、本物が前に来る
/// （Shibubu が前でなくなる）まで待つ上限（ms・wall）。過ぎたら本物を下へ戻し、理由を右上に 1 行（光ったまま何もしない形を残さない）
// ponytail: 1 秒の固定。重い app（起動直後の GarageBand 等）で足りなければ伸ばす
const FRONT_WAIT_MS: f64 = 1000.0;
/// L5-STICK ②（owner 109 通目 10「Zoom の画面共有のように」）: Shibubu の窓を動かしてから本物が付いて来るまでの目安（ms）。超えた読み戻しだけ app.log に 1 行
const FOLLOW_MS: f64 = 100.0;
/// B90（owner 11-03「shibubuの中でorca開いているけどずっと揺れて作業に集中できないよ」「揺れたりすることは絶対なし。」）: 人が枠を動かした合図
/// （Shibubu の窓の移動・大きさ・Page の wheel・何かを引く・押す / 離す・⌘ の key・引いた大きさの答え）の後、本物を枠へ合わせ直す間（秒・ctx の time）。
/// この外で枠が替わっても（layout の揺れ・app が自分で替えた大きさ）本物は動かさない（触った時の `go_direct` が今の枠へ置いてから前に出す）
// ponytail: 1 秒の固定（panel・窓の形の animation を覆う）。もっと長い animation の途中で本物が止まれば、animation の間も合図に数える
const FOLLOW_S: f64 = 1.0;
/// G91 改 2: 本物が外れた（自分で大きさを替えた・読み戻しが枠と違う）時に自動で枠へ合わせ直す回数の上限（直接触った後の `put_back` で数え直す・
/// `Fit to frame` は数えない）。外れたのを見てから合わせ直すまでの間（秒・ctx の time）。app が大きさを拒む窓に毎 frame 送り続けない
const REFIT_MAX: u8 = 3;
const REFIT_GAP_S: f64 = 1.0;
/// MIRROR-FIT-1101（owner 200）: 四隅で引き終わった大きさを本物の読み戻しまで待つ上限（秒・過ぎたら枠は本物の大きさに戻る）
const RESIZE_WAIT_S: f64 = 2.0;
/// 選んだ枠の四隅の取っ手の見える大きさと、押せる大きさ（隅を中心に半分ずつ枠の外と中）
const HANDLE: f32 = 8.0;
const HANDLE_HIT: f32 = 16.0;
/// 四隅で引いて小さくできる映像の下限（point）
const MIN_VIDEO: Vec2 = Vec2::new(160.0, 100.0);
/// MIRROR-HYBRID-1101 追記 240 の 3（owner 242「右sidebarの枠を左に広げようとしたらbarと画面の間がうぬねして掴めない」）: 映像の縁の ±この point は Shibubu の境目
/// （右 sidebar・下の panel・四隅の取っ手）を掴みに行く所 = 止まった pointer でも押しでも本物を前に出さない・前に居る本物は pointer がここに来たら下へ
const EDGE: f32 = 6.0;
/// 追記 240 の 2: 本物の窓が頼んだ大きさより大きく残った差がこれを超えた向きだけ、その app の窓の最小と覚える（AX に窓の最小の大きさの公開の属性は無い =
/// 断られた読み戻しで学ぶ）。これ以下は app の刻み（文字の升目の丸め）と見て覚えない
// ponytail: 24 point の固定。刻みの大きい app で誤って覚えたら、窓ごとの刻みを読み戻しの差から数える
const LEAST_SLACK: f32 = 24.0;

/// UI-5m: 面の上の題の帯の高さ（point）。左に赤黄緑・残りに `{App} · {書類}`。映像はこの下だけ（帯の上の click・key は元の app へ送らない）。
/// Wave: UI-5m
pub const BAND_H: f32 = 28.0;
/// G93: 帯の右端に取る幅（確かめていない面の印 `○`・題はここまで）。
pub const STATUS_W: f32 = 24.0;

/// G93: 面の矩形 → 帯の右端の印の矩形（`renderer::surface_status` の `○` を置く所・映像の矩形の外）。
/// 例: `let slot = band_status_rect(rect);`
pub fn band_status_rect(rect: Rect) -> Rect {
    let (band, _) = split_band(rect);
    Rect::from_min_max(pos2(band.right() - STATUS_W, band.top()), band.max)
}

/// G93: 帯の題（と拒否の 1 行）の左端 = 緑の円の右 + 間。
fn title_left(band: Rect) -> f32 {
    band.left() + DOT_LEFT + DOT + 2.0 * (DOT + DOT_GAP) + DOT_GAP + theme::SP_1
}
/// UI-5m: 窓の形の保存の口 `ui_state(session_id = WINDOW_UI_STATE_SESSION, key = surface id)`（値 = `MirrorWindow` の serde の語）。
/// 同意（`CONSENT_UI_STATE_SESSION`）と同じ表で別の session（同意の読みは `ConsentRecord` だけを拾うので混ぜない）。
/// Wave: UI-5m
pub const WINDOW_UI_STATE_SESSION: &str = "mirror-window";

/// UI-5m: Shibubu の中の Mirror の面の窓の形（元の app の窓ではない・macOS の窓の赤黄緑と同じ 4 つ）。
/// 赤 = `Closed`（上の辺から畳んで帯だけ・helper の stream を止める）・黄 = `Minimized`（同じ畳み方・stream は保つ）・緑 = `Zoomed`（Page の中身いっぱい）↔ `Open`。
/// 帯を押す / Enter で `Open`（閉じた面は同意を尋ねずに窓を探し直す）。拡大中は Mirror に keyboard の focus が無い時だけ Esc でも `Open`。
/// 例: `pages.set_window("docs", MirrorWindow::Closed, ctx.input(|i| i.time))`・保存は `take_window_changes`・起動の時は `restore_windows`。
/// 保証する: 形は面（surface id）ごとに 1 つで、`Pane`（stream と texture）が消えても残る・保存した `Zoomed` は `Open` で戻す。
/// 保証しない: 元の app の窓の形（Shibubu は元の窓を閉じない・しまわない）。
/// Wave: UI-5m
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorWindow {
    #[default]
    Open,
    Minimized,
    Closed,
    Zoomed,
}

/// UI-5m: 面ごとの窓の形と animation（`MirrorPages::windows`）。keep_alive でない面の `Pane` は Page を離れると消えるので、形は Pane に置かない
/// （置くと戻った時に `Open` から始まり、閉じた面が stream を始める）。
#[derive(Clone, Copy)]
struct Shape {
    now: MirrorWindow,
    /// 前の形（拡大から戻る animation の間は Page の他の region を隠したまま）
    was: MirrorWindow,
    /// 形を替えた時刻（ctx の time・秒）。進みは `motion::progress(ctx, t0, shape_secs(ctx))`
    t0: f64,
    /// 替えた時に描いていた高さ（animation の出発点）
    from_h: f32,
    /// 前の frame に描いた高さ（0 = まだ描いていない）
    last_h: f32,
    /// 開いた形で最後に落ち着いた高さ（拡大から戻る先・拡大の間は他の region が隠れて幅が違うので、呼ぶ側の open の値と違う）
    open_h: Option<f32>,
    /// I32: 映像だけの拡大（`Lens`・面ごと・Page を移っても・窓の形を替えても残る）
    lens: Lens,
    /// I33b: 最後に題の帯で枠を動かし始めた時刻（ctx の time）。重なった枠は大きい方が上（`MirrorPages::floating` の順）・動かしていない = −∞
    lifted: f64,
}

impl Default for Shape {
    fn default() -> Shape {
        Shape { now: MirrorWindow::Open, was: MirrorWindow::Open, t0: f64::NEG_INFINITY, from_h: 0.0, last_h: 0.0, open_h: None, lens: Lens::default(), lifted: f64::NEG_INFINITY }
    }
}

/// UI-5m: 面の矩形 → 上の題の帯（`BAND_H`・矩形が低ければその高さまで）と、その下の映像の矩形。`show` の描く所・`fit`・`mirror_inputs`・
/// frame の AX の矩形は映像の矩形だけを使う（帯の上の click は元の app へ送らない）。
/// 例: `let (band, video) = split_band(rect); let inputs = mirror_inputs(&events, fit(video, &meta), &meta, focused);`
/// Wave: UI-5m
pub fn split_band(rect: Rect) -> (Rect, Rect) {
    let y = (rect.min.y + BAND_H).min(rect.max.y);
    (Rect::from_min_max(rect.min, pos2(rect.max.x, y)), Rect::from_min_max(pos2(rect.min.x, y), rect.max))
}

/// G91: 本物の窓が前に出て直接触っている間の帯の 1 行と frame の node の名前。MIRROR-HYBRID-1101: 触った最初の押しも本物に届く（`relay_front`）ので
/// 「もう一度 click」は無い
fn direct_line(app: &str) -> String {
    format!("Using {app} directly · click outside to go back")
}

/// MIRROR-HYBRID-1101 × 追記 214: 触った面の本物を前に出せない理由（右上の 1 行・押すたびに・Mirror のまま）。None = 出せる（Dock が枠に合っている）。
/// 本物が枠より大きい（縮めて見せている）は理由にしない = 呼ぶ側が本物を映像の大きさにしてから前に出す（`Pane::fit_touch`）。
/// AX に答えない / 別の Space に居る（Dock が合っていない）・置けない（窓の対応が付かない・許可が無い・bridge が起きない）・Dock を作っている途中の 3 つ
fn stay_reason(pane: &Pane, app: &str) -> Option<String> {
    match &pane.dock {
        Some(d) if !matches!(d.life, Lifecycle::Active | Lifecycle::Resizing) => Some(format!("{app}'s window isn't lined up with this frame · it stays a mirror")),
        Some(_) => None,
        None if pane.tuck_err.is_some() => Some(format!("Can't bring {app} here · it stays a mirror")),
        None => Some(format!("{app}'s window isn't ready here yet · try again in a moment")),
    }
}

/// I33b: 面 `surface_id` の映像 `video` に、この frame で上に描く枠（`MirrorPages::floating` が返した順で後ろの物 = `stack`）が重なっているか。
/// 重なっている間は本物を前に出さず、出ていれば Shibubu の下へ（`covered` と同じ扱い = 下の枠の本物が上の枠の映像の上に出ない）。
/// 例: `if covered(&ctx, ui.layer_id(), video) || above(&self.stack, &spec.id, video) { self.put_back(id) }`
fn above(later: &[(String, Rect)], surface_id: &str, video: Rect) -> bool {
    later.iter().position(|(id, _)| id == surface_id).is_some_and(|k| later[k + 1..].iter().any(|(_, r)| r.intersects(video)))
}

/// MIRROR-HYBRID-1101: 枠（`video`）の上に、この面の layer（`layer`）より上の物が被さっているか: 下の panel（`terminal_panel::covering`）・
/// menu・popup・toast・chat の Area（egui の Memory の見えている layer の矩形・前の frame の物）。被さっている間は本物を前に出さず、出ていれば Shibubu の下へ戻す。
/// 例: `if covered(&ctx, ui.layer_id(), video) { self.put_back(id) }`
/// Wave: MIRROR-HYBRID-1101
fn covered(ctx: &egui::Context, layer: egui::LayerId, video: Rect) -> bool {
    if crate::ui::terminal_panel::covering(ctx).is_some_and(|c| c.intersects(video)) {
        return true;
    }
    ctx.memory(|m| {
        // layer_ids は奥から手前の順。この面の layer より手前（order が上・同じ order なら後ろ）の見えている Area の矩形が映像に重なるか（Debug の層は数えない）
        let order: Vec<egui::LayerId> = m.layer_ids().collect();
        let mine = order.iter().position(|l| *l == layer);
        order.iter().enumerate().any(|(i, l)| {
            let above = l.order > layer.order || (l.order == layer.order && mine.is_some_and(|k| i > k));
            above && l.order != egui::Order::Debug && m.areas().is_visible(l) && m.area_rect(l.id).is_some_and(|r| r.intersects(video))
        })
    })
}

/// UI-5m: 3 つの button と畳んだ帯の AX の名前（docs/ui-copy/additions.md の「Mirror window buttons」）。
const AX_CLOSE: &str = "Close mirror";
const AX_MINIMIZE: &str = "Minimize mirror";
const AX_ZOOM: &str = "Zoom mirror";
const AX_RESTORE: &str = "Restore mirror";
/// G91 改 2: 本物が枠に合わない時だけ題の帯の右に出す button（docs/ui-copy/additions.md）
const AX_FIT: &str = "Fit to frame";
const AX_OPEN: &str = "Open mirror";
/// RQ4（RQ-018）: 帯の右の元の app を前に出す button（`PortalAction::ContinueInSource` の文字・新しい手を足さない）
const AX_ORIGINAL: &str = PortalAction::ContinueInSource.label();
/// RQ4（RQ-019）: 窓を選び直す menu の opener（帯の題と、窓が消えた時の次の一手）
const AX_CHOOSE: &str = "Choose window…";
/// UI-5m: macOS の窓の赤黄緑（塗りと 1 段暗い縁）・円の直径と間と帯の左からの位置（point）。Shibubu の窓が key でない時は 3 つとも theme の灰。
const TRAFFIC: [(Color32, Color32); 3] = [
    (Color32::from_rgb(0xFF, 0x5F, 0x57), Color32::from_rgb(0xE0, 0x44, 0x3E)),
    (Color32::from_rgb(0xFE, 0xBC, 0x2E), Color32::from_rgb(0xDE, 0xA1, 0x23)),
    (Color32::from_rgb(0x28, 0xC8, 0x40), Color32::from_rgb(0x1A, 0xAB, 0x29)),
];
const DOT: f32 = 12.0;
const DOT_GAP: f32 = 8.0;
const DOT_LEFT: f32 = 10.0;

/// UI-5m: 開いた面の題の帯（赤黄緑と `{App} · {書類}`）。押された button の行き先の形を返す（赤 = Closed・黄 = Minimized・緑 = Zoomed ↔ Open）。
/// glyph（× − と斜めの三角）は group に pointer が乗った時だけ・font に依らず painter で描く。
/// G91 改 2: `misfit` = 本物が枠に合わない時の hover の理由。在る時だけ右端に `Fit to frame`（Ghost）。2 つ目の値 = その button か題の double-click
/// （macOS の窓の題と同じ癖）で枠へ合わせ直す。
/// RQ4: 右端に `Open original`（いつも 1 つ・3 つ目の値 = 押した）・4 つ目の値 = 題の Response（押すと呼ぶ側が窓を選び直す menu を開く・AX は `Choose window… · {題}`）。
fn title_band(ui: &mut egui::Ui, band: Rect, ax: &str, title: &str, window: MirrorWindow, misfit: Option<&str>, t: &theme::Tokens) -> (Option<MirrorWindow>, bool, bool, egui::Response) {
    let p = ui.painter_at(band);
    p.rect_filled(band, 0.0, t.surface_1);
    p.hline(band.x_range(), band.bottom() - 0.5, theme::hairline(t));
    let key = ui.input(|i| i.focused);
    let zoomed = window == MirrorWindow::Zoomed;
    let buttons = [(MirrorWindow::Closed, AX_CLOSE), (MirrorWindow::Minimized, AX_MINIMIZE), if zoomed { (MirrorWindow::Open, AX_RESTORE) } else { (MirrorWindow::Zoomed, AX_ZOOM) }];
    let dot = |i: usize| Rect::from_center_size(pos2(band.left() + DOT_LEFT + DOT / 2.0 + i as f32 * (DOT + DOT_GAP), band.center().y), Vec2::splat(DOT));
    let hover = ui.rect_contains_pointer(dot(0).union(dot(2)).expand(DOT_GAP / 2.0));
    let mark = Color32::from_black_alpha(150);
    let mut pressed = None;
    for (i, (to, name)) in buttons.into_iter().enumerate() {
        let r = dot(i);
        let resp = ui.interact(r.expand(2.0), egui::Id::new((ax, "window", i)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
        let (fill, edge) = if key { TRAFFIC[i] } else { (t.surface_3, t.border_strong) };
        p.circle(r.center(), DOT / 2.0, fill, egui::Stroke::new(0.5, edge));
        if hover && key {
            let g = r.shrink(3.0);
            let line = egui::Stroke::new(1.2, mark);
            match i {
                0 => {
                    p.line_segment([g.left_top(), g.right_bottom()], line);
                    p.line_segment([g.right_top(), g.left_bottom()], line);
                }
                1 => {
                    p.line_segment([g.left_center(), g.right_center()], line);
                }
                // 拡大 = 角へ向く 2 つの三角・戻す = 中へ向く 2 つの三角
                _ => {
                    let (c, s) = (g.center(), g.width() * 0.55);
                    let tri = |a: Pos2, dx: f32, dy: f32| vec![a, a + Vec2::new(dx, 0.0), a + Vec2::new(0.0, dy)];
                    let (tl, br) = if zoomed { (tri(c - Vec2::splat(0.8), -s, -s), tri(c + Vec2::splat(0.8), s, s)) } else { (tri(g.left_top(), s, s), tri(g.right_bottom(), -s, -s)) };
                    p.add(egui::Shape::convex_polygon(tl, mark, egui::Stroke::NONE));
                    p.add(egui::Shape::convex_polygon(br, mark, egui::Stroke::NONE));
                }
            }
        }
        if resp.has_focus() {
            p.circle_stroke(r.center(), DOT / 2.0 + 2.0, egui::Stroke::new(2.0, t.focus_ring));
        }
        if resp.clicked() {
            pressed = Some(to);
        }
    }
    // G93: 右端の STATUS_W は確かめていない面の印 `○`（`band_status_rect`）・RQ4: その左に `Open original`（どの状態でも 1 つ・許可を拒まれた面でも押せる）・
    // その左に Fit to frame・題はその左まで
    let room = Rect::from_min_max(pos2(band.center().x, band.top()), pos2(band.right() - STATUS_W - theme::SP_2, band.bottom()));
    let mut c = ui.new_child(UiBuilder::new().id_salt((ax, "fit")).max_rect(room).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let original = theme::button_compact(&mut c, AX_ORIGINAL, theme::Variant::Ghost).clicked();
    let mut fit = misfit.is_some_and(|why| theme::button_compact(&mut c, AX_FIT, theme::Variant::Ghost).on_hover_text(why).clicked());
    let right = c.min_rect().left() - theme::SP_1;
    let x = title_left(band);
    // RQ4（RQ-019）: 題を押すと窓を選び直す menu（呼ぶ側の `theme::menu_popup`）。macOS の窓の題と同じ癖: 題の double-click で枠へ合わせ直す（合っている面では呼ぶ側が何もしない）
    // I33: 引くと枠ごと動かす（`MirrorPages::handles`）。引かずに離せば今まで通り menu（egui の click は drag の閾値を越えない押しだけ）
    let menu = ui.interact(Rect::from_min_max(pos2(x, band.top()), pos2(right.max(x), band.bottom())), egui::Id::new((ax, "title")), Sense::click_and_drag());
    menu.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if title.is_empty() { AX_CHOOSE.to_string() } else { format!("{AX_CHOOSE} · {title}") }));
    fit |= menu.double_clicked();
    let galley = theme::truncated(ui, title, theme::Type::Small.font(), t.text_secondary, right - x);
    p.galley(pos2(x, band.center().y - galley.size().y / 2.0), galley, t.text_secondary);
    (pressed, fit, original, menu)
}

/// MIRROR-FRAME-1101: 窓の題を保存する題の語に（`SurfaceTarget::App::file` は `/` を含むと Project の repo の書類の path と読まれる = `show` の `want`）。
/// `/` を含む題は `/` の無い一番長い所・前後の空白を落とす。空なら None（= app の前の窓）。
/// 例: `title_word("owner/repo: Fix it")` = `Some("repo: Fix it")`・`title_word("notes.md")` = `Some("notes.md")`
fn title_word(title: &str) -> Option<String> {
    let best = title.split('/').map(str::trim).fold("", |a, b| if b.chars().count() > a.chars().count() { b } else { a });
    (!best.is_empty()).then(|| best.to_owned())
}

/// MIRROR-FRAME-1101: 空の枠の題・AX の名前・真ん中の button・その下の 1 行・窓の一覧の字の欄・最後の行（docs/ui-copy/additions.md）
const AX_FRAME: &str = "Mirror frame";
const AX_PICK: &str = "Choose a window";
const AX_DRAG_HINT: &str = "Or drag a window's title bar here";
const AX_FILTER: &str = "Filter windows";
const AX_OPEN_APP: &str = "Open an app…";
/// MIRROR-FRAME-1101: 窓の一覧の幅・見える高さ（それより多い行は scroll）・1 行の高さ・行の左の小さな絵の大きさ（point）
const PICK_W: f32 = 360.0;
const PICK_H: f32 = 320.0;
const PICK_ROW_H: f32 = 44.0;
const THUMB: Vec2 = Vec2::new(56.0, 36.0);
/// MIRROR-FRAME-1101: 小さな絵を撮る幅（pixel = `THUMB` の幅の 2 倍・Retina でぼやけない）
const SHOT_PX: u32 = 112;

/// MIRROR-FRAME-1101: 窓の一覧の 1 行（左から小さな絵〈無ければ灰の地〉・app の icon・`{App} · {題}`）。AX = Button `{App} · {題}`（題が空なら `{App}`）。
fn window_row(ui: &mut egui::Ui, w: &MirrorTarget, title: &str, shot: Option<TextureHandle>, t: &theme::Tokens) -> egui::Response {
    let name = if title.trim().is_empty() { w.app_name.clone() } else { format!("{} · {}", w.app_name, title.trim()) };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), PICK_ROW_H), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    let p = ui.painter_at(rect);
    if resp.hovered() || resp.has_focus() {
        p.rect_filled(rect, egui::CornerRadius::same(theme::RADIUS_SM), t.surface_3);
    }
    let thumb = Rect::from_min_size(pos2(rect.left() + theme::SP_2, rect.center().y - THUMB.y / 2.0), THUMB);
    match shot {
        // 縦横比のまま小さな絵の枠に収める
        Some(tex) => {
            let s = tex.size_vec2();
            let k = (THUMB.x / s.x.max(1.0)).min(THUMB.y / s.y.max(1.0));
            p.image(tex.id(), Rect::from_center_size(thumb.center(), s * k), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.rect_filled(thumb, egui::CornerRadius::same(theme::RADIUS_SM), t.surface_2);
        }
    }
    let icon = pos2(thumb.right() + theme::SP_2 + theme::ICON_UI / 2.0, rect.center().y);
    // 窓の app の icon（`theme::app_icon`・読めない app は無し = 右の名前が言う）。kind_icon は PROVIDER-ICONS 640a49e1 で引数が替わるので使わない
    if let Some(tex) = theme::app_icon(ui.ctx(), &w.bundle_id) {
        p.image(tex.id(), Rect::from_center_size(icon, Vec2::splat(theme::ICON_UI)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let x = icon.x + theme::ICON_UI / 2.0 + theme::SP_2;
    let galley = theme::truncated(ui, &name, theme::Type::Body.font(), t.text_primary, rect.right() - theme::SP_2 - x);
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, t.text_primary);
    if resp.has_focus() {
        p.rect_stroke(rect, egui::CornerRadius::same(theme::RADIUS_SM), egui::Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Inside);
    }
    resp
}

/// MIRROR-FRAME-1101 (b): 離した所 `at`（画面の point）で、引いて枠に重ねた窓（`windows` = `MirrorTarget::windows` の前から後ろの順・`me` = Shibubu の pid）。
/// pointer の下の一番前の窓が Shibubu でなく・`at` がその窓の題の帯（上から `TITLE_BAND`）の中で・pointer の下でその次の窓が Shibubu の時だけ Some（題と一緒に）。
/// 枠に別の app の窓が被っている（見えない枠に落とした）・本文の中の drag・Shibubu が前 = None。
fn dropped_window(windows: &[(MirrorTarget, String, [f64; 4])], at: [f64; 2], me: i64) -> Option<(MirrorTarget, String)> {
    let [x, y] = at;
    let mut under = windows.iter().filter(|(_, _, [wx, wy, ww, wh])| x >= *wx && x <= wx + ww && y >= *wy && y <= wy + wh);
    let (w, title, [_, top, _, _]) = under.next()?;
    let (next, _, _) = under.next()?;
    (w.pid != me && y - top <= TITLE_BAND && next.pid == me).then(|| (w.clone(), title.clone()))
}

/// MIRROR-FRAME-1101 (b): 題の帯の高さ（窓の上から・point）。Safari の toolbar と tab の列（約 80）まで持てる・本文の drag は入れない
const TITLE_BAND: f64 = 80.0;
/// MIRROR-FRAME-1101 (b): 押した所からこれ以上動いて初めて「引いている」（click を引いたと読まない・point）
const FRAME_DRAG_MIN: f64 = 24.0;
/// MIRROR-FRAME-1101 (b): 押したまま枠の上に来た時の帯の 1 行（docs/ui-copy/additions.md）
const AX_RELEASE: &str = "Release to mirror this window";
/// MIRROR-FRAME-1101 (b): Shibubu が前でない間に `watch_drag` が button を読む間（押した所は最初に起きた frame の所 = これより速い引きは押した所がずれる）
const DRAG_POLL: Duration = Duration::from_millis(100);

/// MIRROR-FRAME-1101 (b): Shibubu が前でない間、空の枠へ窓を引いてくる手を待つ裏の thread（W2b-AX: main thread に idle の frame を頼まない・main.rs の契約）。
/// `DRAG_POLL` ごとに `drag_step` で決め、押している間と離した時だけ egui を起こす（押していない間・空の枠を描かなくなった後の frame は 0）。
/// 返す印 = 空の枠を前でない間に描いた pass + 1（0 = まだ）。`MirrorPages` が印を落としたら（strong count 1）終わる
fn watch_drag(ctx: egui::Context, pointer: fn() -> (Option<[f64; 2]>, bool)) -> Arc<AtomicU64> {
    let armed = Arc::new(AtomicU64::new(0));
    let mine = armed.clone();
    let _ = std::thread::Builder::new().name("mirror-frame-drag".into()).spawn(move || {
        let mut held = false;
        while Arc::strong_count(&mine) > 1 {
            std::thread::sleep(DRAG_POLL);
            let (wake, down) = drag_step(mine.load(Ordering::Relaxed), ctx.cumulative_pass_nr(), || pointer().1, held);
            held = down;
            if wake {
                ctx.request_repaint();
            }
        }
    });
    armed
}

/// `watch_drag` の 1 回分 → (egui を起こすか, 次の held)。`armed` = 空の枠を前でない間に描いた pass + 1（0 = まだ）・`pass` = 今の
/// `cumulative_pass_nr`。その後に 1 pass でも空の枠を描かずに進んだ（Page を替えた・Shibubu が前に来た）なら button も読まない
fn drag_step(armed: u64, pass: u64, read: impl FnOnce() -> bool, held: bool) -> (bool, bool) {
    let down = armed != 0 && pass <= armed && read();
    (down || held, down)
}

/// RQ4: 元の app（と書類）を開く製品の口。`background` = `open -g`（窓の消えた面の `Open {App}`・前面を取らない）・false = 前に出す（帯の `Open original`）。
/// 追記 245: 書類は隠さずに開く（`-j` で開いた書類の窓は隠しを解く道〈`Pane::lost` は launcher で起こした時だけ〉が無く、`Show {App}` を押すまで映らない）。
/// Err = `open` の言った理由の最後の 1 行。
fn open_app_window(bundle_id: &str, doc: Option<&Path>, background: bool) -> Result<(), String> {
    let mut cmd = std::process::Command::new("/usr/bin/open");
    if background {
        cmd.arg("-g");
    }
    cmd.args(["-b", bundle_id]);
    if let Some(d) = doc {
        cmd.arg(d);
    }
    let out = cmd.stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(err.lines().map(str::trim).filter(|l| !l.is_empty()).last().map_or_else(|| format!("open ended with {}", out.status), str::to_string))
}

/// UI-5m: 畳んだ面の帯（全部が 1 つの Button `Open mirror · {App}`・押す / Enter で開く）。右端に開く向きの小さな三角。
fn folded_band(ui: &mut egui::Ui, band: Rect, ax: &str, title: &str, app: &str, t: &theme::Tokens) -> egui::Response {
    let resp = ui.interact(band, egui::Id::new((ax, "open")), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{AX_OPEN} · {app}")));
    let p = ui.painter_at(band);
    let fill = if resp.is_pointer_button_down_on() { t.surface_press } else if resp.hovered() { t.surface_3 } else { t.surface_1 };
    p.rect(band, egui::CornerRadius::same(theme::RADIUS_SM), fill, theme::hairline(t), egui::StrokeKind::Inside);
    let c = pos2(band.right() - theme::SP_4, band.center().y);
    p.add(egui::Shape::convex_polygon(vec![c + Vec2::new(-4.0, 2.0), c + Vec2::new(4.0, 2.0), c + Vec2::new(0.0, -3.0)], t.text_secondary, egui::Stroke::NONE));
    let x = band.left() + theme::SP_3;
    let galley = theme::truncated(ui, title, theme::Type::Small.font(), t.text_secondary, band.right() - x - theme::SP_7);
    p.galley(pos2(x, band.center().y - galley.size().y / 2.0), galley, t.text_secondary);
    if resp.has_focus() {
        p.rect_stroke(band, egui::CornerRadius::same(theme::RADIUS_SM), egui::Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Inside);
    }
    resp
}

/// Page の Mirror の面の全部（surface ごとの stream・texture・app ごとの同意）。殻が 1 つ持ち、`keep_alive` の面は Page を移っても捨てない
/// （`keep_alive` が false の面は、描かなかった frame の終わりで閉じる）。
/// MIRROR-HYBRID-1101 追記 245（owner「shibubu開いた時に全てのappのmirrorしている中身を開くのはやめよう」）: 映すのは今描いている Page の面だけ。
/// 描かなかった面は keep_alive でも stream を止める（面・Dock・人が選んだ大きさは残す・本物の app は閉じない）。戻った frame で探し直す。
/// 例: `let shown = pages.show(ui, rect, &spec); for r in pages.take_granted() { store.put_ui_state(CONSENT_UI_STATE_SESSION, &r.bundle_id, &serde_json::to_value(&r)?)? }`
/// 保証する: 同意（`ConsentGate`）の無い app の窓は open しない（stream も relay も 0 件・W16M-0）。映像の無い間は黒い面を出さず、状態の 1 行と骨格を出す（D-e）。
/// MIRROR-AUTO-1101 B（owner 192）: 面が Project に在る = その app を含む案を作った（Build now・chat で足した）時の同意。窓が見つかったら押さずに同意を記録して映す
/// （`take_granted` に出る = 再起動を跨ぐ）。`Mirror here` を待つのは Settings › Privacy で取り消した app（`restore_revoked`）だけ。
/// 保証しない: 映っている物が今の元の窓と同じ中身である事（試験の側の E0）・入力が届く事（W16M-2）。
pub struct MirrorPages {
    helper: PathBuf,
    gate: ConsentGate,
    panes: HashMap<String, Pane>,
    granted: Vec<ConsentRecord>,
    /// MIRROR-AUTO-1101 B: Privacy で取り消した app の bundle id（自動で同意しない・`Mirror here` を押すと外れる）
    revoked: HashSet<String>,
    /// W16M-1b: Mirror の Page を描いている間（`show` を呼んだ frame）で、開いている面が 1 つでも在る間だけ App Nap を止める。
    /// 決めるのは `end_frame`（Page を離れた frame で放す）
    awake: Option<ActivityHold>,
    /// この frame で `show` した面の id（`end_frame` が読んで戻す。keep_alive でない面を閉じる判定にも使う）
    drawn: HashSet<String>,
    /// W16M-1b: `KANGAE_MIRROR_FRAME_LOG` が在る時だけ、texture の更新ごとに 1 行
    frame_log: Option<FrameLog>,
    /// MIRROR-SPEED-1101: egui に登録した IOSurface の texture（IOSurface の id ごと・`end_frame` が描いていない古い物を放す）と、1 分ごとの遅れの 1 行
    natives: HashMap<u32, Native>,
    speed: Speed,
    /// W16M-3（窓の選び方）: Project の repo の dir（`SurfaceTarget::App.file` はこの dir からの相対）。在れば窓を書類の path で選ぶ（`MirrorTarget::find` の `want`）
    workspace: Option<PathBuf>,
    /// 元の app へ送った入力の記録の file（`relay_log_path()`。試験は面の dir に替える = 作業ツリーの `.shibubu-data` に書かない）
    relay_log: PathBuf,
    /// UI-5m: 面ごとの窓の形（Pane が消えても残る）と、保存を待つ替わった形（`take_window_changes`）
    windows: HashMap<String, Shape>,
    window_changes: Vec<(String, MirrorWindow)>,
    /// LAYOUT: Page の高さのための畳み（`fold_for_room`）の面ごとの (この足りない間に畳んだ, 今の黄は Shibubu が畳んだ物)。保存しない
    room: HashMap<String, (bool, bool)>,
    /// G87: 窓の無い app を起こす口（製品は `launch_app`・試験は偽の fn = 実の app を起こさない）
    launch: Launcher,
    /// G90: 今 前面の app の bundle id を読む口（製品は `front_app`・試験は偽の fn）
    front: fn() -> Option<String>,
    /// G90（DOGFOOD-X 周 2 回 1）: 動いている app（bundle id）を前に出す口（製品は `mirror::activate_app`・試験は偽の fn = 実の app を前に出さない）
    activate: fn(&str) -> Result<(), String>,
    /// G91: 本物の窓を枠の下にしまう・触る時だけ前に出す口（`KangaeSurfaceBridge`・重ねの `Dock` と同じ bridge）。None = まだ起こしていない
    /// （最初に映った面の `tuck` が `find_bridge` で起こす・試験は偽の bridge を入れておく）・Err = 起きなかった理由（今の Mirror の道のまま・起こし直さない）
    bridge: Option<Result<Bridge, String>>,
    /// MIRROR-QUIET-1101: 描き直しを頼む口（最初の `show` で持つ）。`pump_docks`（frame の終わり）が app の起動の知らせで窓の無い面にすぐ探す印を付けた時に頼む
    ctx: Option<egui::Context>,
    /// G91: Shibubu の窓を本物の窓の真上（しまう）/ 真下（触る）へ置く口（製品は `surface::order_host_above` / `order_host_below`・試験は偽の fn）
    order_above: fn(u32) -> Result<(), String>,
    order_below: fn(u32) -> Result<(), String>,
    /// G91（改）: 画面の pointer の位置と button の押下を読む口（製品は `mirror::pointer_now`・試験は偽の fn）
    pointer: fn() -> (Option<[f64; 2]>, bool),
    /// MIRROR-HYBRID-1101: 人が最後に触ってからの秒を読む口（製品は `mirror::idle_secs`・試験は偽の fn）。本物を触っている間だけ読む
    idle: fn() -> f64,
    /// MIRROR-HYBRID-1101: 本物を下へ戻した瞬間に Mirror の絵が無かった（地だけが見えた）回数（起動から・app.log に 1 行ずつ）
    blank_backs: u32,
    /// 追記 245 の 3: Shibubu が起こした app が前に出た回数（起動から・G90 の見張りが見た分・app.log に 1 行ずつ）
    fronted: u32,
    /// RQ4（RQ-019）: 人が menu で選んだ窓（面の id と題の語・`take_chosen_windows` が取る）。MIRROR-FRAME-1101: 3 つ目 = 入れる app の bundle id（空の枠だけ Some）
    chosen: Vec<(String, Option<String>, Option<String>)>,
    /// MIRROR-FRAME-1101: 空の枠の `Open an app…` の口（製品は `choose_app` = macOS の開く panel で .app を選び bundle id を読む・試験は偽の fn = panel を開かない）。
    /// None = 取り消した・Some(Err) = 選んだ物の bundle id が読めない 1 行
    pick_app: fn() -> Option<Result<String, String>>,
    /// MIRROR-FRAME-1101: 空の枠の `Choose a window` の窓の一覧（裏の thread の `MirrorTarget::windows`・着くまで `Looking for windows`）と字で絞る欄（開くたびに空に）
    pick: Option<std::sync::mpsc::Receiver<Result<Vec<(MirrorTarget, String, [f64; 4])>, MirrorError>>>,
    picked: Option<Result<Vec<(MirrorTarget, String, [f64; 4])>, MirrorError>>,
    filter: String,
    /// MIRROR-FRAME-1101: 一覧の行の小さな絵（窓の id → texture・開くたびに空にして撮り直す）と、裏の thread が撮って読んだ絵の口
    shots: HashMap<u64, TextureHandle>,
    shooting: Option<std::sync::mpsc::Receiver<Vec<(u64, ColorImage)>>>,
    /// MIRROR-FRAME-1101 (b): Shibubu が前でない間に空の枠を描いた pass + 1 の印（最初に要った時に `watch_drag` の thread を起こす・None = まだ起こしていない）
    drag_armed: Option<Arc<AtomicU64>>,
    /// MIRROR-HYBRID-1101 追記 246 の 3: 四隅で引き終わった枠（面の id と `SurfaceFrame`・`take_frames` が取る）
    frames: Vec<(String, SurfaceFrame)>,
    /// I33b: この frame に Page の木の後で描く枠（面の id と矩形・`later` が足し `floating` が取り出す）
    later: Vec<(String, Rect)>,
    /// I33b: 最後の `floating` が返した枠（下から上の順）。下の枠が上の枠に隠れているか（`above`）をこれで見る・`end_frame` で空に
    stack: Vec<(String, Rect)>,
    /// L5（DOGFOOD-X 周 2 題 6）: 直前の frame で Mirror の面でない物が持っていた keyboard の focus（押していない面へ focus が移った時に返す先）
    last_focus: Option<egui::Id>,
    /// RQ4: 元の app と書類を開く口（製品は `open_app_window`・試験は偽の fn = 実の app を開かない）
    open: fn(&str, Option<&Path>, bool) -> Result<(), String>,
}

/// MIRROR-FIT-1101（owner 200）: 選んだ枠の四隅の取っ手（左上・右上・左下・右下）。引いている間は向かいの隅を動かさない。
/// I33（owner 11-03「あとmirrorの枠を自由にshibubuの中で動かせるように手動で。」）: `Move` = 題の帯で枠ごと引く（中身 = 掴んだ点の枠の左上からのずれ・大きさは替えない）。
/// 置き場は四隅と同じ（`Pane::pin` の左上 = `Se` と同じ読み・離したら `SurfaceFrame` の x / y）
#[derive(Debug, Clone, Copy, PartialEq)]
enum Corner {
    Nw,
    Ne,
    Sw,
    Se,
    Move(Vec2),
}

/// surface 1 つ分（key = `SurfaceSpec.id`）。
struct Pane {
    surface: Result<MirrorSurface, MirrorError>,
    /// 見つけた窓（同意の button を押した時にこの窓で open する）
    target: Option<MirrorTarget>,
    texture: Option<Tex>,
    last: Option<MirrorFrameMeta>,
    tried_ms: f64,
    /// W16M-2: 面ごとの lease（開いた時に人）。`relay_human` に渡す
    lease: ControlLease,
    /// W16M-2: 入力 1 つの拒否と出した時刻（面の Err にしない・`LINE_MS` の間だけ映像の上に 1 行で出す）
    line: Option<(MirrorError, f64)>,
    /// W16M-2: 前の frame で押したまま、まだ離していない primary の押下（click と drag は押下と離しの組で 1 つ。組は frame をまたぐ）
    held: Option<Event>,
    /// W16M-3: `SurfaceSpec.keep_alive`。false の面は描かなかった frame の終わりで閉じる。
    /// 追記 245 から true の面も stream は止める（`Pane::stop`・面の持ち物だけ残す）。
    keep_alive: bool,
    /// W16M-3（不具合 ②）: helper が click の答えで読み戻した選択の位置の caret（元の窓の中の割合 [x, y, w, h]）。背景の NSTextView は
    /// insertion point を描かないので Shibubu が映像の上に描く。次の Mirror の入力・Page を離れる・面の Err で捨てる（古い caret を今の物として出さない）。
    /// 保証しない（未対応・W16M-3 の反証）: Mirror の外の手（TextEdit を前に出して直接打つ・menu の Undo・自動の口）で元の窓が変わった時は、
    /// 次の Mirror の入力まで古い caret が残る。新しい frame で捨てると click 自身が起こす描き直しの frame でも消える（実機で未測）ので、捨てる規則は frame の時刻を測ってから決める
    caret: Option<[f64; 4]>,
    /// W16M-3: 最後に送った入力が click ならその relay の seq（helper の答えの `id`）。click の後すぐ打つと、click の答えは打った後に着く
    /// （stream の人の口で 800 ms・実測）。その答えの caret は打つ前の位置なので、今の click の答えだけを描く
    click_seq: Option<u64>,
    /// W16M-3: 裏の thread で窓を探している間の受け口（`attach` の答え）。着いたら次の frame で使う
    pending: Option<std::sync::mpsc::Receiver<(Option<MirrorTarget>, Result<MirrorSurface, MirrorError>)>>,
    /// W16M-3（反証）: 窓探しの答えが 1 度でも着いた。着く前だけ「探しています」を出し、探し直しの間は前の Err の状態と 1 行を出したまま
    /// （許可が無い時の「許可が要ります」が 2 秒ごとに「探しています」に替わらない）
    answered: bool,
    /// M4 ③: 次の窓探しで、許可がまだ決まっていない app に許可を尋ねる（`Ambiguous` の button を押した時だけ・macOS の確認が前に出る）
    ask: bool,
    /// G87: 次の窓探しの前に app を起こす（`Launcher` の if_stopped）。新しい面 = Some(true)（最初の探しで、書類の指定の無い面の app が動いていなければ起こす）・
    /// `Open {App}` = Some(false)（動いていても `open`）・探しに出したら None
    open_app: Option<bool>,
    /// G87: app を起こした時刻（`LAUNCH_WAIT_MS` の間は窓が無くても `Starting`）
    starting: Option<f64>,
    /// G87: launcher と窓の一覧が言った app の名前（窓が無い時の 1 行と button に使う。bundle id の最後の語〈`reaper`〉にしない）
    name: Option<String>,
    /// G90: 前面を戻す見張りを始めた時刻。app を起こす探しを出した時に Shibubu の窓が前か、前の app が読めたら Some・起こさなかった（動いていた）答え・
    /// 見張りが終わった（戻した・人が別の app を前に出した）・`LAUNCH_WAIT_MS` を過ぎたら None
    reclaim: Option<f64>,
    /// G90（DOGFOOD-X 周 2 回 1）: Shibubu の窓が前でない時に app を起こした時に前だった app の bundle id と、次に前面を読む時刻（`FRONT_POLL_MS` ごと）。
    /// None = Shibubu が前だった（Shibubu の窓が前でなくなった frame に読む）
    front_before: Option<(String, f64)>,
    /// L5-STICK ①（DOGFOOD-X 周 2 回 1）: Shibubu が app を起こした時刻。`LAUNCH_WAIT_MS` の間に映している app が前に来た 1 回目は、起動の自分の activate（人の手でない）と数える
    launched: Option<f64>,
    /// 追記 245 の 3: Shibubu が隠して起こした app の隠れた窓の隠しを、`Show {App}` を待たずに 1 回解いた（2 回目の隠れは今まで通り button を待つ = 繰り返し `open` しない）
    unhid: bool,
    /// MIRROR-QUIET-1101: 窓の無い面が探し始めた時刻（ms・`search_gap` に渡す）。映った・すぐ探す印（`tried_ms` = -∞）で None
    since: Option<f64>,
    /// MIRROR-QUIET-1101: 最後に `show` で描いた egui の pass の番号。前の pass に描いていない（Page が見えた）なら すぐ探す
    seen_pass: u64,
    /// MIRROR-SPEED-1101: helper に送った描く大きさ（pixel）と送った時刻（`MirrorSurface::view`・替わった時だけ・`VIEW_GAP_MS` ごとまで）。面の Err で None
    view: Option<((u32, u32), f64)>,
    /// MIRROR-SPEED-1101: 描いている IOSurface の今と 1 つ前（use count を持つ = GPU が読んでいる間に SCK の pool が書かない）。CPU で描いた・面の Err で空
    shown: [Option<Arc<IoSurface>>; 2],
    /// G91: 映している本物の窓を枠と同じ位置・大きさで Shibubu の下に置く Dock（bridge の窓が CG の番号で映している窓と対応した時だけ）。
    /// 赤（Closed）と Shibubu の終了で元の位置に戻して捨てる・黄（Minimized）と探し直しの間は持ち越す
    dock: Option<Dock>,
    /// G91: Dock を作る前に送った bridge の `windows` の要求の id（応答は `pump_docks` が拾う）
    tuck_wait: Option<u64>,
    /// G91: Dock を作れなかった理由（許可が無い・映している窓と対応する窓が無い）。在る間は聞き直さない（Closed で新しい Pane になるまで）
    tuck_err: Option<String>,
    /// G91: 本物の窓が前に出て人が直接触っている（`go_direct`）。値 = その後に Shibubu が一度前でなくなったか（前に戻った frame で `put_back`）
    direct: Option<bool>,
    /// MIRROR-HYBRID-1101: 触って本物を前に出した時刻（ms・`now_ms`）。本物が前に来た（Shibubu が前でなくなった）frame に「触ってから前に出るまで」を app.log に 1 行して捨てる
    touch: Option<f64>,
    /// MIRROR-HYBRID-1101 追記 214: 押した時に本物が枠より大きかった（ctx の time）。本物を映像の大きさにして（`fit_to_frame`）、大きさの答え（`resize` が消えた）で
    /// 合っていれば `go_direct`・縮まなければ理由を 1 行。答えを待つ間は次の押しで重ねて送らない
    fit_touch: Option<f64>,
    /// 追記 235 の 1: 本物が枠より大きかった面で、押す前に本物を合わせるよう頼んだ映像の大きさ（同じ大きさは 1 回だけ頼む・選んだ大きさの面は縮めて見せなくなったら捨てる）。
    /// I31: 人が大きさを選んでいない面では、既定で枠の大きさへ頼んだ・頼む前から同じだった映像の大きさ（本物がこれと違う間 `frame_size` は本物に付いて行く）。
    /// Dock を付け直したら捨てる（B88）
    prefit: Option<Vec2>,
    /// 追記 235 の 2: pointer が映像の上で止まった所・止まった時刻（ctx の time）・その止まりで本物を前に出したか（出した後は pointer が動くまで出し直さない）
    dwell: Option<(Pos2, f64, bool)>,
    /// 追記 235 の 2: 別の欄が focus を持つ間に最後に打った時刻（ctx の time・`TYPING_S`）
    typed_at: f64,
    /// MIRROR-HYBRID-1101: Page の scroll・枠の移動で本物を下へ戻した後、枠が最後に動いた時刻（ctx の time）。`SETTLE_S` 動かなければ本物に戻して捨てる。
    /// 被さった・枠の外の click・何もしない・Page を替えたで戻した時は置かない（戻さない）
    resume: Option<f64>,
    /// MIRROR-HYBRID-1101: 前の frame の枠の画面座標（`slot`）。本物を触っている間に替わった = Page の scroll か Shibubu の窓が動いた
    slot_was: Option<Frame>,
    /// B90: 本物を枠へ合わせ直してよい時刻の終わり（ctx の time・人が枠を動かした合図と、引いた大きさの答えの後 `FOLLOW_S`）・本物の大きさを頼んでよい
    /// 時刻の終わり（人の合図の後だけ = 断られた大きさの後に頼み直し続けない）・前の frame の Shibubu の窓の中身の矩形（替わった = 人が窓を動かした・
    /// 大きさを替えた）・今の frame の本物を置く所（`slot`・触った時の `go_direct` が置く）・人が動かしていない間に枠が替わって本物を動かさなかった回数
    /// （app.log は最初の 3 回だけ）
    follow_until: f64,
    ask_until: f64,
    host_was: Option<Rect>,
    slot_now: Option<Frame>,
    held_moves: u32,
    /// G91 改 2: 外れた Dock を自動で枠へ合わせ直した回数（`REFIT_MAX` で止める・直接触った後の `put_back` で 0）。
    /// B14: Dock が付く前は、答えなかった `windows` を聞き直した回数（Dock が付いたら 0）
    refits: u8,
    /// G91 改 2: 次に自動で合わせ直す時刻（ctx の time・外れたのを見た frame + `REFIT_GAP_S`・送ったら None）。
    /// B14: Dock が付く前は、答えなかった `windows` を次に聞く時刻（答えなかった時 + `REFIT_GAP_S`・送ったら None）
    refit_at: Option<f64>,
    /// RQ4: この面を開いた時の元（bundle id と `SurfaceTarget::App::file`）。Project の側で替わったら（`Choose window…`・Undo）面を探し直す
    source: Option<(String, Option<String>)>,
    /// RQ4（RQ-019）: 窓を選び直す menu の題の一覧（裏の thread の `MirrorTarget::titles`・着くまで `Looking for windows`）
    titles: Option<std::sync::mpsc::Receiver<Result<Vec<String>, MirrorError>>>,
    titles_got: Option<Result<Vec<String>, MirrorError>>,
    /// RQ4（RQ-018）× MIRROR-FIT-1101 追記 218 の 3: 帯の題の所に `LINE_MS` の間だけ出す 1 行（状態・文・時刻）。元の app を開けなかった・本物が引いた大きさを
    /// 受けなかった・Dock の無い面で枠の大きさだけ替えた
    band_line: Option<(RegionState, String, f64)>,
    /// MIRROR-FIT-1101 追記 218 の 3 × MIRROR-HYBRID-1101 追記 246: 人が四隅で選んだ映像の大きさ（引いている間は今の大きさ・引き終わりと Project の `frame` から読む）。
    /// `frame_size` の開いた形の置ける所（後ろの部品の分に縛られない）。Dock の無い面は枠の大きさそのもの・Dock の窓が在れば本物の大きさを比のまま収める
    sized: Option<Vec2>,
    /// L5-STICK: Shibubu の窓が最小化されている間 true（本物も一緒に最小化している = 取り残さない）。黄の最小化（`MirrorWindow::Minimized`）とは別
    host_min: bool,
    /// L5（DOGFOOD-X 周 2 題 6）: 面を押した（click・drag・AX の Press）後で、まだ focus を持っている間 true。false の間に focus が来ても打鍵を送らない
    armed: bool,
    /// MIRROR-AUTO-1101 B: 書類の面の最初の窓探しの答えを待っている（窓が無ければ書類ごと後ろで開く = 再起動の後も押さずに映す）
    doc_wait: bool,
    /// MIRROR-FIT-1101（owner 200）: 枠を 1 回押して選んだ（四隅の取っ手・枠の細い線 = 光るのは本物が前に来た時だけ・追記 214）。MIRROR-HYBRID-1101 から選ぶ押しも触る合図（本物へ送る）。
    /// 枠の外の押し・Esc（keyboard の focus が無い時）・Page を離れると外す
    selected: bool,
    /// MIRROR-PIN-1102: 人が最後にこの面を押した時刻（`now_ms`・映像を押して枠を選んだ押し = 打鍵はその後にだけ届く・Page を離れた `stop` でも残す・押していなければ NEG_INFINITY）。
    /// `go_direct` では打たない（止まった pointer・scroll の後の戻し・大きさの答えの後の前出しは人が指した事にならない）。
    /// `shown` の答え = 同じ app の面が Page に 2 つ以上ある時、Engine に渡す窓を決める（`generated::pin_window`）
    touched: f64,
    /// MIRROR-FIT-1101（owner 200）: 四隅の取っ手で引いている隅（追記 240 から向かいの隅は毎 frame の今の枠の物 = Page が流れても中身の座標で止まる）
    grab: Option<Corner>,
    /// MIRROR-FIT-1101（owner 200）: 引き終わった映像の大きさ（本物へ送る大きさ・egui の points = 画面の points）・引き終わった時刻（ctx の time）・その時の本物の frame。
    /// 本物の frame が替わる（読み戻し・`resized`）か `RESIZE_WAIT_S` を過ぎたら捨てる（断られた大きさは本物の大きさに戻る）
    resize: Option<(Vec2, f64, Option<Frame>)>,
    /// MIRROR-FIT-1101（operator 21:1x）: 最後に引いた隅と、その反対の隅（Page の並びの所 = 枠の左上からずらす前の所から見た point）。
    /// 右下以外の隅で引いても反対の隅が Page の上で動かない（`shift`）。Shibubu を閉じるまで覚える（Project には保存しない）
    pin: Option<(Corner, Vec2)>,
    /// MIRROR-FIT-1101: この frame で枠をずらした量（`shift` の答え・`handles` が並びの所を出す）
    shifted: Vec2,
    /// I33b: この frame の Page の列の矩形（並びの所 = ずらす前の枠の左上から見た物・`shift` の `room`）。題の帯と四隅で引ける所（`handles`）。
    /// 既定（`shift` をまだ呼んでいない）= 並びの所から右・下だけ（I33 までと同じ）
    room: Rect,
    /// 追記 240 の 2: 本物の窓が断った小さい方の限り（映像の point・断られた読み戻しで覚える・0 = 知らない）。四隅で引く間はここで止める
    least: Vec2,
    /// 追記 240 の 1（owner 240・242）: Shibubu の中で何かを引いている間（四隅・右 sidebar・下の panel の境目）、本物を置いておく映像の矩形（引き始めの所・Shibubu の窓の中の point）
    frozen: Option<Rect>,
    /// 追記 240 の 4: 引き終わった時刻（wall の ms）。本物の読み戻しが来たら「離してから本物が合うまで」を app.log に 1 行して捨てる
    released: Option<f64>,
    /// 追記 246 の 3: 最後に見た Project の `SurfaceSpec::frame` と、自分が送った物（`take_frames`）。見た物が替わり、送った物の戻りでもない（⌘Z・⇧⌘Z・再起動の後の初め）なら読み直す
    frame_seen: Option<SurfaceFrame>,
    frame_sent: Option<SurfaceFrame>,
    /// MIRROR-FRAME-1101 (b): 空の枠の面で、Shibubu が前でない間に pointer の button が押されていた最初の所（画面の point）。離すか Shibubu が前に来たら空に
    press: Option<[f64; 2]>,
    /// MIRROR-FRAME-1101 (b): 枠の上で離した所と、その時に裏の thread で引いた窓の一覧（`dropped_window` で決める）
    dropped: Option<([f64; 2], std::sync::mpsc::Receiver<Result<Vec<(MirrorTarget, String, [f64; 4])>, MirrorError>>)>,
}

impl Pane {
    fn new(surface: Result<MirrorSurface, MirrorError>) -> Pane {
        let answered = surface.is_ok();
        Pane { surface, target: None, texture: None, last: None, tried_ms: f64::NEG_INFINITY, lease: ControlLease::human(crate::surface::now_ms()), line: None, held: None, keep_alive: false, caret: None, click_seq: None, pending: None, answered, ask: false, open_app: Some(true), starting: None, name: None, reclaim: None, front_before: None, launched: None, unhid: false, since: None, seen_pass: 0, view: None, shown: [None, None], dock: None, tuck_wait: None, tuck_err: None, direct: None, touch: None, fit_touch: None, prefit: None, dwell: None, typed_at: f64::NEG_INFINITY, resume: None, slot_was: None, follow_until: f64::NEG_INFINITY, ask_until: f64::NEG_INFINITY, host_was: None, slot_now: None, held_moves: 0, refits: 0, refit_at: None, source: None, titles: None, titles_got: None, band_line: None, sized: None, host_min: false, armed: false, doc_wait: false, selected: false, touched: f64::NEG_INFINITY, grab: None, resize: None, pin: None, shifted: Vec2::ZERO, room: Rect::from_min_max(Pos2::ZERO, pos2(f32::INFINITY, f32::INFINITY)), least: Vec2::ZERO, frozen: None, released: None, frame_seen: None, frame_sent: None, press: None, dropped: None }
    }

    /// 追記 245: 映していた stream が終わった（helper の stopped・窓が消えた・隠れた）。理由を app.log に 1 行して面の Err にする。
    /// 契約（本体の commit）: Shibubu が起こした app（`launched` から `LAUNCH_WAIT_MS` の間）の窓が隠れていた = 隠して起こした窓ができた → 1 回だけ
    /// （`unhid`）次の探しで launcher を if_stopped = false で呼ぶ（`open -g -b` = 前に出さずに隠しを解く・G90 (3) の `Show {App}` と同じ道）。その間の面は `Starting {App}…`。
    /// 例: `Err(e) => pane.lost(&spec.id, e, now)`（`show` の stream の Err）
    /// 保証しない: 隠しを解いた窓が Shibubu の後ろに出る事（出る所は macOS が決める・出た後は Dock が Shibubu を真上へ置く）。
    /// Wave: MIRROR-HYBRID-1101
    fn lost(&mut self, id: &str, e: MirrorError, now: f64) {
        eprintln!("W16M-3: Mirror {id}: {e:?}");
        if let (MirrorError::WindowHidden { app_name }, false, Some(t)) = (&e, self.unhid, self.launched.filter(|t| now - t < LAUNCH_WAIT_MS)) {
            eprintln!("MIRROR-HYBRID-1101 追記 245: Mirror {id}: 隠して起こした {app_name} の窓ができたので、Shibubu の後ろで隠しを解きます（起こしてから {:.0} ms）", now - t);
            let app_name = self.name.clone().unwrap_or_else(|| app_name.clone());
            (self.unhid, self.open_app, self.starting, self.tried_ms) = (true, Some(false), Some(now), f64::NEG_INFINITY);
            self.surface = Err(MirrorError::Starting { app_name });
            return;
        }
        self.surface = Err(e);
    }

    /// 追記 245 の 1: 描かなかった面の stream を止める（MirrorSurface を落とす = helper の stream が止まる・本物の app は閉じない・本物の窓は置いた所のまま）。
    /// Dock・名前・元（`source`）・人が選んだ大きさ（`sized`・`pin`・`least`）・Project の枠の読み（`frame_seen`・`frame_sent`）は残し、他は新しい面と同じ
    /// （戻った frame ですぐ探す・最初の探しで止まっている app は起こす = 自動で開くのは今見ている Page だけ）。
    /// 例: `if p.surface.is_ok() { p.stop() }`（`end_frame` の描かなかった面）
    /// Wave: MIRROR-HYBRID-1101
    fn stop(&mut self) {
        let old = std::mem::replace(self, Pane::new(Err(MirrorError::WindowGone)));
        (self.keep_alive, self.dock, self.name, self.source, self.touched) = (old.keep_alive, old.dock, old.name, old.source, old.touched);
        (self.sized, self.pin, self.least, self.frame_seen, self.frame_sent) = (old.sized, old.pin, old.least, old.frame_seen, old.frame_sent);
    }

    /// 裏の窓探し（`attach`）の答えが着いていれば面に入れる。true = まだ答えを待っている。
    /// G82: 描いている面の `show` と、描かなかった keep_alive の面の `end_frame` の両方が呼ぶ（呼ばないと pending が残り、他の面が永久に待つ）
    fn take_answer(&mut self, id: &str, now: f64) -> bool {
        let Some(rx) = &self.pending else { return false };
        match rx.try_recv() {
            Ok((target, surface)) => {
                // G90: app を起こした答え（Starting）か、起こした app の窓を待っている間でなければ、起こしていない = 前面を戻す見張りは要らない
                if self.starting.is_none() && !matches!(surface, Err(MirrorError::Starting { .. })) {
                    (self.reclaim, self.front_before, self.launched) = (None, None, None);
                }
                // 画面の 1 行は内部の語を出さない（D-e）ので、reason は stderr に（BG の run.sh は app.log に取る）。同じ Err の探し直しは書かない
                if let Err(e) = &surface {
                    if self.surface.as_ref().err() != Some(e) {
                        eprintln!("W16M-3: Mirror {id}: {e:?}");
                    }
                }
                (self.target, self.surface, self.pending, self.answered) = (target, surface, None, true);
                self.tried_ms = now;
                // B89（41 回目・REAPER: 一覧に無い窓を 1 度映すと、その理由〈tuck_err〉が残り、後で別の窓を映しても二度と Dock を付けなかった）: 新しく映した窓は
                // 前の窓の「置けない」理由と聞き直しの数を持ち越さない（WindowGone・stream が終わった後の探し直し・窓が閉じた後）。Dock の在る面の数は Dock の合わせ直しの数
                if self.surface.is_ok() {
                    self.tuck_err = None;
                    if self.dock.is_none() {
                        (self.refits, self.refit_at) = (0, None);
                    }
                }
                // G87: 起こした app の窓を待つ間（LAUNCH_WAIT_MS）は窓が無くても Starting・名前は launcher と窓の一覧が言った表示名（bundle id の語にしない）
                match (&mut self.surface, &self.target) {
                    (Err(MirrorError::Starting { app_name }), _) => (self.starting, self.name) = (Some(now), Some(app_name.clone())),
                    (Err(MirrorError::NoWindow { app_name, title_has: None }), _) if self.starting.is_some_and(|t| now - t < LAUNCH_WAIT_MS) => {
                        let app_name = self.name.clone().unwrap_or_else(|| app_name.clone());
                        self.surface = Err(MirrorError::Starting { app_name });
                    }
                    (Err(MirrorError::NoWindow { app_name, .. } | MirrorError::NotInstalled { app_name }), _) => {
                        self.starting = None;
                        match &self.name {
                            Some(n) => *app_name = n.clone(),
                            None => self.name = Some(app_name.clone()),
                        }
                    }
                    (_, Some(t)) => (self.starting, self.name) = (None, Some(t.app_name.clone())),
                    _ => self.starting = None,
                }
                self.lease = ControlLease::human(now);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                eprintln!("W16M-3: Mirror {id}: 窓を探す thread が答えずに終わりました");
                (self.surface, self.pending, self.answered) = (Err(MirrorError::Helper("Stopped while looking for windows".into())), None, true);
                self.tried_ms = now;
                false
            }
        }
    }
}

/// 面の枠の状態（`show` が返す）。searching = 最初の窓探しの答えを待っている（探し直しの間は前の Err の状態のまま）。
fn pane_state(pane: &Pane, searching: bool) -> PortalState {
    match (&pane.surface, &pane.texture) {
        _ if searching => PortalState::Attaching,
        (Ok(_), Some(_)) => PortalState::Mirror,
        (Ok(_), None) => PortalState::Attaching,
        (Err(e), _) => state_of(e),
    }
}

/// Err → 枠の状態。
fn state_of(e: &MirrorError) -> PortalState {
    match e {
        MirrorError::NoConsent { .. } | MirrorError::ScreenCaptureDenied => PortalState::PermissionRequired,
        MirrorError::NoWindow { .. } | MirrorError::WindowGone | MirrorError::WindowHidden { .. } => PortalState::SourceUnavailable,
        // G87: 起こした app の窓を待っている・入っていない（`Undiscovered` = Not found と Find again）
        MirrorError::Starting { .. } => PortalState::Attaching,
        MirrorError::NotInstalled { .. } => PortalState::Undiscovered,
        // M4 ③: 許可で見分けられる（尋ねる・システム設定）なら許可待ち、窓を閉じるしかないなら元が使えない
        MirrorError::Ambiguous { ask_pid, denied, .. } => if ask_pid.is_some() || *denied { PortalState::PermissionRequired } else { PortalState::SourceUnavailable },
        MirrorError::WrongPort { .. } | MirrorError::HelperMissing(_) | MirrorError::Helper(_) | MirrorError::Log(_) => PortalState::IntegrationFailed,
        // W16M-2: 入力 1 つの拒否で、面の Err（pane.surface）にはしない。映像は映したまま、拒否の 1 行だけを Page に出す
        MirrorError::NeedsFront { .. } | MirrorError::AutomationDenied { .. } | MirrorError::AccessibilityDenied | MirrorError::NoAnswer { .. } => PortalState::Mirror,
    }
}

/// W16M-3（不具合 ①: Mirror の矩形が 試験の側の AX の道具の木に出ず element_token で focus できない・R-M と W16M-2 の D-f）: Mirror の矩形の AccessKit の role。
/// 前の `show` は `Role::Group`（macOS の AXGroup）で、試験の側の AX の道具の `get_window_state` は AXGroup を一覧に出さなかった（W16M-2 の手 05: 98 要素の木に無い。
/// 同じ node は映っている間 AXPress を持ち `oracle/ax.swift press` で押せた＝落ちる理由は action ではなく role）。`Role::Image`（AXImage = 元の窓の今の絵）にする。
/// 契約（`show` が毎 frame 付ける）: role = `FRAME_ROLE`・identifier = `surface-frame:<surface id>`・label = 状態の語と 1 行・
/// 映っている間だけ Focus と Click（egui が `Sense::click_and_drag` で足す）・映っていない間は action 0（押しても何も起きない物を出さない）。
/// 例: `ctx.accesskit_node_builder(resp.id, |n| { n.set_role(FRAME_ROLE); n.set_author_id(ax.clone()); })`
/// 保証する: AXGroup ではない（holdout `w16m3_live_mirror_frame_is_an_ax_node_test_tools_list`）。
/// 保証しない: 試験の側の AX の道具 が AXImage を一覧に出す事（仮説: 試験の側の AX の道具の実行 file の role の表に AXImage が在る。BG の実機で element_token を引けたかで確かめ、
/// 出なければ理由を 1 行書いてこの値を替える）・VoiceOver の読み方（owner の手）。
/// Wave: W16M-3
pub const FRAME_ROLE: accesskit::Role = accesskit::Role::Image;

/// frame（`meta` の大きさ）を縦横比のまま矩形の中央に収めた範囲（余白は背景色）。描く所（`show` の image）と点を直す所（`mirror_inputs` の `shown`）で同じ物を使う。
/// W16M-3（不具合 ②: 映像の本文を 1 回 click しても TextEdit の選択が動かない）: 試験の側（`tests/mirror_e0.rs` の `w16m3_click_*`）が、本物の frame の
/// `meta` で「映像のその文字の点 → `fit` → `mirror_inputs` → 窓の中の割合」を製品と同じ範囲で組むために pub（private → pub・本体は同じ）。
/// 例: `let shown = fit(rect, &meta); let inputs = mirror_inputs(&events, shown, &meta, focused);`
/// 保証する: 縦横比を保ち `rect` の中に収め中央に置く。保証しない: `meta` が今の元の窓の大きさである事（helper が frame ごとに送る）。
/// Wave: W16M-3
pub fn fit(rect: Rect, meta: &MirrorFrameMeta) -> Rect {
    let (fw, fh) = (meta.width as f32, meta.height as f32);
    let k = (rect.width() / fw).min(rect.height() / fh);
    Rect::from_center_size(rect.center(), Vec2::new(fw * k, fh * k))
}

/// MIRROR-FIT-1101: 映像の中身（contentRect = 本物の窓）が `video` にちょうど重なる frame 全体の矩形（stream の余白は `video` の外 = 呼ぶ側が切って描く）。
/// 中身の比が `video` と違う時（Dock の前・stream の大きさの直しの前）は中身を中に収める。中身が 0（frame 全体が中身）なら `fit` と同じ。
/// 描く・caret・`mirror_inputs` に同じ矩形を渡す（押した点の割合が描いた物と合う）。
fn shown_rect(video: Rect, meta: &MirrorFrameMeta) -> Rect {
    let [cx, cy, cw, ch] = meta.content;
    if cw <= 0.0 || ch <= 0.0 || meta.width == 0 || meta.height == 0 {
        return fit(video, meta);
    }
    let f = meta.factor.max(0.01);
    let (cx, cy, cw, ch) = ((cx * f) as f32, (cy * f) as f32, (cw * f) as f32, (ch * f) as f32);
    let k = (video.width() / cw).min(video.height() / ch);
    let content = Rect::from_center_size(video.center(), Vec2::new(cw * k, ch * k));
    Rect::from_min_size(content.min - Vec2::new(cx * k, cy * k), Vec2::new(meta.width as f32 * k, meta.height as f32 * k))
}

/// MIRROR-HYBRID-1101 追記 240 の 1（owner「mirrorをおきくしようとしているのに戻るなんかうねうねする」）: 四隅を引いている間の絵 = 映像の中身（contentRect = 本物の窓）を
/// `video` に縦横それぞれの倍率で重ねる frame 全体の矩形（本物は離すまで元の大きさのまま・縦横の比は離した後の本物の絵で戻る）。中身が 0 なら frame の全部を伸ばす。
fn stretched(video: Rect, meta: &MirrorFrameMeta) -> Rect {
    let [cx, cy, cw, ch] = meta.content;
    let (fw, fh) = (meta.width as f32, meta.height as f32);
    if cw <= 0.0 || ch <= 0.0 || fw <= 0.0 || fh <= 0.0 {
        return video;
    }
    let f = meta.factor.max(0.01);
    let (kx, ky) = (video.width() / (cw * f) as f32, video.height() / (ch * f) as f32);
    Rect::from_min_size(video.min - Vec2::new((cx * f) as f32 * kx, (cy * f) as f32 * ky), Vec2::new(fw * kx, fh * ky))
}

/// MIRROR-SPEED-1101: 本物の窓（frame の中身）を `video` に描く大きさ（point）。helper の `view` はこれ × 画面の倍率（stream の余白は入れない = 窓の pixel と比べる）
fn window_drawn(video: Rect, meta: &MirrorFrameMeta) -> Vec2 {
    let r = shown_rect(video, meta);
    let [_, _, cw, ch] = meta.content;
    if cw <= 0.0 || ch <= 0.0 || meta.width == 0 || meta.height == 0 {
        return r.size();
    }
    let f = meta.factor.max(0.01);
    Vec2::new(r.width() * (cw * f) as f32 / meta.width as f32, r.height() * (ch * f) as f32 / meta.height as f32)
}

/// I32（owner 11-03「orcaの隣にshibubu置いてるから全画面のorca見れないし。mirrorの中でappを自由に拡大縮小できるように。」）: 映像の拡大の上限
/// （倍・1 = 枠に収めた今の映像）と、⌘+ / ⌘- の 1 回の倍率（pinch は trackpad の倍率そのまま）
const LENS_MAX: f32 = 8.0;
const LENS_STEP: f32 = 1.25;

/// I32: Mirror の映像だけの拡大（本物の窓の大きさ・位置は替えない）。面ごとに 1 つ（`Shape::lens` = Pane が消えても・Page を移っても残る・再起動は跨がない）。
/// `k` = 倍率（1〜`LENS_MAX`）・`at` = 映像の真ん中に来る本物の窓の点（倍率 1 の中身の矩形の中の割合 0〜1）。
/// 例: `let lens = lens.zoom(2.0, pointer, content, video); let img = lens.apply(shown_rect(video, &meta), content, video); mirror_inputs(&evs, img, &meta, focused)`
/// 保証する: 倍率 1 は今の映像と同じ矩形（`apply` が何も替えない）・拡大した絵は、中身が映像より大きい軸で映像の外の余白を見せない・
/// `zoom` は `at` の画面の点の下の本物の点を動かさない（端に当たった時を除く）・倍率と点は有限。保証しない: 映像の外を描かない事（描く側が `painter_at(video)` で切る）。
/// Wave: I32
#[derive(Debug, Clone, Copy, PartialEq)]
struct Lens {
    k: f32,
    at: Vec2,
}

impl Default for Lens {
    fn default() -> Lens {
        Lens { k: 1.0, at: Vec2::splat(0.5) }
    }
}

impl Lens {
    /// 映像の真ん中に来る倍率 1 の画面の点
    fn focus(self, content: Rect) -> Pos2 {
        content.min + self.at * content.size()
    }

    /// 倍率 1 の画面の点 `f` を映像の真ん中に置く（中身が映像より大きい軸は端の余白を見せない所へ寄せる・小さい軸は真ん中）。
    /// `f32::clamp` は下 > 上で panic するので max / min
    fn centered(self, f: Pos2, content: Rect, video: Rect) -> Lens {
        let half = video.size() / (2.0 * self.k);
        let axis = |f: f32, lo: f32, hi: f32, h: f32| if hi - lo > 2.0 * h { f.max(lo + h).min(hi - h) } else { (lo + hi) / 2.0 };
        let f = pos2(axis(f.x, content.min.x, content.max.x, half.x), axis(f.y, content.min.y, content.max.y, half.y));
        let at = (f - content.min) / content.size().max(Vec2::splat(1.0));
        Lens { at: if at.is_finite() { at } else { Vec2::splat(0.5) }, ..self }
    }

    /// 倍率 1 の矩形 `r`（`shown_rect`・caret）→ 拡大した画面の矩形。`content` = 倍率 1 の本物の窓の矩形（`content_rect`）・`video` = 映像の矩形
    fn apply(self, r: Rect, content: Rect, video: Rect) -> Rect {
        if self.k == 1.0 {
            return r;
        }
        let f = self.focus(content);
        let to = |p: Pos2| video.center() + (p - f) * self.k;
        Rect::from_min_max(to(r.min), to(r.max))
    }

    /// `by` 倍（pinch・⌘+ は `LENS_STEP`・⌘- は 1 / `LENS_STEP`）。`at` = 動かさない画面の点（pinch は pointer・key は映像の真ん中）。倍率は 1〜`LENS_MAX`・
    /// 有限でない / 0 以下の `by` は何もしない
    fn zoom(self, by: f32, at: Pos2, content: Rect, video: Rect) -> Lens {
        if !by.is_finite() || by <= 0.0 || !at.to_vec2().is_finite() {
            return self;
        }
        let k = (self.k * by).clamp(1.0, LENS_MAX);
        // at の下の本物の点（倍率 1 の画面の点）を、新しい倍率でも at の下に
        let under = self.focus(content) + (at - video.center()) / self.k;
        Lens { k, ..self }.centered(under - (at - video.center()) / k, content, video)
    }

    /// 見る所を画面の `d` point だけ動かす（wheel・映像の中の drag = 絵が `d` の向きに付いて来る）
    fn pan(self, d: Vec2, content: Rect, video: Rect) -> Lens {
        if !d.is_finite() {
            return self;
        }
        self.centered(self.focus(content) - d / self.k, content, video)
    }
}

/// I32: 倍率 1 で本物の窓（frame の中身）を描く矩形（`shown_rect` の中身の所・映像の真ん中）。中身が 0 なら `fit`
fn content_rect(video: Rect, meta: &MirrorFrameMeta) -> Rect {
    let [_, _, cw, ch] = meta.content;
    if cw <= 0.0 || ch <= 0.0 || meta.width == 0 || meta.height == 0 {
        return fit(video, meta);
    }
    let f = meta.factor.max(0.01);
    let (cw, ch) = ((cw * f) as f32, (ch * f) as f32);
    let k = (video.width() / cw).min(video.height() / ch);
    Rect::from_center_size(video.center(), Vec2::new(cw * k, ch * k))
}

/// MIRROR-FIT-1101（owner 200）: 隅 `c` を `to` まで引いた枠（題の帯 + 映像）。向かいの隅は `from`（今の枠）のまま・映像は `MIN_VIDEO` と `least`
/// （追記 240 の 2: 本物が断った小さい方の限り）以上・`to` は `page`（引ける所）の中に寄せる。
fn dragged(c: Corner, from: Rect, to: Pos2, page: Rect, least: Vec2) -> Rect {
    let to = page.clamp(to);
    let (fixed, sx, sy) = match c {
        // I33: 掴んだ点が `to` に来る大きさの同じ枠（`page` = 引ける所の中・I33b から並びの所より左・上へも）
        Corner::Move(at) => {
            let tl = to - at;
            return Rect::from_min_size(pos2(tl.x.min(page.max.x - from.width()).max(page.min.x), tl.y.max(page.min.y)), from.size());
        }
        Corner::Nw => (from.right_bottom(), -1.0, -1.0),
        Corner::Ne => (from.left_bottom(), 1.0, -1.0),
        Corner::Sw => (from.right_top(), -1.0, 1.0),
        Corner::Se => (from.left_top(), 1.0, 1.0),
    };
    let w = (sx * (to.x - fixed.x)).max(MIN_VIDEO.x.max(least.x));
    let h = (sy * (to.y - fixed.y)).max(MIN_VIDEO.y.max(least.y) + BAND_H);
    Rect::from_two_pos(fixed, pos2(fixed.x + sx * w, fixed.y + sy * h))
}

/// W16M-3（不具合 ②）: helper の click の答え（`sent`・`ax: select`）が付けた caret の矩形（元の窓の中の割合 [x, y, w, h]・KangaeMirror.swift の
/// axAtPoint が選択を読み戻した位置の文字の左の辺）。押した（`ax: press`）・拒否・stats・最後の入力でない click の答え（`id` が `click_seq` でない）は None。
fn caret_of(ev: &serde_json::Value, click_seq: Option<u64>) -> Option<[f64; 4]> {
    if ev["kind"] != "sent" || ev["ax"] != "select" || click_seq.is_none() || ev["id"].as_u64() != click_seq {
        return None;
    }
    let c = &ev["caret"];
    Some([c["x"].as_f64()?, c["y"].as_f64()?, c["w"].as_f64()?, c["h"].as_f64()?])
}

/// caret（元の窓の中の割合）→ 映像（`shown` = `fit` の範囲）の上の幅 2 point の縦の線。`mirror_inputs` の点の写しの逆。
fn caret_rect(shown: Rect, meta: &MirrorFrameMeta, c: [f64; 4]) -> Rect {
    let to = |nx: f64, ny: f64| {
        let [px, py] = window_point_to_frame(meta, nx, ny);
        pos2(shown.min.x + (px / meta.width as f64) as f32 * shown.width(), shown.min.y + (py / meta.height as f64) as f32 * shown.height())
    };
    let (top, bottom) = (to(c[0], c[1]), to(c[0], c[1] + c[3]));
    Rect::from_min_size(top, Vec2::new(2.0, bottom.y - top.y))
}

/// helper の key の名前（KangaeMirror.swift の keyCodes）。文字を作らない key と、⌘ / ⌃ と組む英数字・space だけ。表に無い key は送らない（helper が bad_key で断る）。
fn key_name(key: egui::Key, chord: bool) -> Option<String> {
    use egui::Key as K;
    let named = match key {
        K::Enter => "return",
        K::Tab => "tab",
        K::Backspace => "delete",
        K::Delete => "forwarddelete",
        K::Escape => "escape",
        K::ArrowLeft => "left",
        K::ArrowRight => "right",
        K::ArrowUp => "up",
        K::ArrowDown => "down",
        K::Home => "home",
        K::End => "end",
        K::PageUp => "pageup",
        K::PageDown => "pagedown",
        K::Space if chord => "space",
        // 英数字（"A"・"0"）は ⌘ / ⌃ と組んだ時だけ（修飾の無い文字は Text が運ぶ）
        k if chord && k.name().len() == 1 && k.name().chars().all(|c| c.is_ascii_alphanumeric()) => return Some(k.name().to_lowercase()),
        _ => return None,
    };
    Some(named.into())
}

/// W16M-2: この frame の egui の入力のうち Mirror の矩形に向いた物 → `MirrorInput` の並び（純関数・`show` が毎 frame 呼ぶ）。
/// `shown` = 矩形の中で frame を描いた範囲（縦横比を保った余白を除く）・`meta` = 描いている frame・`focused` = Mirror の矩形が keyboard の focus を持つ。
/// 点 → frame の pixel（`(p - shown.min) × meta.width / shown.width()`）→ `frame_point_to_window`。余白と `shown` の外の点は捨てる。
/// pointer: primary の押下と離し 1 組 → `Click{count: 1}`（離した点が押した点から 4 point 以上動いていれば `Drag`）・`MouseWheel` → `Scroll`（直前の pointer の点）。
/// key（`focused` の時だけ）: `Text` → `Text`・文字を作らない key（return・tab・delete・escape・矢印・home / end / page）か ⌘ / ⌃ つきの key → `Key`
/// （egui が ⌘C / ⌘X / ⌘V を置き換えた `Copy`・`Cut`・`Paste` も `Key` に戻す）。修飾の無い文字の key は `Text` と二重に送らない。
/// 例: `for input in mirror_inputs(&ui.input(|i| i.events.clone()), shown, &meta, ui.memory(|m| m.has_focus(resp.id))) { pages.relay(&spec.id, input) }`
/// 保証する: 余白・矩形の外・focus の無い時の key は 0 件。保証しない: 2 回の click の判定（`count` は 1。語の選択は元の app を前に出す）。
pub fn mirror_inputs(events: &[egui::Event], shown: Rect, meta: &MirrorFrameMeta, focused: bool) -> Vec<MirrorInput> {
    let (kx, ky) = (meta.width as f64 / shown.width() as f64, meta.height as f64 / shown.height() as f64);
    let to_window = |p: Pos2| -> Option<[f64; 2]> {
        (shown.contains(p) && kx.is_finite() && ky.is_finite())
            .then(|| frame_point_to_window(meta, (p.x - shown.min.x) as f64 * kx, (p.y - shown.min.y) as f64 * ky))
            .flatten()
    };
    let mut out = Vec::new();
    let (mut press, mut hover): (Option<Pos2>, Option<Pos2>) = (None, None);
    for e in events {
        match e {
            Event::PointerMoved(p) => hover = Some(*p),
            Event::PointerGone => (press, hover) = (None, None),
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => (press, hover) = (Some(*pos), Some(*pos)),
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, .. } => {
                let Some((a, [x, y])) = press.take().and_then(|a| Some((a, to_window(a)?))) else { continue };
                if a.distance(*pos) < DRAG_MIN {
                    out.push(MirrorInput::Click { x, y, count: 1 });
                } else {
                    // 終点が余白や矩形の外でも drag は 1 つ（道が無いので NeedsFront の 1 行になる・点は矩形に寄せる）
                    let [x2, y2] = to_window(shown.clamp(*pos)).unwrap_or([x, y]);
                    out.push(MirrorInput::Drag { x, y, x2, y2, steps: None });
                }
            }
            Event::MouseWheel { unit, delta, .. } => {
                let Some([x, y]) = hover.and_then(to_window) else { continue };
                let per = match unit {
                    egui::MouseWheelUnit::Point => 1.0,
                    egui::MouseWheelUnit::Line => 40.0,
                    egui::MouseWheelUnit::Page => shown.height(),
                };
                let (dx, dy) = ((delta.x * per).round() as i32, (delta.y * per).round() as i32);
                match out.last_mut() {
                    // 同じ frame の続きの wheel は 1 つにまとめる（trackpad は 1 frame に何通も来る）
                    Some(MirrorInput::Scroll { dx: ax, dy: ay, .. }) => (*ax, *ay) = (*ax + dx, *ay + dy),
                    _ if dx != 0 || dy != 0 => out.push(MirrorInput::Scroll { x, y, dx, dy }),
                    _ => {}
                }
            }
            Event::Text(t) if focused && !t.is_empty() => out.push(MirrorInput::Text { text: t.clone() }),
            Event::Copy | Event::Cut | Event::Paste(_) if focused => {
                let key = match e {
                    Event::Copy => "c",
                    Event::Cut => "x",
                    _ => "v",
                };
                out.push(MirrorInput::Key { key: key.into(), mods: vec![KeyMod::Cmd] });
            }
            Event::Key { key, pressed: true, modifiers: m, .. } if focused => {
                let cmd = m.mac_cmd || (m.command && !m.ctrl);
                let Some(name) = key_name(*key, cmd || m.ctrl) else { continue };
                let mods = [(cmd, KeyMod::Cmd), (m.shift, KeyMod::Shift), (m.alt, KeyMod::Option), (m.ctrl, KeyMod::Control)];
                out.push(MirrorInput::Key { key: name, mods: mods.into_iter().filter_map(|(on, k)| on.then_some(k)).collect() });
            }
            _ => {}
        }
    }
    out
}

/// 形（帯と映像の高さ）が移る時間（秒）。UI-6a の契約（`motion::MotionMode` の doc）で大きさは Reduce Motion（Fade）でも動かさない =
/// `Full` の時だけ `motion::RAIL`・`Fade` と `Cut` は 0（押した frame で終わりの形）。`motion::dur` は不透明度と色の時間なので使わない（Fade で 0.12 s 滑った）。
fn shape_secs(ctx: &egui::Context) -> f32 {
    if motion::motion_mode(ctx) == motion::MotionMode::Full {
        motion::RAIL
    } else {
        0.0
    }
}

impl Drop for MirrorPages {
    fn drop(&mut self) {
        self.release_sources();
    }
}

impl MirrorPages {
    /// helper = KangaeMirror.swift を build した実行 file（`helper_path`）・consents = `ui_state(CONSENT_UI_STATE_SESSION)` の全行・
    /// workspace = Project の repo の dir（W16M-3・無ければ None = 窓を題名だけで選ぶ）。
    /// ponytail: workspace は最初に Mirror の面を描いた Project の物（面の鍵が surface id だけなのと同じ天井）。Project ごとに持つのは 2 つ目の Project に Mirror が要る時
    pub fn new(helper: PathBuf, consents: impl IntoIterator<Item = ConsentRecord>, workspace: Option<PathBuf>) -> MirrorPages {
        let frame_log = frame_log_path().and_then(|p| FrameLog::open(&p).map_err(|e| eprintln!("W16M-1b: {e}")).ok());
        // 試験（cfg(test)・tests/ の #[path] も）は実の app を起こさない: 動いている事にして何もしない
        // （`#[cfg(test)]` の属性の行にしない = interfaces.py と check_app_agnostic.py はその行から後ろを読まない）
        let launch: Launcher = if cfg!(test) { |id, _| Ok((id.rsplit('.').next().unwrap_or(id).to_string(), false)) } else { crate::surface::mirror::launch_app };
        let front: fn() -> Option<String> = if cfg!(test) { || None } else { crate::surface::mirror::front_app };
        MirrorPages {
            helper, gate: ConsentGate::from_records(consents), panes: HashMap::new(), granted: Vec::new(), revoked: HashSet::new(), awake: None, drawn: HashSet::new(), ctx: None, frame_log, natives: HashMap::new(), speed: Speed::default(), workspace,
            relay_log: relay_log_path(), windows: HashMap::new(), window_changes: Vec::new(), room: HashMap::new(), launch, front,
            activate: if cfg!(test) { |_| Ok(()) } else { crate::surface::mirror::activate_app },
            // 試験は実の bridge を起こさず Shibubu の窓も並べ替えない（偽の bridge と fn を入れた試験だけが G91 の道を通る）
            bridge: if cfg!(test) { Some(Err("No bridge in tests".into())) } else { None },
            order_above: crate::surface::order_host_above, order_below: crate::surface::order_host_below,
            pointer: if cfg!(test) { || (None, false) } else { crate::surface::mirror::pointer_now },
            idle: if cfg!(test) { || 0.0 } else { crate::surface::mirror::idle_secs },
            blank_backs: 0,
            fronted: 0,
            chosen: Vec::new(),
            pick_app: if cfg!(test) { || None } else { || crate::ui::STARTUP.get().and_then(|s| (s.pick_app)()) },
            pick: None,
            picked: None,
            filter: String::new(),
            shots: HashMap::new(),
            shooting: None,
            drag_armed: None,
            frames: Vec::new(),
            later: Vec::new(),
            stack: Vec::new(),
            last_focus: None,
            open: if cfg!(test) { |_, _, _| Ok(()) } else { open_app_window },
        }
    }

    /// G91: 映っている面の本物の窓を枠（`slot` = 映像の矩形の画面座標・`slot_on_screen`）と同じ位置・大きさに置き、Shibubu の窓の下にしまう（`show` が映っている面で毎 frame）。
    /// 最初の呼び出しで bridge を起こし（起きなければ理由を持って今の Mirror の道のまま）、`windows`（映している app の pid）を待たずに送る。
    /// 応答（`pump_docks`）の窓のうち CG の番号が映している窓（`MirrorTarget.window_id`）の物にだけ `Dock` を付ける（`front_on_attach = false`・
    /// `host_order = order_above` = 枠に合ったら Shibubu を真上へ = 本物は見えない）。許可が無い → Dock を作らず `tuck_err`（聞き直さない）。
    /// B89: 書類も題も選んでいない面は app の主窓（`pick_main_window`）に付け、映している窓と違えば主窓を映し直す。一覧にまだ無い窓・決まらない主窓は B14 と同じく
    /// `REFIT_GAP_S` 空けて `REFIT_MAX` 回まで聞き直してから、映している窓（app だけの面は動かせる窓の時だけ）に付けるか `tuck_err`。
    /// `tuck_err` は新しく映した窓（`take_answer`）と映している app の起動・前に来た・出た知らせで捨てる。
    /// 以後は `Dock::on_host` が枠の動きを追う（Shibubu の窓・枠が動いた・大きさが替わった）。`focused` = Shibubu の窓が前か（viewport の focused）:
    /// `direct` の間に一度前でなくなった Shibubu がまた前に来た（枠の外の click）→ `put_back`。
    /// MIRROR-HYBRID-1101: `direct` の間は人の静けさ（`idle`）を `DIRECT_POLL` ごとに読み、`IDLE_S` 何もしなければ `put_back`
    /// （pointer が Shibubu の窓の上なら `show` が Shibubu を前に）。`time` = ctx の time（秒）。
    /// 例: `if live { pages.tuck(&spec.id, slot_on_screen(&ctx, video), ctx.input(|i| i.viewport().focused), time) }`
    /// 保証する: app 別の分岐を持たない・対応の付かない窓を動かさない（当て推量で別の窓を重ねない）・UI の thread で `windows` を待たない。
    /// B90: 本物を枠へ合わせ直すのは人が枠を動かした合図の後（`FOLLOW_S`・`show` が `Pane::follow_until` を置く）・大きさの答えを待つ間・まだ置いていない Dock だけ
    /// （人が動かしていない間は set_frame 0・枠の替わりは最初の 3 回だけ app.log に 1 行）。
    /// L5-STICK: 直接触っている間に別の app が前に来たら本物を下へ・しまっている間に本物の app が前に来たら Shibubu を本物の真下へ（どちらも bridge の app_activated）。
    /// Shibubu の最小化・⌘H は `show` が本物も最小化する。
    /// 保証しない: 本物の窓が枠の大きさになる事（最小の大きさが枠より大きい app は Dock が合わないと言い、映像は `fit` で縦横比のまま収める）・
    /// Shibubu を別の Space へ移した時に本物が付いて行く事（他の app の窓を Space の間で動かす公開 API が無い）。
    fn tuck(&mut self, surface_id: &str, slot: Option<Frame>, focused: Option<bool>, time: f64) {
        self.bridge_up(surface_id);
        // B88（Desktop の 38 回目・REAPER: 起こした app の最初の窓が消え、Mirror は新しい主窓を映し直したのに、Dock は消えた窓のまま〈Dock は題で繋ぎ直すので
        // 題の違う主窓に付かない〉= 押すと isn't lined up with this frame・本物は枠の外のまま）: Dock の窓が今映している窓（CG の番号）でなくなったら Dock を捨て
        // （残っている前の窓は元の位置へ）、下の `windows` で今の窓に付け直す。前の窓に頼んだ大きさ・合わせ直しの数は持ち越さない（新しい窓も既定で枠の大きさへ・I31）
        let stale = self.panes.get(surface_id).is_some_and(|p| {
            let shown = p.surface.as_ref().ok().map(|m| m.target().window_id as i64);
            shown.is_some() && p.dock.as_ref().is_some_and(|d| d.window.as_ref().and_then(|w| w.cg_id) != shown)
        });
        if stale {
            eprintln!("B88: Mirror {surface_id}: 映している窓が替わったので、本物を置く Dock を今の窓に付け直します");
            self.untuck(surface_id);
            if let Some(p) = self.panes.get_mut(surface_id) {
                (p.prefit, p.refits, p.refit_at) = (None, 0, None);
            }
        }
        let Some(pane) = self.panes.get_mut(surface_id) else { return };
        let Ok(pid) = pane.surface.as_ref().map(|m| m.target().pid) else { return };
        // 追記 214: bridge が起きない（`Err`・起こし直さない）面は理由を持つ = 触った時に `stay_reason` が黙らない
        let b = match self.bridge.as_mut() {
            Some(Ok(b)) => b,
            Some(Err(e)) => {
                pane.tuck_err.get_or_insert_with(|| e.clone());
                return;
            }
            None => return,
        };
        // MIRROR-FIT-1101: 引き終わった大きさは本物の frame が替わる（読み戻し・resized）か RESIZE_WAIT_S で捨てる（断られたら枠は本物の大きさに戻る）
        if let Some((want, at, before)) = pane.resize {
            let now_frame = pane.dock.as_ref().and_then(|d| d.window.as_ref()?.frame);
            if now_frame != before || time - at >= RESIZE_WAIT_S {
                // B90: 答えの後の枠（断られた大きさ・反対の隅に合わせた位置）へも本物を置く（引いた人の手の続き）
                (pane.resize, pane.follow_until) = (None, time + FOLLOW_S);
                // 追記 240 の 2: 頼んだより LEAST_SLACK を超えて大きく残った向きは、その app の窓の最小（次から四隅で引く間にそこで止める = 離した後に断られない）。
                // 覚えるのは読み戻しが替わった時だけ（答えが無いまま RESIZE_WAIT_S を過ぎた = 遅いだけの app の元の大きさを最小と覚えない）
                let bigger = now_frame.map(|f| Vec2::new(f.w as f32 - want.x, f.h as f32 - want.y));
                if let (Some(f), Some(b), true) = (now_frame, bigger, now_frame != before) {
                    if b.x > LEAST_SLACK {
                        pane.least.x = f.w as f32;
                    }
                    if b.y > LEAST_SLACK {
                        pane.least.y = f.h as f32;
                    }
                }
                // 追記 218 の 3: 本物が引いた大きさを受けなかった（読み戻しが引いた大きさと違う）→ 帯に理由の 1 行（枠は本物の大きさに戻る）。
                // 押して枠に合わせる途中（214 の `fit_touch`）は右上の知らせが言うので重ねない。I31: 既定で枠の大きさへ頼んだ（`prefit`）のを断られた時も言う
                // （前の 235 は人が頼んでいないので言わなかった・既定で合わせるようになって、枠が本物に付いて行く・縮めて見せる理由を他に言う所が無い）
                if pane.fit_touch.is_none() && now_frame.is_some_and(|f| (f.w - want.x as f64).abs() > 1.0 || (f.h - want.y as f64).abs() > 1.0) {
                    let name = pane.surface.as_ref().map(|m| m.target().app_name.clone()).unwrap_or_default();
                    // 追記 240 の 2: 小さくできなかった = 枠は引いた所のまま本物を比のまま縮めて見せる（跳ね戻さない）・大きくできなかった = 枠が本物の大きさになる
                    let line = if bigger.is_some_and(|b| b.x > 1.0 || b.y > 1.0) {
                        format!("{name}'s window can't be that small · showing it scaled")
                    } else {
                        format!("{name}'s window didn't take that size · the frame follows the window")
                    };
                    pane.band_line = Some((RegionState::Partial, line, crate::surface::now_ms()));
                }
            }
        }
        if let Some(d) = pane.dock.as_mut() {
            d.tuck = pane.direct.is_none();
            d.host_order = Some(if d.tuck { self.order_above } else { self.order_below });
            // L5-STICK: Shibubu と一緒に本物を最小化している間は Hidden のまま（本物の minimized の通知を外れたと数えない・最小化した窓へ合わせ直さない）
            // B90（42 回目 30bafd46 の Orca: 本物が枠に付いて来るまで の 1 行が 1385 行以上・人が触っていない間も本物が動き続けて揺れた）: 本物を枠へ
            // 合わせ直すのは、人が枠を動かした合図の後（`FOLLOW_S`）・引いた / 頼んだ大きさを待つ間・まだ一度も置いていない Dock（付けた直後・付け直した後）だけ。
            // 他の枠の替わり（layout の揺れ・app が自分で替えた大きさ）では本物を動かさない（前は毎 frame 追った）。最初の 3 回の替わりは app.log に 1 行
            let follow = time < pane.follow_until || pane.resize.is_some() || d.tracker.last_target.is_none();
            let held = !follow && slot != pane.slot_now && slot.zip(d.tracker.last_target).is_some_and(|(s, t)| s.error_px(&t) >= 0.5);
            if let (true, Some(s), Some(t)) = (held, slot, d.tracker.last_target) {
                pane.held_moves += 1;
                if pane.held_moves <= 3 {
                    eprintln!("B90: Mirror {surface_id}: 人が動かしていない間に枠が {} から {} に替わったので、本物は動かしません（{} 回目）", t.text(), s.text(), pane.held_moves);
                }
            }
            pane.slot_now = slot;
            d.on_host(b, Host { slot: slot.filter(|_| follow), minimized: pane.host_min, ..Host::default() }, Instant::now());
            // G91 改 2: 外れた本物（外から動かされた・答えが無い）は、直接触っていない間だけ `REFIT_GAP_S` 空けて合わせ直す（`REFIT_MAX` 回まで）。
            // MIRROR-FIT-1101: 大きさは本物の物（`slot`）なので位置だけを戻す・四隅 / Fit to frame の大きさは `refit_at` を今にして待たずに送る
            if d.life == Lifecycle::Degraded && pane.direct.is_none() && pane.refits < REFIT_MAX {
                let at = *pane.refit_at.get_or_insert(time + REFIT_GAP_S);
                if time >= at && d.redock(b, Host { slot, ..Host::default() }, Instant::now()) {
                    (pane.refits, pane.refit_at) = (pane.refits + 1, None);
                }
            }
        } else if pane.tuck_wait.is_none() && pane.tuck_err.is_none() && pane.refit_at.is_none_or(|at| time >= at) {
            pane.refit_at = None;
            match b.send("windows", serde_json::json!({ "pid": pid })) {
                Ok(id) => pane.tuck_wait = Some(id),
                Err(e) => pane.tuck_err = Some(e.to_string()),
            }
        }
        // L5-STICK ①: 前に来た app（bridge の app_activated・取ったら消える = 来た時の 1 回だけ数える）
        let came = pane.dock.as_mut().and_then(|d| d.activated.take());
        let (host, src) = (std::process::id(), pid as u32);
        // しまっている間に映している app が前に来た（Cmd+Tab・Dock の icon・Open original・遅れて着いた自分の raise）: 本物だけが他の app の窓の上に
        // 浮かないよう、Shibubu を本物の真下へ置いて直接触る間にする（owner 109 通目 10「本物の窓も一緒に」）
        // L5-STICK ①（DOGFOOD-X 周 2 回 1）: Shibubu が起こした app が起動の自分の activate で前に来た（`open -g` でも REAPER は前に出る・人の手でない）→
        // 直接触る間にせず Shibubu を本物の真上へ（本物は Shibubu の後ろのまま・前面は G90 の見張りが戻す）。起こしてから 1 回だけ
        let own = came == Some(src) && pane.launched.take_if(|t| crate::surface::now_ms() - *t < LAUNCH_WAIT_MS).is_some();
        if pane.direct.is_none() && own {
            match pane.dock.as_ref().and_then(|d| d.window.as_ref()).and_then(|w| w.cg_id) {
                Some(cg) => {
                    if let Err(e) = (self.order_above)(cg as u32) {
                        eprintln!("L5-STICK: Mirror {surface_id}: 起こした app の窓の真上へ Shibubu を置けません: {e}");
                    }
                }
                None => eprintln!("L5-STICK: Mirror {surface_id}: 本物の窓の番号が無いので Shibubu を真上へ置けません"),
            }
        } else if pane.direct.is_none() && came == Some(src) {
            if let Some(d) = pane.dock.as_mut() {
                (d.host_order, d.tuck) = (Some(self.order_below), false);
                match d.window.as_ref().and_then(|w| w.cg_id) {
                    Some(cg) => {
                        if let Err(e) = (self.order_below)(cg as u32) {
                            eprintln!("L5-STICK: Mirror {surface_id}: 前に来た本物の窓の真下へ Shibubu を置けません: {e}");
                        }
                    }
                    None => eprintln!("L5-STICK: Mirror {surface_id}: 本物の窓の番号が無いので Shibubu を真下へ置けません"),
                }
            }
            // Some(false) = Shibubu が前でなくなったのをまだ見ていない（focused の遅れで同じ frame に戻さない）
            (pane.direct, pane.reclaim) = (Some(false), None);
        }
        // MIRROR-HYBRID-1101: 直接触っている間に人が IDLE_S 何もしない（どの app の keyboard / mouse も）か、本物が前に出て Shibubu が前でなくなった後に
        // Shibubu がまた前に来た（枠の外の click）→ 本物を下へ。G91 改の「pointer が枠の外へ離れたら戻す」はやめた（触っている間は pointer が外へ出ても本物のまま）
        let idle = pane.direct.is_some() && (self.idle)() >= IDLE_S;
        // L5-STICK ①: 直接触っている間に Shibubu でも映している app でもない app が前に来た（Cmd+Tab・別の app の click）→ pointer が枠の上に
        // 残っていても待たずに本物を Shibubu の下へ（Shibubu と一緒に後ろへ）
        let third = pane.direct.is_some() && came.is_some_and(|p| p != host && p != src);
        let back = third || idle || matches!((pane.direct, focused), (Some(true), Some(true)));
        if let (Some(false), Some(false)) = (pane.direct, focused) {
            pane.direct = Some(true);
            // MIRROR-HYBRID-1101: 触ってから本物が前に来た（Shibubu が前でなくなった）までを app.log に 1 行
            if let Some(t) = pane.touch.take() {
                eprintln!("MIRROR-HYBRID-1101: Mirror {surface_id}: 触ってから本物が前に出るまで {:.0} ms", crate::surface::now_ms() - t);
            }
        }
        if back {
            self.put_back(surface_id);
        }
    }

    /// MIRROR-HYBRID-1101（owner 207「両ドリ」）: 触った面（映像の押し・focus の在る面への打鍵）の本物を枠の位置のまま前に出して直接触らせる
    /// （`host_order = order_below`・`Dock::bring_front` = AXRaise + activate・位置は動かさない）。G90 の前面の見張り（`reclaim`）を捨てる（人が触るために前に出した本物を
    /// Shibubu へ引き戻さない）。触った押しは呼ぶ側（`show`）が離した所で `relay_front` で本物へ送り直す（G91 の「もう一度 click」は無い）。
    /// scroll・枠の移動で戻した後に枠が止まった時も呼ぶ（送り直しは無い）。Dock が無い・合っていない面は何もしない（`show` が理由を右上に 1 行・Mirror の道のまま）。
    fn go_direct(&mut self, surface_id: &str) {
        let (Some(p), Some(Ok(b))) = (self.panes.get_mut(surface_id), self.bridge.as_mut()) else { return };
        let Some(d) = p.dock.as_mut().filter(|d| matches!(d.life, Lifecycle::Active | Lifecycle::Resizing)) else { return };
        (p.reclaim, p.direct, p.line, p.resume, p.touch) = (None, Some(false), None, None, Some(crate::surface::now_ms()));
        // 追記 235 の 2: 今 止まっている pointer では出し直さない（何もしない 10 秒・1 秒の戻しの後に同じ所で繰り返さない）
        if let Some(w) = p.dwell.as_mut() {
            w.2 = true;
        }
        // L5-STICK ①: 前に来た app は前に出した後の物だけ数える（前に出す前の物で戻さない）
        (d.host_order, d.tuck, d.activated) = (Some(self.order_below), false, None);
        // B90: 人が動かしていない間に替わった枠（本物を動かさなかった）へ、前に出す前に置く（同じ所なら何も送らない・bridge は届いた順に処理する）
        d.on_host(b, Host { slot: p.slot_now, ..Host::default() }, Instant::now());
        d.bring_front(b);
        // 追記 214（spec の 1「前に出す手を Shibubu 本体から」）: bridge は前面の app でない（Contents/Helpers の別 bundle）ので macOS 14 の協調の activate を
        // 黙って断られうる。今 前面の Shibubu からも同じ app を前へ。来なければ `FRONT_WAIT_MS` で下へ戻して理由（show）
        let bundle = d.spec.app_identity.bundle_id.clone();
        if let Err(e) = (self.activate)(&bundle) {
            eprintln!("MIRROR-HYBRID-1101: Mirror {surface_id}: Shibubu から {bundle} を前に出せません: {e}");
        }
    }

    /// G91: 本物を Shibubu の下へ戻す（`host_order = order_above` にして Shibubu を真上へ・`direct` を捨てる）。枠の外の click（Shibubu が前に戻った `tuck`）・
    /// Page の切り替え（描かなかった面の `end_frame`）。本物は枠の位置に置いたまま（Page が見えない間も Shibubu の下）。
    /// MIRROR-HYBRID-1101: 戻す間（`resume`）は捨てる（scroll・移動で戻した呼ぶ側が戻した後に置き直す）。戻した瞬間に Mirror の絵が無ければ app.log に 1 行。
    fn put_back(&mut self, surface_id: &str) {
        let Some(p) = self.panes.get_mut(surface_id) else { return };
        (p.resume, p.touch) = (None, None);
        let (Some(_), Some(d)) = (p.direct.take(), p.dock.as_mut()) else { return };
        if p.texture.is_none() {
            self.blank_backs += 1;
            eprintln!("MIRROR-HYBRID-1101: Mirror {surface_id}: 本物を下へ戻した時に Mirror の絵が無かった（起動から {} 回）", self.blank_backs);
        }
        (p.refits, p.refit_at) = (0, None);
        (d.host_order, d.tuck) = (Some(self.order_above), true);
        match d.window.as_ref().and_then(|w| w.cg_id) {
            Some(cg) => {
                if let Err(e) = (self.order_above)(cg as u32) {
                    eprintln!("G91: Mirror {surface_id}: 本物の窓を Shibubu の下へ戻せません: {e}");
                }
            }
            None => eprintln!("G91: Mirror {surface_id}: 本物の窓の番号が無いので Shibubu の下へ戻せません"),
        }
    }

    /// MIRROR-FIT-1101（spec 3「Fit to frame の button は残してよい」）: 人が `Fit to frame`（題の帯の double-click）を押した: 本物を今の映像の大きさ `size` にする
    /// （四隅で引き終わった時と同じ `resize`・外れた Dock は `tuck` が待たずに送り直す）。呼ぶのは `misfit` が在る時だけ。直接触っている面は何もしない。
    fn fit_to_frame(&mut self, surface_id: &str, size: Vec2, time: f64) {
        let Some(p) = self.panes.get_mut(surface_id).filter(|p| p.direct.is_none()) else { return };
        let before = p.dock.as_ref().and_then(|d| d.window.as_ref()?.frame);
        (p.resize, p.refits, p.refit_at) = (Some((size, time, before)), 0, Some(time));
    }

    /// MIRROR-FIT-1101: 帯の右に `Fit to frame` を出す理由（hover の 1 行）。① 本物の窓が置ける所より大きく、映像（`video`）を縮めて見せている
    /// `{App}'s window is larger than this page · showing it scaled`（押すと本物をこの映像の大きさに）② 本物が外から動かされ、自動の合わせ直しを使い切った
    /// `{App}'s window was moved off the frame · showing it here`（押すと位置を合わせ直す）。合っている・引いた大きさを待つ間・直接触っている・Dock の無い面は None。
    fn misfit(&self, surface_id: &str, video: Rect) -> Option<String> {
        let p = self.panes.get(surface_id)?;
        let d = p.dock.as_ref().filter(|_| p.direct.is_none() && p.resize.is_none() && p.grab.is_none())?;
        let name = &d.spec.app_identity.name;
        let real = d.window.as_ref().and_then(|w| w.frame);
        if matches!(d.life, Lifecycle::Active | Lifecycle::Resizing) && real.is_some_and(|r| (video.width() as f64) < r.w - 1.0 || (video.height() as f64) < r.h - 1.0) {
            return Some(format!("{name}'s window is larger than this page · showing it scaled"));
        }
        (d.life == Lifecycle::Degraded && p.refits >= REFIT_MAX).then(|| format!("{name}'s window was moved off the frame · showing it here"))
    }

    /// MIRROR-FIT-1101: 本物の窓を置く所（画面の point）= 映像の左上・大きさは本物の物（四隅・Fit to frame・I31 の既定の大きさを待つ間は頼んだ大きさ）=
    /// 置くだけでは本物の大きさを替えない（既定で枠の大きさへ頼むのは `show` の prefit）。
    /// 本物の大きさが読めない（Dock の前）は映像の矩形。
    fn slot(&self, surface_id: &str, ctx: &egui::Context, video: Rect) -> Option<Frame> {
        let s = slot_on_screen(ctx, video)?;
        let p = self.panes.get(surface_id);
        let size = p.and_then(|p| p.resize.map(|(v, ..)| (v.x as f64, v.y as f64)).or_else(|| p.dock.as_ref()?.window.as_ref()?.frame.map(|f| (f.w, f.h))));
        Some(match size {
            Some((w, h)) => Frame { w, h, ..s },
            None => s,
        })
    }

    /// MIRROR-FIT-1101（owner 200）× 追記 218 の 3: 映っている開いた枠（`rect` = 題の帯 + 映像）の四隅の取っ手。選ばなくても隅に pointer を載せると斜めの矢印・引ける
    /// （四角の印は選んだ枠と引いている間だけ）。MIRROR-HYBRID-1101 追記 240・246: 引いている間は引いた所が今の枠（`sized` と `pin` → `frame_size` と `shift` = 後ろの部品は
    /// 縦の流れで下へどく・絵は伸ばして描く・本物は離すまで動かさない）・本物が断った最小（`least`）で止まる・Page の下の端の外へ出たら Page を流して伸ばす。
    /// 離すとその映像の大きさを本物へ送り（`resize`）、枠を Project へ（`take_frames` = ⌘Z で戻る）。Dock の窓が無い面は Page の上の枠の大きさだけで、帯に理由の 1 行
    /// （`app` = app の名前）。引き始めに本物が前に出ていれば Shibubu の下へ戻す
    /// I33: `band` = 題の帯の題の所（`title_band` の 4 つ目・押すと窓を選ぶ menu・double-click で枠へ合わせる）。引くと枠ごと動かす（`Corner::Move`）
    fn handles(&mut self, ui: &mut egui::Ui, surface_id: &str, rect: Rect, band: &egui::Response, time: f64, t: &theme::Tokens, app: &str) {
        let ctx = ui.ctx().clone();
        let page = ui.clip_rect();
        let pos = ctx.input(|i| i.pointer.interact_pos());
        let Some(p) = self.panes.get_mut(surface_id) else { return };
        // MIRROR-FIT-1101: 並びの所（ずらす前の枠の左上・`shift`）。I33b: 引ける所は Page の列（`room`）= 並びの所より左・上へも（前の部品に重ねてよい）
        let origin = rect.min - p.shifted;
        let room = p.room.translate(origin.to_vec2());
        // 追記 246 の 2: 下へは Page の見えている所の外へも引ける（枠が伸びた分 Page が伸び、`edge_scroll` が流す = 枠を止めない）・横は Page の列の幅まで（横の scroll は無い）
        let bound = Rect::from_min_max(page.min.max(room.min), pos2(page.max.x.min(room.max.x), f32::INFINITY));
        let (mut back, mut stop) = (false, false);
        for (c, at) in [(Corner::Nw, rect.left_top()), (Corner::Ne, rect.right_top()), (Corner::Sw, rect.left_bottom()), (Corner::Se, rect.right_bottom())] {
            let r = ui.interact(Rect::from_center_size(at, Vec2::splat(HANDLE_HIT)), egui::Id::new((surface_id, "mirror-corner", format!("{c:?}"))), Sense::drag());
            if r.hovered() || r.dragged() {
                ctx.set_cursor_icon(if matches!(c, Corner::Nw | Corner::Se) { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
            }
            if r.drag_started() {
                p.grab = Some(c);
                back |= p.direct.is_some();
            }
            stop |= r.drag_stopped();
        }
        // I33: 題の帯の題の所を引くと枠ごと動かす（四隅を引いていない時だけ・掴んだ点を覚える・前に出ていた本物は下へ）
        if band.drag_started_by(PointerButton::Primary) && p.grab.is_none() {
            p.grab = Some(Corner::Move(ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.min) - rect.min));
            back |= p.direct.is_some();
            // I33b: 動かしている枠が一番上（`floating` の順）・離した後もそのまま
            self.windows.entry(surface_id.to_string()).or_default().lifted = time;
        }
        stop |= matches!(p.grab, Some(Corner::Move(_))) && band.drag_stopped();
        // 追記 240 の 1・246: 引いた所が今の枠（向かいの隅は今の枠の物）。引いた隅の反対を Page の上に残し（並びの所から見た所・次の frame から `shift` が枠の左上をずらす）、
        // 引いた大きさを `frame_size` へ（後ろの部品は下へどく）
        let f = p.grab.zip(pos).map(|(c, to)| (c, dragged(c, rect, to, bound, p.least)));
        if let Some((c, f)) = f {
            let anchor = match c {
                Corner::Nw => f.right_bottom(),
                Corner::Ne => f.left_bottom(),
                Corner::Sw => f.right_top(),
                Corner::Se | Corner::Move(_) => f.left_top(),
            };
            (p.pin, p.sized) = (Some((c, anchor - origin)), Some(Vec2::new(f.width(), f.height() - BAND_H)));
        }
        if stop {
            let moved = matches!(p.grab, Some(Corner::Move(_)));
            p.grab = None;
            if let Some((_, f)) = f {
                // 送るのは映像の大きさ（帯の下）
                let video = Vec2::new(f.width(), f.height() - BAND_H);
                if moved {
                    // I33: 動かしただけ = 大きさは頼まない。本物は離した後の枠へ Dock が 1 回（引いている間は `show` の frozen で 0・離した = B90 の合図）
                } else if p.dock.as_ref().is_some_and(|d| d.window.is_some()) {
                    let before = p.dock.as_ref().and_then(|d| d.window.as_ref()?.frame);
                    (p.resize, p.refits, p.refit_at) = (Some((video, time, before)), 0, Some(time));
                } else {
                    // 追記 218 の 3: 本物の窓をしまう口（Dock）の無い面 = Page の上の枠の大きさだけ替える（映像は枠に縦横比のまま収める）・帯に理由の 1 行
                    p.band_line = Some((RegionState::Partial, format!("{app}'s window keeps its size · only the frame changed"), crate::surface::now_ms()));
                }
                // 追記 246 の 3: 離したら Project へ（並びの所からのずれと枠の大きさ・⌘Z で枠とどいた部品が 1 回で戻る）
                let saved = SurfaceFrame { x: (f.min.x - origin.x) as f64, y: (f.min.y - origin.y) as f64, w: f.width() as f64, h: f.height() as f64 };
                p.frame_sent = Some(saved);
                self.frames.push((surface_id.to_string(), saved));
            }
        }
        // 引いている間: 引いた所に枠の線・Page の下の端の外へ出たら Page を流す（枠は次の frame でその大きさ）
        let shown = match f.filter(|_| p.grab.is_some()) {
            Some((_, f)) => {
                ui.painter().rect_stroke(f, 0.0, egui::Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Outside);
                theme::edge_scroll(ui, true);
                ctx.request_repaint();
                f
            }
            None => rect,
        };
        // 四角の印は選んだ枠と引いている間だけ（選んでいない枠は隅の斜めの矢印だけ）
        if p.selected || p.grab.is_some() {
            for at in [shown.left_top(), shown.right_top(), shown.left_bottom(), shown.right_bottom()] {
                ui.painter().rect(Rect::from_center_size(at, Vec2::splat(HANDLE)), 1.0, t.surface_1, egui::Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
            }
        }
        if back {
            self.put_back(surface_id);
        }
    }

    /// G91: 黄（Minimize mirror）で本物の窓も最小化し（bridge の `minimize`・Dock は Hidden = 追わない）、開く時に解く（Dock が枠へ合わせ直す）。Dock が無い面は何もしない。
    fn minimize_source(&mut self, surface_id: &str, minimized: bool) {
        let (Some(p), Some(Ok(b))) = (self.panes.get_mut(surface_id), self.bridge.as_mut()) else { return };
        let Some(d) = p.dock.as_mut() else { return };
        let Some(handle) = d.window.as_ref().map(|w| w.handle.clone()) else { return };
        p.direct = None;
        if minimized {
            // 追わない（Hidden）。開く時は次の `tuck` の on_host が Hidden → Active に戻して枠へ合わせ直す
            d.on_host(b, Host { minimized: true, ..Host::default() }, Instant::now());
        }
        if let Err(e) = b.send("minimize", serde_json::json!({ "handle": handle, "minimized": minimized })) {
            eprintln!("G91: Mirror {surface_id}: 本物の窓の最小化を替えられません: {e}");
        }
    }

    /// G91: 本物の窓を Shibubu が動かす前の位置・大きさ（`Dock::original`）へ戻して Dock を捨てる（赤 = Closed・Shibubu の終了 = `Drop`）。
    fn untuck(&mut self, surface_id: &str) {
        let Some(p) = self.panes.get_mut(surface_id) else { return };
        let Some(mut d) = p.dock.take() else { return };
        (p.direct, p.tuck_wait) = (None, None);
        if let Some(Ok(b)) = self.bridge.as_mut() {
            d.close(b);
        }
    }

    /// G91 / MIRROR-QUIET-1101: bridge を起こす（1 回だけ・起きなければ理由を持って今の Mirror の道のまま・試験は偽の bridge か Err を先に入れておく）。
    /// 映った面の `tuck` と、Mirror の面を描く `show`（窓の無い面が app の起動の知らせ app_launched を受けてすぐ探すため）が呼ぶ
    fn bridge_up(&mut self, surface_id: &str) {
        if self.bridge.is_none() {
            // ponytail: 起こすのは UI の thread（ready まで最長 5 秒・最初に Mirror の面を描いた 1 回だけ）。混んだ機械で止まりが目立てば裏の thread で起こす
            let b = crate::surface::find_bridge().and_then(|p| Bridge::spawn(&p, Duration::from_secs(5))).map_err(|e| e.to_string());
            if let Err(e) = &b {
                eprintln!("G91: Mirror {surface_id}: 本物の窓をしまう口が起きません（今の Mirror のまま）: {e}");
            }
            self.bridge = Some(b);
        }
    }

    /// G91: bridge に届いた物（`windows` の応答・Dock の要求の応答・窓と app の event）を全部の面へ配る（`end_frame` が毎 frame 1 回）。
    /// MIRROR-QUIET-1101: 映す app が起きた・前に来た・隠れていたのが出た（app_launched / app_activated / app_unhidden）→ その app の窓の無い面はすぐ探す。
    fn pump_docks(&mut self) {
        let Some(Ok(b)) = self.bridge.as_mut() else { return };
        let msgs = b.poll();
        let (now, order_above) = (Instant::now(), self.order_above);
        // B89: 主窓を映し直す時の口（`show` の同意の後と同じ open）
        let (gate, helper, relay_log) = (&self.gate, &self.helper, &self.relay_log);
        // B14: 答えなかった `windows` を聞き直す時刻の時計（`tuck` の `time` と同じ ctx の秒）
        let time = self.ctx.as_ref().map_or(0.0, |c| c.input(|i| i.time));
        for v in msgs.iter().filter_map(|m| match m {
            Msg::Event(v) => Some(v),
            _ => None,
        }) {
            if matches!(v["event"].as_str(), Some("app_launched" | "app_activated" | "app_unhidden")) {
                let bundle = v["bundle_id"].as_str();
                let mut hit = false;
                for p in self.panes.values_mut().filter(|p| p.surface.is_err() && p.source.as_ref().is_some_and(|(want, _)| Some(want.as_str()) == bundle)) {
                    // 探している途中（10 秒の間で出した探し）なら、その答えの後を 2 秒の間から数え直す・探していなければ今すぐ
                    p.since = None;
                    if p.pending.is_none() {
                        p.tried_ms = f64::NEG_INFINITY;
                    }
                    hit = true;
                }
                // B89: 映している app が起きた・前に来た・出た → 本物を置けなかった面（一覧に無い・答えない）は、その時の窓の一覧で Dock をもう 1 度探す（主窓を選び直す）
                for p in self.panes.values_mut().filter(|p| p.dock.is_none() && p.tuck_err.is_some() && p.surface.as_ref().is_ok_and(|m| Some(m.target().bundle_id.as_str()) == bundle)) {
                    (p.tuck_err, p.refits, p.refit_at) = (None, 0, None);
                    hit = true;
                }
                // 印を付けたのは frame の終わり（show の後）: 背景で止まっている Shibubu でも次の frame の show が探すよう描き直しを頼む
                if let Some(c) = self.ctx.as_ref().filter(|_| hit) {
                    c.request_repaint();
                }
            }
        }
        for (id, p) in self.panes.iter_mut() {
            for m in &msgs {
                match (p.tuck_wait, m) {
                    (Some(w), Msg::Reply { id: rid, result }) if *rid == w => {
                        p.tuck_wait = None;
                        let target = p.surface.as_ref().map(|s| s.target().clone()).ok().or_else(|| p.target.clone());
                        match (result, target) {
                            (Ok(v), Some(t)) => {
                                let list = crate::surface::parse_windows(v);
                                // B89（41 回目・Project「Humming to REAPER」: 起こした REAPER の窓が出た直後に映した窓〈起動の評価版の dialog か浮いた panel〉が
                                // 本物の窓の一覧と対応せず、押すと Can't bring REAPER here · it stays a mirror のまま）: 書類も題も選んでいない面（`file` が None =
                                // Any window）は、その app の主窓（`pick_main_window` = 普通の窓で外から動かせる main・Dock が置ける窓と同じ条件）を映して置く。
                                // 主窓が決まらない（動かせる窓が無い・幾つも在って決められない・CG の番号が付かない）間は聞き直し、使い切ったら映している窓で今まで通り
                                // （app だけの面は動かせる窓の時だけ = 動かせない dialog に Dock を付けない・`tuck_err` なら app が前に来た時にもう 1 度探す）
                                let any = p.source.as_ref().is_some_and(|(_, f)| f.is_none());
                                let main = crate::surface::pick_main_window(&list).ok().filter(|w| any && w.cg_id.is_some());
                                let wait = any && main.is_none() && p.refits < REFIT_MAX;
                                let pick = main.or_else(|| list.into_iter().filter(|w| !wait && (!any || w.movable)).find(|w| w.cg_id == Some(t.window_id as i64)));
                                // 主窓が映している窓でなければ映し直す（同じ app の窓・同意も同じ app の物）。stream の無い間・映し直せない時は Dock を付けない
                                let aim = pick.as_ref().and_then(|w| w.cg_id).filter(|c| *c != t.window_id as i64).map(|c| MirrorTarget { window_id: c as u64, ..t.clone() });
                                let pick = match aim {
                                    None => pick,
                                    Some(_) if p.surface.is_err() => None,
                                    Some(a) => match gate.check(&a).and_then(|g| RelayLog::open(relay_log).and_then(|log| MirrorSurface::open(helper, a.clone(), g, log))) {
                                        Ok(s) => {
                                            eprintln!("B89: Mirror {id}: 映していた窓 {} は {} の主窓でないので、主窓 {} を映し直します", t.window_id, t.app_name, a.window_id);
                                            // 前の窓の映像・押しかけ・描いた IOSurface・前の窓に頼んだ大きさは持ち越さない（新しい窓の frame で描き直す）
                                            (p.surface, p.target, p.texture, p.last, p.line, p.held, p.caret) = (Ok(s), Some(a), None, None, None, None, None);
                                            (p.view, p.shown, p.prefit) = (None, [None, None], None);
                                            pick
                                        }
                                        Err(e) => {
                                            p.tuck_err = Some(e.to_string());
                                            None
                                        }
                                    },
                                };
                                match pick {
                                    Some(w) => {
                                        let app = AppIdentity { bundle_id: t.bundle_id.clone(), name: t.app_name.clone(), pid: Some(t.pid as u32), ..AppIdentity::default() };
                                        let mut d = Dock::new(app, std::process::id());
                                        (d.spec.surface_id, d.front_on_attach, d.host_order, d.tuck) = (id.clone(), false, Some(order_above), true);
                                        // MIRROR-FIT-1101: 枠の方が本物の大きさに合わせる（本物の大きさは替えない・本物が自分で替えたら枠が追う）
                                        d.follow_size = true;
                                        d.attach_to(b, w, Host::default());
                                        p.dock = Some(d);
                                        // B14: 聞き直した数は Dock の合わせ直しの数と別に数える
                                        (p.refits, p.refit_at) = (0, None);
                                    }
                                    // 映し直せなかった（理由は tuck_err）・映していない（次に映った tuck が聞き直す）
                                    None if p.tuck_err.is_some() || p.surface.is_err() => {}
                                    // B89: 映している窓が一覧にまだ無い（起こした app の窓ができた直後・CG の番号がまだ付かない）・主窓を待つ → B14 と同じく
                                    // `REFIT_GAP_S` 空けて `REFIT_MAX` 回まで聞き直す（その間の帯は isn't ready here yet）
                                    None if p.refits < REFIT_MAX => {
                                        (p.refits, p.refit_at) = (p.refits + 1, Some(time + REFIT_GAP_S));
                                        eprintln!("B89: Mirror {id}: 映している窓 {} がまだ本物の窓の一覧に無い・主窓が決まらないので、{REFIT_GAP_S} 秒後に聞き直します（{} 回目）", t.window_id, p.refits);
                                    }
                                    // 当て推量で別の窓を重ねない（今の Mirror の映像のまま）
                                    None => p.tuck_err = Some(format!("{}'s window isn't in the window list, so it stays where it is", t.app_name)),
                                }
                            }
                            // B14（DOGFOOD-X 周 5: 映した直後の Numbers の windows が 1 回「app が応答しません」で、その面は二度と Dock を付けず、
                            // 枠に合わせる・前に出す事が無いまま stays a mirror だった）: 答えない（AX の cannot_complete・bridge の時間切れ）は
                            // `REFIT_GAP_S` 空けて `REFIT_MAX` 回まで聞き直す（その間の帯は isn't ready here yet）。許可が無い等は今まで通り聞き直さない
                            (Err(e), _) if matches!(e.code(), "ax_cannot_complete" | "cannot_complete" | "timeout") && p.refits < REFIT_MAX => {
                                (p.refits, p.refit_at) = (p.refits + 1, Some(time + REFIT_GAP_S));
                                eprintln!("B14: Mirror {id}: 本物の窓の一覧が答えません（{e}）・{REFIT_GAP_S} 秒後に聞き直します（{} 回目）", p.refits);
                            }
                            (Err(e), _) => p.tuck_err = Some(e.to_string()),
                            (Ok(_), None) => {}
                        }
                        if let Some(e) = &p.tuck_err {
                            eprintln!("G91: Mirror {id}: 本物の窓をしまいません: {e}");
                        }
                    }
                    _ => {
                        if let Some(d) = p.dock.as_mut() {
                            let before = d.metrics.samples.last().copied();
                            d.on_msg(b, m.clone(), now);
                            // L5-STICK ②: 枠が動いたのを見た frame から本物の読み戻しまでが FOLLOW_MS を超えた時だけ（実機の追従を app.log で数える）
                            if let Some(s) = d.metrics.samples.last().filter(|s| Some(**s) != before && s.lag_ms > FOLLOW_MS) {
                                eprintln!("L5-STICK: Mirror {id}: 本物が枠に付いて来るまで {:.0} ms（目安 {FOLLOW_MS:.0} ms）", s.lag_ms);
                            }
                            // MIRROR-HYBRID-1101 追記 240 の 4: 引き終わってから本物の読み戻しが来るまで（RESIZE_WAIT_S を過ぎた古い印は数えない）
                            if d.metrics.samples.last().is_some_and(|s| Some(*s) != before) {
                                if let Some(ms) = p.released.take().map(|t| crate::surface::now_ms() - t).filter(|ms| *ms < RESIZE_WAIT_S * 1000.0) {
                                    eprintln!("MIRROR-HYBRID-1101 追記 240: Mirror {id}: 離してから本物が合うまで {ms:.0} ms");
                                }
                            }
                        }
                    }
                }
            }
            if let Some(d) = p.dock.as_mut() {
                d.tick(b, now);
            }
        }
    }

    /// UI-5m: 面の今の窓の形（替えたことが無ければ `Open`）。
    /// Wave: UI-5m
    pub fn window(&self, surface_id: &str) -> MirrorWindow {
        self.windows.get(surface_id).map_or(MirrorWindow::Open, |s| s.now)
    }

    /// MIRROR-PIN-1102: 面が今映している本物の窓と、人が最後にこの面を触った時刻（`now_ms`・触っていなければ NEG_INFINITY）。
    /// 返すのは helper の stream が付いている時だけ（`MirrorSurface` が Ok）。探している・閉じた（Closed）・空の枠・同意待ち・知らない面は None。
    /// 触った = 映像を押して枠を選んだ押し（`selected`・本物を前に出せたかに依らない）。止まった pointer・scroll の後の戻しは数えない。
    /// 呼ぶ側 = `Generated::engine_request`（`pin_window` で Engine に渡す窓を決める）。
    /// 例: `if let Some((t, touched)) = pages.shown("garageband-guitar") { (t.pid, t.window_id) }`
    /// Wave: MIRROR-PIN-1102
    pub fn shown(&self, surface_id: &str) -> Option<(MirrorTarget, f64)> {
        let p = self.panes.get(surface_id)?;
        p.surface.as_ref().ok().map(|s| (s.target().clone(), p.touched))
    }

    /// G94: 拡大（緑）の面を全部 `Open` に戻す（animation 無し）。拡大の間は Page の他の部品（足した Terminal・Browser）が描かれず、見せる手（reveal）が捨てられるので、見せる前に呼ぶ。
    /// 例: `pages.unzoom()`。保証する: 拡大の面が無ければ何もしない・戻した形は `take_window_changes` に出る（再起動を跨ぐ）。
    pub fn unzoom(&mut self) {
        let ids: Vec<String> = self.windows.iter().filter(|(_, s)| s.now == MirrorWindow::Zoomed).map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.set_window(&id, MirrorWindow::Open, f64::NEG_INFINITY);
        }
    }

    /// UI-5m: 窓の形を替える（3 つの button・帯・Esc が `show` の中で呼ぶ）。t = ctx の time（秒・animation の起点）。同じ形なら何もしない。
    /// `Closed` は helper の stream を止める（`MirrorSurface` を落とす = `end_frame` が `ActivityHold` を放す）・`Closed` から開くと同意を尋ねずに
    /// 窓を探し直す（次の `show` が裏の thread で attach）・`Zoomed` は他の面の `Zoomed` を `Open` に戻す（拡大は 1 つ）。替えた形は `take_window_changes` に出る。
    /// 例: `pages.set_window("docs", MirrorWindow::Minimized, ctx.input(|i| i.time))`
    /// 保証する: `Closed` の面は `Open` に替えるまで stream も relay も 0 件・`Minimized` の面は stream を保つが texture に上げない。
    /// Wave: UI-5m
    pub fn set_window(&mut self, surface_id: &str, w: MirrorWindow, t: f64) {
        let was = self.window(surface_id);
        if was == w {
            return;
        }
        // G91: 赤 = 本物を元の位置へ・黄 = 本物も最小化（取り込みも止める）・黄から開く = 最小化を解く
        match w {
            MirrorWindow::Closed => self.untuck(surface_id),
            MirrorWindow::Minimized => self.minimize_source(surface_id, true),
            _ if was == MirrorWindow::Minimized => self.minimize_source(surface_id, false),
            _ => {}
        }
        if w == MirrorWindow::Zoomed {
            let others: Vec<String> = self.windows.iter().filter(|(id, s)| s.now == MirrorWindow::Zoomed && id.as_str() != surface_id).map(|(id, _)| id.clone()).collect();
            for id in others {
                self.set_window(&id, MirrorWindow::Open, t);
            }
        }
        let s = self.windows.entry(surface_id.to_string()).or_default();
        *s = Shape { now: w, was: s.now, t0: t, from_h: s.last_h, ..*s };
        if let Some(p) = self.panes.get_mut(surface_id) {
            // 畳んだら押しかけの押下と caret は持ち越さない
            (p.held, p.caret, p.click_seq) = (None, None, None);
            // G91: 本物をしまった面の黄も取り込みを止める（最小化した窓は映らない）・Dock は持ち越す（開いて映れば同じ窓を枠へ合わせ直す）
            if w == MirrorWindow::Closed || (w == MirrorWindow::Minimized && p.dock.is_some()) {
                // 取り込みを止める = MirrorSurface を落とす（helper の stream が止まる）。texture は畳む animation の間だけ残す（畳み終わった show が捨てる）。
                // 窓を探している途中の答えも捨てる（着いても使わない）。開く時は新しい Pane の tried_ms = -∞ で次の show がすぐ探す（同意は gate に在る）
                let (keep_alive, texture, last, dock) = (p.keep_alive, p.texture.take(), p.last, p.dock.take());
                *p = Pane { keep_alive, texture, last, dock, ..Pane::new(Err(MirrorError::WindowGone)) };
            }
        }
        self.window_changes.push((surface_id.to_string(), w));
    }

    /// UI-5m: 保存した形で始める（起動の時に 1 回・`MirrorPages::new` の後）。`Zoomed` は `Open` で戻し、animation はしない・`take_window_changes` に出さない。
    /// 閉じた・しまった面は帯のまま、`show` は窓を探さず stream を始めない（帯を押すまで）。
    /// 例: `pages.restore_windows(store.list_ui_state(WINDOW_UI_STATE_SESSION)?.into_iter().filter_map(|(id, v)| Some((id, serde_json::from_value(v).ok()?))))`
    /// Wave: UI-5m
    pub fn restore_windows(&mut self, saved: impl IntoIterator<Item = (String, MirrorWindow)>) {
        for (id, w) in saved {
            let now = if w == MirrorWindow::Zoomed { MirrorWindow::Open } else { w };
            self.windows.insert(id, Shape { now, was: now, ..Shape::default() });
        }
    }

    /// MIRROR-AUTO-1101 B: Settings › Privacy で取り消した app（`mirror-consent` の行の `{"revoked": true}`）。その app だけ自動で同意せず `Mirror here` を待つ。
    /// 例: `pages.restore_revoked(rows.filter(|(_, v)| v["revoked"] == true).map(|(b, _)| b))`
    /// Wave: MIRROR-AUTO-1101
    pub fn restore_revoked(&mut self, revoked: impl IntoIterator<Item = String>) {
        self.revoked.extend(revoked);
    }

    /// LAYOUT（operator 10-31）: Page の高さのための畳み（下の panel を開いた・窓が低い = `short`・床が入らない）。足りなくなった時に開いている面を 1 回だけ黄にし、
    /// 足りるようになったら自分で畳んだ面だけ開き直す。利用者の手（`set_window`）と違い保存しない（`take_window_changes` に出さない = 再起動を跨がない）。
    /// 利用者が自分で畳んだ面は利用者の物（保存する・戻さない）。足りない間に利用者が開き直した面は畳み直さない。
    /// 戻すのは Shibubu が前面の時だけ（`front`・L5 の規則「Shibubu が前でない時は本物を前に出さない」= 最小化を解くと本物が前に出うる）。前でない間は黄のまま待つ。
    /// 承認の前の面は Dock が無いので本物に触らない（`minimize_source` が何もしない）。
    /// 例（`source_surface`・毎 frame）: `pages.fold_for_room(&spec.id, !grows && rest - below < floor, ui.ctx().input(|i| i.focused), ui.ctx().input(|i| i.time));`
    /// Wave: LAYOUT
    pub fn fold_for_room(&mut self, surface_id: &str, short: bool, front: bool, t: f64) {
        let now = self.window(surface_id);
        let (acted, ours) = self.room.get(surface_id).copied().unwrap_or_default();
        // 利用者が開き直した・閉じた黄は Shibubu の物でない
        let ours = ours && now == MirrorWindow::Minimized;
        let (acted, ours, to) = match (short, now) {
            (true, MirrorWindow::Open) if !acted => (true, true, Some(MirrorWindow::Minimized)),
            (true, _) => (acted, ours, None),
            (false, MirrorWindow::Minimized) if ours && !front => (acted, ours, None),
            (false, MirrorWindow::Minimized) if ours => (false, false, Some(MirrorWindow::Open)),
            (false, _) => (false, false, None),
        };
        self.room.insert(surface_id.to_string(), (acted, ours));
        if let Some(w) = to {
            let saved = self.window_changes.len();
            self.set_window(surface_id, w, t);
            self.window_changes.truncate(saved);
        }
    }

    /// UI-5m: `set_window` で替わった形（呼ぶ側が `ui_state(WINDOW_UI_STATE_SESSION, surface id)` に保存する）。取ったら空になる。
    /// Wave: UI-5m
    pub fn take_window_changes(&mut self) -> Vec<(String, MirrorWindow)> {
        std::mem::take(&mut self.window_changes)
    }

    /// MIRROR-FIT-1101（owner 198 の 8・200）: この面の枠（上の題の帯 + 映像）の大きさ。枠は映している本物の窓に合わせる = 映像は本物の窓の大きさ
    /// （Dock が読んだ AX の frame の幅・高さ。四隅で引き終わって本物の読み戻しを待つ間はその大きさ）。`room` = 置ける所（幅・その形の高さ）に入らない時は
    /// 縦横の比を保って縮める（本物の窓は動かさない）。Dock の無い面（許可が無い・まだ対応が付かない）は映像の中身の縦横の比で `room` に収める・映像も無ければ `room` のまま。
    /// 呼ぶ側（`source_surface`）はこの高さを `height` の open / zoom に渡し、この幅で矩形を取る（枠の外に灰色の余白を作らない）。
    /// 例: `let open = pages.frame_size(&spec.id, Vec2::new(w, open_h)); let zoom = pages.frame_size(&spec.id, Vec2::new(w, rest)); let h = pages.height(ctx, &spec.id, open.y, zoom.y);`
    /// MIRROR-HYBRID-1101 追記 240・246: 四隅を引いている間は引いた大きさそのまま。人が選んだ大きさ（`Pane::sized` = 引き終わり・Project の `frame`）の在る開いた面は、
    /// room.y（後ろの部品の分を残した高さ）の代わりに選んだ高さ（後ろの部品が下へどき Page が伸びる）・本物がそれより大きい（断った）時は比のまま縮めて収める（跳ね戻さない）。
    /// I31（owner 273）: 人が大きさを選んでいない Dock の面は置ける所の全部（`show` が本物をその映像の大きさへ頼む）。頼んだ大きさを本物が受けなかった・
    /// 後で自分で替えた（`Pane::prefit` と違う）間は本物の大きさ（上の通り）。
    /// 保証する: 幅 ≤ room.x・高さ ≤ room.y（選んだ面は選んだ高さ・room が `BAND_H` より低い時は `BAND_H`）・本物の大きさより大きくしない
    /// （Dock の在る面で、人が大きさを選んだ・既定の大きさを本物が受けなかった時）。
    /// Wave: MIRROR-FIT-1101
    pub fn frame_size(&self, surface_id: &str, room: Vec2) -> Vec2 {
        let p = self.panes.get(surface_id);
        if let Some(s) = p.filter(|p| p.grab.is_some()).and_then(|p| p.sized) {
            return Vec2::new(s.x.min(room.x), s.y + BAND_H);
        }
        let chosen = p.and_then(|p| p.sized).filter(|_| self.window(surface_id) == MirrorWindow::Open);
        let room = match chosen {
            Some(c) => Vec2::new(room.x.min(c.x), c.y + BAND_H),
            None => room,
        };
        // I31（owner 273「全画面出すようにdefaultで」・274「mirrorの枠とreaperの枠があってない」）: 人が大きさを選んでいない Dock の面は置ける所の全部が枠。
        // 前は MIRROR-FIT-1101 の「既定では大きさを替えない」で枠が小さい本物に縮んだ。まだ頼んでいない・本物が受けた間だけ（大きさの答えを待つ間は頼んだ大きさ）
        let docked = p.filter(|p| chosen.is_none() && p.resize.is_none()).and_then(|p| Some((p.dock.as_ref()?.window.as_ref()?.frame?, p.prefit)));
        if docked.is_some_and(|(f, asked)| asked.is_none_or(|s| (f.w - s.x as f64).abs() <= 1.0 && (f.h - s.y as f64).abs() <= 1.0)) {
            return Vec2::new(room.x, room.y.max(BAND_H));
        }
        // 追記 218 の 3: Dock の無い面で四隅を引いた枠（`sized`）はその大きさ（本物の大きさが読めれば本物が先）
        let real = p.and_then(|p| p.resize.map(|(v, ..)| v).or_else(|| p.dock.as_ref()?.window.as_ref()?.frame.map(|f| Vec2::new(f.w as f32, f.h as f32))).or(p.sized));
        // 映像の中身の縦横（Dock の前・許可の無い面）
        let aspect = p.and_then(|p| p.last).map(|m| if m.content[2] > 0.0 && m.content[3] > 0.0 { Vec2::new(m.content[2] as f32, m.content[3] as f32) } else { Vec2::new(m.width as f32, m.height as f32) });
        let video = Vec2::new(room.x, (room.y - BAND_H).max(0.0));
        let size = match (real, aspect) {
            (Some(r), _) if r.x >= 1.0 && r.y >= 1.0 => r * (video.x / r.x).min(video.y / r.y).min(1.0),
            (_, Some(a)) if a.x >= 1.0 && a.y >= 1.0 => a * (video.x / a.x).min(video.y / a.y),
            _ => return Vec2::new(room.x, room.y.max(BAND_H)),
        };
        Vec2::new(size.x, size.y + BAND_H)
    }

    /// MIRROR-FIT-1101（operator 21:1x「NW / NE / SW の隅を引いて離すと枠が左上に寄る」・owner 200）: 開いた枠を Page の並びの所（左上）から右・下へずらす量。
    /// 右下以外の隅で大きさを替えた枠は、引いた隅の反対の隅（`Pane::pin`）を Page の上に残す = 枠の左上 = 反対の隅 − 枠の大きさ（`size` = `frame_size` の open）。
    /// 本物が引いた大きさを断った・自分で替えた時も反対の隅に合わせる。I33b: `room`（Page の列）からはみ出さない。
    /// 開いた形（`MirrorWindow::Open`）の間だけ・他の形は 0。呼ぶ側は `size + shift` の矩形を取り、その左上 + shift に枠を描く（Dock の slot も同じ所 = 映像の左上）。
    /// I33b（owner 11-03「自由に」）: `room` = Page の列の矩形を並びの所（ずらす前の枠の左上）から見た物（左・上は 0 以下・下は無限）。枠は `room` の中ならどこへでも
    /// （並びの所より左・上 = 前の部品に重ねてよい・横は Page の列の幅まで）。答えが負なら呼ぶ側の取る矩形は 0 で止め、描く所は `slot.min + shift` のまま。
    /// 例: `let s = pages.shift(&spec.id, fit_open, Rect::from_min_max(page.min - at.to_vec2(), pos2(page.max.x - at.x, f32::INFINITY))); let (slot, _) = ui.allocate_exact_size((Vec2::new(fw, h) + s).max(Vec2::ZERO).min(Vec2::new(w, f32::INFINITY)), Sense::hover()); let rect = Rect::from_min_size(slot.min + s, Vec2::new(fw, h));`
    /// Wave: MIRROR-FIT-1101, I33b
    pub fn shift(&mut self, surface_id: &str, size: Vec2, room: Rect) -> Vec2 {
        let open = self.window(surface_id) == MirrorWindow::Open;
        let Some(p) = self.panes.get_mut(surface_id) else { return Vec2::ZERO };
        let tl = match p.pin.filter(|_| open) {
            // 反対の隅から枠の大きさだけ戻した所（右下を引いた枠は左上がそのまま）
            Some((c, at)) => Vec2::new(
                if matches!(c, Corner::Nw | Corner::Sw) { at.x - size.x } else { at.x },
                if matches!(c, Corner::Nw | Corner::Ne) { at.y - size.y } else { at.y },
            ),
            // 追記 246 の 3: 再起動・⌘Z の後（引いた隅を知らない）は Project の枠のずれ
            None => p.frame_seen.filter(|f| open && f.valid()).map_or(Vec2::ZERO, |f| Vec2::new(f.x as f32, f.y as f32)),
        };
        p.room = room;
        p.shifted = Vec2::new(tl.x.min(room.max.x - size.x).max(room.min.x), tl.y.max(room.min.y));
        p.shifted
    }

    /// I33b: 面の枠を Page の木の後で描く（`floating`）ために、この frame の矩形を覚える。egui の同じ layer では後に描いた物が描きも hit も上なので、
    /// 前の部品・他の列・他の枠に重ねた枠が上に来る。呼ぶ側（`source_surface`）は並びの所で `show` を呼ばず、`render_with` が木の後で `floating` の順に描く。
    /// 例: `pages.later(&spec.id, rect)`
    /// Wave: I33b
    pub fn later(&mut self, surface_id: &str, rect: Rect) {
        self.later.push((surface_id.to_string(), rect));
    }

    /// I33b: この frame に覚えた枠を下から上の順（最後に題の帯で動かした枠が最後 = 一番上・同じなら覚えた順）。覚えた枠は取り出す（同じ frame にもう一度 Page を描いても前の Page の枠を描き直さない）
    /// （下の枠が上の枠に隠れているかを `show` が見る）。
    /// 例: `for (id, rect) in pages.floating() { pages.show(ui, rect, spec_of(&id)) }`
    /// Wave: I33b
    pub fn floating(&mut self) -> Vec<(String, Rect)> {
        let windows = &self.windows;
        let lifted = |id: &str| windows.get(id).map_or(f64::NEG_INFINITY, |s| s.lifted);
        let mut stack = std::mem::take(&mut self.later);
        stack.sort_by(|a, b| lifted(&a.0).total_cmp(&lifted(&b.0)));
        self.stack = stack.clone();
        stack
    }

    /// UI-5m: この frame で面に取る高さ（題の帯を含む）。open = 開いた時の高さ・zoom = 拡大の時の高さ（Page の中身の残り）。畳んだ形 = `BAND_H`。
    /// 形を替えた後 `motion::RAIL`（reduce motion なら 0 = 1 frame）の間は替えた時の高さから ease で移り、その間だけ repaint を頼む。
    /// 呼ぶ側（`source_surface`）はこの高さで矩形を取って `show` に渡す（下の region が空いた所を使う・拡大は Page の中身を取る）。
    /// 例: `let h = pages.height(ui.ctx(), &spec.id, (rest - below).clamp(floor, 1440.0), rest); let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());`
    /// Wave: UI-5m
    pub fn height(&mut self, ctx: &egui::Context, surface_id: &str, open: f32, zoom: f32) -> f32 {
        let s = self.windows.entry(surface_id.to_string()).or_default();
        let p = motion::progress(ctx, s.t0, shape_secs(ctx));
        let to = match s.now {
            // 拡大から戻る間は他の region が隠れたまま（幅が Page の全部）なので、呼ぶ側の open でなく前に開いて落ち着いた高さへ
            MirrorWindow::Open if s.was == MirrorWindow::Zoomed && p < 1.0 => s.open_h.unwrap_or(open),
            MirrorWindow::Open => open,
            MirrorWindow::Zoomed => zoom,
            MirrorWindow::Minimized | MirrorWindow::Closed => BAND_H,
        };
        let h = if p >= 1.0 || s.from_h <= 0.0 { to } else { s.from_h + (to - s.from_h) * motion::ease_out(p) };
        if s.now == MirrorWindow::Open && p >= 1.0 {
            s.open_h = Some(open);
        }
        s.last_h = h;
        h
    }

    /// UI-5m: 拡大の形の面（拡大から別の形へ移る animation の間も）。renderer はこの面の `source_surface` を含まない node を描かない（Page の他の region を隠す・
    /// 祖先の容れ物は描く = AX の node の親を替えない）。
    /// 例: `if pages.zoomed(ui.ctx()) == Some(sid) { /* Page の中身の残りの高さ */ }`
    /// Wave: UI-5m
    pub fn zoomed(&self, ctx: &egui::Context) -> Option<&str> {
        let (now, d) = (ctx.input(|i| i.time), shape_secs(ctx) as f64);
        let leaving = |s: &Shape| s.was == MirrorWindow::Zoomed && now - s.t0 < d;
        let pick = |f: &dyn Fn(&Shape) -> bool| self.windows.iter().find(|(_, s)| f(s)).map(|(id, _)| id.as_str());
        pick(&|s| s.now == MirrorWindow::Zoomed).or_else(|| pick(&leaving))
    }

    /// G91: Shibubu を閉じる時、しまった本物の窓を全部元の位置・大きさへ戻す（殻の `on_exit` と `Drop` の両方から・eframe は終了で app を drop しない事がある）。
    pub fn release_sources(&mut self) {
        let ids: Vec<String> = self.panes.iter().filter(|(_, p)| p.dock.is_some()).map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.untuck(&id);
        }
    }

    /// 製品の helper の置き場: `KANGAE_MIRROR_HELPER` → .app の `Contents/Helpers/kangae-mirror-helper`（MIRROR-QUIET-1101: Contents/MacOS に居ると
    /// Shibubu の bundle として check in する）→ この実行 file の隣（cargo の target/<profile>・前の .app）。
    /// 製品は swiftc を呼ばない（無ければ `show` が IntegrationFailed と 1 行）。
    pub fn helper_path() -> PathBuf {
        std::env::var_os("KANGAE_MIRROR_HELPER").map(PathBuf::from).unwrap_or_else(|| {
            let exe = std::env::current_exe().unwrap_or_default();
            // canonicalize = 在る時だけ（`..` を畳んだ path が PROCS の鍵と HelperMissing の 1 行になる）
            exe.parent()
                .and_then(|d| std::fs::canonicalize(d.join("../Helpers/kangae-mirror-helper")).ok())
                .unwrap_or_else(|| exe.with_file_name("kangae-mirror-helper"))
        })
    }

    /// W14b（使える検査の ①）: その面に元の窓の映像が 1 枚でも来たか。None = まだ描いていない（`show` を呼んでいない）・
    /// Ok = 映像が来た・Err = 来ない理由の枠の状態（同意・許可 = `PermissionRequired`・窓が無い = `SourceUnavailable`・open 済みで待っている = `Attaching`）。
    /// 読むだけ（helper を呼ばない）。UI-5m: 人が閉じた面（`MirrorWindow::Closed`）は None（映していない・帯を押すまで何も起こさない）。
    /// Wave: W14b, UI-5m
    pub fn reach(&self, surface_id: &str) -> Option<Result<(), PortalState>> {
        if self.window(surface_id) == MirrorWindow::Closed {
            return None;
        }
        let pane = self.panes.get(surface_id)?;
        Some(match (&pane.surface, &pane.last) {
            (Err(e), _) => Err(state_of(e)),
            (Ok(_), Some(_)) => Ok(()),
            (Ok(_), None) => Err(PortalState::Attaching),
        })
    }

    /// 窓を探し、同意があれば open する。同意が無ければ `NoConsent` と見つけた窓。want = 書類の絶対 path（W16M-3・`MirrorTarget::find`）・
    /// ask = 許可を尋ねてから書類を引く（M4 ③・button を押した時だけ）。
    /// G87: launch = 探す前に app を起こす（`Launcher` と if_stopped）。起こしたら探さず `Starting`（窓はまだ無い）・入っていなければ `NotInstalled`・
    /// 動いていれば探す（窓が無ければ `NoWindow` の名前を launcher の表示名に）。
    fn attach(helper: &Path, relay_log: &Path, gate: &ConsentGate, bundle_id: &str, file: Option<&str>, want: Option<&Path>, ask: bool, launch: Option<(Launcher, bool)>) -> (Option<MirrorTarget>, Result<MirrorSurface, MirrorError>) {
        if !helper.exists() {
            return (None, Err(MirrorError::HelperMissing(helper.to_path_buf())));
        }
        let name = match launch.map(|(f, if_stopped)| f(bundle_id, if_stopped)) {
            Some(Ok((app_name, true))) => return (None, Err(MirrorError::Starting { app_name })),
            Some(Ok((app_name, false))) => Some(app_name),
            Some(Err(e)) => return (None, Err(e)),
            None => None,
        };
        // 題名は file の名前（TextEdit の窓の題名は path ではなく名前）
        let title = file.map(|f| Path::new(f).file_name().and_then(|n| n.to_str()).unwrap_or(f));
        let target = match MirrorTarget::find(helper, bundle_id, title, want, ask) {
            Ok(t) => t,
            // 書類の path で探した時の 1 行は Project の中の file（docs/notes.md）で言う（同じ名前の別の書類の窓が開いていても、それは選ばない）
            Err(MirrorError::NoWindow { app_name, .. }) if want.is_some() => return (None, Err(MirrorError::NoWindow { app_name, title_has: file.map(str::to_string) })),
            Err(MirrorError::NoWindow { app_name, title_has }) => return (None, Err(MirrorError::NoWindow { app_name: name.unwrap_or(app_name), title_has })),
            Err(MirrorError::Ambiguous { app_name, count, ask_pid, denied, .. }) => return (None, Err(MirrorError::Ambiguous { app_name, title_has: file.map(str::to_string), count, ask_pid, denied })),
            Err(e) => return (None, Err(e)),
        };
        let opened = gate.check(&target).and_then(|g| MirrorSurface::open(helper, target.clone(), g, RelayLog::open(relay_log)?));
        (Some(target), opened)
    }

    /// 毎 frame、Mirror の Page の矩形 `rect` に描く。最初の呼び出しで `MirrorTarget::find`（`SurfaceTarget::App{bundle_id, file}` の file の名前を題名で探し、workspace が在れば書類の path で選ぶ・W16M-3）→
    /// 同意の確認 → `MirrorSurface::open`。以後は `stream` の最新の frame を texture に上げ、元の窓の縦横比のまま矩形に収めて描く（余白は背景色）。
    /// 矩形の AX の identifier は `AxScope::SurfaceFrame.id(&spec.id)`（試験の側の E0 が写真のどこを OCR するかをこれで知る）。
    /// 窓を探す手（`attach`）は裏の thread で走らせ、答えを次の frame で受け取る（helper が答えない間も描画・入力・AX を止めない・W16M-3）。
    /// 返す枠の状態: 映っている = `Mirror`・最初の窓探しの答えを待つ / open 済みで frame がまだ = `Attaching`（探し直しの間は前の Err の状態のまま）・同意が無い / 画面収録の許可が無い = `PermissionRequired`
    /// （同意の 1 行と button〈AX identifier `consent_button_id`〉か、許可の 1 行）・窓が無い / 消えた = `SourceUnavailable`・helper が動かない /
    /// backend が Mirror でない / target が App でない = `IntegrationFailed`。どの状態でも、前に映した frame を今の物として出し続けない。
    /// W16M-2: 映っている間だけ矩形が click・drag・keyboard の focus を取る（押す・AX の AXPress / AXFocused で focus・他の所を押すと放す）。
    /// focus の間の key と矩形の上の pointer を `mirror_inputs` で `MirrorInput` に直して `relay` へ。拒否の 1 行（`relay` の Err と helper の
    /// 答えの `refusal`）は `LINE_MS` の間だけ映像の上に出し、面の状態は `Mirror` のまま。映っていない間は矩形は何も取らない（押しても何も起きない物を出さない）。
    /// UI-5m: `rect` は題の帯を含む面の全部（高さは呼ぶ側が `height` で決める）。上の `BAND_H` は題の帯（赤黄緑 = AX の Button `Close mirror` /
    /// `Minimize mirror` / `Zoom mirror`〈拡大中は `Restore mirror`〉と `{App} · {書類}`）で、frame の node・`mirror_inputs`・`fit` は帯の下の映像の矩形だけ。
    /// 開いている時の frame の node の役と名前（`Mirroring · TextEdit` 等）は変えない。畳んだ形（`Minimized`・`Closed`）が落ち着いたら frame の node は出さず、
    /// 帯の全部が 1 つの Button `Open mirror · {App}`（押す / Enter で `Open`）。畳んだ面は窓を探さず frame を読まず repaint も頼まない。stream の無い畳んだ面（閉じた面・再起動の後のしまった面）は `PortalState::Undiscovered` を返す。
    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, spec: &SurfaceSpec) -> PortalState {
        self.drawn.insert(spec.id.clone());
        self.bridge_up(&spec.id);
        let ctx = ui.ctx().clone();
        let t = theme::tokens(&ctx);
        let ax = crate::ui::ax::AxScope::SurfaceFrame.id(&spec.id);
        // UI-5m: 上の BAND_H は題の帯。映像・frame の node・入力は帯の下の矩形だけ（帯の上の click は元の app へ送らない）
        let (band, video) = split_band(rect);
        let child = |ui: &mut egui::Ui| ui.new_child(UiBuilder::new().id_salt(&ax).max_rect(video.shrink(theme::SP_2)).layout(egui::Layout::top_down(egui::Align::Min)));
        // 矩形の場所は呼ぶ側（source_surface）が取ってある。ここは cursor を進めるだけ（id は面ごとに固定＝focus が frame をまたいで残る）
        ui.advance_cursor_after_rect(rect);
        let (bundle_id, file) = match (&spec.backend, &spec.target) {
            (SurfaceBackendKind::Mirror, SurfaceTarget::App { bundle_id, file }) => (bundle_id.clone(), file.clone()),
            _ => {
                let s = PortalState::IntegrationFailed;
                states::inline(&mut child(ui), s.region(), "Only app windows can be mirrored");
                return s;
            }
        };
        let now = crate::surface::now_ms();
        // UI-5m: 窓の形。畳んだ形（しまう・閉じる）は窓を探さず frame も読まない・形を替えた後の RAIL の間（moving）は入力を取らない
        let time = ctx.input(|i| i.time);
        let window = self.window(&spec.id);
        let folded = matches!(window, MirrorWindow::Minimized | MirrorWindow::Closed);
        let moving = self.windows.get(&spec.id).is_some_and(|s| time - s.t0 < shape_secs(&ctx) as f64);
        // 畳む / 開く animation の映像は、開いて落ち着いた時の映像の矩形の下の辺に揃えて描く（上の辺から隠れる = 上から下へ）
        let open_video_h = self.windows.get(&spec.id).and_then(|s| s.open_h).map(|h| h - BAND_H);
        let mut want: Option<MirrorWindow> = None;
        // 他の面が探している間（裏の thread の `list` が動いていて、着けばその面が stream を開く）は探さない（2 本の `list` を並べない）。
        // G69: 他の面が映っている間は探してよい。G32 の後は `MirrorTarget::find` が映っている helper の process の stdin で探し、
        // `MirrorSurface::open` も同じ process に stream を足すので、別の process の `list --all` が動いている stream を -3805 で止める道が無い
        // ponytail: 窓がまだ無い面は `search_gap`（20 秒までは 2 秒・その後 10 秒）ごとに常駐の process の中で SCShareableContent を呼び続ける
        // （3 回までは落ちない = G32 の probe・何分も続けた時は未測）。負荷か落ちが出たら、映っている面が在る間の探し直しの間隔を伸ばす
        // G82: 答えない探し（helper の list が混んで返らない）は `WAIT_OTHER_MS` までしか待たない
        let other_searching = self.panes.iter().any(|(id, p)| id != &spec.id && p.pending.is_some() && now - p.tried_ms < WAIT_OTHER_MS);
        // RQ4（RQ-019）: Project の側で映す窓が替わった（`Choose window…`・その Undo / Redo）→ 今の stream を落として新しい語で探し直す（本物は元の位置へ）
        let source = (bundle_id.clone(), file.clone());
        if self.panes.get(&spec.id).is_some_and(|p| p.source.as_ref().is_some_and(|s| *s != source)) {
            self.untuck(&spec.id);
            if let Some(p) = self.panes.get_mut(&spec.id) {
                let (keep_alive, name) = (p.keep_alive, p.name.take());
                // MIRROR-FRAME-1101: 空の枠に入れた app（`Open an app…` = Some(false)）は最初の探しで後ろで起こす（入れる前に決めた印を持ち越す）
                let open_app = p.source.as_ref().filter(|(b, _)| b.trim().is_empty()).and(p.open_app);
                *p = Pane { keep_alive, name, open_app, ..Pane::new(Err(MirrorError::WindowGone)) };
            }
        }
        // B33: 押していない面の守りが focus を返す先（`last_focus`）に、どの Mirror の面も選ばない（面を 2 つ並べた時）
        let frames: Vec<egui::Id> = self.panes.keys().map(|id| egui::Id::new(crate::ui::ax::AxScope::SurfaceFrame.id(id))).collect();
        let pane = self.panes.entry(spec.id.clone()).or_insert_with(|| Pane::new(Err(MirrorError::WindowGone)));
        pane.keep_alive = spec.keep_alive;
        pane.source = Some(source);
        // MIRROR-HYBRID-1101 追記 246 の 3: Project の枠（`SurfaceSpec::frame`）が替わった。自分が送った物の戻りでなければ（⌘Z・⇧⌘Z・再起動の後の初め）、その大きさと
        // ずれで描き（`sized`・`shift`）、本物をその映像の大きさへ頼む（Dock の窓が在り直接触っていない時）
        if pane.frame_seen != spec.frame && pane.grab.is_none() {
            pane.frame_seen = spec.frame;
            if spec.frame != pane.frame_sent {
                let chosen = spec.frame.filter(|f| f.valid()).map(|f| Vec2::new(f.w as f32, (f.h as f32 - BAND_H).max(MIN_VIDEO.y)));
                (pane.sized, pane.pin, pane.frame_sent) = (chosen, None, None);
                let before = pane.dock.as_ref().and_then(|d| d.window.as_ref()).map(|w| w.frame);
                if let (Some(v), Some(before), None) = (chosen, before, pane.direct) {
                    (pane.resize, pane.refits, pane.refit_at) = (Some((v, time, before)), 0, Some(time));
                }
            }
        }
        // MIRROR-QUIET-1101: app の起動の知らせ（bridge の reader が Waker を起こす）で背景の Shibubu も描き直す・pump_docks が描き直しを頼む口
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
        if let Some(Ok(b)) = &self.bridge {
            b.wake_with(|| Waker::from(Arc::new(Repaint(ctx.clone()))));
        }
        // MIRROR-QUIET-1101: Page が見えた（前の pass にこの面を描いていない）→ 窓の無い面はすぐ探す（2 秒の間から数え直す）
        let pass = ctx.cumulative_pass_nr();
        if pane.seen_pass + 1 < pass && pane.pending.is_none() && pane.surface.is_err() {
            pane.tried_ms = f64::NEG_INFINITY;
        }
        pane.seen_pass = pass;
        // MIRROR-FRAME-1101: 中身の無い枠は窓を探さず app を起こさない（帯 `Mirror frame`・真ん中の `Choose a window`・引いて入れる = `blank`）
        if spec.blank_frame() {
            return self.blank(ui, rect, spec, &ax, &t);
        }
        let app = match (&pane.surface, &pane.target) {
            (Ok(m), _) => m.target().app_name.clone(),
            (_, Some(t)) => t.app_name.clone(),
            // G87: launcher と前の答えが言った表示名・再起動の後の閉じた面（窓を探していない）は bundle id の最後の語
            _ => pane.name.clone().unwrap_or_else(|| bundle_id.rsplit('.').next().unwrap_or(&bundle_id).to_string()),
        };
        let direct = pane.direct.is_some();
        // 追記 214: 光る（縁を focus の色・帯の Using …）のは本物が前に来たのを読み戻してから（Shibubu が前でなくなった = direct が Some(true)）
        let lit = pane.direct == Some(true);
        let title = match file.as_deref().and_then(|f| Path::new(f).file_name()).and_then(|n| n.to_str()) {
            // G91: 本物が前に出ている間は映像が本物に覆われるので、帯の 1 行で今と戻り方を言う
            _ if lit => direct_line(&app),
            Some(doc) => format!("{app} · {doc}"),
            None => app.clone(),
        };
        if folded && !moving {
            // 畳み終わった: 帯の全部が 1 つの Button だけ（frame の node は出さない = 状態の語が嘘にならない）。閉じた面は前の映像も捨てる・repaint も頼まない
            if window == MirrorWindow::Closed {
                (pane.texture, pane.last, pane.shown) = (None, None, [None, None]);
            }
            let st = if pane.surface.is_ok() { pane_state(pane, false) } else { PortalState::Undiscovered };
            if folded_band(ui, band, &ax, &title, &app, &t).clicked() {
                self.set_window(&spec.id, MirrorWindow::Open, time);
            }
            return st;
        }
        // W16M-3（製品の不具合・BG で実測）: 窓を探す helper の `list` は ScreenCaptureKit が混むと何十秒も答えない（helper の期限が効かず 70 分返らなかった）。
        // UI の thread で待つと Shibubu 全体が止まる（AX も答えず frame も来ない）ので、裏の thread で探し、答えだけをここで受け取る
        if pane.take_answer(&spec.id, now) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // MIRROR-AUTO-1101 B: 窓が見つかって同意が無い = 作った時の同意を記録して、押さずに映す（Privacy で取り消した app だけ下の `Mirror here` を待つ）
        if matches!(pane.surface, Err(MirrorError::NoConsent { .. })) {
            if let Some(target) = pane.target.clone().filter(|t| !self.revoked.contains(&t.bundle_id)) {
                let (g, rec) = self.gate.grant(&target, now);
                self.granted.push(rec);
                pane.surface = RelayLog::open(&self.relay_log).and_then(|log| MirrorSurface::open(&self.helper, target, g, log));
                (pane.tried_ms, pane.lease) = (now, ControlLease::human(now));
            }
        }
        // 書類の面（app だけは起こさない・G87）: 最初の答えで窓が無ければ、`Open {App}` を押した時と同じく書類ごと後ろで開く（前面は取らない・下の `open_source`）
        let mut auto_doc = false;
        if pane.doc_wait && pane.pending.is_none() {
            pane.doc_wait = false;
            auto_doc = matches!(pane.surface, Err(MirrorError::NoWindow { .. }));
            if auto_doc {
                (pane.open_app, pane.starting, pane.tried_ms) = (Some(false), None, f64::NEG_INFINITY);
            }
        }
        // 画面収録の許可だけは macOS の確認が要る: 右上の知らせを起動の間 1 回（× で消える・許可が付けば 探し直しで押さずに映る）
        if matches!(pane.surface, Err(MirrorError::ScreenCaptureDenied)) {
            crate::ui::toast::post_once(&ctx, "mirror-screen-recording", "Screen recording is off", true, Some(crate::ui::connections::SCREEN_CAPTURE_ACTION));
        }
        // MIRROR-QUIET-1101: 窓を探す helper の常駐が続けて落ちる → 理由つきの右上の知らせを起動の間 1 回（Shibubu は間を空けて起こし直し続ける）
        if pane.surface.is_err() {
            if let Some(line) = crate::surface::mirror::helper_down(&self.helper) {
                crate::ui::toast::post_once(&ctx, "mirror-helper-down", line, true, None);
            }
        }
        // G90: Shibubu が起こした app が前面を取った → 1 回だけ前面を戻す。Shibubu の窓が前の時に起こした（`front_before` = None）なら、Shibubu の窓が前でなくなった
        // 最初の frame に前面がその app なら Shibubu へ。前でない時に起こした（背景の起動・AX の press = DOGFOOD-X 周 2 回 1）なら `FRONT_POLL_MS` ごとに前面を読み、
        // その app なら起こす前に前だった app へ（`activate`）。人が自分で別の app を前に出した・読めない → 戻さない。app 別の分岐は書かない（前に出るかは app が決める）
        // ponytail: 前面は UI の thread で lsappinfo を FRONT_POLL_MS ごと（最長 LAUNCH_WAIT_MS・Shibubu が前でない間だけ）。止まりが目立てば NSWorkspace の frontmostApplication を AppKit で読む
        if let Some(t) = pane.reclaim.filter(|_| pane.pending.is_none()) {
            let focused = ctx.input(|i| i.viewport().focused);
            let read = match &pane.front_before {
                None => focused == Some(false),
                Some((_, at)) => now >= *at,
            };
            if now - t >= LAUNCH_WAIT_MS {
                (pane.reclaim, pane.front_before) = (None, None);
                // 追記 245 の 3: 見張りの間に前に出なかった起動も 1 行（前に出た回数の分母・`mirror-press-ms.py` が数える）
                eprintln!("MIRROR-HYBRID-1101 追記 245: Mirror {}: 起こした {bundle_id} は {:.0} ms の間 前に出ませんでした（前に出たのはこの起動で {} 回）", spec.id, LAUNCH_WAIT_MS, self.fronted);
            } else if read {
                let front = (self.front)();
                let took = front.as_deref() == Some(bundle_id.as_str());
                pane.reclaim = None;
                if took {
                    self.fronted += 1;
                    eprintln!("MIRROR-HYBRID-1101 追記 245: Mirror {}: 起こした {bundle_id} が前に出ました（起こしてから {:.0} ms・この起動で {} 回目）", spec.id, now - t, self.fronted);
                    // B33（bfae19b2 の独立 review 黄）: 起動の自分の activate を Dock より先にここで見た → 起動の印を消す（次にその app が前に来るのは人の手）。
                    // 残すと Dock が付いた後 60 秒以内の人の ⌘Tab を `tuck` が起動と数え、本物を Shibubu の下に置いた。Dock が在れば、その activate は
                    // Dock の app_activated として `tuck` が起動と数えて印を取る（ここで消すと起動の activate を人の手と読む）
                    if pane.dock.is_none() {
                        pane.launched = None;
                    }
                }
                match pane.front_before.take() {
                    None if took => {
                        eprintln!("G90: Mirror {}: 起こした app が前面を取ったので Shibubu に戻します", spec.id);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    Some((before, _)) if took => {
                        eprintln!("G90: Mirror {}: 起こした app が前面を取ったので前の app（{before}）に戻します", spec.id);
                        if let Err(e) = (self.activate)(&before) {
                            eprintln!("G90: Mirror {}: 前の app を前に出せません: {e}", spec.id);
                        }
                    }
                    // 前の app が前のまま: 見張り続ける
                    Some((before, _)) if front.as_ref() == Some(&before) => (pane.reclaim, pane.front_before) = (Some(t), Some((before, now + FRONT_POLL_MS))),
                    _ => {}
                }
            }
            if pane.front_before.is_some() {
                ctx.request_repaint_after(Duration::from_millis(FRONT_POLL_MS as u64));
            }
        }
        // 開いていない（窓が無い・許可が無い・消えた）なら間を置いて探し直す。同意待ちは button を待つ。Ok の面は find しない（keep_alive の stream を保つ）
        // G87: 入っていない app も探し直さない（入れるまで同じ答え・Find again の button〈open_app〉を待つ）
        let waits = match pane.surface {
            Err(MirrorError::NoConsent { .. }) => true,
            Err(MirrorError::NotInstalled { .. }) => pane.open_app.is_none(),
            _ => false,
        };
        // MIRROR-QUIET-1101（owner 208）: 探し直しの間は探し始めて 20 秒までは 2 秒・その後 10 秒（`search_gap`）。すぐ探す印（tried_ms = -∞）と映った面は数え直す
        if pane.surface.is_ok() || pane.tried_ms == f64::NEG_INFINITY {
            pane.since = None;
        }
        let gap = if pane.surface.is_ok() { RETRY_MS } else { search_gap(now - *pane.since.get_or_insert(now)) };
        // L5-STICK: Shibubu と一緒に本物を最小化している間は探さない（隠れた窓しか見つからない）
        if pane.pending.is_none() && pane.surface.is_err() && !waits && now - pane.tried_ms >= gap && !other_searching && !folded && !pane.host_min {
            let (tx, rx) = std::sync::mpsc::channel();
            let (helper, relay_log, gate, bundle, file) = (self.helper.clone(), self.relay_log.clone(), self.gate.clone(), bundle_id.clone(), file.clone());
            // RQ4: 書類の path で選ぶのは Project の repo の file の時だけ（`Choose window…` で選んだ窓の題の語は題だけで探す）。在る file か dir を含む path
            let want = self.workspace.as_ref().zip(file.as_ref()).filter(|(w, f)| f.contains('/') || w.join(f).exists()).map(|(w, f)| w.join(f));
            let ask = std::mem::take(&mut pane.ask);
            // G87: 書類の指定の無い面だけ app を起こす（書類の面で app だけ起こすと空の窓か書類を選ぶ panel になる・W16M-1 の「file を開かない」）。
            // G90 (3): 隠れた窓の `Show {App}` は書類の面でも（窓は在る = `open -g -b` は隠れた窓を出すだけ）
            let hidden = matches!(pane.surface, Err(MirrorError::WindowHidden { .. }));
            let opened = pane.open_app.take();
            pane.doc_wait = opened == Some(true) && file.is_some() && !hidden;
            let launch = opened.filter(|_| file.is_none() || hidden).map(|if_stopped| (self.launch, if_stopped));
            if launch.is_some() {
                // G90: Shibubu の窓が前なら Shibubu へ、前でなければ起こす前に前だった app へ前面を戻す見張り（前が読めない・起こす app 自身なら見張らない）
                let focused = ctx.input(|i| i.viewport().focused) == Some(true);
                pane.front_before = if focused { None } else { (self.front)().filter(|b| *b != bundle_id).map(|b| (b, now)) };
                pane.reclaim = (focused || pane.front_before.is_some()).then_some(now);
                pane.launched = Some(now);
            }
            // ponytail: 答えない helper の thread は期限まで残る（helper の withTimeout が 30 秒で exit する）。Rust の側の期限は helper が直らない時に足す
            std::thread::spawn(move || {
                let _ = tx.send(Self::attach(&helper, &relay_log, &gate, &bundle, file.as_deref(), want.as_deref(), ask, launch));
            });
            (pane.pending, pane.tried_ms) = (Some(rx), now);
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // 最初の答えが着くまで（他の面が探している間に待っている時も）だけ「探しています」。探し直しの間は前の Err の状態と 1 行のまま
        let searching = !pane.answered && pane.surface.is_err();
        // しまう animation の間も frame は読まない（stream の最新の 1 枚は helper の側に残り、開いた時に読む）
        if let (false, Ok(m)) = (folded, &mut pane.surface) {
            // helper の出来事（sent・stats 等）は溜めない。入力の拒否の答えだけ 1 行に（窓が消えた・止まったは stream の Err で来る）
            while let Some(ev) = m.take_event() {
                // 追記 235 の 3: 送った押しの答え（届いた / 断った + 理由・押してからの ms）を relay の記録にもう 1 行
                m.note_answer(&ev);
                // MIRROR-SPEED-1101: helper の今の枚数（auto が混み具合で下げた数・1 分の 1 行に添える）
                if ev["kind"] == "stats" {
                    self.speed.fps = ev["fps"].as_u64().or(self.speed.fps);
                }
                if let Some(e) = m.refusal(&ev) {
                    // M4: 画面の 1 行は内部の語を出さないので、helper の原文は stderr に（BG の run.sh は app.log に取る・M2 は E1 の未達の理由が残らなかった）
                    eprintln!("M4: Mirror {}: helper が入力を断りました: {ev}", spec.id);
                    pane.line = Some((e, now));
                } else if ev["route"] == "save_document" {
                    // M4 ③: 受け付けた ⌘S の答え（helper の saved）も stderr に（BG の判定が ⌘S の効果と TextEdit の自動保存を分ける・受け付けたのに disk が変わらない時の切り分け）
                    eprintln!("M4: Mirror {}: 保存の答え: {ev}", spec.id);
                }
                if let Some(c) = caret_of(&ev, pane.click_seq) {
                    pane.caret = Some(c);
                }
            }
            // frame は変化の時だけ来る。着いた frame が描き直しを起こす（stream を読む前に渡す。読んだ後に Err で surface を替えると m が無い・前面でない Shibubu もこれで映る）
            m.wake_with(|| Waker::from(Arc::new(Repaint(ctx.clone()))));
            // MIRROR-SPEED-1101（owner 202「画質も荒い」）: 撮る大きさ = 描く大きさ（本物の窓を描く point × 画面の倍率・本物より細かくは helper が撮らない）。
            // 替わった時だけ・四隅を引く間は `VIEW_GAP_MS` ごとまで・形を替える animation の間は送らない（1 px ごとに SCK を組み直さない）
            if let (Some(meta), false) = (pane.last, moving) {
                // I32: 拡大している面は × 倍率（拡大した絵も本物の pixel まで細かく・helper は本物より細かく撮らない）
                let px = window_drawn(video, &meta) * ctx.pixels_per_point() * self.windows.get(&spec.id).map_or(1.0, |s| s.lens.k);
                let want = (px.x.round().max(2.0) as u32, px.y.round().max(2.0) as u32);
                if pane.view.is_none_or(|(sent, _)| sent != want) {
                    if pane.view.is_none_or(|(_, at)| now - at >= VIEW_GAP_MS) {
                        m.view(want.0, want.1);
                        pane.view = Some((want, now));
                    } else {
                        ctx.request_repaint_after(Duration::from_millis(VIEW_GAP_MS as u64));
                    }
                }
            }
            match m.stream() {
                Ok(Some(mut f)) => {
                    // MIRROR-SPEED-1101（owner 205）: IOSurface は GPU の texture にそのまま（写し 0 回・IOSurface ごとに登録は 1 回）。
                    // 載せられない時（wgpu の口が無い・Metal が断った）だけ CPU で 1 回写す・pipe の RGBA は今まで通り
                    let native = f.surface.as_deref().and_then(|s| native_texture(&mut self.natives, s, now));
                    let rgba = match (native, &f.surface) {
                        (Some(_), _) => None,
                        (None, Some(s)) => s.to_rgba(),
                        (None, None) => Some(std::mem::take(&mut f.rgba)),
                    };
                    if let Some(id) = native {
                        pane.texture = Some(Tex::Native(id));
                    } else if let Some(rgba) = rgba {
                        let img = ColorImage::from_rgba_unmultiplied([f.meta.width as usize, f.meta.height as usize], &rgba);
                        match pane.texture.as_mut() {
                            Some(Tex::Handle(tx)) => tx.set(img, TextureOptions::LINEAR),
                            _ => pane.texture = Some(Tex::Handle(ctx.load_texture(ax.clone(), img, TextureOptions::LINEAR))),
                        }
                    }
                    // 描いている IOSurface は今と 1 つ前の 2 枚だけ持つ（GPU が前の frame をまだ読む間に SCK の pool が書かない・全部持つと pool が尽きて止まる）
                    pane.shown = if native.is_some() { [f.surface.clone(), pane.shown[0].take()] } else { [None, None] };
                    let shown = crate::surface::now_ms();
                    self.speed.add(shown - f.meta.captured_ms, native.is_some(), shown);
                    if let Some(log) = &mut self.frame_log {
                        let r = FrameRecord { v: FRAME_SCHEMA_V, surface_id: spec.id.clone(), seq: f.meta.seq, captured_ms: f.meta.captured_ms, received_ms: f.received_ms, shown_ms: shown };
                        if let Err(e) = log.append(&r) {
                            eprintln!("W16M-1b: {e}");
                        }
                    }
                    pane.last = Some(f.meta);
                }
                Ok(None) => {}
                Err(e) => pane.lost(&spec.id, e, now),
            }
            // G97: 前面の間は今まで通り 33ms でも見に来る。後ろ・しまった間にも回すと、後ろの Shibubu が CPU と WindowServer を食い続けた（AUDIT2 D）
            if ctx.input(|i| i.viewport().focused != Some(false) && i.viewport().minimized != Some(true)) {
                ctx.request_repaint_after(Duration::from_millis(33));
            }
        }
        if pane.surface.is_err() && !folded {
            // 前に映した frame を今の物として出さない・前の拒否の 1 行と押しかけの押下も捨てる（閉じる animation の間だけは前の映像で畳む）
            (pane.texture, pane.last, pane.line, pane.held, pane.caret) = (None, None, None, None, None);
            // MIRROR-SPEED-1101: 次の stream にも描く大きさを送り直す・描いていない IOSurface を持ち続けない
            (pane.view, pane.shown) = (None, [None, None]);
            ctx.request_repaint_after(Duration::from_millis(gap as u64));
        }
        let live = match (&pane.surface, &pane.texture, pane.last) {
            (Ok(_), Some(_), Some(meta)) if !folded && !moving => Some(meta),
            _ => None,
        };
        let resp = ui.interact(video, egui::Id::new(&ax), if live.is_some() { Sense::click_and_drag() } else { Sense::hover() });
        // 追記 245 の 2: 映っていない面の状態の 1 行と次の一手は面の真ん中（横は中央揃え・縦は映像の中ほどから）。映る前の骨格（Ok）は今まで通り左上から
        let mut inner = if pane.surface.is_err() {
            let top = (video.center().y - theme::INPUT_H).max(video.top() + theme::SP_2).min(video.bottom() - theme::SP_2);
            let r = Rect::from_min_max(pos2(video.left() + theme::SP_2, top), pos2(video.right() - theme::SP_2, video.bottom() - theme::SP_2));
            ui.new_child(UiBuilder::new().id_salt(&ax).max_rect(r).layout(egui::Layout::top_down(egui::Align::Center)))
        } else {
            child(ui)
        };
        let state = pane_state(pane, searching);
        let label = match &pane.surface {
            _ if searching => "Looking for the original window".to_string(),
            Ok(_) if lit => direct_line(&app),
            Err(e) => e.to_string(),
            Ok(m) => m.target().app_name.clone(),
        };
        // I32: 拡大している面は frame の AX の名前にも倍率（映像の右下の印と同じ語）
        let zoom_word = self.windows.get(&spec.id).map(|s| s.lens.k).filter(|k| *k > 1.0).map_or(String::new(), |k| format!(" · {:.0}%", k * 100.0));
        ctx.accesskit_node_builder(resp.id, |n| {
            n.set_role(FRAME_ROLE);
            n.set_author_id(ax.clone());
            n.set_label(format!("{} · {label}{zoom_word}", state.wording()));
        });
        // M4 ③: 確かめられない同じ題名の窓の面は、許可を尋ねる button（押した時だけ macOS が尋ねる）
        let ask_app = match &pane.surface {
            Err(MirrorError::Ambiguous { ask_pid: Some(_), app_name, .. }) if !searching => Some(app_name.clone()),
            _ => None,
        };
        let mut inputs = Vec::new();
        // MIRROR-HYBRID-1101: この frame に面を触った（本物を前に出す）
        let mut touch = false;
        // I32: この frame に拡大を始めた / 替えた面の本物が前に出ている → 下へ戻す（1:1 の本物が拡大の絵を覆わない）
        let mut back = false;
        // 追記 214: この frame に押した本物が枠より大きい（映像の大きさにしてから前に出す）
        let mut fit = false;
        // MIRROR-FIT-1101: 本物の大きさ（Dock が読んだ AX の frame）より映像が小さい = 縮めて見せている（本物を前に出すと枠からはみ出すので出さない）
        let real = pane.dock.as_ref().and_then(|d| d.window.as_ref()?.frame);
        let scaled = real.is_some_and(|r| (video.width() as f64) < r.w - 1.0 || (video.height() as f64) < r.h - 1.0);
        let mut esc_taken = false;
        if let Some(meta) = live {
            // MIRROR-HYBRID-1101（owner 207「両ドリ」）: 枠の押しは選ぶのと同時に「触る」合図（MIRROR-FIT の「選ぶ押しは本物へ送らない」を反転）。
            // 押した所で本物を前に出し（下の `touch`）、離した押しを本物へ送り直す。keyboard の focus も最初の押しで取る（打ち始めも触る合図）
            // B31（bfae19b2 の独立 review 赤 1）: 押した = 本物の pointer の click・drag・AX の Press だけ。`clicked()` は focus を持つ面の Space / Enter の
            // 偽の click も含む = AX の Focus で面へ移った frame の Space で、押していない面が打鍵を送り・選ばれ・触った時刻（MIRROR-PIN の窓の決め）を持った
            let clicked = resp.clicked_by(PointerButton::Primary) || resp.drag_started() || ctx.input(|i| i.has_accesskit_action_request(resp.id, egui::accesskit::Action::Click));
            if clicked {
                resp.request_focus();
                (pane.armed, pane.selected, pane.touched) = (true, true, crate::surface::now_ms());
            }
            if ctx.input(|i| i.pointer.any_pressed()) && !resp.hovered() {
                resp.surrender_focus();
            }
            // 枠（四隅の取っ手を含む）の外・枠の上に被さった物（panel・popup）の上の押しで選びを外す。panel（Background）は Area に載らず
            // `layer_id_at` が None を返すので、None は「被さった物は無い」と読む（Some の別の layer だけが被さった物）
            let layer = ui.layer_id();
            if ctx.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None }).is_some_and(|p| !rect.expand(HANDLE_HIT / 2.0).contains(p) || ctx.layer_id_at(p).is_some_and(|l| l != layer)) {
                (pane.selected, pane.grab) = (false, None);
            }
            // keyboard の focus は egui の Memory の focus で見る（`Response::has_focus` は窓の focus も要る）。key が来るのは Shibubu の
            // key の窓だけなので窓の focus を足しても守る物は無く、背景の Shibubu（key の窓だが非 active）への pid 宛ての key が捨てられる
            // （W16M-2 で実測: Memory の focus は矩形・`has_focus` は false・relay 0 件）
            let mut focused = ctx.memory(|m| m.has_focus(resp.id));
            // L5（DOGFOOD-X 周 2 題 6: Terminal の欄に打った残り 57 字が REAPER の Mirror から本物へ送られ、REAPER の窓が 2 つ開いた）: 面を押していないのに
            // focus が面へ移った（AX の Focus・他の欄で打っている途中）→ 打鍵を本物へ送らず、focus を前の持ち主へ返す。送るのは面を押した後だけ
            if focused && !pane.armed {
                eprintln!("L5: Mirror {}: 押していない面へ focus が移ったので、打鍵を送らず前の欄へ返します", spec.id);
                match self.last_focus {
                    Some(prev) => ctx.memory_mut(|m| m.request_focus(prev)),
                    None => resp.surrender_focus(),
                }
                focused = false;
            }
            pane.armed &= focused;
            // 返す先は直前の frame の 1 つで、どの Mirror の面でもない欄（B33: 別の面を返す先にすると、押していない面の守りが自分へ focus を返し続けて
            // 欄にも前の面にも戻らなかった = 面 A の show が面 B へ移った focus を返す先に覚える）。欄が無ければ返さず外す
            self.last_focus = ctx.memory(|m| m.focused()).filter(|f| *f != resp.id && !frames.contains(f));
            if focused {
                // Tab・矢印・Escape も元の app へ（egui の focus の移動に使わない）
                ctx.memory_mut(|m| m.set_focus_lock_filter(resp.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
            }
            // MIRROR-FIT-1101: Esc（keyboard の focus が無い時）で選びを外す（focus が在る時の Esc は今の規則のまま元の app へ・拡大の戻しより先）
            if pane.selected && crate::ui::renderer::escape_for_app(&ctx) {
                (pane.selected, pane.grab, esc_taken) = (false, None, true);
            }
            // I32（owner 11-03「mirrorの中でappを自由に拡大縮小できるように」）: 映像だけの拡大。pinch（pointer が映像の上）・⌘+ / ⌘= / ⌘- / ⌘0（面に focus）。
            // key は打鍵を数える前に consume（本物へ送らない・egui の end_pass の Shibubu の文字の大きさにも行かない）。拡大の間は映像の中の drag と wheel が
            // 見る所を動かし（Page は scroll しない・本物へ送らない）、押し・止まりで本物を前に出さない（1:1 の本物が拡大の絵を覆う）・本物の大きさは替えない
            let content = content_rect(video, &meta);
            let was = self.windows.get(&spec.id).map(|s| s.lens).unwrap_or_default();
            let mut lens = was;
            let pinch = if resp.hovered() { ctx.input(|i| i.zoom_delta()) } else { 1.0 };
            if pinch != 1.0 {
                lens = lens.zoom(pinch, resp.hover_pos().unwrap_or(video.center()), content, video);
            }
            if focused {
                use egui::gui_zoom::kb_shortcuts as kb;
                if ctx.input_mut(|i| i.consume_shortcut(&kb::ZOOM_RESET)) {
                    lens = Lens::default();
                }
                if ctx.input_mut(|i| i.consume_shortcut(&kb::ZOOM_IN) || i.consume_shortcut(&kb::ZOOM_IN_SECONDARY)) {
                    lens = lens.zoom(LENS_STEP, video.center(), content, video);
                }
                if ctx.input_mut(|i| i.consume_shortcut(&kb::ZOOM_OUT)) {
                    lens = lens.zoom(1.0 / LENS_STEP, video.center(), content, video);
                }
            }
            let zoomed = lens.k > 1.0;
            if zoomed && resp.hovered() && pinch == 1.0 {
                let wheel = ctx.input_mut(|i| {
                    let d = i.raw_scroll_delta;
                    (i.raw_scroll_delta, i.smooth_scroll_delta) = (Vec2::ZERO, Vec2::ZERO);
                    d
                });
                lens = lens.pan(wheel, content, video);
            }
            if zoomed && resp.dragged_by(PointerButton::Primary) {
                lens = lens.pan(resp.drag_delta(), content, video);
            }
            if lens != was {
                self.windows.entry(spec.id.clone()).or_default().lens = lens;
                back = zoomed && direct;
                ctx.request_repaint();
            }
            // pointer は矩形が一番上の時（hovered）か、矩形の上で押したままの時だけ。押下は離すまで次の frame へ持ち越す
            let pointer_ok = resp.hovered() || pane.held.is_some();
            let mut evs: Vec<Event> = pane.held.take().into_iter().chain(resp.hover_pos().map(Event::PointerMoved)).collect();
            evs.extend(ctx.input(|i| i.events.clone()).into_iter().filter(|e| pointer_ok || !matches!(e, Event::PointerMoved(_) | Event::PointerButton { .. } | Event::MouseWheel { .. } | Event::PointerGone)));
            pane.held = evs.iter().fold(None, |held, e| match e {
                Event::PointerButton { button: PointerButton::Primary, pressed: true, .. } => Some(e.clone()),
                Event::PointerButton { button: PointerButton::Primary, pressed: false, .. } | Event::PointerGone => None,
                _ => held,
            });
            inputs = mirror_inputs(&evs, lens.apply(shown_rect(video, &meta), content, video), &meta, focused);
            if zoomed {
                inputs.retain(|i| !matches!(i, MirrorInput::Drag { .. } | MirrorInput::Scroll { .. }));
            }
            // MIRROR-HYBRID-1101: 触る合図 = 映像の上の押し（取っ手の上の押しは取っ手が取る）・focus を持った面への打鍵・追記 235: 止まった pointer（下の dwell）。
            // 動いている pointer と wheel では出さない
            // （本物を前に出していない面の上の wheel は Page の scroll）。
            // 本物を前に出せる面は前に（枠の全部が Page に見えていて被さった物が無い時だけ = 本物が Page の外や被さった物の上に出ない）。
            // 出せない面は理由を右上に 1 行（追記 214: 押すたびに・失敗 = × まで残る・出ている間は同じ文を重ねない）で Mirror のまま（入力は今までどおり Mirror の道）。
            // 追記 214（17 回目の GarageBand = 窓 1680×956 が枠より大きく、理由は起動の間 1 回の 6 秒の知らせだけで黙って Mirror の道だった）: 本物が枠より
            // 大きければ本物を映像の大きさにしてから前に出す（`fit` → 下で `fit_to_frame`・合ったら live の所で `go_direct`）
            // 追記 240 の 3（owner 242）: 映像の縁から EDGE の内の押し・止まりでは前に出さない（Shibubu の境目を掴みに行く所・押しは Mirror の道で届ける）
            let inner = video.shrink(EDGE);
            let pressed = resp.is_pointer_button_down_on() && ctx.input(|i| i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|o| inner.contains(o)));
            let typed = focused && ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. } | Event::Text(_) | Event::Paste(_) | Event::Ime(_))));
            if pane.direct.is_none() && pane.fit_touch.is_none() && ((pressed && !zoomed) || typed) {
                let whole = ui.clip_rect().contains_rect(video) && !covered(&ctx, ui.layer_id(), video) && !above(&self.stack, &spec.id, video);
                match stay_reason(pane, &app) {
                    Some(line) => crate::ui::toast::post(&ctx, line, true, None),
                    None if !whole => crate::ui::toast::post(&ctx, format!("Show all of {app}'s frame on the page to use it here · it stays a mirror"), true, None),
                    // I32: 拡大の間は本物の大きさを替えない（打鍵は送らず B16 の 1 行）
                    None if scaled => fit = !zoomed,
                    None => touch = true,
                }
            }
            // 追記 235 の 2（owner「mirrorの中で操作するとdelayがある これ限りなく0にして」「反応しない時もあるし」）: pointer が映像の上で DWELL_S 止まったら押す前に
            // 本物を前へ（最初の押しから本物に直に届く = Mirror の道〈AX の点の押し〉の遅れも、AX に答えない部品も無い・送り直す押しも無い）。止まりは pointer が
            // 動いた所から数える（開いただけ・button を離しただけ・前に出した後に同じ所で止まったままでは出さない）。10-27 の決まりの例外はそのまま = Shibubu が
            // 前でない・Shibubu の欄に打っている（この面の外の欄が focus を持ち `TYPING_S` の内に打った）・button を押している間は出さない
            let rest = resp.hover_pos().filter(|p| resp.hovered() && inner.contains(*p));
            let (stirred, down) = ctx.input(|i| (i.pointer.delta() != Vec2::ZERO, i.pointer.any_down()));
            pane.dwell = match (rest, pane.dwell) {
                (None, _) => None,
                (Some(_), _) if down => None,
                (Some(p), Some(w)) if p.distance(w.0) <= 1.0 => Some(w),
                (Some(p), _) if stirred => Some((p, time, false)),
                (Some(_), w) => w,
            };
            if ctx.memory(|m| m.focused()).is_some_and(|f| f != resp.id)
                && ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. } | Event::Text(_) | Event::Paste(_) | Event::Ime(_))))
            {
                pane.typed_at = time;
            }
            let others_typing = time - pane.typed_at < TYPING_S;
            if let Some((_, since, false)) = pane.dwell {
                let quiet = !touch && !fit && !scaled && !zoomed && pane.direct.is_none() && pane.fit_touch.is_none() && pane.resize.is_none() && pane.resume.is_none();
                let front = ctx.input(|i| i.viewport().focused) == Some(true);
                if quiet && front && !others_typing && stay_reason(pane, &app).is_none() && ui.clip_rect().contains_rect(video) && !covered(&ctx, ui.layer_id(), video) && !above(&self.stack, &spec.id, video) {
                    if time - since >= DWELL_S {
                        touch = true;
                    } else {
                        ctx.request_repaint_after(Duration::from_secs_f64(DWELL_S - (time - since)));
                    }
                }
            }
        } else {
            // B32（bfae19b2 の独立 review 赤 2）: 映っていない間（畳んだ・stream が切れた・形を替える animation）は押した事を忘れる。覚えたままだと、
            // 映り直した後に押していない面へ focus が来た（AX の Focus）時に守りが開いたままで打鍵を送る。Page を替えた面は `end_frame` の `stop` が Pane ごと作り直す
            pane.armed = false;
        }
        let mut note: Option<(RegionState, String)> = None;
        // RQ4: 窓が消えた面の `Open {App}` を書類の面で押した（pane を放した後に `open_source`）・`Choose window…` の button（帯の題と 1 つの opener にする）
        let (mut open_doc, mut chooser): (bool, Option<egui::Response>) = (false, None);
        match (&pane.surface, &pane.texture, pane.last) {
            (s, Some(tx), Some(meta)) if s.is_ok() || folded => {
                let p = ui.painter_at(video);
                p.rect_filled(video, 0.0, t.surface_2);
                let lens = self.windows.get(&spec.id).map(|s| s.lens).unwrap_or_default();
                // 畳む / 開く間（映像の矩形が開いた時より低い）は、開いた時の映像を下の辺に揃えて上の辺から隠す
                let img = match open_video_h {
                    // 追記 240 の 1: 四隅を引いている間は絵を枠に伸ばす（本物は離すまで動かさない）
                    _ if pane.grab.is_some_and(|c| !matches!(c, Corner::Move(_))) => stretched(video, &meta),
                    Some(h) if video.height() < h - 0.5 => {
                        let open = Rect::from_min_size(video.min, Vec2::new(video.width(), h));
                        shown_rect(open, &meta).translate(Vec2::new(0.0, video.height() - h))
                    }
                    _ => lens.apply(shown_rect(video, &meta), content_rect(video, &meta), video),
                };
                p.image(tx.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                if let (Some(c), false) = (pane.caret, folded) {
                    p.rect_filled(caret_rect(lens.apply(shown_rect(video, &meta), content_rect(video, &meta), video), &meta, c), 0.0, t.focus_ring);
                }
                // I32: 拡大している間は映像の右下に倍率（⌘0・pinch で戻る・AX の名前と同じ語）
                if lens.k > 1.0 && !folded {
                    let font = egui::TextStyle::Small.resolve(&ctx.style());
                    let g = ctx.fonts(|f| f.layout_no_wrap(format!("{:.0}%", lens.k * 100.0), font, t.text_secondary));
                    let pad = Vec2::new(theme::SP_1, 2.0);
                    let r = Rect::from_min_size(video.right_bottom() - Vec2::splat(theme::SP_2) - g.size() - 2.0 * pad, g.size() + 2.0 * pad);
                    p.rect_filled(r, 4.0, t.surface_2);
                    p.galley(r.min + pad, g, t.text_secondary);
                }
                if resp.has_focus() {
                    p.rect_stroke(video, 0.0, egui::Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Inside);
                }
                // G93: 拒否の 1 行は映像の上に被せず、題の帯（題の所）に出す（本物が前に出ている間は帯の 1 行 `Using … directly` が先）
                match pane.line.clone() {
                    Some((e, at)) if now - at < LINE_MS => {
                        let region = match e {
                            MirrorError::AutomationDenied { .. } | MirrorError::AccessibilityDenied => RegionState::Blocked,
                            _ => RegionState::Error,
                        };
                        note = Some((region, e.to_string())).filter(|_| !direct);
                    }
                    Some(_) => pane.line = None,
                    None => {}
                }
            }
            // 閉じる animation の間で前の映像が無い: 状態の 1 行（窓が閉じた 等の嘘）を出さず地だけ
            _ if folded => {
                ui.painter_at(video).rect_filled(video, 0.0, t.surface_2);
            }
            (Ok(_), _, _) => {
                states::inline(&mut inner, state.region(), &label);
                crate::ui::skeleton::region_skeleton(&mut inner, "block");
            }
            (Err(MirrorError::NoConsent { .. }), _, _) => {
                states::inline(&mut inner, state.region(), &label);
                let v = states::next_step_variant(&inner);
                let b = theme::button_compact(&mut inner, "Mirror here", v);
                if let Some(target) = pane.target.clone() {
                    ctx.accesskit_node_builder(b.id, |n| n.set_author_id(consent_button_id(&target.bundle_id)));
                    if b.clicked() {
                        self.revoked.remove(&target.bundle_id);
                        let (g, rec) = self.gate.grant(&target, now);
                        self.granted.push(rec);
                        pane.surface = RelayLog::open(&self.relay_log).and_then(|log| MirrorSurface::open(&self.helper, target, g, log));
                        pane.tried_ms = now;
                        pane.lease = ControlLease::human(now);
                    }
                }
            }
            (Err(MirrorError::Ambiguous { .. }), _, _) if ask_app.is_some() => {
                states::inline(&mut inner, state.region(), &label);
                let app = ask_app.unwrap_or_default();
                let v = states::next_step_variant(&inner);
                if theme::button_compact(&mut inner, &crate::surface::mirror::ask_button(&app), v).clicked() && pane.pending.is_none() {
                    (pane.ask, pane.tried_ms) = (true, f64::NEG_INFINITY);
                    ctx.request_repaint();
                }
            }
            // G94: 文が言う `Open settings` を button にする（Connections と同じ道・許可が付けば 探し直しで映り直す）
            (Err(MirrorError::ScreenCaptureDenied), _, _) if !searching => {
                states::inline(&mut inner, state.region(), &label);
                let v = states::next_step_variant(&inner);
                if theme::button_compact(&mut inner, "Open settings", v).clicked() {
                    crate::ui::connections::open_settings(crate::ui::connections::SCREEN_CAPTURE_SETTINGS);
                }
            }
            (Err(e), _, _) => {
                states::inline(&mut inner, state.region(), &label);
                // G87・RQ4（RQ-019）: 次の一手は文でなく button（押しても何も起きない文を出さない・L2）。窓が無い / 消えた = `Open {App}`（書類の無い面は動いていても
                // `open -g`・書類の面はその書類を `open -g`）と `Choose window…`（帯の題と同じ menu・探し直しが `Try again` の代わり）・
                // 入っていない = `Find again`（入れた後に起こす）
                match e {
                    _ if searching => {}
                    // G90 (3): 隠れた窓 = `Show {App}`（`open -g -b` で前に出さずに出す・出れば 探し直しで映る）
                    MirrorError::WindowHidden { app_name } => {
                        let v = states::next_step_variant(&inner);
                        if theme::button_compact(&mut inner, &format!("Show {app_name}"), v).clicked() && pane.pending.is_none() {
                            (pane.open_app, pane.starting, pane.tried_ms) = (Some(false), None, f64::NEG_INFINITY);
                            ctx.request_repaint();
                        }
                    }
                    MirrorError::NoWindow { .. } | MirrorError::WindowGone => {
                        let name = match e {
                            MirrorError::NoWindow { app_name, .. } => app_name.clone(),
                            _ => app.clone(),
                        };
                        // 追記 245 の 2: Project の repo に在る書類の面は開く書類の名前も言う（`open_source` が渡すのと同じ書類だけ・`Choose window…` の題の語では言わない）。
                        // 真ん中の塊の中で縦に並べる（横の並びは中央に揃わない）
                        let doc = self.workspace.as_ref().zip(file.as_deref()).filter(|(w, f)| w.join(f).is_file()).and_then(|(_, f)| Path::new(f).file_name()?.to_str());
                        let open_label = doc.map_or_else(|| format!("Open {name}"), |d| format!("Open {name} · {d}"));
                        let v = states::next_step_variant(&inner);
                        let (open, choose) = (theme::button_compact(&mut inner, &open_label, v), theme::button_compact(&mut inner, AX_CHOOSE, theme::Variant::Ghost));
                        if open.clicked() && pane.pending.is_none() {
                            (pane.open_app, pane.starting, pane.tried_ms) = (Some(false), None, f64::NEG_INFINITY);
                            open_doc = file.is_some();
                            ctx.request_repaint();
                        }
                        chooser = Some(choose);
                    }
                    MirrorError::NotInstalled { .. } => {
                        let v = states::next_step_variant(&inner);
                        if theme::button_compact(&mut inner, PortalAction::Rediscover.label(), v).clicked() && pane.pending.is_none() {
                            (pane.open_app, pane.starting, pane.tried_ms) = (Some(true), None, f64::NEG_INFINITY);
                            ctx.request_repaint();
                        }
                    }
                    _ => {}
                }
                // FX-036: 書類の指定の file が repo に無い時の復旧路 = `Create {file}`（空の file を作るだけ。開くのは app の側で・作ったら窓の探し直しを急ぐ）
                if let (MirrorError::NoWindow { title_has: Some(name), .. }, Some(path)) = (e, self.workspace.as_ref().zip(file.as_ref()).map(|(w, f)| w.join(f))) {
                    let v = states::next_step_variant(&inner);
                    if !searching && !path.exists() && theme::button_compact(&mut inner, &format!("Create {name}"), v).clicked() {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        if std::fs::write(&path, b"").is_ok() {
                            pane.tried_ms = f64::NEG_INFINITY;
                            ctx.request_repaint();
                        }
                    }
                }
            }
        }
        // RQ4（RQ-018）× 追記 218 の 3: 元の app を開けなかった・大きさを受けなかった・枠だけ替えた 1 行は帯の題の所（映っている面の拒否の 1 行と同じ所・新しい方）
        if let Some((state, line, _)) = pane.band_line.clone().filter(|(.., at)| now - at < LINE_MS) {
            note = Some((state, line));
            ctx.request_repaint_after(Duration::from_millis(LINE_MS as u64));
        }
        // 窓を選び直す menu の題の一覧が着いたか
        if let Some(r) = pane.titles.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (pane.titles, pane.titles_got) = (None, Some(r));
        } else if pane.titles.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // MIRROR-FIT-1101（owner 200）: 選んだ枠に縁（四隅の取っ手は `handles`）。追記 214（owner「触ると光るんだけど何も起きない」）: focus の色で光らせるのは本物が
        // 前に来た面だけ（`lit`）。選んだだけ・前に出す途中の枠は細い線（光ったまま何もしない形にしない）
        if lit || self.panes.get(&spec.id).is_some_and(|p| p.selected || p.direct.is_some()) {
            // 本物の窓は映像の矩形の上・帯と縁は Shibubu の側に見えている
            let stroke = if lit { egui::Stroke::new(2.0, t.focus_ring) } else { theme::hairline(&t) };
            ui.painter_at(rect.expand(3.0)).rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);
        }
        // G93 の拒否の 1 行が帯の題の所に出ている間は Fit to frame を出さない（同じ所に重ねない）
        let misfit_line = self.misfit(&spec.id, video);
        let misfit = misfit_line.clone().filter(|_| note.is_none());
        let (w, fit, original, menu) = title_band(ui, band, &ax, if note.is_some() { "" } else { title.as_str() }, window, misfit.as_deref(), &t);
        if let Some(w) = w {
            want = Some(w);
        }
        // 題の double-click も同じ手（合っている面では何もしない）
        if fit && misfit_line.is_some() {
            self.fit_to_frame(&spec.id, video.size(), time);
        }
        // MIRROR-FIT-1101（owner 200）× 追記 218 の 3（owner「mirrorがまだ斜め矢印で拡大縮小できないんだけど」= 選んだ後・Dock の在る面だけだった）: 映っている
        // 開いた枠の四隅は選ばなくても斜めの矢印で引ける（帯と映像の後に置く = 隅の上の押しは取っ手が取る・HYBRID の触る押しにならない）
        if live.is_some() && window == MirrorWindow::Open {
            self.handles(ui, &spec.id, rect, &menu, time, &t, &app);
        }
        if original {
            // 失敗の 1 行は `open_source` が面に残す（次の frame に帯の題の所）
            let _ = self.open_original(&spec.id);
            ctx.request_repaint();
        }
        if open_doc || auto_doc {
            let _ = self.open_source(&spec.id, true);
        }
        // RQ4（RQ-019）: 帯の題（と窓が消えた時の `Choose window…`）で開く menu。開いた時に題の一覧を裏の thread で引き直す
        let menu_rect = menu.rect;
        let opener = match chooser {
            Some(b) => menu.union(b),
            None => menu,
        };
        if opener.clicked() && !ui.memory(|m| m.is_popup_open(opener.id.with("kangae-menu"))) {
            let (tx, rx) = std::sync::mpsc::channel();
            let (helper, bundle) = (self.helper.clone(), bundle_id.clone());
            std::thread::spawn(move || {
                let _ = tx.send(MirrorTarget::titles(&helper, &bundle));
            });
            if let Some(p) = self.panes.get_mut(&spec.id) {
                (p.titles, p.titles_got) = (Some(rx), None);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        enum Pick {
            Window(Option<String>),
            OpenApp,
        }
        let got = self.panes.get(&spec.id).and_then(|p| p.titles_got.clone());
        let quiet = |ui: &mut egui::Ui, s: &str| {
            ui.add(egui::Label::new(theme::Type::Small.text(s).color(t.text_secondary)).truncate());
        };
        let pick = theme::menu_popup(ui, &opener, |ui| {
            let mut picked = None;
            match &got {
                None => quiet(ui, "Looking for windows"),
                Some(Err(e)) => quiet(ui, &e.to_string()),
                Some(Ok(v)) if v.is_empty() => {
                    quiet(ui, "No windows");
                    if theme::button_menu_item(ui, &format!("Open {app}"), "", theme::Variant::Ghost).clicked() {
                        picked = Some(Pick::OpenApp);
                    }
                }
                Some(Ok(v)) => {
                    for w in v {
                        let hint = if file.as_deref() == Some(w.as_str()) { "Current" } else { "" };
                        if theme::button_menu_item(ui, w, hint, theme::Variant::Ghost).clicked() {
                            picked = Some(Pick::Window(Some(w.clone())));
                        }
                    }
                }
            }
            theme::rule(ui);
            if theme::button_menu_item(ui, "Any window", if file.is_none() { "Current" } else { "" }, theme::Variant::Ghost).clicked() {
                picked = Some(Pick::Window(None));
            }
            picked
        });
        match pick {
            // 今と同じ窓は何もしない（menu を閉じるだけ）
            // MIRROR-FRAME-1101: 保存するのは題の語（`/` を含む題を書類の path に読ませない）
            Some(Pick::Window(w)) if w != file => self.chosen.push((spec.id.clone(), w.as_deref().and_then(title_word), None)),
            Some(Pick::OpenApp) => {
                if file.is_some() {
                    let _ = self.open_source(&spec.id, true);
                }
                if let Some(p) = self.panes.get_mut(&spec.id) {
                    (p.open_app, p.starting, p.tried_ms) = (Some(false), None, f64::NEG_INFINITY);
                }
                ctx.request_repaint();
            }
            _ => {}
        }
        if let Some((region, line)) = &note {
            let r = Rect::from_min_max(pos2(title_left(band), band.top()), pos2(menu_rect.right().max(title_left(band)), band.bottom()));
            states::inline(&mut ui.new_child(UiBuilder::new().id_salt((&ax, "refusal")).max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center))), *region, line);
        }
        // 拡大は Esc でも戻す。Mirror（や欄）に keyboard の focus が在る時の Esc は今の規則のまま元の app（や欄）へ
        if window == MirrorWindow::Zoomed && !esc_taken && crate::ui::renderer::escape_for_app(&ctx) {
            want = Some(MirrorWindow::Open);
        }
        if back {
            self.put_back(&spec.id);
        }
        // MIRROR-HYBRID-1101: 触った面は本物を前に出してから（押した frame・離す前）、離した押しを本物へ
        if touch {
            self.go_direct(&spec.id);
        }
        // 追記 214: 枠より大きい本物を押した → 本物を今の映像の大きさに（`Fit to frame` と同じ手・MIRROR-FIT 200 の「引き終わったら本物も同じ大きさ」と同じ向き）
        if fit {
            self.fit_to_frame(&spec.id, video.size(), time);
            if let Some(p) = self.panes.get_mut(&spec.id).filter(|p| p.resize.is_some()) {
                p.fit_touch = Some(time);
            }
        }
        let front = self.panes.get(&spec.id).is_some_and(|p| p.direct.is_some());
        for input in inputs {
            // 拒否は relay が面の 1 行に置く（ここでは捨てない・面の Err にもしない）。本物が前の面の押し・drag・wheel は本物へ HID で
            // （Mirror の道に乗せない = 2 回届けない）・文字と key は Mirror の道（前に出す前の frame に打った分・前に出た後は本物へ直に行く）
            let _ = match input {
                MirrorInput::Click { .. } | MirrorInput::Drag { .. } | MirrorInput::Scroll { .. } if front => self.relay_front(&spec.id, input),
                // 本物を前に出していない面の上の wheel は Page の scroll（Mirror の道は wheel を届けられない = 拒否の 1 行を出さない）
                MirrorInput::Scroll { .. } => continue,
                // B16（DOGFOOD-X 周 3: Obsidian が後ろの間は 1 字も入らず、前に来た時に note の頭へまとめて入った）: 本物を前に出していない面
                // （前に出せない `stay_reason`・枠の一部が隠れている・縮めた枠を本物に合わせている間）の打鍵は送らない。後ろの app へ postToPid した
                // 文字はその app の key window に溜まる。断りは帯の 1 行（This action doesn't reach {App} from here. Nothing was sent.）。
                // 前に出す途中（`go_direct` の後 = direct が Some）は今までどおり送る（前に出る前の frame の分は前に来た本物が受ける）
                _ if !front && crate::surface::mirror::route_for(&input) == crate::surface::mirror::InputRoute::PostToPid => {
                    if let Some(p) = self.panes.get_mut(&spec.id) {
                        p.line = Some((MirrorError::NeedsFront { app_name: app.clone(), kind: input.kind() }, crate::surface::now_ms()));
                    }
                    continue;
                }
                _ => self.relay(&spec.id, input),
            };
        }
        // G91: 映っている面の本物を枠の下にしまう・Mirror が送れない入力（relay の Err か helper の拒否の答え）が来たら本物を前に出す
        if live.is_some() {
            let focused = ctx.input(|i| i.viewport().focused);
            // 追記 240 の 1・242: Shibubu の中で何かを引いている（四隅・右 sidebar・下の panel の境目・Page の部品。映像の中の drag は本物への入力なので数えない）
            let drag = ctx.dragged_id().is_some_and(|d| d != resp.id);
            // MIRROR-HYBRID-1101: 本物を前に出している間に Mirror へ戻す（本物を Shibubu の下へ・映像は Mirror の最後の 1 枚のまま一緒に動く）。
            // 枠に被さった（下の panel・menu・popup・toast・chat）= 戻すだけ。Page の wheel（枠の外 = Shibubu に来た）か枠が動いた（Page の scroll・Shibubu の窓の
            // 移動と大きさ）= 戻して、枠が SETTLE_S 止まったら本物に戻す（動いている間に本物を追いかけさせない・10-23 owner「遅れて追いかけるのはダメ」）
            let wheel = ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::MouseWheel { .. })));
            let cover = covered(&ctx, ui.layer_id(), video) || above(&self.stack, &spec.id, video);
            let clipped = !ui.clip_rect().contains_rect(video);
            let at = slot_on_screen(&ctx, video);
            let (was_direct, moved) = match self.panes.get_mut(&spec.id) {
                Some(p) => {
                    // 0.5 point までの揺れ（Retina の端数・layout の丸め）は動いたと数えない（毎 frame 戻す = ちらつく）
                    let moved = p.slot_was.is_some_and(|s| at.is_none_or(|a| [s.x - a.x, s.y - a.y, s.w - a.w, s.h - a.h].iter().any(|d| d.abs() > 0.5)));
                    p.slot_was = at;
                    (p.direct.is_some(), moved)
                }
                None => (false, false),
            };
            // 追記 240 の 3（owner 242）: 本物が前に居る間に pointer が映像の縁の ±EDGE に来た（button を押していない）→ 本物を下へ（境目の当たりを本物の窓が隠さない）。
            // 中へ戻って止まれば dwell がまた前に出す
            let (gp, down) = if was_direct { (self.pointer)() } else { (None, false) };
            let edge = !down
                && gp.zip(at).is_some_and(|([x, y], a)| {
                    let (r, q) = (Rect::from_min_size(pos2(a.x as f32, a.y as f32), Vec2::new(a.w as f32, a.h as f32)), pos2(x as f32, y as f32));
                    r.expand(EDGE).contains(q) && !r.shrink(EDGE).contains(q)
                });
            if was_direct && edge && !(cover || wheel || moved) {
                self.put_back(&spec.id);
            }
            if was_direct && (cover || wheel || moved) {
                self.put_back(&spec.id);
                if let Some(p) = self.panes.get_mut(&spec.id).filter(|_| !cover) {
                    p.resume = Some(time);
                }
            } else if let Some(p) = self.panes.get_mut(&spec.id).filter(|p| p.direct.is_none() && p.resume.is_some()) {
                // 戻した後: 動いている間は数え直す・Shibubu が前に来た（人が Shibubu を押した）・被さった・はみ出した間は戻さない
                if moved || wheel || drag {
                    p.resume = Some(time);
                }
                if focused == Some(true) || cover || clipped {
                    p.resume = None;
                }
            }
            // 追記 240 の 1・242: 引いている間は本物を引き始めの所と大きさのまま（Shibubu の窓が動いた分だけは付いて行く = 窓の左上から数える）。離した frame に
            // 1 回だけ合わせ（Dock の on_host）、離してから合うまでを app.log に 1 行（`pump_docks`）
            let held = match self.panes.get_mut(&spec.id) {
                Some(p) if drag => *p.frozen.get_or_insert(video),
                Some(p) => {
                    if p.frozen.take().is_some_and(|r| r != video) {
                        p.released = Some(crate::surface::now_ms());
                    }
                    video
                }
                None => video,
            };
            // B90: 人が枠を動かした合図（Shibubu の窓が動いた・大きさが替わった・Page の wheel・何かを引く・押す / 離す・⌘ の key）→ `FOLLOW_S` の間だけ本物を
            // 枠へ合わせ直す（`tuck`）。枠が動いた事（`moved`）は合図に数えない（layout の揺れも本物の大きさの替わりも同じ量 = 揺れを本物へ移す）
            let host_rect = ctx.input(|i| i.viewport().inner_rect);
            let human = wheel
                || drag
                || ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released() || i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, modifiers, .. } if modifiers.command)));
            if let Some(p) = self.panes.get_mut(&spec.id) {
                if human || p.host_was != host_rect {
                    (p.follow_until, p.ask_until) = (time + FOLLOW_S, time + FOLLOW_S);
                }
                p.host_was = host_rect;
            }
            let slot = self.slot(&spec.id, &ctx, held);
            self.tuck(&spec.id, slot, focused, time);
            let p = self.panes.get(&spec.id);
            // 止まって戻すのを待っている間は Shibubu を前にしない（前に来ると戻すのを止める）
            if direct && p.is_some_and(|p| p.direct.is_none() && p.resume.is_none()) && focused != Some(true) {
                // pointer が枠の外へ離れて本物を下へ戻した: pointer が Shibubu の窓の上なら Shibubu を前に（keyboard も Shibubu へ）・外なら前面は替えない。
                // 前面が映している app の時だけ（owner 10-30「orcaを押すとmirror先のtexteditだけが浮き出て」: 人が別の app〈Orca〉を押した後に
                // その窓が Shibubu の矩形に重なっていても、前面を Shibubu へ取り返さない）
                let host = ctx.input(|i| i.viewport().inner_rect);
                if let (Some([x, y]), Some(h)) = ((self.pointer)().0, host) {
                    if h.contains(pos2(x as f32, y as f32)) && (self.front)().as_deref() == Some(bundle_id.as_str()) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
            }
            // 枠が SETTLE_S 止まった・本物の位置を Dock が合わせ直した（Active）→ 本物に戻す（押しの送り直しは無い）
            let settled = self.panes.get(&spec.id).is_some_and(|p| p.direct.is_none() && p.resume.is_some_and(|t| time - t >= SETTLE_S) && p.dock.as_ref().is_some_and(|d| d.life == Lifecycle::Active));
            if settled && !scaled && !drag {
                self.go_direct(&spec.id);
            }
            // 追記 214: 押した時に大きかった本物の大きさの答えが来た（`resize` が消えた = 読み戻しが替わった・RESIZE_WAIT_S を過ぎた）→ 合っていれば前に出す・
            // 縮まない（app の最小の大きさ）・Dock が外れた → 理由を 1 行（Mirror のまま）。はみ出した・被さった間は黙って捨てる（押し直しは上の枝が理由を言う）
            let fitted = self.panes.get_mut(&spec.id).filter(|p| p.fit_touch.is_some() && p.resize.is_none()).map(|p| {
                p.fit_touch = None;
                stay_reason(p, &app).or_else(|| scaled.then(|| format!("{app}'s window won't shrink to this frame · it stays a mirror")))
            });
            match fitted {
                Some(Some(line)) => crate::ui::toast::post(&ctx, line, true, None),
                Some(None) if !cover && !clipped => self.go_direct(&spec.id),
                _ => {}
            }
            // 追記 235 の 1: 本物が枠より大きい（縮めて見せている）面は、押す前に本物を今の映像の大きさへ（押した時に大きさを替えない = 押しは前後を入れ替えるだけ）。
            // 枠が動いている間・答えを待つ間・直接触っている間は頼まない。同じ映像の大きさは 1 回だけ（`prefit`・断られたら押した時に 214 の Fitting が理由を言う）・
            // 縮めて見せなくなったら捨てる（後で本物が自分で大きくなったらもう 1 回）
            // 追記 246 の 3: 人が選んだ大きさの在る開いた面はその大きさへ（再起動の後も保存した大きさに戻る・本物の比で縮めた映像の大きさに落ち着かない）
            // I31（owner 273「全画面出すようにdefaultで」）: 人が大きさを選んでいない面は、枠より小さい本物も枠の大きさへ（枠は置ける所の全部 = `frame_size`）。
            // 前は MIRROR-FIT-1101 の「既定では大きさを替えない」で小さいまま映した。断られたら帯に今の 1 行（`tuck`）・枠は本物に付いて行く（頼み直さない）
            let chosen = self.panes.get(&spec.id).and_then(|p| p.sized).filter(|_| window == MirrorWindow::Open);
            let size = chosen.unwrap_or(video.size());
            let grow = chosen.is_none() && real.is_some_and(|r| r.w < size.x as f64 - 1.0 || r.h < size.y as f64 - 1.0);
            let ready = self.panes.get(&spec.id).is_some_and(|p| {
                p.direct.is_none() && p.resize.is_none() && p.fit_touch.is_none() && p.prefit != Some(size) && p.dock.as_ref().is_some_and(|d| matches!(d.life, Lifecycle::Active | Lifecycle::Resizing))
            });
            // B90: 頼むのは人が枠を動かした合図の後（`FOLLOW_S`・`ask_until`）か、この Dock でまだ頼んでも合わせてもいない時（`prefit` が None）だけ
            // （置ける所が人の手なしに替わっても本物の大きさを替えない・映像は今の本物を比のまま収める）
            let asking = self.panes.get(&spec.id).is_some_and(|p| p.prefit.is_none() || time < p.ask_until);
            // I32: 拡大している面は頼まない（本物の大きさは替えない）
            let zoomed = self.windows.get(&spec.id).is_some_and(|s| s.lens.k > 1.0);
            if (scaled || grow) && ready && asking && !zoomed && !moved && !wheel && !drag {
                self.fit_to_frame(&spec.id, size, time);
                if let Some(p) = self.panes.get_mut(&spec.id).filter(|p| p.resize.is_some()) {
                    p.prefit = Some(size);
                }
            } else if let Some(p) = self.panes.get_mut(&spec.id) {
                match chosen {
                    Some(_) if !scaled => p.prefit = None,
                    // I31: 頼む前から枠の大きさだった本物も合わせた事にする（後で本物が自分で替えた大きさは `frame_size` が追う = 頼み直して争わない）。
                    // まだ何も合わせていない時だけ（断られて本物に付いて行く枠の大きさで上書きすると、次の frame で置ける所の全部に戻って頼み直し続ける）
                    None if ready && !scaled && !grow && p.prefit.is_none() => p.prefit = Some(size),
                    _ => {}
                }
            }
            // 追記 214（spec の 2・3）: 前に出す手を打ってから FRONT_WAIT_MS たっても Shibubu が前のまま（本物が前に来ない = 系が activate を断った）→ 下へ戻して理由
            if self.panes.get(&spec.id).is_some_and(|p| p.direct == Some(false) && p.touch.is_some_and(|t| crate::surface::now_ms() - t > FRONT_WAIT_MS)) {
                self.put_back(&spec.id);
                crate::ui::toast::post(&ctx, format!("{app} didn't come to the front · it stays a mirror"), true, None);
            }
            match self.panes.get(&spec.id) {
                // 大きさの答え（`resize`）を待つ間も読む（235 の押す前に合わせた答え・動かない窓は frame が来ない）
                Some(p) if p.direct.is_some() || p.fit_touch.is_some() || p.resize.is_some() => ctx.request_repaint_after(DIRECT_POLL),
                Some(p) if p.resume.is_some() => ctx.request_repaint_after(Duration::from_secs_f64(SETTLE_S)),
                _ => {}
            }
        }
        // L5-STICK: Shibubu を最小化する・隠す（⌘H = bridge の app_hidden）と本物の窓も一緒に最小化（独りで画面に残らない）・戻すと一緒に戻す。
        // 黄で自分から最小化した面は触らない。映っていない frame でも見る（本物を最小化すると helper が 2 秒で stream を止め〈window_hidden〉、戻した frame には映像が無い）。
        // 隠れているのに前（focused）なら戻っている（app_unhidden が bridge から着く前の frame）
        let front = ctx.input(|i| i.viewport().focused);
        let dock = self.panes.get_mut(&spec.id).and_then(|p| p.dock.as_mut());
        if let (Some(d), Some(true)) = (dock, front) {
            // 前の app は隠れていない（app_unhidden を取りこぼしても、前でなくなる度に本物を最小化しない）
            d.host_hidden = false;
        }
        let hidden = self.panes.get(&spec.id).and_then(|p| p.dock.as_ref()).is_some_and(|d| d.host_hidden);
        let host_min = ctx.input(|i| i.viewport().minimized) == Some(true) || hidden;
        let flip = match self.panes.get_mut(&spec.id) {
            Some(p) if p.host_min != host_min && p.dock.is_some() => {
                p.host_min = host_min;
                if !host_min && p.surface.is_err() {
                    // 戻した: 自分で最小化して止まった stream の「隠れた」を会話に出さず「探しています」から・最小化が解けるのを RETRY_MS 待って探し直す
                    (p.answered, p.tried_ms) = (false, now);
                }
                true
            }
            _ => false,
        };
        if flip && !matches!(self.window(&spec.id), MirrorWindow::Minimized | MirrorWindow::Closed) {
            self.minimize_source(&spec.id, host_min);
        }
        if let Some(w) = want {
            self.set_window(&spec.id, w, time);
        }
        state
    }

    /// 殻が毎 frame の終わりに 1 回呼ぶ（Mirror の Page を描かなかった frame でも）。この frame で `show` を呼び、開いている面が在る時だけ
    /// App Nap を止め（`ActivityHold`）、それ以外は放す。止めないと背景で 30〜60 秒後から描き直しが約 21 秒に 1 回になる（W16M-1b の実測）。
    /// `keep_alive` が false でこの frame に描かなかった面は閉じる。
    /// 追記 245 の 1: keep_alive の面も、描かなかった frame で映っていれば stream を止める（`Pane::stop`・面と Dock は残る・本物の app は閉じない）。
    /// G82: 描かなかった keep_alive の面の窓探しの答えも受け取る（今の Page の面が、他の面の答えを永久に待たない・2 本の list を並べない）。着いた答えが映る面なら次の終わりで止める。
    /// 保証する: Mirror の Page を離れた次の frame の終わりで App Nap を放す（毎 frame begin / end しない）。keep_alive の面は残る。
    pub fn end_frame(&mut self) {
        self.later.clear();
        self.stack.clear();
        let drawn = std::mem::take(&mut self.drawn);
        // G91: 描かなかった面の本物は Shibubu の下へ（Page を替えた）・閉じる面（keep_alive でない）の本物は元の位置へ
        let ids = |f: &dyn Fn(&Pane) -> bool| self.panes.iter().filter(|(id, p)| !drawn.contains(*id) && f(p)).map(|(id, _)| id.clone()).collect::<Vec<_>>();
        let (away, gone) = (ids(&|p| p.direct.is_some()), ids(&|p| !p.keep_alive && p.dock.is_some()));
        away.iter().for_each(|id| self.put_back(id));
        gone.iter().for_each(|id| self.untuck(id));
        self.pump_docks();
        self.panes.retain(|id, p| p.keep_alive || drawn.contains(id));
        // Page を離れた間に元の app で直接打たれても古い caret を出さない（戻った時は次の click で描き直す）。
        // G82: 描かなかった keep_alive の面の窓探しの答えもここで受け取る（受け取らないと pending が残り、今の Page の面が「探しています」のまま待ち続ける）
        let now = crate::surface::now_ms();
        // MIRROR-SPEED-1101: 閉じた面・描いていない古い IOSurface の texture を放す
        evict_natives(&mut self.natives, &self.panes, now);
        self.panes.iter_mut().filter(|(id, _)| !drawn.contains(*id)).for_each(|(id, p)| {
            // MIRROR-FIT-1101: Page を離れたら枠の選びも外す・MIRROR-HYBRID-1101: 止まったら本物に戻すのも捨てる（戻った時に勝手に前に出さない）
            (p.caret, p.selected, p.grab, p.resume, p.slot_was, p.fit_touch, p.dwell, p.frozen) = (None, false, None, None, None, None, None, None);
            p.take_answer(id, now);
            // 追記 245 の 1: Page を離れた面は映すのを止める（探している途中に着いた答えが映る面ならここで止める・app と本物の窓はそのまま）
            if p.surface.is_ok() {
                eprintln!("MIRROR-HYBRID-1101 追記 245: Mirror {id}: Page を離れたので映すのを止めました（app と本物の窓はそのまま）");
                p.stop();
            }
        });
        // UI-5m: 拡大した面を描かなかった（別の Page・同じ Page の別の tab）なら開いた形に戻す（renderer が隠した region を残さない）
        for (id, s) in self.windows.iter_mut().filter(|(id, s)| s.now == MirrorWindow::Zoomed && !drawn.contains(*id)) {
            *s = Shape { now: MirrorWindow::Open, was: MirrorWindow::Open, ..*s };
            self.window_changes.push((id.clone(), MirrorWindow::Open));
        }
        // UI-5m: しまった面は stream を保つが映していないので App Nap を止める理由にしない
        let open = !drawn.is_empty() && self.panes.iter().any(|(id, p)| p.surface.is_ok() && !matches!(self.window(id), MirrorWindow::Minimized | MirrorWindow::Closed));
        if !open {
            self.awake = None;
        } else if self.awake.is_none() {
            self.awake = ActivityHold::begin("Mirroring the original app's window");
        }
    }

    /// W16M-2: surface `surface_id` の Mirror へ人の入力を 1 つ送る（`show` が `mirror_inputs` の結果ごとに呼ぶ）。面ごとの `ControlLease`
    /// （open した時に `ControlLease::human`）を `MirrorSurface::relay_human` に渡す。Err（`NeedsFront`・`Log` 等）は面の Err にせず、その面の
    /// 拒否の 1 行として次の frame の `show` が出す（helper から後で来る拒否は `MirrorSurface::refusal` で同じ 1 行に）。
    /// 例: `pages.relay("s1", MirrorInput::Key { key: "s".into(), mods: vec![KeyMod::Cmd] })?`
    /// 保証する: 開いていない面（同意の前・窓が無い・消えた）には送らず、relay の記録も書かない（その時の Err は面の状態の 1 行）。
    /// 保証しない: 届いた事（E1）。
    pub fn relay(&mut self, surface_id: &str, input: MirrorInput) -> Result<RelayRecord, MirrorError> {
        let pane = self.panes.get_mut(surface_id).ok_or_else(|| MirrorError::Helper(format!("The mirror screen {surface_id} isn't open yet")))?;
        let m = pane.surface.as_mut().map_err(|e| e.clone())?;
        // どの入力でも前の caret は古くなる（click の新しい caret は helper の答えで来る）
        pane.caret = None;
        let click = matches!(input, MirrorInput::Click { .. });
        let r = m.relay_human(&mut pane.lease, input);
        pane.click_seq = r.as_ref().ok().filter(|_| click).map(|rec| rec.seq);
        // 送れたら前の拒否の 1 行は消す（新しい入力が通った）
        pane.line = r.as_ref().err().map(|e| (e.clone(), crate::surface::now_ms()));
        r
    }

    /// MIRROR-HYBRID-1101: 触って本物を前に出した面の、離した最初の 1 押し（click・drag・scroll）を本物へ（`MirrorSurface::relay_front`）。拒否は `relay` と同じく面の 1 行に
    /// （本物が前に居る間は帯の `Using … directly` が先なので出ない）。
    /// Wave: MIRROR-HYBRID-1101
    fn relay_front(&mut self, surface_id: &str, input: MirrorInput) -> Result<RelayRecord, MirrorError> {
        let pane = self.panes.get_mut(surface_id).ok_or_else(|| MirrorError::Helper(format!("The mirror screen {surface_id} isn't open yet")))?;
        let m = pane.surface.as_mut().map_err(|e| e.clone())?;
        pane.caret = None;
        let r = m.relay_front(&mut pane.lease, input);
        pane.line = r.as_ref().err().map(|e| (e.clone(), crate::surface::now_ms()));
        r
    }

    /// MIRROR-FRAME-1101: 空の枠（`SurfaceSpec::blank_frame`）の 1 frame（`show` の早い return・窓を探さない・app を起こさない・契約は `take_chosen_windows` の doc）。
    /// 帯 = `Mirror frame`・真ん中 = `Choose a window`（menu = 全部の app の窓・最後に `Open an app…`）と 1 行。選んだ窓は `chosen` に bundle id つきで入る。
    fn blank(&mut self, ui: &mut egui::Ui, rect: Rect, spec: &SurfaceSpec, ax: &str, t: &theme::Tokens) -> PortalState {
        use std::sync::mpsc::TryRecvError::Empty;
        let ctx = ui.ctx().clone();
        let (band, video) = split_band(rect);
        let me = std::process::id() as i64;
        // (b) 引いて入れる: Shibubu が前でない間だけ pointer の位置と button を読む（`pointer_now` = 読むだけ・cursor は動かさない・前面も取らない）。
        // 押したまま枠の上 = 光らせる・枠の上で離した = 窓の一覧を裏の thread で 1 回引いて `dropped_window`。
        // 前でない間は Shibubu に event が来ないので、`watch_drag` の thread が押している間と離した時だけ frame を起こす（押していない間の frame は 0）
        let (focused, at, pointer) = (ctx.input(|i| i.viewport().focused), slot_on_screen(&ctx, video), self.pointer);
        if focused == Some(false) {
            let pass = ctx.cumulative_pass_nr() + 1;
            self.drag_armed.get_or_insert_with(|| watch_drag(ctx.clone(), pointer)).store(pass, Ordering::Relaxed);
        }
        let mut lit = false;
        if let Some(pane) = self.panes.get_mut(&spec.id) {
            if focused == Some(false) {
                let (gp, down) = pointer();
                let over = gp.zip(at).is_some_and(|([x, y], a)| x >= a.x && x <= a.x + a.w && y >= a.y && y <= a.y + a.h);
                let far = |p0: [f64; 2]| gp.is_some_and(|[x, y]| (x - p0[0]).hypot(y - p0[1]) > FRAME_DRAG_MIN);
                match (down, pane.press) {
                    (true, None) => pane.press = gp,
                    (true, Some(p0)) => lit = over && far(p0),
                    (false, Some(p0)) => {
                        pane.press = None;
                        if let Some(g) = gp.filter(|_| over && far(p0) && pane.dropped.is_none()) {
                            let (tx, rx) = std::sync::mpsc::channel();
                            let helper = self.helper.clone();
                            std::thread::spawn(move || {
                                let _ = tx.send(MirrorTarget::windows(&helper));
                            });
                            pane.dropped = Some((g, rx));
                        }
                    }
                    (false, None) => {}
                }
            } else {
                pane.press = None;
            }
            match pane.dropped.as_ref().map(|(_, rx)| rx.try_recv()) {
                Some(Ok(list)) => {
                    let g = pane.dropped.take().map_or([0.0; 2], |(g, _)| g);
                    let why = list.as_ref().err().map_or(String::new(), |e| e.to_string());
                    match list.ok().and_then(|l| dropped_window(&l, g, me)) {
                        Some((w, title)) => {
                            eprintln!("MIRROR-FRAME-1101: Mirror {}: 題の帯で引いて枠に重ねた {} の窓を入れます", spec.id, w.bundle_id);
                            self.chosen.push((spec.id.clone(), title_word(&title), Some(w.bundle_id)));
                            // 引いた本物は枠の上に居る = Shibubu を前に（映った後は今の Mirror の面と同じ決まりで本物を枠の下へ）
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        None => eprintln!("MIRROR-FRAME-1101: Mirror {}: 枠の上で離した物は題の帯で引いた窓ではない（入れない・{why}）", spec.id),
                    }
                }
                Some(Err(Empty)) => ctx.request_repaint_after(Duration::from_millis(100)),
                Some(Err(_)) => pane.dropped = None,
                None => {}
            }
        }
        let p = ui.painter_at(rect);
        p.rect_filled(band, 0.0, t.surface_1);
        p.hline(band.x_range(), band.bottom() - 0.5, theme::hairline(t));
        p.rect_stroke(rect, 0.0, theme::hairline(t), egui::StrokeKind::Inside);
        let (word, color) = if lit { (AX_RELEASE, t.text_primary) } else { (AX_FRAME, t.text_secondary) };
        let galley = theme::truncated(ui, word, theme::Type::Small.font(), color, band.width() - 2.0 * theme::SP_3);
        p.galley(pos2(band.left() + theme::SP_3, band.center().y - galley.size().y / 2.0), galley, color);
        if lit {
            p.rect_stroke(rect, 0.0, egui::Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Inside);
        }
        // 枠の node（role と identifier は映っている面と同じ・名前は `Mirror frame`・状態の語は持たない = 何も映していない）
        let resp = ui.interact(video, egui::Id::new(ax), Sense::hover());
        ctx.accesskit_node_builder(resp.id, |n| {
            n.set_role(FRAME_ROLE);
            n.set_author_id(ax.to_owned());
            n.set_label(AX_FRAME);
        });
        // 真ん中に button と 1 行（追記 245 の 2 の映っていない面と同じ置き方）
        let top = (video.center().y - theme::INPUT_H).max(video.top() + theme::SP_2).min(video.bottom() - theme::SP_2);
        let r = Rect::from_min_max(pos2(video.left() + theme::SP_2, top), pos2(video.right() - theme::SP_2, video.bottom() - theme::SP_2));
        let mut inner = ui.new_child(UiBuilder::new().id_salt((ax, "blank")).max_rect(r).layout(egui::Layout::top_down(egui::Align::Center)));
        let opener = theme::button_compact(&mut inner, AX_PICK, theme::Variant::Primary);
        inner.add(egui::Label::new(theme::Type::Small.text(AX_DRAG_HINT).color(t.text_secondary)).truncate());
        // 開いた時に窓の一覧を裏の thread で引き直し、一覧を渡した後に同じ thread で行の小さな絵を撮る
        // （帯の `Choose window…` と同じ形・UI の thread で helper を待たない・絵は一覧を待たせない）
        if opener.clicked() && !ui.memory(|m| m.is_popup_open(opener.id.with("kangae-menu"))) {
            let ((tx, rx), (shot_tx, shot_rx)) = (std::sync::mpsc::channel(), std::sync::mpsc::channel());
            let helper = self.helper.clone();
            std::thread::spawn(move || {
                let list = MirrorTarget::windows(&helper);
                let ids: Vec<u64> = list.iter().flatten().filter(|(w, _, _)| w.pid != me && !w.bundle_id.is_empty()).map(|(w, _, _)| w.window_id).collect();
                let _ = tx.send(list);
                let shots = MirrorTarget::shots(&helper, &ids, SHOT_PX).into_iter().filter_map(|(id, png)| {
                    let img = image::load_from_memory(&png).ok()?.to_rgba8();
                    Some((id, ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())))
                });
                let _ = shot_tx.send(shots.collect());
            });
            (self.pick, self.picked, self.shooting) = (Some(rx), None, Some(shot_rx));
            self.shots.clear();
            self.filter.clear();
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // 着いた・thread が落ちた（Disconnected）= 待つのをやめる
        match self.pick.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(r)) => (self.pick, self.picked) = (None, Some(r)),
            Some(Err(Empty)) => ctx.request_repaint_after(Duration::from_millis(100)),
            Some(Err(_)) => self.pick = None,
            None => {}
        }
        match self.shooting.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(v)) => {
                self.shooting = None;
                for (id, img) in v {
                    self.shots.insert(id, ctx.load_texture(format!("mirror-shot {id}"), img, TextureOptions::LINEAR));
                }
            }
            Some(Err(Empty)) => ctx.request_repaint_after(Duration::from_millis(100)),
            Some(Err(_)) => self.shooting = None,
            None => {}
        }
        enum Pick {
            Window(MirrorTarget, String),
            App,
        }
        let (filter, picked, shots) = (&mut self.filter, &self.picked, &self.shots);
        let pick = theme::menu_popup(&mut inner, &opener, |ui| {
            ui.set_width(PICK_W);
            theme::ax_name(ui.add(egui::TextEdit::singleline(&mut *filter).hint_text(AX_FILTER).desired_width(f32::INFINITY)), AX_FILTER);
            let quiet = |ui: &mut egui::Ui, s: &str| {
                ui.add(egui::Label::new(theme::Type::Small.text(s).color(t.text_secondary)).truncate());
            };
            let mut out = None;
            match picked {
                None => quiet(ui, "Looking for windows"),
                Some(Err(e)) => quiet(ui, &e.to_string()),
                Some(Ok(list)) => {
                    let needle = filter.trim().to_lowercase();
                    // Shibubu 自身と bundle id の無い窓（app として保存できない）は出さない・字は app の名前か題に含む物
                    let rows: Vec<&(MirrorTarget, String, [f64; 4])> = list
                        .iter()
                        .filter(|(w, title, _)| w.pid != me && !w.bundle_id.is_empty() && (needle.is_empty() || w.app_name.to_lowercase().contains(&needle) || title.to_lowercase().contains(&needle)))
                        .collect();
                    if rows.is_empty() {
                        quiet(ui, if needle.is_empty() { "No open windows" } else { "No windows match" });
                    }
                    // 見えている行だけ描く（小さな絵を読むのも見えている行だけ）
                    egui::ScrollArea::vertical().max_height(PICK_H).show_rows(ui, PICK_ROW_H, rows.len(), |ui, range| {
                        for (w, title, _) in &rows[range] {
                            if window_row(ui, w, title, shots.get(&w.window_id).cloned(), t).clicked() {
                                out = Some(Pick::Window(w.clone(), title.clone()));
                            }
                        }
                    });
                }
            }
            theme::rule(ui);
            if theme::button_menu_item(ui, AX_OPEN_APP, "", theme::Variant::Ghost).clicked() {
                out = Some(Pick::App);
            }
            out
        });
        match pick {
            Some(Pick::Window(w, title)) => {
                eprintln!("MIRROR-FRAME-1101: Mirror {}: 一覧で選んだ {} の窓を入れます", spec.id, w.bundle_id);
                self.chosen.push((spec.id.clone(), title_word(&title), Some(w.bundle_id)));
            }
            // 245 の決まり: 入れた app は最初の探しで後ろで起こす（動いていても `open -g` = 窓の無い app は窓を作る）
            Some(Pick::App) => match (self.pick_app)() {
                Some(Ok(bundle)) => {
                    eprintln!("MIRROR-FRAME-1101: Mirror {}: Open an app… で {bundle} を入れます", spec.id);
                    if let Some(p) = self.panes.get_mut(&spec.id) {
                        p.open_app = Some(false);
                    }
                    self.chosen.push((spec.id.clone(), None, Some(bundle)));
                }
                Some(Err(line)) => crate::ui::toast::post(&ctx, line, true, None),
                None => {}
            },
            None => {}
        }
        PortalState::Undiscovered
    }

    /// RQ4（RQ-019）: 帯の app 名の menu（`Choose window…`）か、窓が消えた時の帯の `Choose window…` で人が選んだ窓（面の id と題名・None = `Any window` = app の前の窓）。
    /// 取ったら空になる。呼ぶ側（generated.rs の `draw_page` の後）が 1 件ずつ `PageIntent::ChooseWindow { surface_id, title }` にして送る（Project に残り Undo できる）。
    /// 契約（本体の `show`）: 帯の app 名を押すと menu に `MirrorTarget::titles` の題名（裏の thread で引く・引いている間は `Looking for windows`）と `Any window`。
    /// 窓が消えた時（`WindowGone`・`NoWindow`）は帯の次の一手を `Open {app}` と `Choose window…` の 2 つにする（今の `Try again` は 探し直しが代わる）。
    /// 追記 245 の 2: 映っていない面の状態の 1 行と次の一手は面の真ん中に置き、Project の repo に在る書類の面は `Open {App} · {書類}`。
    /// 保証しない: 選んだ窓がその後も在る事（消えたら同じ 2 つの手に戻る）。
    /// 契約（MIRROR-FRAME-1101・owner 247）: 3 つ目 = 入れる app の bundle id（`PageIntent::ChooseWindow { bundle_id }`）。空の枠（`SurfaceSpec::blank_frame`）の `show` は
    /// 窓を探さず app を起こさず、題の帯 `Mirror frame`（AX の枠の名前も）と真ん中に `Choose a window`（Primary）と 1 行 `Or drag a window's title bar here` を描く。
    /// (a) `Choose a window` の menu = `MirrorTarget::windows` の Shibubu 以外の窓（裏の thread・着くまで `Looking for windows`）を 1 行ずつ（小さな絵〈helper の `shot` が
    /// 在る時〉・app の icon・`{App} · {窓の題}` = AX の名前）・上に字で絞る欄 `Filter windows`・最後に `Open an app…`（`pick_app` → 入れて 245 の決まりで後ろで起こす）。
    /// (b) Shibubu が前でない間に pointer の button を押したまま枠の上に来たら枠を光らせ（focus の色の縁と `Release to mirror this window`）、離した所で
    /// `MirrorTarget::windows` を 1 回引き、`dropped_window`（前の窓の題の帯が pointer の下・その下が Shibubu）の窓を入れて Shibubu を前に。
    /// どちらも題は `title_word`（`/` を含む題は `/` の無い一番長い所 = 書類の path に読まない）。保存は bundle id と題の語（窓の id は保存しない = 再起動の後は同じ app の同じ題）。
    /// Wave: RQ4, MIRROR-FRAME-1101
    pub fn take_chosen_windows(&mut self) -> Vec<(String, Option<String>, Option<String>)> {
        std::mem::take(&mut self.chosen)
    }

    /// MIRROR-HYBRID-1101 追記 246 の 3（owner「離したら保存・⌘Z で枠とどいた部品が 1 回で戻る」）: 人が四隅で引き終わった枠（面の id と `SurfaceFrame` =
    /// Page の並びの所からのずれ x・y と、題の帯を含む枠の w・h）。取ったら空になる。呼ぶ側（generated.rs の `draw_page` の後）が 1 件を
    /// `PageIntent::SetSurfaceFrame` にして送る（Project の revision = ⌘Z で戻る・どいた部品は縦の流れで戻る = 部品ごとの位置は持たない）。
    /// 契約（本体の `show`）: Project の `SurfaceSpec::frame` が替わったら（⌘Z・⇧⌘Z・再起動の後の初め）その大きさとずれで枠を描き、本物をその映像の大きさへ頼む
    /// （自分が送った frame が戻って来ただけなら何もしない）。
    /// 例: `if let Some((surface_id, f)) = pages.take_frames().pop() { intent = Some(PageIntent::SetSurfaceFrame { surface_id, x: f.x, y: f.y, w: f.w, h: f.h }) }`
    /// 保証しない: 本物の app がその大きさを受ける事（断った時は枠を戻さず、本物を比のまま縮めて収める）。
    /// Wave: MIRROR-HYBRID-1101
    pub fn take_frames(&mut self) -> Vec<(String, SurfaceFrame)> {
        std::mem::take(&mut self.frames)
    }

    /// RQ4（RQ-018・UIUX N06「モデル/Agentが失敗しても手動操作できる」）: 面の元の app の窓を前に出す（人が帯の `Open original` を押した時だけ・AI の手からは呼ばない = MH-S5）。
    /// 映している窓が在ればその窓を・無ければ app を `open -b <bundle id>` で（書類の面は `open -b <bundle id> <file>`）。Mirror は止めない（戻れば同じ面に映る）。
    /// 契約（本体の `show`）: 帯の右に `Open original`（`portal_frame::PortalAction::ContinueInSource` の文字）を常に 1 つ。許可を拒まれた面（画面収録が off）でも押せる。
    /// 保証する: Shibubu の同意・Project・Page を変えない。失敗は Err の 1 行（`Couldn't open {app} · {理由}`・帯の題の所に出す）。
    /// 保証しない: 元の app が前に来た事を確かめる事（OS の前面の判断）。
    /// Wave: RQ4
    pub fn open_original(&mut self, surface_id: &str) -> Result<(), String> {
        self.open_source(surface_id, false)
    }

    /// `open_original`（前に出す）と窓が消えた面の `Open {App}`（書類の面・`background`）の本体。書類は Project の repo の中に在る file の時だけ渡す
    /// （`Choose window…` の題の語は app だけを開く）。失敗の 1 行を面に残す（`show` が帯の題の所に `LINE_MS` の間）。
    // ponytail: 前に出るのは app の前の窓（映している窓とは限らない）。同じ app の窓が複数で別の窓が前に来るなら、helper に窓の番号で raise させる
    fn open_source(&mut self, surface_id: &str, background: bool) -> Result<(), String> {
        let Some(p) = self.panes.get(surface_id) else { return Err("Couldn't open the original app · this mirror isn't on screen".into()) };
        let Some((bundle, file)) = p.source.clone() else { return Err("Couldn't open the original app · this mirror isn't on screen".into()) };
        let app = p.surface.as_ref().ok().map(|m| m.target().app_name.clone()).or_else(|| p.target.as_ref().map(|t| t.app_name.clone())).or_else(|| p.name.clone()).unwrap_or_else(|| bundle.rsplit('.').next().unwrap_or(&bundle).to_string());
        let doc = self.workspace.as_ref().zip(file.as_ref()).map(|(w, f)| w.join(f)).filter(|d| d.is_file());
        let r = (self.open)(&bundle, doc.as_deref(), background).map_err(|why| format!("Couldn't open {app} · {why}"));
        if let Some(p) = self.panes.get_mut(surface_id) {
            p.band_line = r.as_ref().err().map(|e| (RegionState::Error, e.clone(), crate::surface::now_ms()));
        }
        r
    }

    /// `show` の中で人が同意の button を押した記録（呼ぶ側が `ui_state` に保存する）。取ったら空になる。
    pub fn take_granted(&mut self) -> Vec<ConsentRecord> {
        std::mem::take(&mut self.granted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::mirror::{tests::fake_stream_helper, ConsentGate};

    /// MIRROR-QUIET-1101（owner 208）: 窓が見つからない間の探し直しは、探し始めて 20 秒までは 2 秒ごと・その後は 10 秒ごと
    #[test]
    fn mirror_quiet_search_slows_to_10_s_after_20_s() {
        assert_eq!([search_gap(0.0), search_gap(19_999.0), search_gap(20_000.0), search_gap(600_000.0)], [2000.0, 2000.0, 10_000.0, 10_000.0]);
    }

    /// 反証（W16M-1b）: App Nap を止めるのは Mirror の Page を描いている間だけ。Page を離れた frame の終わりで放す
    #[test]
    fn w16m1b_activity_hold_is_released_when_the_mirror_page_is_not_drawn() {
        let (sh, dir) = fake_stream_helper("cat >/dev/null");
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("s1".into(), Pane::new(Ok(m)));
        // Mirror の Page を描いた frame: 止める（macOS のみ）
        pages.drawn.insert("s1".into());
        pages.end_frame();
        assert_eq!(pages.awake.is_some(), cfg!(target_os = "macos"), "Mirror を描いている間に App Nap を止めていない");
        // 別の Page へ移った frame（show を呼ばない）: 放す
        pages.end_frame();
        assert!(pages.awake.is_none(), "Mirror の Page を離れても App Nap を止めたまま");
        // 面が開いていない（Err）なら描いていても止めない
        pages.panes.insert("s1".into(), Pane::new(Err(MirrorError::WindowGone)));
        pages.drawn.insert("s1".into());
        pages.end_frame();
        assert!(pages.awake.is_none(), "開いている面が無いのに App Nap を止めた");
    }

    /// W16M-3: keep_alive の面は Page を離れても残る。App Nap は描いていない frame では放す。
    #[test]
    fn w16m3_keep_alive_pane_survives_leaving_the_page() {
        let (sh, dir) = fake_stream_helper("cat >/dev/null");
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let mut pages = MirrorPages::new(sh, [], None);
        let mut pane = Pane::new(Ok(m));
        pane.keep_alive = true;
        pages.panes.insert("notes".into(), pane);
        pages.end_frame();
        assert!(pages.panes.contains_key("notes"), "keep_alive の面を Page を離れて閉じた");
        assert!(pages.awake.is_none(), "描いていないのに App Nap を止めたまま");
        pages.panes.insert("gone".into(), Pane::new(Err(MirrorError::WindowGone)));
        pages.end_frame();
        assert!(!pages.panes.contains_key("gone"), "keep_alive でない面が残った");
        assert!(pages.panes.contains_key("notes"));
    }

    /// MIRROR-HYBRID-1101 追記 245 の 1（owner「shibubu開いた時に全てのappのmirrorしている中身を開くのはやめよう」）: 描かなかった（Page を離れた）面は
    /// keep_alive でも stream を止める（面と人が選んだ大きさは残す・app は閉じない）。描いた面は止めない。戻った frame はすぐ探し、止まっている app は起こす。
    /// 負の対照（未実行・cargo なし）: `end_frame` の `stop` を外す → notes が Ok のまま赤
    #[test]
    fn hybrid_245_leaving_the_page_stops_mirroring_but_keeps_the_face() {
        let (sh, dir) = fake_stream_helper("cat >/dev/null");
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let mut pages = MirrorPages::new(sh, [], None);
        let mut notes = Pane::new(Ok(m));
        (notes.keep_alive, notes.sized) = (true, Some(Vec2::new(500.0, 300.0)));
        pages.panes.insert("notes".into(), notes);
        // 描いた frame（Page を見ている）: 止めない
        pages.drawn.insert("notes".into());
        pages.end_frame();
        assert!(pages.panes["notes"].surface.is_ok(), "描いている面まで止めた");
        // 描かなかった frame（Page を離れた）: 止める・面と人が選んだ大きさは残る
        pages.end_frame();
        let p = &pages.panes["notes"];
        assert!(p.surface.is_err(), "Page を離れた面が映し続けている（stream を止めていない）");
        assert_eq!(p.sized, Some(Vec2::new(500.0, 300.0)), "人が選んだ大きさまで捨てた");
        assert!(!p.answered && p.tried_ms == f64::NEG_INFINITY && p.open_app == Some(true), "戻った frame ですぐ探さない・止まっている app を起こさない");
    }

    /// W16M-3: 画面収録の許可が無い面には送らず、relay の記録も書かない（Err は面の状態の 1 行）。
    #[test]
    fn w16m3_denied_pane_does_not_relay() {
        let mut pages = MirrorPages::new(std::path::PathBuf::from("/dev/null"), [], None);
        pages.panes.insert("notes".into(), Pane::new(Err(MirrorError::ScreenCaptureDenied)));
        let e = pages.relay("notes", MirrorInput::Text { text: "a".into() }).unwrap_err();
        assert_eq!(e, MirrorError::ScreenCaptureDenied);
    }

    /// LAYOUT（operator 10-31・DOGFOOD-X 周 2）: 下の panel を開いて床が入らない間だけ Shibubu が畳んだ面は保存せず、panel を閉じたら戻る
    /// （完成予想図の Music 右上を、Terminal を 1 回開いただけで崩したままにしない）。利用者が自分で畳んだ面は保存し、panel を閉じても戻さない。
    /// 足りない間に利用者が開き直した面は畳み直さない。Shibubu が前でない間は戻さない。負の対照（docs/internal/layout-arm.sh の room）: 自動の畳みも保存する形 → 赤。
    #[test]
    fn layout_room_fold_is_not_saved_and_comes_back() {
        let mut pages = MirrorPages::new(std::path::PathBuf::from("/dev/null"), [], None);
        // 開く → 足りない = 黄（保存しない）→ 閉じる = 足りる → 開いた形に戻る（保存しない）
        pages.fold_for_room("music", true, true, 1.0);
        assert_eq!(pages.window("music"), MirrorWindow::Minimized, "足りない時に畳まない");
        assert!(pages.take_window_changes().is_empty(), "panel のための畳みを保存した（再起動を跨いで右上が崩れたまま）");
        pages.fold_for_room("music", true, true, 1.1);
        // Shibubu が前でない間は戻さない（最小化を解くと本物が前に出うる・L5 の前面の規則）
        pages.fold_for_room("music", false, false, 1.5);
        assert_eq!(pages.window("music"), MirrorWindow::Minimized, "Shibubu が前でないのに本物の最小化を解いた");
        pages.fold_for_room("music", false, true, 2.0);
        assert_eq!(pages.window("music"), MirrorWindow::Open, "panel を閉じても Music が戻らない");
        assert!(pages.take_window_changes().is_empty(), "戻す形を保存した");
        // 利用者が自分で畳んだ面: 保存する・panel を開いて閉じても戻さない
        pages.set_window("music", MirrorWindow::Minimized, 3.0);
        assert_eq!(pages.take_window_changes(), vec![("music".to_string(), MirrorWindow::Minimized)], "利用者の畳みを保存しない");
        pages.fold_for_room("music", true, true, 4.0);
        pages.fold_for_room("music", false, true, 5.0);
        assert_eq!(pages.window("music"), MirrorWindow::Minimized, "利用者が畳んだ面を開き直した");
        // 足りない間に利用者が開き直した面は畳み直さず、その後に利用者が畳んだ黄は戻さない
        pages.set_window("music", MirrorWindow::Open, 6.0);
        pages.fold_for_room("music", true, true, 7.0);
        assert_eq!(pages.window("music"), MirrorWindow::Minimized, "前提: 足りない時に畳まない");
        pages.set_window("music", MirrorWindow::Open, 7.5);
        pages.fold_for_room("music", true, true, 8.0);
        assert_eq!(pages.window("music"), MirrorWindow::Open, "利用者が開き直した面を畳み直した");
        pages.set_window("music", MirrorWindow::Minimized, 8.5);
        pages.fold_for_room("music", false, true, 9.0);
        assert_eq!(pages.window("music"), MirrorWindow::Minimized, "利用者が畳んだ黄を戻した");
    }

    /// G97（AUDIT2 D・後ろの qa の Shibubu が CPU 44%・WindowServer 24%）: 前面でない Shibubu は映っている Mirror を 33ms で描き直さない。
    /// 着いた frame が描き直しを起こす口（`MirrorSurface::wake_with`）を置く。前面なら今まで通り 33ms。
    /// 負の対照: 前面かを見ずに 33ms を頼む → 赤・`wake_with` を呼ばない → 赤。
    #[test]
    fn g97_a_mirror_behind_other_apps_does_not_repaint_every_33ms() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        use crate::surface::mirror::tests::msg;
        let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; cat >/dev/null");
        let hd = serde_json::to_vec(&serde_json::json!({"seq": 1, "w": 4, "h": 2, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 2.0, "h": 1.0}, "factor": 2.0})).unwrap();
        let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
        payload.extend(hd);
        payload.extend([200u8; 4 * 2 * 4]);
        std::fs::write(dir.join("frame.bin"), msg(1, payload)).unwrap();
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages, focused: bool| {
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            input.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { focused: Some(focused), ..Default::default() });
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
            out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay)
        };
        for _ in 0..60 {
            frame(&mut pages, false);
            if pages.panes["notes"].texture.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(pages.panes["notes"].texture.is_some(), "前提: 偽の helper の frame が 3 秒で texture に上がらない");
        // 出の動きが終わるまで回してから測る（1 frame = egui の予測の 1/60 秒）
        let behind = (0..40).map(|_| frame(&mut pages, false)).last().unwrap();
        assert!(behind >= Duration::from_secs(1), "後ろの Shibubu が {behind:?} ごとに描き直す");
        assert!(pages.panes["notes"].surface.as_ref().is_ok_and(|m| m.wakes()), "着いた frame が描き直しを起こす口が無い（後ろでは映らなくなる）");
        let front = frame(&mut pages, true);
        assert!(front <= Duration::from_millis(33), "正の対照: 前面で {front:?}（33ms で見に来ない）");
    }

    /// holdout（W16M-3 の不具合 ①）: 映っている Mirror の矩形は AXGroup でない role（`FRAME_ROLE`）・identifier
    /// `surface-frame:<id>`・Focus と Click を持つ node。映っていない間（窓が消えた）は同じ identifier で Focus も Click も無い。
    #[test]
    fn w16m3_live_mirror_frame_is_an_ax_node_test_tools_list() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        use crate::surface::mirror::tests::msg;
        use egui::accesskit::{Action, Role};
        // 偽の helper: stream は 4×2 の frame を 1 枚流して stdin を待つ
        let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; cat >/dev/null");
        let hd = serde_json::to_vec(&serde_json::json!({"seq": 1, "w": 4, "h": 2, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 2.0, "h": 1.0}, "factor": 2.0})).unwrap();
        let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
        payload.extend(hd);
        payload.extend([200u8; 4 * 2 * 4]);
        std::fs::write(dir.join("frame.bin"), msg(1, payload)).unwrap();
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
            out.platform_output.accesskit_update.into_iter().flat_map(|u| u.nodes).map(|(_, n)| n).find(|n| n.author_id() == Some("surface-frame:notes"))
        };
        // frame が texture に上がる（映っている）まで回す。node は texture を描いた次の frame の形を見る（Sense は前の frame の live で決まる）
        let mut live = None;
        for _ in 0..60 {
            frame(&mut pages);
            if pages.panes["notes"].texture.is_some() {
                live = frame(&mut pages);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let live = live.expect("偽の helper の frame が 3 秒で texture に上がらない");
        assert_ne!(live.role(), Role::Group, "Mirror の矩形が AXGroup のまま（試験の道具の AX の一覧に出ない）");
        assert_eq!(live.role(), FRAME_ROLE);
        assert!(live.supports_action(Action::Focus) && live.supports_action(Action::Click), "映っている矩形が focus / press できない");
        // 映っていない（窓が消えた）: 同じ identifier で action 0
        pages.panes.get_mut("notes").unwrap().surface = Err(MirrorError::WindowGone);
        pages.panes.get_mut("notes").unwrap().tried_ms = crate::surface::now_ms() + 1e9; // 探し直さない（偽の helper に list は無い）
        frame(&mut pages);
        let gone = frame(&mut pages).expect("映っていない間に Mirror の矩形の node が消えた（状態の 1 行を読めない）");
        assert!(!gone.supports_action(Action::Click) && !gone.supports_action(Action::Focus), "映っていない矩形が押せる（押しても何も起きない）");
    }

    /// W16M-3（不具合 ②・click-diagnosis.txt）: 背景の TextEdit は caret を描かないので、映像を click しても何も変わって見えなかった。
    /// helper の click の答え（select と caret）で、映像の上のその文字の左の辺に caret を描き、次の入力（打つ）で消す。
    #[test]
    fn w16m3_click_answer_draws_the_caret_on_the_mirror() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        use crate::surface::mirror::tests::msg;
        // 偽の helper: 4×2 の frame を 1 枚流し、stdin の 1 行目（click）に select と caret で答え、残りは読み捨てる
        let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; read l; cat {d}/answer.bin; cat >/dev/null");
        let hd = serde_json::to_vec(&serde_json::json!({"seq": 1, "w": 4, "h": 2, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 2.0, "h": 1.0}, "factor": 2.0})).unwrap();
        let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
        payload.extend(hd);
        payload.extend([200u8; 4 * 2 * 4]);
        std::fs::write(dir.join("frame.bin"), msg(1, payload)).unwrap();
        let answer = serde_json::json!({"kind": "sent", "op": "click", "id": 1, "ax": "select", "loc": 5, "caret": {"x": 0.25, "y": 0.5, "w": 0.0, "h": 0.5}});
        std::fs::write(dir.join("answer.bin"), msg(2, serde_json::to_vec(&answer).unwrap())).unwrap();
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        let ring = theme::tokens(&ctx).focus_ring;
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        // 映像の矩形は上の題の帯（UI-5m・28）の下 = (20, 48)〜(420, 320)。4×2 の frame は 400×200 で縦の中央（min = (20, 84)）。
        // caret (0.25, 0.5, h 0.5) → (120, 184) から高さ 100・幅 2
        let want = Rect::from_min_size(pos2(120.0, 184.0), Vec2::new(2.0, 100.0));
        let frame = |pages: &mut MirrorPages| -> bool {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
            out.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == ring && (r.rect.min - want.min).length() < 0.5 && (r.rect.max - want.max).length() < 0.5))
        };
        let mut live = false;
        for _ in 0..60 {
            frame(&mut pages);
            if pages.panes["notes"].texture.is_some() {
                live = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(live, "偽の helper の frame が 3 秒で texture に上がらない");
        assert!(!frame(&mut pages), "click の前から caret を描いている");
        pages.relay("notes", MirrorInput::Click { x: 0.25, y: 0.5, count: 1 }).unwrap();
        let mut drawn = false;
        for _ in 0..60 {
            if frame(&mut pages) {
                drawn = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(drawn, "helper の select と caret の答えの後も、映像のその文字の左に caret を描かない（背景の TextEdit は描かない）");
        // 打つと caret の位置は変わる（helper はその後の位置を答えない）ので、古い caret は消す
        pages.relay("notes", MirrorInput::Text { text: "a".into() }).unwrap();
        assert!(!frame(&mut pages), "打った後も click の時の caret を今の物として描いている");
    }

    /// L5（DOGFOOD-X 周 2 題 6: Terminal の欄に打った残り 57 字が REAPER の Mirror から本物へ送られ、REAPER の窓が 2 つ開いた）: 面を押していないのに
    /// focus が面へ移っても（AX の Focus・打っている途中）打鍵は本物へ送らず、focus は前の欄へ戻って続きの文字は欄に入る。面を押した後は今までどおり送る。
    /// B31: AX の Focus で面へ移った frame の Space（egui の偽の click）でも押した事にしない（送らない・選ばない・触った時刻を持たない）。
    /// B32: 映っていない frame（stream が切れた・畳んだ）の後は押した事を忘れる。
    /// B16: 押した面でも、本物を前に出していない間の打鍵は送らず帯の 1 行で断る（前に出す途中 = direct が Some なら送る）。
    /// 負の対照: `focused && !pane.armed` の腕を外す → 欄の続きの文字が本物へ送られて赤・B31（未測）: 押しの判定を `resp.clicked()` に戻す → Space の行が赤・
    /// B32（未測）: `live` が None の時の `pane.armed = false` を外す → stream が切れた後の行が赤・
    /// B16（未測）: relay の loop の `!front && … PostToPid` の腕を外す → 前に出していない面の `x` を送って赤
    #[test]
    fn l5_typing_in_another_field_does_not_leak_into_a_mirror_that_took_focus_without_a_press() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        use crate::surface::mirror::tests::msg;
        let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; cat >/dev/null");
        let hd = serde_json::to_vec(&serde_json::json!({"seq": 1, "w": 4, "h": 2, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 2.0, "h": 1.0}, "factor": 2.0})).unwrap();
        let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
        payload.extend(hd);
        payload.extend([200u8; 4 * 2 * 4]);
        std::fs::write(dir.join("frame.bin"), msg(1, payload)).unwrap();
        let te = MirrorTarget { app_name: "REAPER".into(), bundle_id: "com.cockos.reaper".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.cockos.reaper".into(), file: None }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let (field, mirror) = (egui::Id::new("l5-terminal-field"), egui::Id::new(crate::ui::ax::AxScope::SurfaceFrame.id("notes")));
        let mut typed = String::new();
        let frame = |pages: &mut MirrorPages, typed: &mut String, events: Vec<Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, rect, &spec);
                    ui.put(Rect::from_min_size(pos2(20.0, 400.0), Vec2::new(300.0, 24.0)), egui::TextEdit::singleline(typed).id(field));
                });
            });
            pages.end_frame();
        };
        let mut live = false;
        for _ in 0..60 {
            frame(&mut pages, &mut typed, vec![]);
            if pages.panes["notes"].texture.is_some() {
                live = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(live, "偽の helper の frame が 3 秒で texture に上がらない");
        let texts = || std::fs::read_to_string(dir.join("relay.jsonl")).unwrap_or_default().lines().filter(|l| l.contains("\"kind\":\"text\"")).count();
        // Terminal の欄で打つ
        ctx.memory_mut(|m| m.request_focus(field));
        frame(&mut pages, &mut typed, vec![Event::Text("--allowedTools \"Rea".into())]);
        assert_eq!(typed, "--allowedTools \"Rea", "前提: 欄に入らない");
        // 打っている途中で、押していない面へ focus が移った（AX の Focus）
        ctx.memory_mut(|m| m.request_focus(mirror));
        frame(&mut pages, &mut typed, vec![Event::Text("d".into())]);
        frame(&mut pages, &mut typed, vec![Event::Text("\"".into())]);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(texts(), 0, "押していない面へ移った focus で、欄の続きの文字を本物へ送った");
        assert!(ctx.memory(|m| m.has_focus(field)), "focus を前の欄へ返さない");
        assert_eq!(typed, "--allowedTools \"Read\"", "欄の続きの文字が欄に入らない");
        // B31: AX の Focus で面へ移った frame の Space（egui は focus を持つ面の Space / Enter を click にする）でも押した事にしない
        ctx.memory_mut(|m| m.request_focus(mirror));
        let space = Event::Key { key: egui::Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        frame(&mut pages, &mut typed, vec![space]);
        std::thread::sleep(Duration::from_millis(200));
        let sent = std::fs::read_to_string(dir.join("relay.jsonl")).unwrap_or_default().lines().count();
        let p = &pages.panes["notes"];
        assert!(!p.armed && !p.selected && p.touched == f64::NEG_INFINITY && sent == 0, "focus を持つ面の Space（egui の偽の click）で押した事にした（送った記録 {sent} 行）");
        assert!(ctx.memory(|m| m.has_focus(field)), "Space の後に focus を前の欄へ返さない");
        // 面を押した後は今までどおり送る（MIRROR-FIT-1101: 1 回目の押しは枠を選ぶだけ = 2 回目の押しで focus）
        let at = pos2(220.0, 184.0);
        let button = |pressed: bool| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        for _ in 0..2 {
            frame(&mut pages, &mut typed, vec![Event::PointerMoved(at), button(true)]);
            frame(&mut pages, &mut typed, vec![button(false)]);
        }
        // B16: 押した面でも本物を前に出していない（この試験の偽の窓には Dock が付かない = 前に出せない）間の打鍵は送らない（後ろの app に溜まる）・帯の 1 行で断る
        frame(&mut pages, &mut typed, vec![Event::Text("x".into())]);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(texts(), 0, "本物を前に出していない面の打鍵を後ろの本物へ送った（前に来た時にまとめて入る）");
        assert!(matches!(pages.panes["notes"].line, Some((MirrorError::NeedsFront { kind: crate::surface::mirror::InputKind::Text, .. }, _))), "打鍵を捨てた事を帯の 1 行で言わない: {:?}", pages.panes["notes"].line);
        // 本物を前に出す途中（`go_direct` の後 = direct が Some）は今までどおり送る
        pages.panes.get_mut("notes").unwrap().direct = Some(false);
        frame(&mut pages, &mut typed, vec![Event::Text("y".into())]);
        let t = std::time::Instant::now();
        while texts() == 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(texts(), 1, "面を押して本物を前に出す途中に打っても本物へ送らない");
        assert_eq!(typed, "--allowedTools \"Read\"", "面を押した後の文字が欄にも入った");
        // B32: 映っていない frame（stream が切れた）の後は押した事を忘れる（映り直した後に AX の Focus で面へ来ても打鍵を送らない）
        assert!(pages.panes["notes"].armed, "前提: 押して打った面が armed でない");
        pages.panes.get_mut("notes").unwrap().surface = Err(MirrorError::WindowGone);
        frame(&mut pages, &mut typed, vec![]);
        assert!(!pages.panes["notes"].armed, "stream が切れた（映っていない）面が押した事を覚えている");
    }

    /// B33（bfae19b2 の独立 review 黄・返す先が別の面）: 面 A を押した後で、押していない面 B へ focus が移った（AX の Focus）→ B は focus を返す先に面 A を選ばない
    /// （選ぶと A の守りが B を返す先に覚え、B は自分へ focus を返し続けて B に残る = 欄にも A にも戻らず、毎 frame 断りの行を書く）。
    /// 負の対照（未測）: `last_focus` の `!frames.contains(f)` を外す → B に focus が残って赤
    #[test]
    fn b33_a_mirror_that_took_focus_without_a_press_does_not_hand_it_to_another_mirror() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        use crate::surface::mirror::tests::msg;
        let open = |bundle: &str, pid: i64| {
            let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; cat >/dev/null");
            let hd = serde_json::to_vec(&serde_json::json!({"seq": 1, "w": 4, "h": 2, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 2.0, "h": 1.0}, "factor": 2.0})).unwrap();
            let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
            payload.extend(hd);
            payload.extend([200u8; 4 * 2 * 4]);
            std::fs::write(dir.join("frame.bin"), msg(1, payload)).unwrap();
            let te = MirrorTarget { app_name: bundle.into(), bundle_id: bundle.into(), pid, window_id: 7 };
            let (g, _) = ConsentGate::default().grant(&te, 1.0);
            let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
            (sh, m)
        };
        let (sh, a) = open("com.cockos.reaper", 42);
        let (_, b) = open("com.apple.TextEdit", 43);
        let spec = |id: &str, bundle: &str| SurfaceSpec { id: id.into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: bundle.into(), file: None }, keep_alive: true, frame: None, browser: false };
        let (sa, sb) = (spec("a", "com.cockos.reaper"), spec("b", "com.apple.TextEdit"));
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("a".into(), Pane::new(Ok(a)));
        pages.panes.insert("b".into(), Pane::new(Ok(b)));
        let ctx = egui::Context::default();
        let (ra, rb) = (Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0)), Rect::from_min_size(pos2(440.0, 20.0), Vec2::new(340.0, 300.0)));
        let fb = egui::Id::new(crate::ui::ax::AxScope::SurfaceFrame.id("b"));
        let frame = |pages: &mut MirrorPages, events: Vec<Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, ra, &sa);
                    pages.show(ui, rb, &sb);
                });
            });
            pages.end_frame();
        };
        let mut live = false;
        for _ in 0..60 {
            frame(&mut pages, vec![]);
            if pages.panes["a"].texture.is_some() && pages.panes["b"].texture.is_some() {
                live = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(live, "偽の helper の frame が 3 秒で texture に上がらない");
        // 面 A を押す
        let at = pos2(220.0, 184.0);
        let button = |pressed: bool| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(&mut pages, vec![Event::PointerMoved(at), button(true)]);
        frame(&mut pages, vec![button(false)]);
        assert!(pages.panes["a"].armed, "前提: 面 A を押した事にならない");
        // 押していない面 B へ focus が移った（AX の Focus）
        ctx.memory_mut(|m| m.request_focus(fb));
        for _ in 0..3 {
            frame(&mut pages, vec![]);
        }
        assert!(!ctx.memory(|m| m.has_focus(fb)), "押していない面 B に focus が残った（別の面 A を返す先に覚えて返し合った）");
        assert!(!pages.panes["b"].armed && !pages.panes["a"].armed, "focus を持たない面が押した事を覚えている");
    }

    /// W16M-3（不具合 ② の反証）: click の直後に打つと、click の答え（select と caret）は打った後に着く。その caret は打つ前の位置なので持たない。
    #[test]
    fn w16m3_click_answer_after_typing_is_not_kept() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        use crate::surface::mirror::tests::msg;
        // 偽の helper: 1 行目（click）と 2 行目（文字）を読んでから 1 行目の答えを出す・3 行目（次の click）に答える
        let (sh, dir) = fake_stream_helper("read a; read b; cat {d}/stale.bin; read c; cat {d}/answer.bin; cat >/dev/null");
        let ans = |id: u64| msg(2, serde_json::to_vec(&serde_json::json!({"kind": "sent", "op": "click", "id": id, "ax": "select", "loc": 5, "caret": {"x": 0.25, "y": 0.5, "w": 0.0, "h": 0.5}})).unwrap());
        std::fs::write(dir.join("stale.bin"), ans(1)).unwrap();
        std::fs::write(dir.join("answer.bin"), ans(3)).unwrap();
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
        };
        pages.relay("notes", MirrorInput::Click { x: 0.25, y: 0.5, count: 1 }).unwrap();
        pages.relay("notes", MirrorInput::Text { text: "a".into() }).unwrap();
        for _ in 0..20 {
            frame(&mut pages);
            assert!(pages.panes["notes"].caret.is_none(), "打った後に着いた前の click の caret（打つ前の位置）を持った");
            std::thread::sleep(Duration::from_millis(50));
        }
        // 今の click の答えは持つ（正の対照: 上の 20 frame の間に古い答えは読み終わっている）
        pages.relay("notes", MirrorInput::Click { x: 0.25, y: 0.5, count: 1 }).unwrap();
        let mut kept = false;
        for _ in 0..60 {
            frame(&mut pages);
            if pages.panes["notes"].caret.is_some() {
                kept = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(kept, "今の click の答えの caret を持たない");
    }

    /// W16M-3（反証: 上の試験は打った入力で click_seq が None になるので、`id` の照合を外しても緑だった）: click を 2 回続けると、1 回目の答え
    /// （1 回目の点の caret）は 2 回目を送った後に着く。2 回目の答えが着くまで caret は無い（1 回目の caret を今の物として描かない）。
    #[test]
    fn w16m3_answer_to_the_previous_click_is_not_kept_after_the_next_click() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        use crate::surface::mirror::tests::msg;
        // 偽の helper: 2 行（click 2 つ）を読んでから 1 回目の答えを出し、試験が go を置くまで 2 回目の答えを出さない
        let (sh, dir) = fake_stream_helper("read a; read b; cat {d}/stale.bin; while [ ! -e {d}/go ]; do sleep 0.05; done; cat {d}/answer.bin; cat >/dev/null");
        let ans = |id: u64| msg(2, serde_json::to_vec(&serde_json::json!({"kind": "sent", "op": "click", "id": id, "ax": "select", "loc": 5, "caret": {"x": 0.25, "y": 0.5, "w": 0.0, "h": 0.5}})).unwrap());
        std::fs::write(dir.join("stale.bin"), ans(1)).unwrap();
        std::fs::write(dir.join("answer.bin"), ans(2)).unwrap();
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
        };
        let click = || MirrorInput::Click { x: 0.25, y: 0.5, count: 1 };
        assert_eq!((pages.relay("notes", click()).unwrap().seq, pages.relay("notes", click()).unwrap().seq), (1, 2));
        for _ in 0..20 {
            frame(&mut pages);
            assert!(pages.panes["notes"].caret.is_none(), "2 回目の click の後に着いた 1 回目の答えの caret（前の点）を描いた");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::write(dir.join("go"), "").unwrap();
        let mut kept = false;
        for _ in 0..60 {
            frame(&mut pages);
            if pages.panes["notes"].caret.is_some() {
                kept = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(kept, "2 回目（今）の click の答えの caret を持たない");
    }

    /// W16M-3（反証）: 画面収録の許可が無い面は「許可が要ります」（PermissionRequired）のまま。探し直しの間に「探しています」（Attaching）へ戻らない。
    #[test]
    fn w16m3_denied_face_stays_while_searching_again() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        let (sh, dir) = fake_stream_helper("cat >/dev/null");
        // 許可が無い helper の list（KangaeMirror.swift と同じ `[]` と exit 3）。探し直しが 1 frame で終わらないよう 0.5 秒眠る
        std::fs::write(&sh, format!("#!/bin/sh\ncase \"$1\" in\n status) echo '{{\"screen_capture\":false}}';;\n list) echo 1 >> {}/list.log; sleep 0.5; echo '[]'; exit 3;;\n stream) cat >/dev/null;;\nesac\n", dir.display())).unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages| -> PortalState {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let mut st = PortalState::IntegrationFailed;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    st = pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
            st
        };
        assert_eq!(frame(&mut pages), PortalState::Attaching, "最初の窓探しの間");
        let mut denied = false;
        for _ in 0..100 {
            if frame(&mut pages) == PortalState::PermissionRequired {
                denied = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(denied, "許可が無い list の答えの後も PermissionRequired にならない");
        // 2 秒を待たずに探し直させる
        pages.panes.get_mut("notes").unwrap().tried_ms = f64::NEG_INFINITY;
        assert_eq!(frame(&mut pages), PortalState::PermissionRequired, "探し直しの間に「探しています」へ戻った（許可の 1 行が消える）");
        assert!(pages.panes["notes"].pending.is_some(), "探し直しが起きていない（試験が測っていない）");
        for _ in 0..100 {
            if pages.panes["notes"].pending.is_none() {
                break;
            }
            assert_eq!(frame(&mut pages), PortalState::PermissionRequired, "探し直しの間に「探しています」へ戻った");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(pages.panes["notes"].pending.is_none(), "探し直しの答えが 5 秒で着かない");
        assert_eq!(std::fs::read_to_string(dir.join("list.log")).unwrap().lines().count(), 2);
    }

    /// W16M-3（反証）: A の面が窓を探している間に B の Mirror の面を開いても、B は `list` を並べて起こさない（`list --all` は動いている stream を
    /// -3805 で止める・A は着けば stream を開く）。B は待つ間も「探しています」（「窓が閉じました」と嘘を言わない）。
    #[test]
    fn w16m3_second_mirror_never_runs_a_window_lookup_while_another_is_searching() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        let (sh, dir) = fake_stream_helper("cat >/dev/null");
        std::fs::write(&sh, format!("#!/bin/sh\ncase \"$1\" in\n status) echo '{{\"screen_capture\":true}}';;\n list) echo 1 >> {}/list.log; sleep 1; echo '[{{\"window_id\":7,\"pid\":42,\"bundle_id\":\"com.apple.TextEdit\",\"app\":\"TextEdit\",\"title\":\"notes.md\"}}]';;\n stream) cat >/dev/null;;\nesac\n", dir.display())).unwrap();
        let spec = |id: &str| SurfaceSpec { id: id.into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let mut pages = MirrorPages::new(sh, [], None);
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages, spec: &SurfaceSpec| -> PortalState {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let mut st = PortalState::IntegrationFailed;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    st = pages.show(ui, rect, spec);
                });
            });
            pages.end_frame();
            st
        };
        // A の Page → すぐ B の Page（A は keep_alive で探し続ける）
        assert_eq!(frame(&mut pages, &spec("a")), PortalState::Attaching);
        assert!(pages.panes["a"].pending.is_some());
        let b = frame(&mut pages, &spec("b"));
        // A の list の 1 行が着くのを待ち（混んだ機械では sh の起動が遅い）、B が起こしていれば着くだけの間を足して数える
        let lines = || std::fs::read_to_string(dir.join("list.log")).map(|s| s.lines().count()).unwrap_or(0);
        for _ in 0..100 {
            if lines() >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(500));
        let lists = lines();
        assert_eq!(lists, 1, "A が探している間に B も list を起こした（2 本の list --all が並ぶ）");
        assert_eq!(b, PortalState::Attaching, "待っている B の面の状態（窓が閉じた とは言わない）");
    }

    /// G82 の 2 つの試験の偽の helper（list は `list_sh`・元の TextEdit は終了した = capture できる窓 0）と、Page の 1 frame（`spec` の面だけを描く）。
    fn g82_pages(list_sh: &str) -> (MirrorPages, std::path::PathBuf, impl Fn(&mut MirrorPages, &SurfaceSpec) -> PortalState) {
        let (sh, dir) = fake_stream_helper("cat >/dev/null");
        std::fs::write(&sh, format!("#!/bin/sh\ncase \"$1\" in\n status) echo '{{\"screen_capture\":true}}';;\n list) {list_sh};;\n stream) cat >/dev/null;;\nesac\n")).unwrap();
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = move |pages: &mut MirrorPages, spec: &SurfaceSpec| -> PortalState {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let mut st = PortalState::IntegrationFailed;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    st = pages.show(ui, rect, spec);
                });
            });
            pages.end_frame();
            st
        };
        (MirrorPages::new(sh, [], None), dir, frame)
    }

    /// 27 手の README（Docs の Page）と notes（notes の Page）の Mirror の面（どちらも keep_alive）。
    fn g82_spec(id: &str, file: &str) -> SurfaceSpec {
        use crate::domain::project::{SurfaceBackendKind, SurfaceTarget};
        SurfaceSpec { id: id.into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some(file.into()) }, keep_alive: true, frame: None, browser: false }
    }

    /// G82（r25 / r26 の #26: 元の TextEdit を外から終了した後、notes.md の Mirror の面が ≈ 58 秒「Looking for the original window」のまま・app.log に
    /// `Mirror notes:` が 1 行も無い）: README の面（keep_alive）が 探し直しの途中で notes の Page へ移ると、README の答えは描かない面なので
    /// 誰も受け取らず pending が残り、notes は「他の面が探している」を永久に待っていた（TextEdit が居ないので G78 の書類の問いは走らない）。
    /// 描かなかった面の答えも frame の終わりで受け取る → notes も上限（`WAIT_OTHER_MS` より短い 9 秒）以内に自分で探して Not available・README も NoWindow・list は 1 本ずつ。
    /// 負の対照: G82_MUT=drain（描かなかった面の答えを受け取らない）→ notes が「探しています」のまま
    #[test]
    fn g82_a_mirror_stops_looking_when_the_hidden_face_finishes_its_search() {
        let (mut pages, dir, frame) = g82_pages("echo 1 >> \"$(dirname \"$0\")/list.log\"; sleep 0.3; echo '[]'");
        let (readme, notes) = (g82_spec("readme", "README.md"), g82_spec("notes", "docs/notes.md"));
        // README の Page で探し始めた直後に notes の Page へ（README の面は keep_alive で残る・もう描かない）
        assert_eq!(frame(&mut pages, &readme), PortalState::Attaching);
        assert!(pages.panes["readme"].pending.is_some(), "README の探しが起きていない（試験が測っていない）");
        let t = std::time::Instant::now();
        let mut st = frame(&mut pages, &notes);
        // 混んだ機械（swap が詰まると sh の起動に秒かかる）でも、上限で進む（変異 drain）より前に着く
        while st != PortalState::SourceUnavailable && t.elapsed() < Duration::from_secs(9) {
            std::thread::sleep(Duration::from_millis(50));
            st = frame(&mut pages, &notes);
        }
        assert_eq!(st, PortalState::SourceUnavailable, "notes の面が {:?} の間「探しています」のまま（描かなかった README の面の答えを誰も受け取らない）", t.elapsed());
        assert!(matches!(pages.panes["notes"].surface, Err(MirrorError::NoWindow { .. })), "{:?}", pages.panes["notes"].surface.as_ref().err());
        assert!(matches!(pages.panes["readme"].surface, Err(MirrorError::NoWindow { .. })), "README の面も Not available: {:?}", pages.panes["readme"].surface.as_ref().err());
        let lists = std::fs::read_to_string(dir.join("list.log")).map(|s| s.lines().count()).unwrap_or(0);
        assert_eq!(lists, 2, "list は README・notes の 1 本ずつ");
    }

    /// G82: 他の面の探しが答えない（helper の list が混んで返らない）間も、待つのは `WAIT_OTHER_MS` まで → 超えた後の frame で notes は自分で探して Not available
    /// （README の探しはまだ答えていない）。負の対照: G82_MUT=cap（上限なしで待つ）→ notes が「探しています」のまま
    #[test]
    fn g82_a_mirror_stops_waiting_for_a_search_that_never_answers() {
        // 1 本目の list（README）だけ 20 秒答えない（下の待ちの 8 秒より長い = notes は README の答えでは進めない）
        let (mut pages, dir, frame) = g82_pages("d=$(dirname \"$0\"); first=1; [ -s \"$d/list.log\" ] && first=0; echo 1 >> \"$d/list.log\"; [ $first = 1 ] && sleep 20; echo '[]'");
        let (readme, notes) = (g82_spec("readme", "README.md"), g82_spec("notes", "docs/notes.md"));
        assert_eq!(frame(&mut pages, &readme), PortalState::Attaching);
        // README の list が 1 本目として起きるのを待つ（混んだ機械では sh の起動が遅い）
        let lines = || std::fs::read_to_string(dir.join("list.log")).map(|s| s.lines().count()).unwrap_or(0);
        for _ in 0..100 {
            if lines() >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(lines(), 1, "README の list が起きていない（試験が測っていない）");
        // README が探し始めてから上限を過ぎた事にする（10 秒を待たない）
        pages.panes.get_mut("readme").unwrap().tried_ms -= WAIT_OTHER_MS + 1.0;
        let t = std::time::Instant::now();
        let mut st = frame(&mut pages, &notes);
        while st != PortalState::SourceUnavailable && t.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(50));
            st = frame(&mut pages, &notes);
        }
        assert_eq!(st, PortalState::SourceUnavailable, "答えない README の探しを上限の後も待った（{:?}）", t.elapsed());
        assert!(pages.panes["readme"].pending.is_some(), "README の探しはまだ答えていない（notes は README の答えで進んだのではない）");
        assert_eq!(lines(), 2);
    }

    /// G69（sendback 69・27 手の r5 の #22 で実測: Docs の README の Mirror が映っている間、notes.md の Mirror の面が 60 秒以上「Looking for the original
    /// window」のまま）: A の面が映っている間に B の面を開くと、B は A の helper の process の stdin で窓を探し（`{"cmd":"list"}`）、同じ process に
    /// stream を足す（`{"cmd":"open"}`）。`list` と `stream` の process は 1 本ずつのまま・A は映ったまま・B も映る（G32 の後は別の process の
    /// `list --all` が動いている stream を -3805 で落とす道が無い）。負の対照: 他の面が映っている（surface Ok）間も探さない形に戻すと B は Attaching のまま。
    #[test]
    fn g69_second_mirror_looks_inside_the_live_helper_while_the_first_is_live() {
        use crate::surface::mirror::tests::msg;
        let (sh, dir) = fake_stream_helper("");
        let d = dir.display();
        let list = r#"[{"window_id":7,"pid":42,"bundle_id":"com.apple.TextEdit","app":"TextEdit","title":"README.md"},{"window_id":8,"pid":42,"bundle_id":"com.apple.TextEdit","app":"TextEdit","title":"notes.md"}]"#;
        std::fs::write(
            &sh,
            format!("#!/bin/sh\necho \"$1\" >> {d}/calls\ncase \"$1\" in\n status) echo '{{\"screen_capture\":true}}';;\n list) echo '{list}';;\n stream) echo $$ >> {d}/pid; cat {d}/f1.bin; while read -r l; do echo \"$l\" >> {d}/stdin; case \"$l\" in *'\"cmd\":\"list\"'*) cat {d}/win.bin;; *'\"cmd\":\"open\"'*) cat {d}/f3.bin;; esac; done;;\nesac\n"),
        )
        .unwrap();
        // stream 1 = 起こした時の窓（A）の frame・id 2 = B の list の答え・id 3 = B の stream（HelperProc の番号は 2 から: list → open の順）
        let frame = |stream: Option<u64>| {
            let mut h = serde_json::json!({"seq": 1, "w": 2, "h": 3, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 1.0, "h": 1.5}, "factor": 2.0});
            if let Some(s) = stream {
                h["stream"] = serde_json::json!(s);
            }
            let hd = serde_json::to_vec(&h).unwrap();
            let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
            payload.extend(hd);
            payload.extend([200u8; 2 * 3 * 4]);
            msg(1, payload)
        };
        std::fs::write(dir.join("f1.bin"), frame(None)).unwrap();
        let win = serde_json::json!({"kind": "windows", "id": 2, "windows": serde_json::from_str::<serde_json::Value>(list).unwrap()});
        std::fs::write(dir.join("win.bin"), msg(2, serde_json::to_vec(&win).unwrap())).unwrap();
        let mut f3 = msg(2, serde_json::to_vec(&serde_json::json!({"kind": "started", "stream": 3})).unwrap());
        f3.extend(frame(Some(3)));
        std::fs::write(dir.join("f3.bin"), f3).unwrap();
        let spec = |id: &str, file: &str| SurfaceSpec { id: id.into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some(file.into()) }, keep_alive: true, frame: None, browser: false };
        let (docs, notes) = (spec("docs", "README.md"), spec("notes", "docs/notes.md"));
        let mut pages = MirrorPages::new(sh, [ui5m_consent()], None);
        pages.relay_log = dir.join("relay.jsonl");
        let ctx = egui::Context::default();
        let mut time = 0.0;
        assert!(ui5m_live(&ctx, &mut pages, &docs, &mut time), "前提: A（README）の面が 5 秒で映らない");
        // #22: notes の tab に移る（A は keep_alive で映ったまま・この Page では描かない）
        let mut b = None;
        for _ in 0..100 {
            time += 0.05;
            ui5m_frame(&ctx, &mut pages, &notes, time, true, vec![]);
            b = pages.reach("notes");
            if b == Some(Ok(())) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap_or_default();
        let stdin = std::fs::read_to_string(dir.join("stdin")).unwrap_or_default();
        assert_eq!(b, Some(Ok(())), "A が映っている間に B（notes）が映らない（calls {calls:?}・stdin {stdin:?}）");
        assert_eq!(calls.lines().filter(|l| *l == "list").count(), 1, "A が映っている間に list の process を起こした（A の stream を -3805 で落とす道）: {calls:?}");
        assert_eq!(calls.lines().filter(|l| *l == "stream").count(), 1, "B のために helper の process をもう 1 本起こした: {calls:?}");
        // MIRROR-SPEED-1101: 映った A に描く大きさ（`view`）を送るのは窓探しの外（比べから外す）
        let sent: Vec<serde_json::Value> = stdin.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).filter(|v| v["cmd"] != "view").collect();
        assert_eq!(sent.iter().map(|v| (v["cmd"].as_str(), v["window"].as_u64())).collect::<Vec<_>>(), vec![(Some("list"), None), (Some("open"), Some(8))], "B は A の process の中で探して notes.md の窓（8）を足す");
        assert!(pages.panes["docs"].surface.is_ok(), "B を開いたら A の stream が落ちた: {:?}", pages.panes["docs"].surface.as_ref().err());
    }

    /// W16M-3（製品の不具合・BG で実測: helper の `list --all` が 70 分返らず、それを UI の thread で待った Shibubu が止まった・AX も答えない）:
    /// 窓を探す間も frame を止めない（面は Attaching と「探しています」の 1 行）・答えが着いたら次の frame から使う。
    #[test]
    fn w16m3_slow_window_lookup_never_blocks_the_frame() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        let (sh, _dir) = fake_stream_helper("cat >/dev/null");
        // list が 2 秒眠ってから TextEdit の窓を 1 つ返す helper（ScreenCaptureKit が混んで答えない時の形）
        std::fs::write(&sh, "#!/bin/sh\ncase \"$1\" in\n status) echo '{\"screen_capture\":true}';;\n list) sleep 2; echo '[{\"window_id\":7,\"pid\":42,\"bundle_id\":\"com.apple.TextEdit\",\"app\":\"TextEdit\",\"title\":\"notes.md\"}]';;\n stream) cat >/dev/null;;\nesac\n").unwrap();
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        // 同意が無いので、見つかると確認待ち（PermissionRequired）
        let mut pages = MirrorPages::new(sh, [], None);
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages| -> (PortalState, f64) {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
            let t0 = crate::surface::now_ms();
            let mut st = PortalState::IntegrationFailed;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    st = pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
            (st, crate::surface::now_ms() - t0)
        };
        let (st, ms) = frame(&mut pages);
        assert!(ms < 1000.0, "窓を探す helper を UI の thread で待った（{ms:.0} ms・その間 Shibubu 全体が止まる）");
        assert_eq!(st, PortalState::Attaching, "探している間の面の状態");
        let mut found = false;
        for _ in 0..100 {
            let (st, ms) = frame(&mut pages);
            assert!(ms < 1000.0, "窓を探す helper を UI の thread で待った（{ms:.0} ms）");
            if st == PortalState::PermissionRequired {
                found = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(found, "list の答えが着いた後も面が決まらない");
        assert_eq!(pages.panes["notes"].target.as_ref().map(|t| t.window_id), Some(7));
    }

    /// MIRROR-SPEED-1101: helper に送る描く大きさは本物の窓の分だけ（stream の余白を入れると窓の pixel より大きく撮り直す）・中身が無い frame は描いた全体
    #[test]
    fn mirror_speed_view_is_the_window_drawn_without_the_stream_margin() {
        // 960×600 の frame（中身 = 窓は pixel 20..940 × 0..600）を 460×300 の映像に描く = frame は 480×300・窓は 460×300
        let meta = MirrorFrameMeta { seq: 1, width: 960, height: 600, captured_ms: 0.0, content: [10.0, 0.0, 460.0, 300.0], factor: 2.0 };
        let video = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(460.0, 300.0));
        assert_eq!(shown_rect(video, &meta).size(), Vec2::new(480.0, 300.0));
        assert_eq!(window_drawn(video, &meta), Vec2::new(460.0, 300.0));
        let whole = MirrorFrameMeta { content: [0.0; 4], ..meta };
        assert_eq!(window_drawn(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(480.0, 300.0)), &whole), Vec2::new(480.0, 300.0));
    }

    /// holdout（W16M-2）: Mirror の矩形の上の click・文字・⌘S が MirrorInput に（余白の click・focus の無い key は 0 件・文字の key を二重に送らない）
    #[test]
    fn w16m2_events_on_the_mirror_become_inputs() {
        use crate::surface::mirror::KeyMod;
        use egui::{Event, Key, Modifiers, PointerButton, Pos2};
        // 960×600 の frame（中身は pixel 20..940 × 0..600）を 480×300 の矩形に描いた
        let meta = MirrorFrameMeta { seq: 1, width: 960, height: 600, captured_ms: 0.0, content: [10.0, 0.0, 460.0, 300.0], factor: 2.0 };
        let shown = Rect::from_min_size(pos2(100.0, 50.0), Vec2::new(480.0, 300.0));
        let press = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let mid = pos2(340.0, 200.0); // 矩形の中心 = 窓の (0.5, 0.5)
        assert_eq!(mirror_inputs(&[press(mid, true), press(mid, false)], shown, &meta, false), vec![MirrorInput::Click { x: 0.5, y: 0.5, count: 1 }]);
        // 余白（frame の左の 20 pixel = 矩形の左 10 point）の click は捨てる
        let pad = pos2(104.0, 200.0);
        assert_eq!(mirror_inputs(&[press(pad, true), press(pad, false)], shown, &meta, false), vec![]);
        let key = |k: Key, m: Modifiers| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
        let typed = [key(Key::A, Modifiers::NONE), Event::Text("a".into()), key(Key::S, Modifiers::MAC_CMD)];
        assert_eq!(mirror_inputs(&typed, shown, &meta, false), vec![], "focus の無い key を送った");
        assert_eq!(
            mirror_inputs(&typed, shown, &meta, true),
            vec![MirrorInput::Text { text: "a".into() }, MirrorInput::Key { key: "s".into(), mods: vec![KeyMod::Cmd] }]
        );
    }

    /// W16M-2: drag は押下と離しの組で 1 つ（4 point 以上）・wheel は 1 frame で 1 つにまとめる・⌘C は Key に戻す・⌘ / ⌃ の移動の key は Key・
    /// helper の知らない key と離しだけの組は送らない
    #[test]
    fn w16m2_pointer_wheel_and_chords_on_the_mirror() {
        use crate::surface::mirror::KeyMod;
        use egui::{Key, Modifiers, MouseWheelUnit};
        let meta = MirrorFrameMeta { seq: 1, width: 960, height: 600, captured_ms: 0.0, content: [10.0, 0.0, 460.0, 300.0], factor: 2.0 };
        let shown = Rect::from_min_size(pos2(100.0, 50.0), Vec2::new(480.0, 300.0));
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let (a, b) = (pos2(340.0, 200.0), pos2(455.0, 200.0));
        assert_eq!(mirror_inputs(&[btn(a, true), btn(b, false)], shown, &meta, false), vec![MirrorInput::Drag { x: 0.5, y: 0.5, x2: 0.75, y2: 0.5, steps: None }]);
        assert_eq!(mirror_inputs(&[btn(a, false)], shown, &meta, true), vec![], "押下の無い離しを click にした");
        let wheel = |dy: f32| Event::MouseWheel { unit: MouseWheelUnit::Point, delta: Vec2::new(0.0, dy), modifiers: Modifiers::NONE };
        assert_eq!(
            mirror_inputs(&[Event::PointerMoved(a), wheel(-30.0), wheel(-12.0)], shown, &meta, false),
            vec![MirrorInput::Scroll { x: 0.5, y: 0.5, dx: 0, dy: -42 }]
        );
        assert_eq!(mirror_inputs(&[wheel(-30.0)], shown, &meta, false), vec![], "点の無い wheel を送った");
        let key = |k: Key, m: Modifiers| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
        let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
        let got = mirror_inputs(&[Event::Copy, key(Key::ArrowLeft, Modifiers::MAC_CMD), key(Key::E, ctrl), key(Key::F1, Modifiers::MAC_CMD), key(Key::Enter, Modifiers::NONE), Event::Text(String::new())], shown, &meta, true);
        let k = |n: &str, m: &[KeyMod]| MirrorInput::Key { key: n.into(), mods: m.to_vec() };
        assert_eq!(got, vec![k("c", &[KeyMod::Cmd]), k("left", &[KeyMod::Cmd]), k("e", &[KeyMod::Control]), k("return", &[])]);
    }

    /// W16M-2: relay は開いていない面に送らず記録も書かない・拒否（NeedsFront）は面の Err にせず 1 行に置き、次に送れた入力で消す
    #[test]
    fn w16m2_relay_keeps_refusals_on_the_pane_and_never_sends_to_a_closed_pane() {
        let (sh, dir) = fake_stream_helper("cat >> {d}/stdin");
        let log = dir.join("relay.jsonl");
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&log).unwrap()).unwrap();
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("open".into(), Pane::new(Ok(m)));
        pages.panes.insert("gone".into(), Pane::new(Err(MirrorError::WindowGone)));
        let text = || MirrorInput::Text { text: "w16m2".into() };
        assert_eq!(pages.relay("gone", text()).unwrap_err(), MirrorError::WindowGone);
        assert!(pages.relay("never-shown", text()).is_err());
        assert!(pages.panes["gone"].line.is_none(), "閉じた面の Err を拒否の 1 行にした");
        let e = pages.relay("open", MirrorInput::Scroll { x: 0.5, y: 0.5, dx: 0, dy: -40 }).unwrap_err();
        assert!(matches!(e, MirrorError::NeedsFront { .. }));
        assert!(pages.panes["open"].surface.is_ok(), "入力 1 つの拒否で面を閉じた");
        assert_eq!(pages.panes["open"].line.as_ref().map(|(e, _)| e.clone()), Some(e));
        assert_eq!(RelayLog::read(&log).unwrap().len(), 0, "送らない入力・閉じた面の入力を記録した");
        let r = pages.relay("open", text()).unwrap();
        assert_eq!((r.seq, RelayLog::read(&log).unwrap().len()), (1, 1));
        assert!(pages.panes["open"].line.is_none(), "送れた後も前の拒否の 1 行を出し続けた");
    }

    // ---- UI-5m: Mirror の窓の赤黄緑（YC_WAVES の「UI-5m」の Done 1〜6）----

    /// 面の左上 (20, 20)・幅 400。題の帯は y 20..48・赤 黄 緑の中心は x 36 / 56 / 76（左から 10 + 半径 6・間 8）
    const UI5M_RED: Pos2 = pos2(36.0, 34.0);
    const UI5M_YELLOW: Pos2 = pos2(56.0, 34.0);
    const UI5M_GREEN: Pos2 = pos2(76.0, 34.0);
    /// 帯の題の所（button の外）
    const UI5M_TITLE: Pos2 = pos2(220.0, 34.0);

    fn ui5m_spec(keep_alive: bool) -> SurfaceSpec {
        SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive, frame: None, browser: false }
    }

    /// 縦長の 2×3 の frame（中身は frame の全部）1 枚の tag 1 の message。映像の矩形（400×272）では高さいっぱい・面の全部（400×300）なら帯の高さまで届く形
    fn ui5m_frame_bin() -> Vec<u8> {
        use crate::surface::mirror::tests::msg;
        let hd = serde_json::to_vec(&serde_json::json!({"seq": 1, "w": 2, "h": 3, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 1.0, "h": 1.5}, "factor": 2.0})).unwrap();
        let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
        payload.extend(hd);
        payload.extend([200u8; 2 * 3 * 4]);
        msg(1, payload)
    }

    /// status・list（`list.log` に 1 行・TextEdit の窓 1 つ）・stream（`pid` に自分の pid・frame を 1 枚流して stdin を `stdin` に）を持つ偽の helper と dir
    fn ui5m_helper() -> (PathBuf, PathBuf) {
        let (sh, dir) = fake_stream_helper("cat >/dev/null");
        std::fs::write(dir.join("frame.bin"), ui5m_frame_bin()).unwrap();
        let d = dir.display();
        std::fs::write(&sh, format!("#!/bin/sh\ncase \"$1\" in\n status) echo '{{\"screen_capture\":true}}';;\n list) echo 1 >> {d}/list.log; echo '[{{\"window_id\":7,\"pid\":42,\"bundle_id\":\"com.apple.TextEdit\",\"app\":\"TextEdit\",\"title\":\"notes.md\"}}]';;\n stream) echo $$ >> {d}/pid; cat {d}/frame.bin; cat >> {d}/stdin;;\nesac\n")).unwrap();
        (sh, dir)
    }

    fn ui5m_lines(p: &Path) -> usize {
        std::fs::read_to_string(p).map(|s| s.lines().count()).unwrap_or(0)
    }

    fn ui5m_consent() -> ConsentRecord {
        ConsentRecord { v: 1, bundle_id: "com.apple.TextEdit".into(), granted_at_ms: 1.0 }
    }

    /// 呼ぶ側（`source_surface`）と同じ形の 1 frame: `height` の高さ（開いた 300・拡大 500）で面を取って `show`・殻の `end_frame`。
    /// time = ctx の time（秒）・focused = Shibubu の窓が key か。返すのは面の状態・使った矩形・egui の出力
    fn ui5m_frame(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, time: f64, focused: bool, events: Vec<Event>) -> (PortalState, Rect, egui::FullOutput) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(time), focused, events, ..Default::default() };
        let (mut st, mut r) = (PortalState::IntegrationFailed, Rect::NOTHING);
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let h = pages.height(ctx, &spec.id, 300.0, 500.0);
                r = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, h));
                st = pages.show(ui, r, spec);
            });
        });
        pages.end_frame();
        (st, r, out)
    }

    /// 点 p の primary の click（押す frame と離す frame）。返すのは離した frame
    fn ui5m_click(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, time: &mut f64, p: Pos2) -> (PortalState, Rect, egui::FullOutput) {
        let press = |pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        *time += 0.016;
        ui5m_frame(ctx, pages, spec, *time, true, vec![Event::PointerMoved(p), press(true)]);
        *time += 0.016;
        ui5m_frame(ctx, pages, spec, *time, true, vec![press(false)])
    }

    /// 映像が texture に上がる（`reach` が Ok）まで frame を回す（animation も終わる・偽の helper は sh なので混んだ機械では遅い）
    fn ui5m_live(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, time: &mut f64) -> bool {
        for _ in 0..100 {
            *time += 0.05;
            ui5m_frame(ctx, pages, spec, *time, true, vec![]);
            if pages.reach(&spec.id) == Some(Ok(())) {
                *time += 1.0;
                ui5m_frame(ctx, pages, spec, *time, true, vec![]);
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn ui5m_alive(pid: &str) -> bool {
        std::process::Command::new("kill").args(["-0", pid.trim()]).status().is_ok_and(|s| s.success())
    }

    /// Done 1: 赤で helper の stream が止まり（process が居ない・`end_frame` の後 `ActivityHold` が None・窓を探し直さない）、
    /// 帯を押すと同意を尋ねずに窓を探し直して映像が戻る。負の対照: `set_window` の Closed で `MirrorSurface` を落とす行を消すと赤。
    #[test]
    fn ui5m_red_stops_the_stream_and_the_band_reattaches_without_asking() {
        let (sh, dir) = ui5m_helper();
        let mut pages = MirrorPages::new(sh, [ui5m_consent()], None);
        pages.relay_log = dir.join("relay.jsonl");
        let spec = ui5m_spec(true);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        assert!(ui5m_live(&ctx, &mut pages, &spec, &mut time), "偽の helper の frame が 5 秒で映らない");
        assert_eq!(pages.awake.is_some(), cfg!(target_os = "macos"), "映している間に App Nap を止めていない（前提）");
        assert!(dir.join("relay.jsonl").exists(), "窓に着いた面の relay の記録を面の dir でなく既定の data dir（作業ツリーの .shibubu-data）に開いた");
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        assert!(ui5m_alive(&pid), "前提: stream の process が動いている");
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_RED);
        assert_eq!(pages.window("notes"), MirrorWindow::Closed, "赤を押しても閉じない");
        assert!(!ui5m_alive(&pid), "赤の後も helper の stream の process が動いている（取り込みが止まらない）");
        for _ in 0..20 {
            time += 0.05;
            ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(pages.awake.is_none(), "閉じた後も App Nap を止めたまま");
        assert_eq!(pages.reach("notes"), None, "閉じた面を「映像が来ない」と言う");
        assert_eq!(ui5m_lines(&dir.join("list.log")), 1, "閉じた面が窓を探し直した");
        let (_, band, _) = ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_TITLE);
        assert_eq!(pages.window("notes"), MirrorWindow::Open, "帯を押しても開かない");
        assert!(band.height() < 300.0, "前提: 押した frame は畳んだ高さから開き始める");
        assert!(ui5m_live(&ctx, &mut pages, &spec, &mut time), "帯を押しても映像が戻らない");
        assert_eq!((ui5m_lines(&dir.join("list.log")), ui5m_lines(&dir.join("pid"))), (2, 2), "開き直しで窓を 1 回探して stream を 1 本開く");
        assert!(pages.take_granted().is_empty(), "開き直しで同意を尋ねた");
        assert_eq!(pages.take_window_changes(), vec![("notes".to_string(), MirrorWindow::Closed), ("notes".to_string(), MirrorWindow::Open)]);
    }

    /// Done 1 の反証: keep_alive でない面は Page を離れると Pane ごと消える。閉じた形は Pane の外に在るので、戻っても窓を探さず stream を始めない。
    #[test]
    fn ui5m_closed_face_never_attaches_after_leaving_the_page() {
        let (sh, dir) = ui5m_helper();
        let mut pages = MirrorPages::new(sh, [ui5m_consent()], None);
        pages.relay_log = dir.join("relay.jsonl");
        let spec = ui5m_spec(false);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        assert!(ui5m_live(&ctx, &mut pages, &spec, &mut time), "偽の helper の frame が 5 秒で映らない");
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_RED);
        // 別の Page へ（show を呼ばない frame）
        pages.end_frame();
        pages.end_frame();
        assert!(!pages.panes.contains_key("notes"), "前提: keep_alive でない面の Pane は Page を離れると消える");
        for _ in 0..10 {
            time += 0.05;
            ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(pages.window("notes"), MirrorWindow::Closed);
        assert_eq!((ui5m_lines(&dir.join("list.log")), ui5m_lines(&dir.join("pid"))), (1, 1), "Page に戻ると閉じた面が窓を探して stream を始めた");
    }

    /// MIRROR-FIT-1101 ②: 枠（帯 + 映像）は本物の窓の大きさ。置ける所に入らなければ縦横の比のまま縮める・大きくはしない・Dock が無ければ映像の中身の比で収める。
    /// I31（owner 273）: 人が大きさを選んでいない面は、既定の大きさをまだ頼んでいない・本物が受けた間は置ける所の全部。頼んだ大きさと本物が違う（断った・自分で替えた）間は上の通り。
    /// 負の対照（commit message）: 縮める倍率の上限 1 を外す → 1000×800 の所で 1000 幅になって赤・比を保たない → 300 幅の所で赤・
    /// I31（未測）: `frame_size` の置ける所の全部の早抜けを外す → 最初の 1000×800 が 600 幅で赤
    #[test]
    fn mirror_fit_frame_takes_the_real_window_size_and_only_scales_down() {
        let mut pages = MirrorPages::new(PathBuf::from("/nonexistent-helper"), [], None);
        let mut pane = Pane::new(Err(MirrorError::WindowGone));
        let mut d = Dock::new(AppIdentity { bundle_id: "com.apple.TextEdit".into(), name: "TextEdit".into(), ..AppIdentity::default() }, 1);
        d.window = Some(crate::surface::WindowInfo { frame: Some(Frame { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }), ..Default::default() });
        pane.dock = Some(d);
        pages.panes.insert("s".into(), pane);
        assert_eq!(pages.frame_size("s", Vec2::new(1000.0, 800.0)), Vec2::new(1000.0, 800.0), "既定の枠が置ける所の全部でない（I31）");
        // 既定の大きさ（1000×772）を頼んだが本物は 600×400 のまま（断った・後で自分で替えた）→ 本物の大きさ
        pages.panes.get_mut("s").unwrap().prefit = Some(Vec2::new(1000.0, 800.0 - BAND_H));
        assert_eq!(pages.frame_size("s", Vec2::new(1000.0, 800.0)), Vec2::new(600.0, 400.0 + BAND_H), "入る所で本物の大きさでない");
        assert_eq!(pages.frame_size("s", Vec2::new(300.0, 800.0)), Vec2::new(300.0, 200.0 + BAND_H), "幅が足りない所で比のまま縮めない");
        assert_eq!(pages.frame_size("s", Vec2::new(1000.0, 200.0 + BAND_H)), Vec2::new(300.0, 200.0 + BAND_H), "高さが足りない所で比のまま縮めない");
        // 引き終わって読み戻しを待つ間は引いた大きさ
        pages.panes.get_mut("s").unwrap().resize = Some((Vec2::new(500.0, 250.0), 0.0, Some(Frame { x: 0.0, y: 0.0, w: 600.0, h: 400.0 })));
        assert_eq!(pages.frame_size("s", Vec2::new(1000.0, 800.0)), Vec2::new(500.0, 250.0 + BAND_H), "引いた大きさを待たずに戻した");
        // Dock が無い・映像も無い面は置ける所のまま
        let p = pages.panes.get_mut("s").unwrap();
        (p.dock, p.resize) = (None, None);
        assert_eq!(pages.frame_size("s", Vec2::new(500.0, 300.0)), Vec2::new(500.0, 300.0));
        // 映像だけ在る（中身が 2:1）: 比で収める（余白を出さない）
        pages.panes.get_mut("s").unwrap().last = Some(MirrorFrameMeta { seq: 1, width: 400, height: 200, captured_ms: 0.0, content: [0.0, 0.0, 200.0, 100.0], factor: 2.0 });
        assert_eq!(pages.frame_size("s", Vec2::new(500.0, 500.0)), Vec2::new(500.0, 250.0 + BAND_H), "Dock の無い面を映像の比で収めない");
    }

    /// Done 3: 題の帯と 3 つの button の上の click は元の app へ送らない（relay の記録 0 件）・映像の上の click は送る（正の対照）。
    /// 負の対照: `show` が `fit` と `mirror_inputs` に面の全部（帯ごと）を渡すと、帯の題の click が映像の点になって赤。
    #[test]
    fn ui5m_band_and_buttons_never_reach_the_original_app() {
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let (band, video) = split_band(rect);
        assert_eq!((band.min, band.height(), video.min.y, video.max), (rect.min, BAND_H, rect.min.y + BAND_H, rect.max));
        let meta = MirrorFrameMeta { seq: 1, width: 2, height: 3, captured_ms: 0.0, content: [0.0, 0.0, 1.0, 1.5], factor: 2.0 };
        let press = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let title = [press(UI5M_TITLE, true), press(UI5M_TITLE, false)];
        assert_eq!(mirror_inputs(&title, fit(video, &meta), &meta, true), vec![], "帯の題の click が映像の点になった");
        assert_eq!(mirror_inputs(&title, fit(rect, &meta), &meta, true).len(), 1, "前提（負の対照の形）: 帯ごとの矩形なら同じ点が映像の点になる");
        // 製品の道（show）
        let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; cat >> {d}/stdin");
        std::fs::write(dir.join("frame.bin"), ui5m_frame_bin()).unwrap();
        let log = dir.join("relay.jsonl");
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&log).unwrap()).unwrap();
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let spec = ui5m_spec(true);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        assert!(ui5m_live(&ctx, &mut pages, &spec, &mut time), "偽の helper の frame が 5 秒で映らない");
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_TITLE);
        assert_eq!(RelayLog::read(&log).unwrap().len(), 0, "帯の題の click を元の app へ送った");
        assert_eq!(pages.window("notes"), MirrorWindow::Open, "帯の題（button の外）の click で形が替わった");
        // 題は Choose window の menu を開く。開いたままの次の click は menu が取るので、枠の外で閉じてから映像を押す
        ui5m_click(&ctx, &mut pages, &spec, &mut time, pos2(700.0, 500.0));
        assert_eq!(RelayLog::read(&log).unwrap().len(), 0, "menu を閉じる click を元の app へ送った");
        // MIRROR-HYBRID-1101（owner 207）: 映像の 1 回目の click から元の app へ（選ぶのと同時に触る・Dock の無い面は Mirror の道）
        ui5m_click(&ctx, &mut pages, &spec, &mut time, pos2(220.0, 184.0));
        assert_eq!(RelayLog::read(&log).unwrap().len(), 1, "映像の 1 回目の click を元の app へ送らない（MIRROR-FIT の選ぶだけの押しのまま）");
        assert!(pages.panes["notes"].selected, "映像を押しても枠を選ばない");
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_GREEN);
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_GREEN);
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_YELLOW);
        assert_eq!(RelayLog::read(&log).unwrap().len(), 1, "赤黄緑の click を元の app へ送った");
        assert_eq!(pages.window("notes"), MirrorWindow::Minimized);
    }

    /// Done 5: 形の移りは `motion::RAIL`（≈ 0.2 s）で終わり、その間だけ repaint を頼む・終わった後（閉じた面）は頼まない・reduce motion なら 1 frame。
    #[test]
    fn ui5m_collapse_takes_motion_rail_and_reduce_motion_is_one_frame() {
        let ctx = egui::Context::default();
        crate::ui::motion::set_reduce_motion(&ctx, false);
        let mut pages = MirrorPages::new(PathBuf::from("/dev/null"), [], None);
        let spec = ui5m_spec(true);
        pages.restore_windows([("notes".to_string(), MirrorWindow::Open)]);
        let (_, r0, _) = ui5m_frame(&ctx, &mut pages, &spec, 10.0, true, vec![]);
        assert_eq!(r0.height(), 300.0, "開いた形の高さ");
        pages.set_window("notes", MirrorWindow::Closed, 10.0);
        let (_, mid, out) = ui5m_frame(&ctx, &mut pages, &spec, 10.0 + crate::ui::motion::RAIL as f64 / 2.0, true, vec![]);
        assert!(mid.height() > BAND_H + 1.0 && mid.height() < 299.0, "途中の高さが前と後の間に無い: {}", mid.height());
        assert!(out.viewport_output.values().any(|v| v.repaint_delay.is_zero()), "animation の間に repaint を頼まない");
        let (_, end, _) = ui5m_frame(&ctx, &mut pages, &spec, 10.0 + crate::ui::motion::RAIL as f64 + 0.01, true, vec![]);
        assert_eq!(end.height(), BAND_H, "RAIL の後も畳み終わらない");
        let (_, _, quiet) = ui5m_frame(&ctx, &mut pages, &spec, 11.0, true, vec![]);
        assert!(quiet.viewport_output.values().all(|v| v.repaint_delay > Duration::from_secs(1)), "閉じた面で repaint を頼み続けた: {:?}", quiet.viewport_output.values().map(|v| v.repaint_delay).collect::<Vec<_>>());
        // reduce motion: 押した frame で開き終わり、次に押した frame で畳み終わる
        crate::ui::motion::set_reduce_motion(&ctx, true);
        pages.set_window("notes", MirrorWindow::Open, 12.0);
        let (_, opened, _) = ui5m_frame(&ctx, &mut pages, &spec, 12.0, true, vec![]);
        assert_eq!(opened.height(), 300.0, "reduce motion で 1 frame で開かない");
        pages.set_window("notes", MirrorWindow::Minimized, 12.1);
        let (_, now, _) = ui5m_frame(&ctx, &mut pages, &spec, 12.1, true, vec![]);
        assert_eq!(now.height(), BAND_H, "reduce motion で 1 frame で畳まれない");
    }

    /// 拡大した面を描かなかった frame（別の Page・同じ Page の別の tab）の終わりで開いた形に戻す（renderer が隠した region を残さない）・保存にも出す。
    /// 負の対照: `end_frame` の戻しを外すと赤。
    #[test]
    fn ui5m_zoom_is_undone_when_the_face_is_not_drawn() {
        let mut pages = MirrorPages::new(PathBuf::from("/nonexistent/kangae-mirror-helper"), [], None);
        let spec = ui5m_spec(true);
        let ctx = egui::Context::default();
        pages.set_window("notes", MirrorWindow::Zoomed, 1.0);
        ui5m_frame(&ctx, &mut pages, &spec, 1.0, true, vec![]);
        assert_eq!(pages.window("notes"), MirrorWindow::Zoomed, "前提: 描いた frame では拡大のまま");
        pages.end_frame();
        assert_eq!(pages.window("notes"), MirrorWindow::Open, "拡大した面を描かなかった frame の後も拡大のまま（別の tab の中身が隠れたまま）");
        assert_eq!(pages.zoomed(&ctx), None);
        assert_eq!(pages.take_window_changes().last(), Some(&("notes".to_string(), MirrorWindow::Open)), "戻した形を保存に出さない");
    }

    /// Done 6: 3 つの button と帯の AX の名前・開いている時の frame の node の名前は前と同じ（`Mirroring · TextEdit`）・畳んだら frame の node は無く帯の Button だけ・
    /// Shibubu の窓が key でない時は 3 つとも灰（赤の色の円が無い）。
    #[test]
    fn ui5m_buttons_have_ax_names_and_the_open_frame_keeps_its_name() {
        use egui::accesskit::Role;
        let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; cat >/dev/null");
        std::fs::write(dir.join("frame.bin"), ui5m_frame_bin()).unwrap();
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let spec = ui5m_spec(true);
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut time = 0.0;
        assert!(ui5m_live(&ctx, &mut pages, &spec, &mut time), "偽の helper の frame が 5 秒で映らない");
        let names = |out: &egui::FullOutput| -> Vec<(Role, String, Option<String>)> {
            out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).map(|(_, n)| (n.role(), n.label().unwrap_or_default().to_string(), n.author_id().map(str::to_string))).collect()
        };
        let has = |out: &egui::FullOutput, name: &str| names(out).iter().any(|(r, l, _)| *r == Role::Button && l == name);
        time += 0.05;
        let (_, _, out) = ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
        for n in ["Close mirror", "Minimize mirror", "Zoom mirror"] {
            assert!(has(&out, n), "Button `{n}` が AX の木に無い: {:?}", names(&out));
        }
        let frame = names(&out).into_iter().find(|(_, _, a)| a.as_deref() == Some("surface-frame:notes")).expect("開いている時の frame の node");
        assert_eq!((frame.0, frame.1.as_str()), (FRAME_ROLE, "Mirroring · TextEdit"), "開いている時の frame の役と名前が替わった");
        let red = Color32::from_rgb(0xFF, 0x5F, 0x57);
        let reds = |out: &egui::FullOutput| out.shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Circle(s) if s.fill == red)).count();
        assert_eq!(reds(&out), 1, "key の窓で赤の円が 1 つでない");
        time += 0.05;
        let (_, _, away) = ui5m_frame(&ctx, &mut pages, &spec, time, false, vec![]);
        assert_eq!(reds(&away), 0, "Shibubu の窓が key でないのに赤のまま（macOS は 3 つとも灰）");
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_GREEN);
        time += 1.0;
        let (_, _, zoomed) = ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
        assert!(has(&zoomed, "Restore mirror") && !has(&zoomed, "Zoom mirror"), "拡大中の緑の名前: {:?}", names(&zoomed));
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_YELLOW);
        time += 1.0;
        let (_, _, folded) = ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
        assert!(has(&folded, "Open mirror · TextEdit"), "畳んだ帯の Button: {:?}", names(&folded));
        assert!(!names(&folded).iter().any(|(_, _, a)| a.as_deref() == Some("surface-frame:notes")), "畳んだ面に frame の node を出した（状態の語が嘘になる）");
        assert!(!has(&folded, "Close mirror"), "畳んだ帯に赤を出した");
    }

    /// Done 4: Page に Mirror（split の main）・side の text・下の text を置いて `render_page` で描く。畳むと下の region が空いた所を使い（rect の高さを数える）、
    /// 緑で Mirror の region が Page の中身の rect（木の左上から右端・見える下端 − 16 まで）と同じになり side と下は描かない・戻すと前の rect。
    /// 拡大と戻しの frame の前後で AX の node が別の親へ移らない（移ると macOS の accesskit_consumer が落ちる・W14b）。
    #[test]
    fn ui5m_folded_space_goes_to_the_next_region_and_zoom_fills_the_page() {
        use crate::domain::project::PageDefinition;
        use crate::domain::ui::UiNode;
        use crate::ui::renderer::{render_page, ActionHost, AppState, PageSurfaces};
        use egui::accesskit::NodeId;
        struct Nil;
        impl ActionHost for Nil {
            fn invoke(&mut self, _inv: crate::domain::action::ActionInvocation) {}
        }
        let node = |key: &str, component: &str, props: serde_json::Value, children: Vec<UiNode>| UiNode { key: key.into(), component: component.into(), props: props.as_object().cloned().unwrap_or_default(), children, ..UiNode::default() };
        let text = |key: &str| node(key, "text", serde_json::json!({"text": key}), vec![]);
        let root = node(
            "root",
            "column",
            serde_json::json!({}),
            vec![node("split", "split", serde_json::json!({}), vec![node("main", "column", serde_json::json!({}), vec![node("m", "source_surface", serde_json::json!({"surface_ref": "notes"}), vec![])]), node("side", "column", serde_json::json!({}), vec![text("side-text")])]), text("below")],
        );
        let app: crate::domain::app::AppDefinitionV3 = serde_json::from_value(serde_json::json!({
            "schema_version": "3.0", "id": "test.ui5m", "version": 1, "title": "Docs",
            "goal": {"literal_goal": "Mirror を畳む", "outcomes": ["畳める"]},
            "composition": {"root": {"key": "root", "component": "column"}},
            "capabilities": [], "actions": [], "sources": [],
            "agent": {"suggestions_enabled": true, "autonomy_level": "suggest_only", "entrypoint": "rail"},
            "state": {"readiness": "ready", "execution_mode": "local_ready"}
        }))
        .unwrap();
        let page = PageDefinition { id: "docs".into(), route: "/docs".into(), title: "Docs".into(), app_ref: None, root, revision: 1 };
        let specs = [ui5m_spec(true)];
        // 高さは窓の形だけで決まる（映像の有無に依らない）ので helper は無い path（開くと HelperMissing の 1 行）
        let mut pages = MirrorPages::new(PathBuf::from("/nonexistent/kangae-mirror-helper"), [], None);
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut state = AppState::default();
        let mut prev: Option<std::collections::HashMap<NodeId, NodeId>> = None;
        let mut moved = Vec::new();
        let mut frame = |pages: &mut MirrorPages, time: f64| -> (std::collections::HashMap<String, Rect>, Rect) {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 900.0))), time: Some(time), ..Default::default() };
            let mut clip = Rect::NOTHING;
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    clip = ui.clip_rect();
                    render_page(ui, &app, &page, &mut state, &mut Nil, PageSurfaces { specs: &specs, mirror: Some(&mut *pages) });
                });
            });
            pages.end_frame();
            let parents: std::collections::HashMap<NodeId, NodeId> = out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).flat_map(|(id, n)| n.children().iter().map(move |c| (*c, *id))).collect();
            if let Some(before) = &prev {
                moved.extend(parents.iter().filter(|(c, p)| before.get(*c).is_some_and(|o| o != *p)).map(|(c, _)| format!("{time}: {c:?}")));
            }
            prev = Some(parents);
            (state.rects.clone(), clip)
        };
        pages.restore_windows([("notes".to_string(), MirrorWindow::Closed)]);
        frame(&mut pages, 1.0);
        let (closed, _) = frame(&mut pages, 1.1);
        pages.set_window("notes", MirrorWindow::Open, 2.0);
        frame(&mut pages, 2.0);
        let (open, clip) = frame(&mut pages, 3.0);
        let grown = open["m"].height() - closed["m"].height();
        assert!(grown > 200.0, "前提: 開いた Mirror は畳んだ帯より高い（{grown}）");
        assert!((open["below"].min.y - closed["below"].min.y - grown).abs() < 1.0, "畳んだ面の空きを下の region が使わない: 開 {:?} / 畳 {:?}", open["below"], closed["below"]);
        // 緑: Page の中身いっぱい・side と下は描かない
        pages.set_window("notes", MirrorWindow::Zoomed, 4.0);
        frame(&mut pages, 4.0);
        let (zoomed, _) = frame(&mut pages, 5.0);
        let content = Rect::from_min_max(open["root"].min, pos2(open["root"].max.x, clip.bottom() - theme::SP_4));
        assert!((zoomed["m"].min - content.min).length() < 1.0 && (zoomed["m"].max - content.max).length() < 1.0, "拡大した Mirror の rect {:?} が Page の中身 {:?} と違う", zoomed["m"], content);
        assert!(!zoomed.contains_key("side-text") && !zoomed.contains_key("below"), "拡大中に他の region を描いた: {:?}", zoomed.keys().collect::<Vec<_>>());
        // 戻す
        pages.set_window("notes", MirrorWindow::Open, 6.0);
        frame(&mut pages, 6.0);
        let (back, _) = frame(&mut pages, 7.0);
        for k in ["m", "side-text", "below"] {
            assert!((back[k].min - open[k].min).length() < 0.5 && (back[k].max - open[k].max).length() < 0.5, "戻した {k} の rect {:?} が拡大の前 {:?} と違う", back[k], open[k]);
        }
        assert!(moved.is_empty(), "拡大・戻しで AX の node が別の親へ移った（consumer が落ちる形）: {:?}", moved.iter().take(5).collect::<Vec<_>>());
    }

    /// G93 (2): 拒否の 1 行（`× Failed · …`）は映像の矩形に被らず題の帯の中に出る。負の対照: 拒否の 1 行を `inner`（映像の child）に描く元の形へ戻すと赤。
    #[test]
    fn g93_refusal_line_sits_in_the_title_band_not_over_the_video() {
        let (sh, dir) = ui5m_helper();
        let mut pages = MirrorPages::new(sh, [ui5m_consent()], None);
        pages.relay_log = dir.join("relay.jsonl");
        let spec = ui5m_spec(true);
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut time = 0.0;
        assert!(ui5m_live(&ctx, &mut pages, &spec, &mut time), "偽の helper の frame が 5 秒で映らない");
        assert!(pages.relay("notes", MirrorInput::Scroll { x: 0.5, y: 0.5, dx: 0, dy: -40 }).is_err(), "前提: scroll は届かず拒否の 1 行になる");
        time += 0.016;
        let (_, rect, out) = ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
        let (band, video) = split_band(rect);
        let b = out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).filter(|(_, n)| n.label().or(n.value()).is_some_and(|l| l.contains("Failed"))).filter_map(|(_, n)| n.bounds()).next().unwrap_or_else(|| panic!("拒否の 1 行が AX の木に無い（試験が測っていない）: line {:?}・AX {:?}", pages.panes["notes"].line, out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).filter_map(|(_, n)| n.label().or(n.value())).collect::<Vec<_>>()));
        let line = Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32));
        assert!(!line.intersects(video.shrink(0.5)), "拒否の 1 行 {line:?} が映像の矩形 {video:?} に被る");
        assert!(band.expand(1.0).contains_rect(line), "拒否の 1 行 {line:?} が題の帯 {band:?} の中に無い");
    }

    /// G93 (1): 確かめていない面は 1 行を取らず、`surface-status:<id>` の AX の node が開いた枠の title bar の右端に在る（NOTIFY-1101 から `○` は描かない・
    /// node の矩形は空の中身の種の位置 = 帯の右端の中央）。負の対照: `components.rs` の `in_band` を常に false にすると（node が面の上の行に戻る）赤。
    #[test]
    fn g93_unchecked_mark_sits_at_the_right_end_of_the_title_band() {
        use crate::domain::project::PageDefinition;
        use crate::domain::ui::UiNode;
        use crate::ui::renderer::{render_page, ActionHost, AppState, PageSurfaces};
        struct Nil;
        impl ActionHost for Nil {
            fn invoke(&mut self, _inv: crate::domain::action::ActionInvocation) {}
        }
        let m = UiNode { key: "m".into(), component: "source_surface".into(), props: serde_json::json!({"surface_ref": "notes"}).as_object().cloned().unwrap_or_default(), ..UiNode::default() };
        let root = UiNode { key: "root".into(), component: "column".into(), children: vec![m], ..UiNode::default() };
        let app: crate::domain::app::AppDefinitionV3 = serde_json::from_value(serde_json::json!({
            "schema_version": "3.0", "id": "test.g93", "version": 1, "title": "Docs",
            "goal": {"literal_goal": "Mirror", "outcomes": ["映る"]},
            "composition": {"root": {"key": "root", "component": "column"}},
            "capabilities": [], "actions": [], "sources": [],
            "agent": {"suggestions_enabled": true, "autonomy_level": "suggest_only", "entrypoint": "rail"},
            "state": {"readiness": "ready", "execution_mode": "local_ready"}
        }))
        .unwrap();
        let page = PageDefinition { id: "docs".into(), route: "/docs".into(), title: "Docs".into(), app_ref: None, root, revision: 1 };
        let specs = [ui5m_spec(true)];
        let mut pages = MirrorPages::new(PathBuf::from("/nonexistent/kangae-mirror-helper"), [], None);
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let mut state = AppState::default();
        let mut out = None;
        for time in [1.0, 2.0] {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 900.0))), time: Some(time), ..Default::default() };
            out = Some(ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| render_page(ui, &app, &page, &mut state, &mut Nil, PageSurfaces { specs: &specs, mirror: Some(&mut pages) }));
            }));
            pages.end_frame();
        }
        let m = state.rects["m"];
        let b = out.unwrap().platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).find(|(_, n)| n.author_id() == Some("surface-status:notes")).and_then(|(_, n)| n.bounds()).expect("surface-status:notes が AX の木に無い");
        let mark = Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32));
        let band = split_band(m).0;
        assert!(band.expand(1.0).contains_rect(mark), "`○` {mark:?} が題の帯 {band:?} の中に無い（面の上の 1 行に戻った）");
        assert!(mark.min.x >= band.right() - STATUS_W - 1.0, "`○` {mark:?} が帯の右端に無い");
    }

    /// G87: 書類の指定の無い Mirror の面（owner の Project の REAPER・Music と同じ形）・偽の helper の list は窓 0。
    fn g87_spec() -> SurfaceSpec {
        use crate::domain::project::{SurfaceBackendKind, SurfaceTarget};
        SurfaceSpec { id: "reaper".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.cockos.reaper".into(), file: None }, keep_alive: true, frame: None, browser: false }
    }

    fn g87_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        ctx
    }

    /// 面の 1 frame（`events` = mouse の入力）。返す = 枠の状態と出力（AX の木）。
    fn g87_frame(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, events: Vec<Event>) -> (PortalState, egui::FullOutput) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), events, ..Default::default() };
        let mut st = PortalState::IntegrationFailed;
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| st = pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0)), spec));
        });
        pages.end_frame();
        (st, out)
    }

    /// 窓探しの答えが着いた frame まで回す（混んだ機械では sh の起動に秒かかる・上限 9 秒）。
    fn g87_settle(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec) -> (PortalState, egui::FullOutput) {
        let t = std::time::Instant::now();
        loop {
            let r = g87_frame(ctx, pages, spec, vec![]);
            let p = &pages.panes[&spec.id];
            if (p.answered && p.pending.is_none()) || t.elapsed() > Duration::from_secs(9) {
                return r;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 2 秒を待たずに探し直させ、答えまで回す。
    fn g87_again(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec) -> (PortalState, egui::FullOutput) {
        pages.panes.get_mut(&spec.id).unwrap().tried_ms = f64::NEG_INFINITY;
        g87_frame(ctx, pages, spec, vec![]);
        assert!(pages.panes[&spec.id].pending.is_some(), "探し直しが起きていない（試験が測っていない）");
        g87_settle(ctx, pages, spec)
    }

    /// 面の枠の node（`surface-frame:<id>`）の label。
    fn g87_label(out: &egui::FullOutput) -> String {
        out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).find(|(_, n)| n.author_id() == Some("surface-frame:reaper")).and_then(|(_, n)| n.label()).unwrap_or_default().to_string()
    }

    /// AX の Button（名前 `name`）の中心。
    fn g87_button(out: &egui::FullOutput, name: &str) -> Option<Pos2> {
        let (_, n) = out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).find(|(_, n)| n.role() == accesskit::Role::Button && n.label() == Some(name))?;
        n.bounds().map(|b| pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32))
    }

    /// 本物の mouse の 1 click（動かす → 押す → 離す・frame を分ける）。
    fn g87_click(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, at: Pos2) {
        let b = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        for e in [Event::PointerMoved(at), b(true), b(false)] {
            g87_frame(ctx, pages, spec, vec![e]);
        }
    }

    /// G94 ③: 画面収録が off の面の `Screen recording is off · Open settings` は、文だけでなく AX の Button `Open settings` を持つ。
    /// 負の対照: `(Err(MirrorError::ScreenCaptureDenied), _, _)` の arm を消す → 赤
    #[test]
    fn g94_screen_recording_off_has_an_open_settings_button() {
        let (mut pages, _dir, _) = g82_pages("echo '[]'; exit 3");
        let (spec, ctx) = (g87_spec(), g87_ctx());
        let (_, out) = g87_settle(&ctx, &mut pages, &spec);
        assert!(g87_label(&out).contains("Screen recording is off"), "{}", g87_label(&out));
        let at = g87_button(&out, "Open settings").expect("文が言う Open settings の button が無い");
        assert!(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)).contains(at), "button が窓の外: {at:?}");
    }

    /// G87（P0・owner 10-27「openreaperがあるけどそこを押してもreaperが開くわけではない」「起動まで自分でやってくれないと」）: 書類の指定の無い Mirror の面で
    /// app が動いていなければ、Page を開いた最初の窓探しで Shibubu が 1 回だけ起こし（偽の launcher に if_stopped = true）、窓が出るまで `Starting REAPER…`。
    /// 2 回目の探しでは起こさない。待ちの上限（`LAUNCH_WAIT_MS`）を過ぎても窓が無ければ `REAPER has no open window`（文に次の一手を書かない）と
    /// AX の Button `Open REAPER`。mouse の 1 click で launcher を if_stopped = false で呼ぶ（動いていても窓を出させる）。
    /// 負の対照（commit message）: 最初の探しで launcher を呼ばない → 赤・B23（未測）: SourceUnavailable の語を Not running に戻す → 窓が無い行が赤
    #[test]
    fn g87_a_stopped_app_is_started_once_and_open_app_is_a_button() {
        static CALLS: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());
        let calls = || CALLS.lock().unwrap().clone();
        let (mut pages, _dir, _) = g82_pages("echo '[]'");
        pages.launch = |id, if_stopped| {
            CALLS.lock().unwrap().push((id.into(), if_stopped));
            Ok(("REAPER".into(), true))
        };
        let (spec, ctx) = (g87_spec(), g87_ctx());
        let (st, out) = g87_settle(&ctx, &mut pages, &spec);
        assert_eq!(calls(), vec![("com.cockos.reaper".to_string(), true)], "最初の探しで、動いていない app を 1 回起こしていない");
        assert_eq!(st, PortalState::Attaching, "{}", g87_label(&out));
        assert!(g87_label(&out).contains("Starting REAPER…"), "起こした後に Starting の 1 行が無い: {}", g87_label(&out));
        // 2 回目の探し（窓はまだ無い）: また起こさない・Starting のまま
        let (st, out) = g87_again(&ctx, &mut pages, &spec);
        assert_eq!(calls().len(), 1, "2 回目の探しでまた起こした: {:?}", calls());
        assert_eq!(st, PortalState::Attaching);
        assert!(g87_label(&out).contains("Starting REAPER…"), "{}", g87_label(&out));
        // 待ちの上限を過ぎた（窓が出なかった）: 窓が無い 1 行と `Open REAPER` の button
        pages.panes.get_mut("reaper").unwrap().starting = Some(crate::surface::now_ms() - LAUNCH_WAIT_MS - 1.0);
        let (st, out) = g87_again(&ctx, &mut pages, &spec);
        assert_eq!(st, PortalState::SourceUnavailable, "{}", g87_label(&out));
        // B23: 起こした（動いている）app の窓が出ない時に「Not running」と言わない
        assert_eq!(g87_label(&out), "Not available · REAPER has no open window", "名前は bundle id の語でなく表示名・文に次の一手を書かない・動いている app を Not running と言わない");
        let at = g87_button(&out, "Open REAPER").expect("「Open REAPER」が AX の Button に無い（文の一部のまま）");
        g87_click(&ctx, &mut pages, &spec, at);
        g87_settle(&ctx, &mut pages, &spec);
        assert_eq!(calls(), vec![("com.cockos.reaper".to_string(), true), ("com.cockos.reaper".to_string(), false)], "Open REAPER を押しても起こさない");
    }

    /// FX6（FX-036・UIUX F08）: Project の書類の file が repo に無い時、窓が無い 1 行に `Create {file}` の button が出て、押すと空の file ができる。在る file には出さない。
    /// 負の対照（commit message）: `Create {name}` の button を描く if を外す → 赤。
    #[test]
    fn fx6_a_missing_document_gets_a_create_button_that_makes_the_file() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceTarget};
        let (mut pages, dir, _) = g82_pages("echo '[]'");
        pages.workspace = Some(dir.join("ws"));
        let spec = SurfaceSpec { id: "reaper".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let ctx = g87_ctx();
        let (_, out) = g87_settle(&ctx, &mut pages, &spec);
        let at = g87_button(&out, "Create docs/notes.md").unwrap_or_else(|| panic!("Create の button が無い: {}", g87_label(&out)));
        assert!(!dir.join("ws/docs/notes.md").exists(), "前提: file が在る");
        g87_click(&ctx, &mut pages, &spec, at);
        assert!(dir.join("ws/docs/notes.md").exists(), "Create を押しても file ができない");
        let (_, out) = g87_again(&ctx, &mut pages, &spec);
        assert!(g87_button(&out, "Create docs/notes.md").is_none(), "在る file に Create が出る");
    }

    /// G90: 1 frame（Shibubu の窓が前か = viewport の focused）。返す = viewport の Focus（前面を Shibubu に戻す依頼）を出したか。
    fn g90_frame(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, focused: bool) -> bool {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(focused);
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0)), spec));
        });
        pages.end_frame();
        out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Focus)))
    }

    /// G90（operator の実機 05: 起こした REAPER が前面を取った）: Shibubu の窓が前の時に起こした app が、起動の後（窓を待つ間 = `LAUNCH_WAIT_MS`）に前面を取ったら 1 回だけ前面を Shibubu に戻す。
    /// 人が自分で別の app を前に出した・Shibubu が前でない時に起こした・app が既に動いていた（起こしていない）→ 戻さない。
    /// 負の対照（commit message）: 戻さない（Focus を出さない）→ 赤
    #[test]
    fn g90_b_an_app_shibubu_started_that_takes_the_front_gives_it_back_once() {
        static FRONT: std::sync::Mutex<&str> = std::sync::Mutex::new("com.cockos.reaper");
        let settle = |ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec| {
            let t = std::time::Instant::now();
            while !(pages.panes.get(&spec.id).is_some_and(|p| p.answered && p.pending.is_none()) || t.elapsed() > Duration::from_secs(9)) {
                assert!(!g90_frame(ctx, pages, spec, true), "Shibubu が前のまま Focus を出した");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let fresh = |started: bool| {
            let (mut pages, dir, _) = g82_pages("echo '[]'");
            pages.launch = if started { |_, _| Ok(("REAPER".into(), true)) } else { |_, _| Ok(("REAPER".into(), false)) };
            pages.front = || Some(FRONT.lock().unwrap().to_string());
            (pages, dir)
        };
        let spec = g87_spec();
        // 起こした REAPER が前面を取った → 1 回だけ戻す
        *FRONT.lock().unwrap() = "com.cockos.reaper";
        let ((mut pages, _d1), ctx) = (fresh(true), g87_ctx());
        settle(&ctx, &mut pages, &spec);
        assert!(g90_frame(&ctx, &mut pages, &spec, false), "起こした app が前面を取ったのに Shibubu に戻さない");
        assert!(!g90_frame(&ctx, &mut pages, &spec, false), "2 回目も戻した（1 回だけ）");
        // 起動の遅い app（混んだ Mac の REAPER）: 起こして 30 秒後に前面を取っても戻す・窓を待つ間（LAUNCH_WAIT_MS）を過ぎたら戻さない
        for (ago, back) in [(30_000.0, true), (LAUNCH_WAIT_MS + 1.0, false)] {
            let ((mut pages, _d), ctx) = (fresh(true), g87_ctx());
            settle(&ctx, &mut pages, &spec);
            pages.panes.get_mut("reaper").unwrap().reclaim = Some(crate::surface::now_ms() - ago);
            assert_eq!(g90_frame(&ctx, &mut pages, &spec, false), back, "起こして {ago} ms 後に前面を取った");
        }
        // 人が自分で別の app を前に出した → 戻さない（後で REAPER が前に出ても戻さない）
        *FRONT.lock().unwrap() = "com.apple.Safari";
        let ((mut pages, _d2), ctx) = (fresh(true), g87_ctx());
        settle(&ctx, &mut pages, &spec);
        assert!(!g90_frame(&ctx, &mut pages, &spec, false), "人が前に出した別の app から Shibubu に戻した");
        *FRONT.lock().unwrap() = "com.cockos.reaper";
        assert!(!g90_frame(&ctx, &mut pages, &spec, false), "見張りを捨てた後に戻した");
        // app が既に動いていた（起こしていない）→ その app を人が前に出しても戻さない
        let ((mut pages, _d3), ctx) = (fresh(false), g87_ctx());
        settle(&ctx, &mut pages, &spec);
        assert!(!g90_frame(&ctx, &mut pages, &spec, false), "起こしていない app から Shibubu に戻した");
        // Shibubu が前でない時に起こした → Shibubu へは戻さない（前の app へ戻すのは l5_stick_an_app_started_while_shibubu_is_behind…・ここは前面が起こす app 自身 = 見張らない）
        let ((mut pages, _d4), ctx) = (fresh(true), g87_ctx());
        g90_frame(&ctx, &mut pages, &spec, false);
        let t = std::time::Instant::now();
        while !(pages.panes["reaper"].answered && pages.panes["reaper"].pending.is_none()) && t.elapsed() < Duration::from_secs(9) {
            assert!(!g90_frame(&ctx, &mut pages, &spec, false), "Shibubu が前でない時に起こした app から戻した");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!g90_frame(&ctx, &mut pages, &spec, false));
    }

    /// G90・L5-STICK ①（DOGFOOD-X 周 2 回 1: Accept の後に Shibubu が起こした REAPER が、背景の起動なのに前面を取って Shibubu を覆った・Shibubu は前でなかった）:
    /// Shibubu の窓が前でない時に起こした app が前面を取ったら、起こす前に前だった app（Orca）へ 1 回だけ戻す（Shibubu へは Focus しない）。前の app が前のままの間は戻さない。
    /// 試験の data dir は g82_pages の新しい dir。
    /// B33: 前面を戻した後は起動の印（`launched`）を消す（Dock より先に見た起動の activate）。
    /// 負の対照: 起こす時に `front_before` を読まない（Shibubu が前の時だけ見張る = 前の形）→ 戻さず赤・Starting を会話に出さない → 赤・
    /// B33（未測）: `took` の所の `pane.launched = None` を外す → 最後の行が赤
    #[test]
    fn l5_stick_an_app_started_while_shibubu_is_behind_gives_the_front_back_to_the_app_that_had_it() {
        static FRONT: std::sync::Mutex<&str> = std::sync::Mutex::new("com.stablyai.orca");
        static ACTIVATED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let (mut pages, _dir, _) = g82_pages("echo '[]'");
        pages.launch = |_, _| Ok(("REAPER".into(), true));
        pages.front = || Some(FRONT.lock().unwrap().to_string());
        pages.activate = |b| {
            ACTIVATED.lock().unwrap().push(b.into());
            Ok(())
        };
        let (spec, ctx) = (g87_spec(), g87_ctx());
        // 起こした後、前の app（Orca）が前のまま → 戻さない・Shibubu へ Focus しない
        let t = std::time::Instant::now();
        while !(pages.panes.get("reaper").is_some_and(|p| p.answered && p.pending.is_none()) || t.elapsed() > Duration::from_secs(9)) {
            assert!(!g90_frame(&ctx, &mut pages, &spec, false), "Shibubu が前でないのに Shibubu へ Focus した");
            std::thread::sleep(Duration::from_millis(50));
        }
        for _ in 0..12 {
            assert!(!g90_frame(&ctx, &mut pages, &spec, false), "Shibubu が前でないのに Shibubu へ Focus した");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ACTIVATED.lock().unwrap().is_empty(), "前の app が前のままなのに前に出した: {:?}", ACTIVATED.lock().unwrap());
        // 起こした REAPER が前面を取った → 前の app（Orca）へ 1 回だけ戻す
        *FRONT.lock().unwrap() = "com.cockos.reaper";
        let t = std::time::Instant::now();
        while ACTIVATED.lock().unwrap().is_empty() && t.elapsed() < Duration::from_secs(5) {
            assert!(!g90_frame(&ctx, &mut pages, &spec, false), "Shibubu が前でなかったのに Shibubu へ Focus した");
            std::thread::sleep(Duration::from_millis(50));
        }
        for _ in 0..12 {
            g90_frame(&ctx, &mut pages, &spec, false);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(*ACTIVATED.lock().unwrap(), ["com.stablyai.orca"], "起こした app が前面を取ったのに、前だった app へ 1 回だけ戻さない");
        // B33: 起動の activate は Dock より先に見た（窓が無く Dock も無い面）→ 起動の印を消す（残すと 60 秒以内の人の ⌘Tab を起動と数える）
        assert!(pages.panes["reaper"].dock.is_none(), "前提: Dock が在る");
        assert!(pages.panes["reaper"].launched.is_none(), "起動の activate を見て前面を戻した後も起動の印が残る（60 秒以内の人の ⌘Tab を起動と数える）");
    }

    // ---- G91: 本物の窓をしまう・触る時だけ前に出す（pane-G91 の試験 1〜4）----

    /// Shibubu の窓の中身の左上（画面の point）。面 (20, 20)・幅 400・高さ 300 → 映像の矩形は (20, 48)〜(420, 320) = 画面の (120, 98) 400×272
    const G91_HOST: Pos2 = pos2(100.0, 50.0);

    /// 偽の bridge（tests/fake-surface-bridge.py・映している TextEdit = pid 42・窓 w1 は CG の 500）と受け取った要求の log。
    fn g91_bridge(name: &str, env: &[(&str, &str)]) -> (Bridge, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("shibubu-g91-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("bridge.log");
        let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
        // MIRROR-FIT-1101: 枠は本物の大きさに合わせる = 始めの窓を試験の枠の映像（400×272）と同じ大きさに（試験ごとに env で替えられる）
        let exports: String = [("FAKE_APP_BUNDLE", "com.apple.TextEdit"), ("FAKE_APP_NAME", "TextEdit"), ("FAKE_APP_PID", "42"), ("FAKE_W", "400"), ("FAKE_H", "272")].iter().chain(env).map(|(k, v)| format!("export {k}='{v}'\n")).collect();
        let sh = dir.join("bridge.sh");
        std::fs::write(&sh, format!("#!/bin/sh\nexport FAKE_LOG='{}'\n{exports}exec python3 '{py}'\n", log.display())).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (Bridge::spawn(&sh, Duration::from_secs(10)).expect("偽の bridge が起きない"), log)
    }

    /// bridge が受け取った `cmd` の要求（log の順）
    fn g91_sent(log: &Path, cmd: &str) -> Vec<serde_json::Value> {
        std::fs::read_to_string(log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|v| v["cmd"] == cmd).collect()
    }

    /// ui5m の helper の窓（7）を偽の bridge の窓（CG の 500）にした物・同意済みの面と偽の order（呼ばれた窓の番号を `seen` に）
    fn g91_pages(bridge: Bridge, above: fn(u32) -> Result<(), String>, below: fn(u32) -> Result<(), String>) -> (MirrorPages, PathBuf) {
        let (sh, dir) = ui5m_helper();
        let script = std::fs::read_to_string(&sh).unwrap().replace("\"window_id\":7", "\"window_id\":500");
        std::fs::write(&sh, script).unwrap();
        let mut pages = MirrorPages::new(sh, [ui5m_consent()], None);
        (pages.relay_log, pages.bridge, pages.order_above, pages.order_below) = (dir.join("relay.jsonl"), Some(Ok(bridge)), above, below);
        (pages, dir)
    }

    /// 呼ぶ側と同じ 1 frame（`ui5m_frame` と同じ面）に、Shibubu の窓の中身の左上（`host`）と前か（`focused` = viewport の focused）を足した物
    fn g91_frame(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, time: f64, host: Pos2, focused: bool, events: Vec<Event>) -> egui::FullOutput {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(time), focused, events, ..Default::default() };
        let vp = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        (vp.inner_rect, vp.focused) = (Some(Rect::from_min_size(host, Vec2::new(800.0, 600.0))), Some(focused));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let h = pages.height(ctx, &spec.id, 300.0, 500.0);
                pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, h)), spec);
            });
        });
        pages.end_frame();
        out
    }

    /// 偽の bridge の log が `f` を満たすまで待つ（送るだけの要求は python が読むまで log に出ない・上限 5 秒）
    fn g91_wait(f: impl Fn() -> bool) -> bool {
        let t = std::time::Instant::now();
        while !f() {
            if t.elapsed() > Duration::from_secs(5) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// `until` が成り立つまで frame を回す（偽の helper と bridge は sh と python なので混んだ機械では遅い・上限 9 秒）
    fn g91_until(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, time: &mut f64, host: Pos2, focused: bool, until: impl Fn(&MirrorPages) -> bool) -> bool {
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(9) {
            *time += 0.05;
            g91_frame(ctx, pages, spec, *time, host, focused, vec![]);
            if until(pages) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    }

    /// B14（DOGFOOD-X 周 5: 映した直後の Numbers の windows が 1 回「app が応答しません」で、その面は Dock を付けず、枠に合わせる・前に出す事が無いまま
    /// stays a mirror だった）: 答えない windows（ax_cannot_complete）は `REFIT_GAP_S` の後に聞き直し、2 回目に答えれば Dock を付ける。
    /// 許可が無い（not_trusted）は今まで通り聞き直さない（下の試験 1 の後半）。
    /// 負の対照（未測）: pump_docks の答えない腕を外す（`tuck_err` にする）→ Dock が付かず赤
    #[test]
    fn b14_a_window_list_that_did_not_answer_once_is_asked_again() {
        let (bridge, log) = g91_bridge("b14-busy", &[("FAKE_BUSY_WINDOWS", "1")]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "1 回答えなかった windows を聞き直さず、Dock が付かない: {:?}", pages.panes.get("notes").and_then(|p| p.tuck_err.clone()));
        assert_eq!(g91_sent(&log, "windows").len(), 2, "聞き直しの数が違う（1 回答えなかった後の 1 回だけ）");
        assert!(pages.panes["notes"].tuck_err.is_none() && pages.panes["notes"].refits == 0, "Dock が付いた後も聞き直しの理由か数を持っている");
    }

    /// B88（Desktop の 38 回目・Project「Humming to REAPER」: 起こした app の最初の窓が消え、Mirror は新しい主窓を映し直したのに、本物を置く Dock は消えた窓のまま
    /// 付き直さず、押すと「{App}'s window isn't lined up with this frame · it stays a mirror」・本物は枠の外のまま）: Dock の窓が閉じた後も、Mirror が映している窓へ
    /// Dock を付け直して本物を枠の位置へ置く（押せば前に出せる）。2 回目の windows で題が替わる = Dock の題で繋ぎ直す道では付かない。
    /// 負の対照（未測）: `tuck` の付け直し（`stale`）を外す → Dock が SourceUnavailable のまま赤
    #[test]
    fn b88_a_dock_whose_window_closed_moves_to_the_window_the_mirror_shows() {
        let (bridge, log) = g91_bridge("b88", &[("FAKE_TITLES", "first window|main window")]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let life = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).map(|d| d.life);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| life(p) == Some(Lifecycle::Active)), "前提: しまわない");
        // 最初の窓が閉じた（bridge の destroyed）→ その窓の Dock は外れる。Mirror は窓 500 を映したまま（新しい主窓を映し直した所）
        {
            let (Some(Ok(b)), Some(p)) = (pages.bridge.as_mut(), pages.panes.get_mut("notes")) else { panic!("前提: bridge と面") };
            let d = p.dock.as_mut().expect("前提: Dock");
            d.on_msg(b, Msg::Event(serde_json::json!({ "event": "destroyed", "handle": "w1" })), Instant::now());
            assert_eq!(d.life, Lifecycle::SourceUnavailable, "前提: 閉じた窓の Dock が外れない");
        }
        let sets = g91_sent(&log, "set_frame").len();
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| life(p) == Some(Lifecycle::Active)), "閉じた窓の Dock のまま、映している窓に付け直さない: {:?}", life(&pages));
        assert_eq!(g91_sent(&log, "windows").len(), 2, "付け直しの windows の数が違う（閉じた後の 1 回だけ）");
        let last = g91_sent(&log, "set_frame").last().map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap()]);
        assert!(g91_sent(&log, "set_frame").len() > sets && last == Some([120.0, 98.0]), "付け直した本物を枠の位置へ置かない: {last:?}");
        assert_eq!(stay_reason(&pages.panes["notes"], "TextEdit"), None, "付け直した後も押すと stays a mirror と言う");
    }

    /// B89（41 回目・Project「Humming to REAPER」: 起こした REAPER の窓が出た直後に映した窓は起動の評価版の dialog〈main だが動かせない〉で、押すと
    /// 「Can't bring REAPER here · it stays a mirror」のまま）: 書類も題も選んでいない面は、その app の主窓（`pick_main_window` = 普通の窓で動かせる main）を
    /// 映し直して Dock を付ける。書類の面は選んだ窓のまま（W16M-3・主窓にすり替えない）。
    /// 負の対照（未測）: `pump_docks` の `main` を外す → app だけの面が dialog（600）を映したまま・Dock も dialog に付いて赤
    #[test]
    fn b89_an_app_only_face_shows_the_main_window_instead_of_the_dialog_it_found_first() {
        for file in [None, Some("docs/notes.md")] {
            let (bridge, _log) = g91_bridge(&format!("b89-main-{}", file.is_some()), &[("FAKE_DIALOG_CG", "600")]);
            let ((mut pages, _dir), ctx, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), 0.0);
            // 偽の helper が最初に見つける窓 = 動かせない dialog（CG の 600）・主窓 w1 は CG の 500
            let script = std::fs::read_to_string(&pages.helper).unwrap().replace("\"window_id\":500", "\"window_id\":600");
            std::fs::write(&pages.helper, script).unwrap();
            let spec = SurfaceSpec { target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: file.map(str::to_string) }, ..ui5m_spec(true) };
            let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).and_then(|d| d.window.as_ref()).map(|w| w.handle.clone());
            assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| docked(p).is_some()), "Dock が付かない（{file:?}）: {:?}", pages.panes.get("notes").and_then(|p| p.tuck_err.clone()));
            let shown = pages.shown("notes").map(|(t, _)| t.window_id);
            match file {
                None => assert_eq!((shown, docked(&pages).as_deref()), (Some(500), Some("w1")), "app だけの面が主窓を映さない・Dock が主窓に付かない"),
                Some(_) => assert_eq!((shown, docked(&pages).as_deref()), (Some(600), Some("w9")), "書類の面の選んだ窓を主窓にすり替えた"),
            }
        }
    }

    /// B89: 映した窓が本物の窓の一覧にまだ無い（起こした app の窓ができた直後・CG の番号がまだ付かない）は、B14 と同じく `REFIT_GAP_S` 空けて聞き直し、
    /// 2 回目の一覧に在れば Dock を付ける（前は 1 回で `tuck_err` = 二度と聞かず Can't bring … here のまま）。前の窓で置けなかった理由が残ったまま窓が消えて
    /// 探し直した（WindowGone の後の繋ぎ直し）時も、新しく映した窓では Dock をもう 1 度探す。
    /// 負の対照（未測）: `pump_docks` の聞き直しの腕を外す → 前半が赤・`take_answer` の `tuck_err` を捨てる所を外す → 後半が赤
    #[test]
    fn b89_a_window_not_yet_in_the_window_list_is_asked_again_and_a_new_window_starts_fresh() {
        let (bridge, log) = g91_bridge("b89-list", &[("FAKE_NO_CG_ROUNDS", "1")]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "一覧に無かった窓を聞き直さず、Dock が付かない: {:?}", pages.panes.get("notes").and_then(|p| p.tuck_err.clone()));
        assert_eq!(g91_sent(&log, "windows").len(), 2, "聞き直しの数が違う（一覧に無かった後の 1 回だけ）");
        assert!(pages.panes["notes"].tuck_err.is_none() && pages.panes["notes"].refits == 0, "Dock が付いた後も聞き直しの理由か数を持っている");
        {
            let p = pages.panes.get_mut("notes").unwrap();
            (p.dock, p.tuck_err) = (None, Some("TextEdit's window isn't in the window list, so it stays where it is".to_string()));
            p.lost("notes", MirrorError::WindowGone, crate::surface::now_ms());
            p.tried_ms = f64::NEG_INFINITY;
        }
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前の窓の理由が残り、新しく映した窓に Dock を付けない: {:?}", pages.panes.get("notes").and_then(|p| p.tuck_err.clone()));
        assert_eq!(g91_sent(&log, "windows").len(), 3, "新しく映した窓の windows の数が違う（1 回だけ）");
    }

    /// I33（owner 11-03「あとmirrorの枠を自由にshibubuの中で動かせるように手動で。」）: 題の帯を引くと枠ごと動く。引いている間は映像だけ（本物へ set_frame 0）・
    /// 離した後に本物を 1 回だけ新しい枠へ置く（大きさは頼まない）。枠の所は Project へ（`take_frames` の x / y = 並びの所からのずれ・大きさは今のまま）。
    /// 並びの所より左・上へは行かない（前の部品に被せない = 四隅と同じ決まり）。
    /// 負の対照（未測・cargo なし）: `handles` の帯の drag の腕を外す → take_frames が空で赤・離した時に大きさを頼む（`moved` の門を外す）→ 離した後の set_frame が 2 つで赤
    #[test]
    fn i33_dragging_the_title_band_moves_the_frame_and_the_real_window_follows_once_on_release() {
        let (bridge, log) = g91_bridge("i33", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| life_of(p) == Some(Lifecycle::Active)), "前提: しまわない");
        // 呼ぶ側（`source_surface`）と同じく、並びの所 (20, 20) から `shift` だけずらした枠（Page の幅 760）
        let frame = |pages: &mut MirrorPages, time: f64, events: Vec<Event>| {
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(time), focused: true, events, ..Default::default() };
            let vp = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            (vp.inner_rect, vp.focused) = (Some(Rect::from_min_size(G91_HOST, Vec2::new(800.0, 600.0))), Some(true));
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let h = pages.height(ctx, &spec.id, 300.0, 500.0);
                    let shift = pages.shift(&spec.id, Vec2::new(400.0, h), Rect::from_min_max(Pos2::ZERO, pos2(760.0, f32::INFINITY)));
                    pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0) + shift, Vec2::new(400.0, h)), &spec);
                });
            });
            pages.end_frame();
        };
        let settle = |pages: &mut MirrorPages, time: &mut f64| {
            for _ in 0..30 {
                *time += 0.05;
                frame(pages, *time, vec![]);
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(300));
        };
        settle(&mut pages, &mut time);
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        // 帯の題の所を押したまま右へ 50・下へ 40（離す前）
        let pull = |pages: &mut MirrorPages, time: &mut f64, from: Pos2, by: Vec2| {
            *time += 0.016;
            frame(pages, *time, vec![Event::PointerMoved(from), btn(from, true)]);
            for i in 1..=5 {
                *time += 0.016;
                frame(pages, *time, vec![Event::PointerMoved(from + by * (i as f32 / 5.0))]);
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let before = g91_sent(&log, "set_frame").len();
        pull(&mut pages, &mut time, UI5M_TITLE, Vec2::new(50.0, 40.0));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(g91_sent(&log, "set_frame").len(), before, "引いている間に本物を動かした（映像だけが動く）");
        assert!(pages.take_frames().is_empty(), "離す前に Project へ書いた");
        // 離す → 本物を 1 回だけ新しい枠（映像の左上 120 + 50・98 + 40・大きさは今のまま）へ・枠の所を Project へ
        time += 0.016;
        frame(&mut pages, time, vec![btn(UI5M_TITLE + Vec2::new(50.0, 40.0), false)]);
        settle(&mut pages, &mut time);
        let sets: Vec<[f64; 4]> = g91_sent(&log, "set_frame").iter().map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap(), v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]).collect();
        assert_eq!(&sets[before..], &[[170.0, 138.0, 400.0, 272.0]], "離した後に本物を 1 回だけ新しい枠へ置かない（大きさも替えない）");
        assert_eq!(pages.take_frames(), vec![("notes".to_string(), SurfaceFrame { x: 50.0, y: 40.0, w: 400.0, h: 300.0 })], "動かした枠の所を Project へ書かない");
        // 並びの所より左・上へは行かない（前の部品に被せない）
        pull(&mut pages, &mut time, UI5M_TITLE + Vec2::new(50.0, 40.0), Vec2::new(-200.0, -200.0));
        time += 0.016;
        frame(&mut pages, time, vec![btn(UI5M_TITLE - Vec2::new(150.0, 160.0), false)]);
        let back = pages.take_frames();
        assert_eq!(back.iter().map(|(_, f)| (f.x, f.y)).collect::<Vec<_>>(), vec![(0.0, 0.0)], "並びの所より左・上へ動いた: {back:?}");
    }

    /// I33b（operator 11-03「owner の原文は「自由に」」）: 題の帯で枠を Page の列の中のどこへでも置ける（並びの所より左・上 = 前の部品に重ねてよい）。
    /// 置いた所に残り（Project の x / y は負でもよい）、引いている間は本物へ set_frame 0・離して 1 回。重なった枠は最後に動かした方が上（`floating` の順）。
    /// 負の対照（未測・cargo なし）: `shift` の左・上の 0 の止めを戻す → 保存が (0, 0) で赤・`floating` の並べ替えを外す → 覚えた順が other の後でない時に赤
    #[test]
    fn i33b_the_frame_goes_anywhere_in_the_page_over_earlier_parts_and_the_moved_one_is_on_top() {
        let (bridge, log) = g91_bridge("i33b", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| life_of(p) == Some(Lifecycle::Active)), "前提: しまわない");
        // 呼ぶ側（`source_surface` → `render_with`）と同じ: 並びの所 (220, 120)・Page の列 (20, 20)〜(780, 下は無限)・木の後で `floating` の順に描く
        let at = pos2(220.0, 120.0);
        let frame = |pages: &mut MirrorPages, time: f64, events: Vec<Event>| {
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(time), focused: true, events, ..Default::default() };
            let vp = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            (vp.inner_rect, vp.focused) = (Some(Rect::from_min_size(G91_HOST, Vec2::new(800.0, 600.0))), Some(true));
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let h = pages.height(ctx, &spec.id, 300.0, 500.0);
                    let shift = pages.shift(&spec.id, Vec2::new(400.0, h), Rect::from_min_max(pos2(20.0, 20.0) - at.to_vec2(), pos2(780.0 - at.x, f32::INFINITY)));
                    pages.later(&spec.id, Rect::from_min_size(at + shift, Vec2::new(400.0, h)));
                    for (id, rect) in pages.floating() {
                        assert_eq!(id, spec.id);
                        pages.show(ui, rect, &spec);
                    }
                });
            });
            pages.end_frame();
        };
        let settle = |pages: &mut MirrorPages, time: &mut f64| {
            for _ in 0..30 {
                *time += 0.05;
                frame(pages, *time, vec![]);
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(300));
        };
        settle(&mut pages, &mut time);
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let pull = |pages: &mut MirrorPages, time: &mut f64, from: Pos2, by: Vec2| {
            *time += 0.016;
            frame(pages, *time, vec![Event::PointerMoved(from), btn(from, true)]);
            for i in 1..=5 {
                *time += 0.016;
                frame(pages, *time, vec![Event::PointerMoved(from + by * (i as f32 / 5.0))]);
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let sets = || g91_sent(&log, "set_frame").iter().map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap(), v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]).collect::<Vec<[f64; 4]>>();
        // 題の帯（並びの所の枠の題 = UI5M_TITLE + (200, 100)）を左上へ 300 ずつ: Page の列の左上 (20, 20) で止まる = 並びの所から (−200, −100)
        let title = UI5M_TITLE + Vec2::new(200.0, 100.0);
        let before = sets().len();
        pull(&mut pages, &mut time, title, Vec2::new(-300.0, -300.0));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(sets().len(), before, "引いている間に本物を動かした（映像だけが動く）");
        time += 0.016;
        frame(&mut pages, time, vec![btn(title - Vec2::new(300.0, 300.0), false)]);
        settle(&mut pages, &mut time);
        assert_eq!(&sets()[before..], &[[120.0, 98.0, 400.0, 272.0]], "離した後に本物を 1 回だけ新しい枠（Page の列の左上）へ置かない");
        assert_eq!(pages.take_frames(), vec![("notes".to_string(), SurfaceFrame { x: -200.0, y: -100.0, w: 400.0, h: 300.0 })], "並びの所より左・上に置けない（前の部品に重ねられない）");
        // 置いた所に残る（離した後の frame で戻らない・本物も動かない）
        settle(&mut pages, &mut time);
        assert!(pages.take_frames().is_empty() && sets().len() == before + 1, "置いた所から動いた");
        // 右へは Page の列の右端まで（並びの所の幅で止めない）: 枠の左上は 780 − 400 = 380 = 並びの所から +160
        let title = UI5M_TITLE;
        pull(&mut pages, &mut time, title, Vec2::new(600.0, 0.0));
        time += 0.016;
        frame(&mut pages, time, vec![btn(title + Vec2::new(600.0, 0.0), false)]);
        let moved = pages.take_frames();
        assert_eq!(moved.iter().map(|(_, f)| (f.x, f.y)).collect::<Vec<_>>(), vec![(160.0, -100.0)], "Page の列の右端で止まらない: {moved:?}");
        // 重なった時は最後に動かした枠が上（後に描く）: 覚えた順に関わらず notes が最後
        let r = Rect::from_min_size(pos2(20.0, 20.0), Vec2::splat(100.0));
        for notes_first in [true, false] {
            if notes_first {
                pages.later(&spec.id, r);
                pages.later("other", r);
            } else {
                pages.later("other", r);
                pages.later(&spec.id, r);
            }
            let order: Vec<String> = pages.floating().into_iter().map(|(id, _)| id).collect();
            assert_eq!(order, vec!["other".to_string(), spec.id.clone()], "動かした枠が上に来ない（notes_first = {notes_first}）");
            pages.end_frame();
        }
    }

    /// I32（owner 11-03「mirrorの中でappを自由に拡大縮小できるように」）: 拡大の計算。倍率 1 は今の絵のまま・pointer の下の点は拡大しても動かない・
    /// 端まで動かしても絵は映像の外の余白を見せない・倍率は 1〜LENS_MAX（NaN と 0 は無視）。
    /// 負の対照（未測・cargo なし）: 端の寄せ（`apply` の前の clamp）を外す → 右へ 1000 の行が赤・`zoom` の pointer の点の引き戻しを外す → pointer の行が赤
    #[test]
    fn i32_the_lens_keeps_the_point_under_the_pointer_and_never_shows_past_the_picture() {
        let video = Rect::from_min_size(pos2(20.0, 48.0), Vec2::new(400.0, 272.0));
        let content = Rect::from_center_size(video.center(), Vec2::new(400.0, 200.0));
        let near = |a: Rect, b: Rect| (a.min - b.min).length() < 0.01 && (a.max - b.max).length() < 0.01;
        let one = Lens::default();
        assert!(near(one.apply(content, content, video), content), "倍率 1 で絵が替わった");
        // 真ん中で 2 倍 → 真ん中から 2 倍の大きさ
        let mid = one.zoom(2.0, video.center(), content, video);
        assert_eq!(mid.k, 2.0);
        assert!(near(mid.apply(content, content, video), Rect::from_center_size(video.center(), Vec2::new(800.0, 400.0))), "真ん中の 2 倍が真ん中から広がらない");
        // 真ん中から左へ 100 の pointer で 2 倍 → その点の下の本物の点は同じ所のまま（端には当たらない所）
        let q = pos2(120.0, 184.0);
        let left = one.zoom(2.0, q, content, video);
        assert!((left.apply(Rect::from_min_size(q, Vec2::ZERO), content, video).min - q).length() < 0.01, "pointer の下の点が動いた");
        // 右へ 1000 引く → 絵の左の辺は映像の左の辺で止まる（余白を見せない）
        let far = mid.pan(Vec2::new(1000.0, 0.0), content, video).apply(content, content, video);
        assert!((far.min.x - video.min.x).abs() < 0.01, "端を越えて余白を見せた: {far:?}");
        // 倍率の端
        assert_eq!(one.zoom(0.1, video.center(), content, video), one, "1 より小さくした");
        assert_eq!(one.zoom(100.0, video.center(), content, video).k, LENS_MAX, "LENS_MAX より大きくした");
        assert_eq!(mid.zoom(f32::NAN, video.center(), content, video), mid, "NaN で替わった");
        assert_eq!(mid.zoom(0.0, video.center(), content, video), mid, "0 で替わった");
    }

    /// I32: pinch・⌘+ / ⌘- / ⌘0 で映像だけが拡大し、拡大した絵の上の click は本物の窓の同じ所（割合）へ Mirror の道で届く。Shibubu の文字の大きさ
    /// （egui の zoom_factor）は替わらず、本物の大きさの頼み（resize）は 0。拡大の間の wheel は見る所を動かす。Pane が消えても倍率は面に残る。
    /// 負の対照（未測・cargo なし）: `mirror_inputs` に拡大前の `shown_rect` を渡す → click の x が 0.625 で赤・⌘ の key の consume を外す → zoom_factor が替わって赤
    #[test]
    fn i32_pinch_and_cmd_keys_zoom_only_the_video_and_a_click_lands_on_the_same_spot() {
        use crate::surface::mirror::tests::msg;
        // 4×2 の frame（中身は全部）→ 映像 (20, 48)〜(420, 320) の真ん中に 400×200（(20, 84)〜(420, 284)・真ん中 (220, 184)）
        let (sh, dir) = fake_stream_helper("cat {d}/frame.bin; cat >/dev/null");
        let hd = serde_json::to_vec(&serde_json::json!({"seq": 1, "w": 4, "h": 2, "captured_ms": 1.0, "content": {"x": 0.0, "y": 0.0, "w": 2.0, "h": 1.0}, "factor": 2.0})).unwrap();
        let mut payload = (hd.len() as u32).to_le_bytes().to_vec();
        payload.extend(hd);
        payload.extend([200u8; 4 * 2 * 4]);
        std::fs::write(dir.join("frame.bin"), msg(1, payload)).unwrap();
        let te = MirrorTarget { app_name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into(), pid: 42, window_id: 7 };
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let log = dir.join("relay.jsonl");
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&log).unwrap()).unwrap();
        let spec = ui5m_spec(true);
        let mut pages = MirrorPages::new(sh, [], None);
        pages.panes.insert("notes".into(), Pane::new(Ok(m)));
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, 300.0));
        let frame = |pages: &mut MirrorPages, events: Vec<Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    pages.show(ui, rect, &spec);
                });
            });
            pages.end_frame();
        };
        assert!(
            (0..60).any(|_| {
                frame(&mut pages, vec![]);
                std::thread::sleep(Duration::from_millis(50));
                pages.panes["notes"].texture.is_some()
            }),
            "前提: 偽の helper の frame が 3 秒で texture に上がらない"
        );
        let k = |pages: &MirrorPages| pages.windows.get("notes").map_or(1.0, |s| s.lens.k);
        let (mid, at) = (pos2(220.0, 184.0), pos2(270.0, 184.0));
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let cmd = |key: egui::Key| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        let clicks = || RelayLog::read(&log).unwrap_or_default().into_iter().filter_map(|r| r.pos).collect::<Vec<_>>();
        // pinch 2 倍（pointer は真ん中）
        frame(&mut pages, vec![Event::PointerMoved(mid), Event::Zoom(2.0)]);
        assert_eq!(k(&pages), 2.0, "pinch で映像を拡大しない");
        // 真ん中から右へ 50 の click → 拡大前なら x 0.625・2 倍なら 0.5625（本物の大きさは替えない = 割合で送る）
        frame(&mut pages, vec![Event::PointerMoved(at), btn(at, true)]);
        frame(&mut pages, vec![btn(at, false)]);
        assert!(g91_wait(|| !clicks().is_empty()), "拡大した絵の click を本物へ送らない");
        let [x, y] = clicks()[0];
        assert!((x - 0.5625).abs() < 1e-3 && (y - 0.5).abs() < 1e-3, "拡大した絵の click が本物の別の所へ届いた: {x} {y}");
        // ⌘+ / ⌘- は focus の在る面の映像だけ（Shibubu の文字の大きさは替わらない）
        frame(&mut pages, vec![cmd(egui::Key::Plus)]);
        assert!((k(&pages) - 2.0 * LENS_STEP).abs() < 1e-4, "⌘+ で映像を拡大しない: {}", k(&pages));
        assert_eq!(ctx.zoom_factor(), 1.0, "⌘+ で Shibubu の文字まで大きくなった");
        frame(&mut pages, vec![cmd(egui::Key::Minus)]);
        assert!((k(&pages) - 2.0).abs() < 1e-4, "⌘- で戻らない: {}", k(&pages));
        // 拡大の間の wheel は見る所を動かす（本物へは送らない）
        let before = pages.windows["notes"].lens;
        frame(&mut pages, vec![Event::PointerMoved(mid), Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, -40.0), modifiers: egui::Modifiers::NONE }]);
        assert_ne!(pages.windows["notes"].lens.at, before.at, "拡大の間の wheel で見る所が動かない");
        frame(&mut pages, vec![cmd(egui::Key::Num0)]);
        assert_eq!(k(&pages), 1.0, "⌘0 で倍率 1 に戻らない");
        assert_eq!(ctx.zoom_factor(), 1.0, "⌘0 が Shibubu の文字の大きさへ行った");
        assert!(pages.panes["notes"].resize.is_none(), "拡大で本物の大きさを頼んだ");
        // Pane が消えても（Page を離れた）倍率は面に残る
        frame(&mut pages, vec![Event::PointerMoved(mid), Event::Zoom(3.0)]);
        pages.panes.remove("notes");
        assert_eq!(k(&pages), 3.0, "Pane が消えると倍率を忘れた");
    }

    /// B90（owner 11-03「shibubuの中でorca開いているけどずっと揺れて作業に集中できないよ」「揺れたりすることは絶対なし。」・42 回目 30bafd46 の app.log に
    /// 「本物が枠に付いて来るまで」が 1385 行以上・人が触っていない間も本物の Orca が動き続けた）: 人が枠を動かしていない間（Shibubu の窓・Page の wheel・
    /// 引く・押す・⌘ の key が無い）は、枠の矩形が替わっても（layout の揺れ）・本物が自分で大きさを替えても、本物の窓へ set_frame を 1 回も送らない。
    /// Shibubu の窓を動かせば今まで通り追う。動かさなかった枠へは、触った時（`go_direct`）に置いてから前に出す（raise より先に set_frame）。
    /// 負の対照（未測・cargo を回していない）: `tuck` の `follow` を外す（いつも `slot` を Dock へ渡す）→ 揺れの間の set_frame が 0 でなく赤
    #[test]
    fn b90_a_still_page_never_moves_the_real_window() {
        let (bridge, log) = g91_bridge("b90", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        // 枠の上の辺が dy ずれた 1 frame（入力も Shibubu の窓の動きも無い = layout だけが替わった）
        let frame = |pages: &mut MirrorPages, time: f64, dy: f32| {
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(time), focused: true, ..Default::default() };
            let vp = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            (vp.inner_rect, vp.focused) = (Some(Rect::from_min_size(G91_HOST, Vec2::new(800.0, 600.0))), Some(true));
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let h = pages.height(ctx, &spec.id, 300.0, 500.0);
                    pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0 + dy), Vec2::new(400.0, h)), &spec);
                });
            });
            pages.end_frame();
        };
        // 付けた時の合わせ（`FOLLOW_S`）が済むまで同じ枠で回し、届いた要求を数える
        for _ in 0..30 {
            time += 0.05;
            frame(&mut pages, time, 0.0);
        }
        std::thread::sleep(Duration::from_millis(300));
        time += 0.05;
        frame(&mut pages, time, 0.0);
        let before = g91_sent(&log, "set_frame").len();
        // ① 人が触っていない間に枠が 1 point 上下に揺れる（layout）② 本物が自分で高さを 1 point 替える（app・同じ位置の resized）
        let jiggle = |pages: &mut MirrorPages, time: &mut f64| {
            for i in 0..20 {
                *time += 0.05;
                frame(&mut *pages, *time, if i % 2 == 0 { 1.0 } else { 0.0 });
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        jiggle(&mut pages, &mut time);
        {
            let (Some(Ok(b)), Some(p)) = (pages.bridge.as_mut(), pages.panes.get_mut("notes")) else { panic!("前提: bridge と面") };
            let d = p.dock.as_mut().expect("前提: Dock");
            d.on_msg(b, Msg::Event(serde_json::json!({ "event": "resized", "handle": "w1", "frame": { "x": 120.0, "y": 98.0, "w": 400.0, "h": 271.0 } })), Instant::now());
        }
        jiggle(&mut pages, &mut time);
        time += 0.05;
        frame(&mut pages, time, 1.0);
        std::thread::sleep(Duration::from_millis(300));
        let sets = g91_sent(&log, "set_frame");
        assert_eq!(sets.len(), before, "人が動かしていない間に本物の窓を動かした（揺れ）: {:?}", &sets[before.min(sets.len())..]);
        assert_eq!(life_of(&pages), Some(Lifecycle::Active), "前提: 揺れの間に Dock が外れた");
        // 触った → 動かさなかった今の枠（y 99・本物の高さ 271）へ置いてから前に出す
        let raises = g91_sent(&log, "raise").len();
        pages.go_direct("notes");
        assert!(g91_wait(|| g91_sent(&log, "raise").len() > raises), "前提: 触っても本物を前に出さない");
        let cmds: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter_map(|v| v["cmd"].as_str().map(str::to_string)).collect();
        let last = g91_sent(&log, "set_frame").last().map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap(), v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]);
        assert_eq!(last, Some([120.0, 99.0, 400.0, 271.0]), "触った時に今の枠へ置かない");
        assert!(cmds.iter().rposition(|c| c == "set_frame") < cmds.iter().rposition(|c| c == "raise"), "本物を前に出してから置いた（前に出た本物が動く）: {cmds:?}");
        // Shibubu の窓を右へ 30 動かす（人の手）→ 今まで通り追う
        pages.put_back("notes");
        let moved = pos2(G91_HOST.x + 30.0, G91_HOST.y);
        assert!(
            g91_until(&ctx, &mut pages, &spec, &mut time, moved, true, |_| g91_sent(&log, "set_frame").iter().any(|v| v["x"].as_f64() == Some(150.0))),
            "Shibubu の窓を動かしても本物を追わない"
        );
    }

    /// B90 の試験の Dock の今の状態
    fn life_of(pages: &MirrorPages) -> Option<Lifecycle> {
        pages.panes.get("notes").and_then(|p| p.dock.as_ref()).map(|d| d.life)
    }

    /// I31（owner 273「全画面出すようにdefaultで」・274「mirrorの枠とreaperの枠があってない」）: 人が大きさを選んでいない面は、枠より小さい本物（300×200）も
    /// 映像の大きさ（400×272）へ頼む（前は MIRROR-FIT-1101 の「既定では大きさを替えない」で小さいまま映した）・同じ大きさは 1 回だけ・受けたら枠は置ける所の全部。
    /// 大きくできない app（幅 300 まで）は断った大きさを頼み直さず、帯に今の 1 行・枠は本物の大きさに付いて行く。
    /// 負の対照（未測）: `show` の `grow` を外す → 400×272 の set_frame が来ず赤・`frame_size` の置ける所の全部を外す → 受けた後の枠が 400 幅で赤
    #[test]
    fn i31_a_window_smaller_than_the_frame_is_asked_to_take_the_frame_size() {
        for max in [None, Some("300")] {
            let mut env = vec![("FAKE_W", "300"), ("FAKE_H", "200")];
            env.extend(max.map(|m| ("FAKE_MAX_W", m)));
            let (bridge, log) = g91_bridge(&format!("i31-{}", max.unwrap_or("0")), &env);
            let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
            let fills = || g91_sent(&log, "set_frame").iter().filter(|v| v["w"].as_f64() == Some(400.0) && v["h"].as_f64() == Some(272.0)).count();
            assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| fills() > 0 && p.panes["notes"].resize.is_none()), "枠より小さい本物を映像の大きさへ頼まない（{max:?}）");
            for _ in 0..10 {
                time += 0.05;
                g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![]);
            }
            assert_eq!(fills(), 1, "同じ大きさを 2 回以上頼んだ（{max:?}）");
            let (line, frame) = (pages.panes["notes"].band_line.as_ref().map(|(_, l, _)| l.clone()), pages.frame_size("notes", Vec2::new(800.0, 600.0)));
            match max {
                None => assert_eq!((line, frame), (None, Vec2::new(800.0, 600.0)), "受けた本物の面の枠が置ける所の全部でない・帯に断りを出した"),
                Some(_) => assert_eq!(
                    (line.as_deref(), frame),
                    (Some("TextEdit's window didn't take that size · the frame follows the window"), Vec2::new(300.0, 272.0 + BAND_H)),
                    "大きくできない本物で、帯に今の 1 行が無い・枠が本物の大きさに付いて行かない"
                ),
            }
        }
    }

    /// pane-G91 の試験 1: 映ると本物の窓を枠の画面座標へ 1 回 set_frame し、合ったら Shibubu を本物の真上へ（= 本物は見えない）。
    /// Shibubu の窓が動くと追う。対応する窓の無い・許可の無い bridge では Dock を作らず、今の Mirror の映像のまま（聞き直さない）。
    /// 負の対照（commit message）: set_frame を送らない → 赤・order_above を呼ばない → 赤
    #[test]
    fn g91_a_a_mirrored_window_is_tucked_under_shibubu_at_the_frame() {
        static ABOVE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let above: fn(u32) -> Result<(), String> = |cg| {
            ABOVE.lock().unwrap().push(cg);
            Ok(())
        };
        let (bridge, log) = g91_bridge("a", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, above, |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let set = |log: &Path| g91_sent(log, "set_frame").iter().map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap(), v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]).collect::<Vec<_>>();
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |_| !ABOVE.lock().unwrap().is_empty()), "映っても本物を Shibubu の下にしまわない（set_frame {:?}）", set(&log));
        assert_eq!(set(&log).first(), Some(&[120.0, 98.0, 400.0, 272.0]), "本物を映像の矩形の画面座標に置かない");
        assert_eq!(ABOVE.lock().unwrap().first(), Some(&500), "Shibubu を映している窓（500）の真上に置かない");
        assert_eq!(g91_sent(&log, "windows").len(), 1, "窓を毎 frame 聞き直した");
        // Shibubu の窓が右へ 30 動く → 本物も追う
        let moved = pos2(G91_HOST.x + 30.0, G91_HOST.y);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, moved, true, |_| set(&log).iter().any(|f| f[0] == 150.0)), "Shibubu の窓が動いても本物を追わない: {:?}", set(&log));
        // 許可の無い bridge: Dock を作らず映像のまま・windows は 1 回だけ
        let (bridge, log) = g91_bridge("a-untrusted", &[("FAKE_MODE", "untrusted")]);
        let ((mut pages, _dir), ctx, mut time) = (g91_pages(bridge, above, |_| Ok(())), egui::Context::default(), 0.0);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| p.panes.get("notes").is_some_and(|p| p.tuck_err.is_some())), "許可が無いのに理由を持たない");
        for _ in 0..5 {
            time += 0.05;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![]);
        }
        assert_eq!(pages.reach("notes"), Some(Ok(())), "許可が無いと映像まで止めた");
        assert!(g91_sent(&log, "set_frame").is_empty() && g91_sent(&log, "windows").len() <= 1, "許可の無い bridge に窓を動かす要求を送った・聞き直した");
    }

    /// pane-G91 の試験 2: 枠の中の drag → 押した所で本物を前に（MIRROR-HYBRID-1101・raise + activate が 1 回・Shibubu を真下へ・G90 の見張りは捨てる）。
    /// 本物が前に出て Shibubu が前でなくなった後、枠の外の click で Shibubu が前に戻る → 本物を Shibubu の下へ戻す。Page を替えても戻す。
    /// 負の対照（commit message）: 前に出さない → 赤・戻さない → 赤
    #[test]
    fn g91_b_touching_the_frame_brings_the_source_forward_and_clicking_outside_puts_it_back() {
        static ABOVE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let above: fn(u32) -> Result<(), String> = |cg| {
            ABOVE.lock().unwrap().push(cg);
            Ok(())
        };
        let (bridge, log) = g91_bridge("b", &[]);
        // 画面の pointer（偽物）: 枠（画面の (120, 98) 400×272）の中・button は離している
        static POINTER: std::sync::Mutex<([f64; 2], bool)> = std::sync::Mutex::new(([200.0, 200.0], false));
        let pointer: fn() -> (Option<[f64; 2]>, bool) = || {
            let (at, down) = *POINTER.lock().unwrap();
            (Some(at), down)
        };
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, above, |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        (pages.front, pages.pointer) = (|| Some("com.apple.TextEdit".into()), pointer);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |_| !ABOVE.lock().unwrap().is_empty()), "前提: しまわない");
        pages.panes.get_mut("notes").unwrap().reclaim = Some(crate::surface::now_ms());
        // MIRROR-FIT-1101（owner 200）: 選んだ枠の押しから本物へ
        pages.panes.get_mut("notes").unwrap().selected = true;
        // 映像の中の drag（(100, 100) → (200, 150)）
        let (a, b) = (pos2(100.0, 100.0), pos2(200.0, 150.0));
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        for evs in [vec![Event::PointerMoved(a), btn(a, true)], vec![Event::PointerMoved(b)], vec![btn(b, false)]] {
            time += 0.016;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |_| !g91_sent(&log, "raise").is_empty()), "枠を押して drag しても本物を前に出さない");
        let raise = g91_sent(&log, "raise");
        assert_eq!((raise.len(), raise[0]["activate"].as_bool()), (1, Some(true)), "本物を前に出す（activate つき）のが 1 回でない: {raise:?}");
        assert!(pages.panes["notes"].direct.is_some() && pages.panes["notes"].reclaim.is_none(), "直接触る間に G90 の見張りが残った（前に出した本物を Shibubu へ引き戻す）");
        // 本物が前（Shibubu は前でない）→ Shibubu へ戻さない（G90 の Focus を出さない）
        let n = ABOVE.lock().unwrap().len();
        time += 0.05;
        let out = g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert!(!out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Focus))), "触るために前に出した本物から Shibubu へ前面を戻した");
        assert_eq!(ABOVE.lock().unwrap().len(), n, "本物が前の間に Shibubu を上へ戻した");
        // 枠の外の click で Shibubu が前に戻る → 本物を下へ
        time += 0.05;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![]);
        assert!(ABOVE.lock().unwrap().len() > n && pages.panes["notes"].direct.is_none(), "Shibubu が前に戻っても本物を下へ戻さない");
        // もう一度前に出し、Page を替える（この面を描かない frame）→ 戻す
        pages.go_direct("notes");
        let n = ABOVE.lock().unwrap().len();
        pages.end_frame();
        assert!(ABOVE.lock().unwrap().len() > n && pages.panes["notes"].direct.is_none(), "Page を替えても本物を前に残した");
    }

    /// G91 改（owner 19:06）の hover で前に出す・枠の外へ離れたら戻すは MIRROR-HYBRID-1101 でやめた（`mirror_hybrid_*` の試験）。ここに残すのは
    /// 「本物を触っている間は pointer が枠の外へ出ても・drag の途中でも戻さない」だけ
    #[test]
    fn g91_b2_leaving_the_frame_while_touching_does_not_put_the_source_back() {
        static ABOVE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let above: fn(u32) -> Result<(), String> = |cg| {
            ABOVE.lock().unwrap().push(cg);
            Ok(())
        };
        static POINTER: std::sync::Mutex<([f64; 2], bool)> = std::sync::Mutex::new(([700.0, 500.0], true));
        let pointer: fn() -> (Option<[f64; 2]>, bool) = || {
            let (at, down) = *POINTER.lock().unwrap();
            (Some(at), down)
        };
        let spec = ui5m_spec(true);
        // 負の対照（commit message）: tuck に「枠の外へ離れたら戻す」を戻す → 赤
        let (bridge, _log) = g91_bridge("b-leave", &[]);
        let ((mut pages, _dir), ctx, mut time) = (g91_pages(bridge, above, |_| Ok(())), egui::Context::default(), 0.0);
        pages.pointer = pointer;
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        pages.go_direct("notes");
        // 本物が前（Shibubu は前でない）・button を押したまま枠の外（drag 中）→ 戻さない・離して枠の外に居続けても戻さない
        let n = ABOVE.lock().unwrap().len();
        for down in [true, false] {
            *POINTER.lock().unwrap() = ([700.0, 500.0], down);
            for _ in 0..6 {
                time += 0.1;
                g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
            }
            assert!(ABOVE.lock().unwrap().len() == n && pages.panes["notes"].direct.is_some(), "触っている間に pointer が枠の外へ出ただけで本物を下へ戻した（button {down}）");
        }
    }

    /// g91_frame と同じ 1 frame に、枠の上に被さる Area（menu・popup の形・`at` の矩形）を足した物
    fn hy_frame(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, time: f64, focused: bool, at: Rect) {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(time), focused, ..Default::default() };
        let vp = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        (vp.inner_rect, vp.focused) = (Some(Rect::from_min_size(G91_HOST, Vec2::new(800.0, 600.0))), Some(focused));
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let h = pages.height(ctx, &spec.id, 300.0, 500.0);
                pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, h)), spec);
            });
            egui::Area::new(egui::Id::new("hy-cover")).fixed_pos(at.min).show(ctx, |ui| {
                ui.allocate_space(at.size());
            });
        });
        pages.end_frame();
    }

    /// MIRROR-HYBRID-1101（owner 207「両ドリ」）: 選んでいない枠でも、押した frame で本物を前に（raise 1 回・離す前）・離した押しは本物へ `front` で 1 回だけ
    /// （Mirror の道には乗せない = 2 回届けない）。映像の上を動いているだけでは出さない（止まった時は `mirror_hybrid_235_*`）。
    /// 負の対照（commit message）: 選ぶ押しを送らない形に戻す → front の行が無く赤・押した frame で go_direct しない → raise 0 で赤
    #[test]
    fn mirror_hybrid_the_first_press_raises_the_real_window_and_resends_it_there() {
        let (bridge, log) = g91_bridge("hy-press", &[]);
        let ((mut pages, dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let raises = || g91_sent(&log, "raise").len();
        let front = || std::fs::read_to_string(dir.join("stdin")).unwrap_or_default().lines().filter(|l| l.contains("\"route\":\"front\"")).count();
        let at = pos2(220.0, 184.0);
        // 映像の上を動き続ける（DWELL_S 止まらない）→ 出さない
        for dx in [0.0, 6.0, 12.0] {
            time += 0.1;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![Event::PointerMoved(at + Vec2::new(dx, 0.0))]);
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(raises(), 0, "止まっていない pointer で本物を前に出した");
        assert!(!pages.panes["notes"].selected, "前提: 選んでいない枠");
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        time += 0.016;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![Event::PointerMoved(at), btn(at, true)]);
        assert!(g91_wait(|| raises() == 1), "押しても本物を前に出さない（raise {}）", raises());
        assert!(pages.panes["notes"].direct.is_some() && pages.panes["notes"].selected, "押した枠が本物の間でない・選ばれていない");
        time += 0.016;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![btn(at, false)]);
        assert!(g91_wait(|| front() == 1), "離した押しを本物へ front で送り直さない（front {}）", front());
        let recs = RelayLog::read(&pages.relay_log).unwrap();
        assert!(recs.iter().all(|r| r.route == crate::surface::mirror::InputRoute::Front), "触った押しを Mirror の道でも送った（2 回届く）: {recs:?}");
        assert_eq!(raises(), 1, "離した時にもう一度 raise した");
    }

    /// MIRROR-HYBRID-1101 追記 214（owner「garagebadをmirrorの中で触ると光るんだけど何も起きないし」）: 17 回目の GarageBand は窓 1680×956 が枠より大きく、
    /// 触る道に入らず（理由は起動の間 1 回の 6 秒の知らせだけ）入力は Mirror の道のまま（relay の記録の 15 押しが全部 ax_at_point・front 0）・光ったのは選んだ枠の縁。
    /// 追記 235 の 1 から: 枠より大きい本物は押す前に映像の大きさに（set_frame 1 回）→ 押した frame で前に出す（raise 1 回・本体からも activate・押した時に大きさを
    /// 替えない）→ Shibubu が前のまま FRONT_WAIT_MS → 下へ戻して理由。本物が縮まない（最小の幅 600）→ 押す前の 1 回は断られ（I31 から帯に理由の 1 行）、押した時にもう 1 回頼み（214）、
    /// 縮まなければ前に出さず理由を右上に 1 行。
    /// 負の対照（commit message）: 押しの枝の `None if scaled => fit = true` を外す → set_frame が来ず赤・`fitted` の `go_direct` を外す → raise が増えず赤・
    /// 1 秒の戻しを外す → direct が残って赤・`go_direct` の本体の activate を外す → activate 0 で赤
    #[test]
    fn mirror_hybrid_214_a_window_bigger_than_the_frame_is_fit_then_brought_front() {
        static ACTIVATED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        // 右上の知らせの文（`toast::draw` を 1 frame 描いて AX の木から読む）
        let toasts = |ctx: &egui::Context| -> Vec<String> {
            let out = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), ..Default::default() }, |ctx| {
                let _ = crate::ui::toast::draw(ctx, 0.0);
            });
            out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect()
        };
        for min in [None, Some("600")] {
            let mut env = vec![("FAKE_W", "900"), ("FAKE_H", "700")];
            env.extend(min.map(|m| ("FAKE_MIN_W", m)));
            let (bridge, log) = g91_bridge(&format!("hy-214-{}", min.unwrap_or("0")), &env);
            let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
            ctx.enable_accesskit();
            pages.activate = |b| {
                ACTIVATED.lock().unwrap().push(b.into());
                Ok(())
            };
            let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
            assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
            let raises = || g91_sent(&log, "raise").len();
            let fits = || g91_sent(&log, "set_frame").iter().filter(|v| v["w"].as_f64() == Some(400.0)).count();
            // 追記 235 の 1: 押す前に本物を映像の大きさへ（400 幅の set_frame・答えまで待つ）
            assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| fits() > 0 && p.panes["notes"].resize.is_none()), "枠より大きい本物を押す前に映像の大きさにしない");
            let (r0, n0, f0) = (raises(), ACTIVATED.lock().unwrap().len(), fits());
            let at = pos2(220.0, 184.0);
            let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            for evs in [vec![Event::PointerMoved(at), btn(at, true)], vec![btn(at, false)]] {
                time += 0.016;
                g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
            }
            if min.is_none() {
                assert_eq!(fits(), f0, "押す前に合わせた本物を押した時にもう一度大きさを替えた");
                assert!(g91_wait(|| raises() == r0 + 1), "枠に合った本物を前に出さない（raise {}）", raises());
                assert!(ACTIVATED.lock().unwrap()[n0..].iter().any(|b| b == "com.apple.TextEdit"), "本体から本物の app を前に出さない");
                // Shibubu が前のまま（本物が前に来ない = 系が断った）→ FRONT_WAIT_MS で下へ戻して理由
                assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| p.panes["notes"].direct.is_none()), "前に来ない本物を前に出した事にし続けた");
                assert!(toasts(&ctx).iter().any(|l| l == "TextEdit didn't come to the front · it stays a mirror"), "前に来なかった理由が無い: {:?}", toasts(&ctx));
            } else {
                assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| fits() > f0 && p.panes["notes"].fit_touch.is_none()), "縮まない本物を押した時に頼み直さない・答えを待ち続けた");
                assert_eq!(raises(), r0, "縮まない本物を前に出した");
                assert!(pages.panes["notes"].direct.is_none(), "縮まない本物の面を前に出した間にした");
                assert!(toasts(&ctx).iter().any(|l| l == "TextEdit's window won't shrink to this frame · it stays a mirror"), "縮まない理由が無い: {:?}", toasts(&ctx));
            }
        }
    }

    /// MIRROR-HYBRID-1101 追記 235 の 2（owner「mirrorの中で操作するとdelayがある これ限りなく0にして」）: pointer が映像の上で DWELL_S 止まったら押す前に本物を前へ
    /// （raise 1 回・送り直す押しは無い = relay の記録 0 行・最初の押しから本物に直に届く）。DWELL_S に足りない間は出さない。10-27 の例外: button を押している間・
    /// 離しただけ（pointer が動いていない）では出さない。前に出した後に戻した（何もしない 10 秒と同じ put_back）所で pointer が止まったままなら出し直さない。
    /// 負の対照（未実行）: dwell の枝を外す → raise 0 で赤・`DWELL_S` を見ずにすぐ出す → 0.1 秒で raise して赤・go_direct で止まりを出した印にしない → 戻した後に 2 回目の raise で赤
    #[test]
    fn mirror_hybrid_235_resting_on_the_frame_brings_the_real_window_forward_before_the_press() {
        let (bridge, log) = g91_bridge("hy-235", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let raises = || g91_sent(&log, "raise").len();
        let step = |pages: &mut MirrorPages, time: &mut f64, dt: f64, evs: Vec<Event>| {
            *time += dt;
            g91_frame(&ctx, pages, &spec, *time, G91_HOST, true, evs);
        };
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        // 面 (20, 20) 400×300 の映像の中の 2 点・外の 1 点
        let (a, b, out) = (pos2(200.0, 180.0), pos2(230.0, 190.0), pos2(600.0, 500.0));
        // 枠の外で押したまま映像の上へ来て止まる → 出さない・離しただけ（動いていない）でも出さない
        step(&mut pages, &mut time, 0.016, vec![Event::PointerMoved(out), btn(out, true)]);
        step(&mut pages, &mut time, 0.016, vec![Event::PointerMoved(a)]);
        step(&mut pages, &mut time, 0.1, vec![]);
        step(&mut pages, &mut time, 0.1, vec![]);
        step(&mut pages, &mut time, 0.016, vec![btn(a, false)]);
        step(&mut pages, &mut time, 0.2, vec![]);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(raises(), 0, "button を押している間・離しただけで本物を前に出した");
        // 動いて止まる: DWELL_S に足りない間は出さない・足りたら出す
        step(&mut pages, &mut time, 0.016, vec![Event::PointerMoved(b)]);
        step(&mut pages, &mut time, 0.1, vec![]);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(raises(), 0, "止まって 0.1 秒で本物を前に出した");
        step(&mut pages, &mut time, 0.1, vec![]);
        assert!(g91_wait(|| raises() == 1), "映像の上で止まっても本物を前に出さない（raise {}）", raises());
        assert!(pages.panes["notes"].direct.is_some(), "前に出した面が本物の間でない");
        assert!(RelayLog::read(&pages.relay_log).unwrap().is_empty(), "押していないのに本物へ押しを送った");
        // 戻した後（何もしない 10 秒と同じ put_back）・pointer は止まったまま → 出し直さない
        pages.put_back("notes");
        for _ in 0..3 {
            step(&mut pages, &mut time, 0.1, vec![]);
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(raises(), 1, "戻した後に同じ所で止まったままの pointer で本物を前に出し直した");
    }

    /// MIRROR-HYBRID-1101: 本物を触っている間に ① Page の wheel（枠の外 = Shibubu に来た）→ 本物を下へ・枠が SETTLE_S 止まったら本物に戻す（raise がもう 1 回・押しの送り直しは無い）
    /// ② 枠に Area（menu・popup）が被った → 下へ（戻す印は置かない）③ 人が IDLE_S 何もしない → 下へ。
    /// 負の対照（commit message）: wheel を見ない → ① で赤・resume を置かない → raise が増えず赤・covered が常に false → ② で赤・idle を見ない → ③ で赤
    #[test]
    fn mirror_hybrid_scroll_cover_and_idle_put_the_real_window_back() {
        static ABOVE: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
        static IDLE: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);
        let above: fn(u32) -> Result<(), String> = |_| {
            *ABOVE.lock().unwrap() += 1;
            Ok(())
        };
        let (bridge, log) = g91_bridge("hy-back", &[]);
        let ((mut pages, dir), ctx, spec, mut time) = (g91_pages(bridge, above, |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        pages.idle = || *IDLE.lock().unwrap();
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let raises = || g91_sent(&log, "raise").len();
        let front = || std::fs::read_to_string(dir.join("stdin")).unwrap_or_default().lines().filter(|l| l.contains("\"route\":\"front\"")).count();
        let at = pos2(220.0, 184.0);
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        // 押して離す → 本物が前に来た（Shibubu は前でない）
        let touch = |pages: &mut MirrorPages, time: &mut f64| {
            for (focused, evs) in [(true, vec![Event::PointerMoved(at), btn(at, true)]), (true, vec![btn(at, false)]), (false, vec![])] {
                *time += 0.05;
                g91_frame(&ctx, pages, &spec, *time, G91_HOST, focused, evs);
            }
        };
        touch(&mut pages, &mut time);
        assert!(pages.panes["notes"].direct.is_some(), "前提: 押しても本物の間にならない");
        // ① 枠の外の wheel（Page の scroll）→ 下へ・止まったら戻す
        let (n, r) = (*ABOVE.lock().unwrap(), raises());
        let wheel = Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, -40.0), modifiers: egui::Modifiers::NONE };
        time += 0.016;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![Event::PointerMoved(pos2(600.0, 450.0)), wheel]);
        assert!(*ABOVE.lock().unwrap() > n && pages.panes["notes"].direct.is_none(), "Page の wheel で本物を下へ戻さない");
        assert!(pages.panes["notes"].resume.is_some(), "wheel で戻した後に、止まったら戻す印が無い");
        for _ in 0..8 {
            time += 0.05;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        }
        assert!(g91_wait(|| raises() == r + 1), "枠が止まっても本物に戻さない（raise {}）", raises());
        assert_eq!(front(), 1, "止まって戻した時に押しを送り直した");
        time += 0.05;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert!(pages.panes["notes"].direct.is_some(), "前提: 本物に戻っている");
        // ② 枠に menu の Area が被る（矩形は前の frame の物 = 2 frame）→ 下へ・戻す印は置かない
        let n = *ABOVE.lock().unwrap();
        for _ in 0..2 {
            time += 0.05;
            hy_frame(&ctx, &mut pages, &spec, time, false, Rect::from_min_size(pos2(150.0, 150.0), Vec2::splat(80.0)));
        }
        assert!(*ABOVE.lock().unwrap() > n && pages.panes["notes"].direct.is_none(), "枠に menu が被っても本物を下へ戻さない");
        assert!(pages.panes["notes"].resume.is_none(), "被さって戻した所で、また本物に戻す印を置いた");
        // ③ Area が消えてから触り直す → IDLE_S の手前では戻さない・超えたら下へ
        for _ in 0..2 {
            time += 0.05;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![]);
        }
        touch(&mut pages, &mut time);
        assert!(pages.panes["notes"].direct.is_some(), "前提: 触り直しても本物の間にならない");
        let n = *ABOVE.lock().unwrap();
        *IDLE.lock().unwrap() = IDLE_S - 1.0;
        time += 0.1;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert!(pages.panes["notes"].direct.is_some(), "IDLE_S の前に本物を下へ戻した");
        *IDLE.lock().unwrap() = IDLE_S + 0.5;
        time += 0.1;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert!(*ABOVE.lock().unwrap() > n && pages.panes["notes"].direct.is_none(), "IDLE_S 何もしなくても本物を下へ戻さない");
        *IDLE.lock().unwrap() = 0.0;
    }

    /// owner 10-30「shibubuを前面に出したところからorcaを押すとmirror先のtexteditだけが浮き出てしまいます」: 本物を前に出した後に人が別の app（Orca）を押し、
    /// その窓が Shibubu の矩形に重なる所に pointer が在っても、本物を下へ戻すだけで前面を Shibubu へ取り返さない（取り返しは背景の起動で一番後ろへの `orderBack:` になり、
    /// 枠の下にしまった本物だけが見えた）。前面が映している app のまま Shibubu の上へ離れた時は今まで通り Shibubu を前に。
    /// 負の対照（commit message）: 前面の app を見ない（`&& (self.front)()…` を外す）→ Orca の場面で Focus を出して赤
    #[test]
    fn g91_f_clicking_another_app_puts_the_source_back_without_taking_the_front() {
        static ABOVE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        static FRONT: std::sync::Mutex<&str> = std::sync::Mutex::new("com.apple.TextEdit");
        let above: fn(u32) -> Result<(), String> = |cg| {
            ABOVE.lock().unwrap().push(cg);
            Ok(())
        };
        let (bridge, _log) = g91_bridge("f", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, above, |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        // MIRROR-HYBRID-1101: 本物を下へ戻す合図は人が IDLE_S 何もしない（枠の外へ離れたら戻すはやめた）。前面を取り返すかどうかはどの戻し方でも同じ
        pages.idle = || IDLE_S + 1.0;
        // 画面の pointer は枠（画面の (120, 98) 400×272）の外・Shibubu の窓（画面の (100, 50) 800×600）の中 = 重なった Orca の窓の上
        (pages.front, pages.pointer) = (|| Some(FRONT.lock().unwrap().to_string()), || (Some([700.0, 500.0]), false));
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let focus = |out: &egui::FullOutput| out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Focus)));
        for (front, want_focus) in [("com.stablyai.orca", false), ("com.apple.TextEdit", true)] {
            pages.go_direct("notes");
            *FRONT.lock().unwrap() = front;
            let (n, mut asked) = (ABOVE.lock().unwrap().len(), false);
            for _ in 0..6 {
                time += 0.1;
                asked |= focus(&g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]));
            }
            assert!(ABOVE.lock().unwrap().len() > n && pages.panes["notes"].direct.is_none(), "{front} が前・何もしなくても本物を下へ戻さない");
            assert_eq!(asked, want_focus, "前面が {front} の時に Shibubu へ前面を取り返す（Focus）の有無が違う");
        }
    }

    /// pane-G91 の試験 3: 黄（Minimize mirror）→ 本物の窓も最小化が 1 回・開くと解く。赤（Close mirror）→ 本物を Shibubu が動かす前の位置・大きさへ。
    /// Shibubu の終了（`release_sources`）も元の位置へ。負の対照（commit message）: 最小化しない → 赤・元に戻さない → 赤
    #[test]
    fn g91_c_yellow_minimizes_the_source_and_red_or_quit_puts_it_where_it_was() {
        let (bridge, log) = g91_bridge("c", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let mins = |log: &Path| g91_sent(log, "minimize").iter().map(|v| v["minimized"].as_bool()).collect::<Vec<_>>();
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_YELLOW);
        g91_wait(|| !mins(&log).is_empty());
        assert_eq!(mins(&log), vec![Some(true)], "黄で本物を最小化しない");
        time += 1.0;
        ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_TITLE);
        g91_wait(|| mins(&log).len() > 1);
        assert_eq!(mins(&log), vec![Some(true), Some(false)], "開いても本物の最小化を解かない");
        // 赤 → 元の位置（偽の窓の最初の frame = 100, 100, 800×600）
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "開いた後にしまい直さない");
        let last = |log: &Path| g91_sent(log, "set_frame").last().map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap(), v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]);
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_RED);
        assert_eq!(last(&log), Some([100.0, 100.0, 800.0, 600.0]), "赤で本物を元の位置へ戻さない");
        assert!(pages.panes.get("notes").is_none_or(|p| p.dock.is_none()), "赤の後も Dock が残った");
        // 終了: 開き直してしまった後に release_sources → 元の位置
        time += 1.0;
        ui5m_frame(&ctx, &mut pages, &spec, time, true, vec![]);
        ui5m_click(&ctx, &mut pages, &spec, &mut time, UI5M_TITLE);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "閉じた後に開き直してもしまわない");
        pages.release_sources();
        assert_eq!(last(&log), Some([100.0, 100.0, 800.0, 600.0]), "Shibubu の終了で本物を元の位置へ戻さない");
    }

    /// L5-STICK（owner 109 通目 10）: Shibubu の窓を最小化すると本物の窓も最小化し、戻すと本物も戻す（独りで画面に残らない）。黄で自分から最小化した面は触らない。
    /// 負の対照: `show` の host_min の腕を外す → 最小化の要求が 0 回で赤
    #[test]
    fn l5_stick_minimizing_shibubu_minimizes_the_source_with_it() {
        let (bridge, log) = g91_bridge("l5-min", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let mins = |log: &Path| g91_sent(log, "minimize").iter().map(|v| v["minimized"].as_bool()).collect::<Vec<_>>();
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, true, false);
        assert!(g91_wait(|| !mins(&log).is_empty()), "Shibubu を最小化しても本物を最小化しない");
        assert_eq!(mins(&log), vec![Some(true)]);
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, true, false);
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, false, true);
        assert!(g91_wait(|| mins(&log).len() > 1), "Shibubu を戻しても本物を戻さない");
        assert_eq!(mins(&log), vec![Some(true), Some(false)], "同じ状態の frame で何度も送った・戻さなかった");
    }

    /// L5-STICK の 1 frame（`g91_frame` と同じ面）: Shibubu の窓が最小化されているか・前か（viewport の minimized / focused）
    fn l5_frame(ctx: &egui::Context, pages: &mut MirrorPages, spec: &SurfaceSpec, time: f64, minimized: bool, focused: bool) -> egui::FullOutput {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(time), focused, ..Default::default() };
        let vp = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        (vp.inner_rect, vp.focused, vp.minimized) = (Some(Rect::from_min_size(G91_HOST, Vec2::new(800.0, 600.0))), Some(focused), Some(minimized));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let h = pages.height(ctx, &spec.id, 300.0, 500.0);
                pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0), Vec2::new(400.0, h)), spec);
            });
        });
        pages.end_frame();
        out
    }

    /// bridge から出来事が 1 つ届いた事にする（面 "notes" の Dock へ・偽の bridge は自分から出来事を出さない）
    fn l5_event(pages: &mut MirrorPages, ev: serde_json::Value) {
        let MirrorPages { panes, bridge, .. } = pages;
        let (Some(Ok(b)), Some(d)) = (bridge.as_mut(), panes.get_mut("notes").and_then(|p| p.dock.as_mut())) else { panic!("前提: notes の面に Dock が無い") };
        d.on_msg(b, Msg::Event(ev), Instant::now());
    }

    /// L5-STICK（M-02）: 本物を最小化すると helper が 2 秒で stream を止める（window_hidden = 映像が無い）。止まった後に Shibubu を戻しても本物を戻す・
    /// 最小化の間は探し直さない・戻した後は「探しています」から。
    /// 負の対照: 最小化の腕を `live` の中へ戻す（b5708b2b の形）→ 戻す要求が 0 回で赤
    #[test]
    fn l5_stick_restoring_shibubu_after_the_stream_stopped_restores_the_source() {
        let (bridge, log) = g91_bridge("l5-restore", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let mins = |log: &Path| g91_sent(log, "minimize").iter().map(|v| v["minimized"].as_bool()).collect::<Vec<_>>();
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, true, false);
        assert!(g91_wait(|| !mins(&log).is_empty()), "前提: Shibubu を最小化しても本物を最小化しない");
        // helper が最小化した窓の stream を止めた・探し直しの間が過ぎた
        let p = pages.panes.get_mut("notes").unwrap();
        (p.surface, p.tried_ms) = (Err(MirrorError::WindowHidden { app_name: "TextEdit".into() }), f64::NEG_INFINITY);
        for _ in 0..3 {
            time += 0.1;
            l5_frame(&ctx, &mut pages, &spec, time, true, false);
        }
        assert!(pages.panes["notes"].pending.is_none(), "Shibubu と一緒に最小化している間に窓を探し直した");
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, false, true);
        assert!(g91_wait(|| mins(&log).len() > 1), "stream が止まった後に Shibubu を戻しても本物を戻さない");
        assert_eq!(mins(&log), vec![Some(true), Some(false)]);
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, false, true);
        assert!(!pages.panes["notes"].answered, "戻した直後に前の「隠れた」のまま（探していますでない）");
    }

    /// L5-STICK（M-02）: ⌘H（bridge の app_hidden・egui には合図が無い）でも本物を最小化し、戻ると（app_unhidden か、前に来た frame）戻す。
    /// 他の app が隠れても何もしない・app_unhidden を取りこぼしても、前に来た後に前でなくなる度に最小化しない。
    /// 負の対照: `hidden` を host_min に足さない → 最小化の要求 0 回で赤
    #[test]
    fn l5_stick_hiding_shibubu_minimizes_the_source_and_showing_it_restores_it() {
        let (bridge, log) = g91_bridge("l5-hide", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let mins = |log: &Path| g91_sent(log, "minimize").iter().map(|v| v["minimized"].as_bool()).collect::<Vec<_>>();
        let host = std::process::id();
        // 他の app が隠れた → 何もしない
        l5_event(&mut pages, serde_json::json!({ "event": "app_hidden", "pid": 333 }));
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, false, false);
        std::thread::sleep(Duration::from_millis(300));
        assert!(mins(&log).is_empty(), "他の app が隠れただけで本物を最小化した");
        l5_event(&mut pages, serde_json::json!({ "event": "app_hidden", "pid": host }));
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, false, false);
        assert!(g91_wait(|| !mins(&log).is_empty()), "Shibubu を隠しても（⌘H）本物を最小化しない");
        // 戻った（app_unhidden より先に前に来た frame）→ 戻す
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, false, true);
        assert!(g91_wait(|| mins(&log).len() > 1), "隠した Shibubu を戻しても本物を戻さない");
        // app_unhidden を取りこぼしたまま、別の app へ（前でない）→ 最小化しない
        time += 0.1;
        l5_frame(&ctx, &mut pages, &spec, time, false, false);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(mins(&log), vec![Some(true), Some(false)], "前に来た後も隠れた事にして、前でなくなっただけで本物を最小化した");
    }

    /// L5-STICK ①（owner 109 通目 10「別の app へ切り替えると、映している本物の窓も一緒に後ろへ」・M-01 Cmd+Tab）: 直接触っている間に別の app が前に来たら、
    /// 画面の pointer が枠の上に残っていても（Cmd+Tab）待たずに本物を Shibubu の下へ。前に来たのが映している app 自身（前に出した結果）なら戻さない。
    /// 負の対照: `third` を `back` に足さない → pointer が枠の上の間は戻らず赤
    #[test]
    fn l5_stick_switching_to_another_app_puts_the_source_back_with_shibubu() {
        static ABOVE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let above: fn(u32) -> Result<(), String> = |cg| {
            ABOVE.lock().unwrap().push(cg);
            Ok(())
        };
        let (bridge, _log) = g91_bridge("l5-switch", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, above, |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        // 画面の pointer は枠（画面の (120, 98) 400×272）の中 = Cmd+Tab で app を替えても pointer は動かない
        (pages.front, pages.pointer) = (|| Some("com.stablyai.orca".into()), || (Some([200.0, 200.0]), false));
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        pages.go_direct("notes");
        // 前に出した本物の app（pid 42）が前に来た → 直接触っている間のまま
        l5_event(&mut pages, serde_json::json!({ "event": "app_activated", "pid": 42 }));
        for _ in 0..4 {
            time += 0.1;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        }
        assert!(pages.panes["notes"].direct.is_some(), "前に出した本物が前に来ただけで下へ戻した");
        let n = ABOVE.lock().unwrap().len();
        // 別の app（Orca・pid 333）が前に来た → 本物を Shibubu の下へ・前面は取り返さない
        l5_event(&mut pages, serde_json::json!({ "event": "app_activated", "pid": 333 }));
        time += 0.1;
        let out = g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert!(ABOVE.lock().unwrap().len() > n && pages.panes["notes"].direct.is_none(), "別の app が前に来ても、pointer が枠の上だと本物を Shibubu の下へ戻さない");
        assert!(!out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Focus))), "別の app から前面を取り返した");
    }

    /// L5-STICK ①（operator の音楽の通し 10-31）: Shibubu が既に本物より手前（間に別の窓 = もう 1 つの Mirror の本物・REAPER の本体）の時、
    /// しまった直後も Shibubu が前に来た時も Shibubu を本物の直上の段へ動かさない（動かすと下の本物の直上へ沈み、間の窓が Shibubu の上に出る）。
    /// 負の対照: `tuck` の `d.tuck = pane.direct.is_none()` を false に → keep_front が前後を読まずに Shibubu を動かして（order_below）赤
    #[test]
    fn l5_stick_shibubu_already_in_front_is_not_moved_down_onto_the_source() {
        static ABOVE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        static BELOW: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let above: fn(u32) -> Result<(), String> = |cg| {
            ABOVE.lock().unwrap().push(cg);
            Ok(())
        };
        let below: fn(u32) -> Result<(), String> = |cg| {
            BELOW.lock().unwrap().push(cg);
            Ok(())
        };
        let host = std::process::id().to_string();
        let (bridge, log) = g91_bridge("l5-front", &[("FAKE_HOST_ABOVE", "1"), ("FAKE_BETWEEN", "1"), ("FAKE_HOST_PID", host.as_str())]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, above, below), egui::Context::default(), ui5m_spec(true), 0.0);
        let checks = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).filter(|d| d.life == crate::surface::Lifecycle::Active).map_or(0, |d| d.metrics.z_order_checks);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| checks(p) > 0), "前提: しまった後に前後を読まない");
        let n = checks(&pages);
        l5_event(&mut pages, serde_json::json!({ "event": "app_activated", "pid": std::process::id() }));
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| checks(p) > n), "Shibubu が前に来た後に前後を読まない");
        assert!(ABOVE.lock().unwrap().is_empty(), "Shibubu が既に本物より手前なのに本物の直上の段へ動かした: {:?}", ABOVE.lock().unwrap());
        assert!(BELOW.lock().unwrap().is_empty(), "しまっている間に Shibubu を本物の真下へ置いた（本物が Shibubu の上に出る）: {:?}", BELOW.lock().unwrap());
        assert!(g91_sent(&log, "raise").is_empty(), "しまっている本物を AXRaise で Shibubu の上へ出した");
    }

    /// L5-STICK ①: しまっている間に映している app が前に来た（Cmd+Tab・Dock の icon・Open original）→ 本物だけが他の app の上に浮かないよう、
    /// Shibubu を本物の真下へ置いて直接触る間に（帯は Using … directly）。別の app・Shibubu 自身が前に来ただけなら何もしない。
    /// 負の対照: しまっている間の `came == Some(src)` の腕を外す → order_below が 0 回で赤
    #[test]
    fn l5_stick_the_source_coming_to_the_front_lifts_shibubu_under_it() {
        static BELOW: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let below: fn(u32) -> Result<(), String> = |cg| {
            BELOW.lock().unwrap().push(cg);
            Ok(())
        };
        let (bridge, _log) = g91_bridge("l5-lift", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), below), egui::Context::default(), ui5m_spec(true), 0.0);
        pages.pointer = || (Some([200.0, 200.0]), false);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == crate::surface::Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        for pid in [333, std::process::id()] {
            l5_event(&mut pages, serde_json::json!({ "event": "app_activated", "pid": pid }));
            time += 0.1;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, pid != 333, vec![]);
        }
        assert!(BELOW.lock().unwrap().is_empty() && pages.panes["notes"].direct.is_none(), "別の app・Shibubu が前に来ただけで Shibubu を本物の真下へ置いた");
        l5_event(&mut pages, serde_json::json!({ "event": "app_activated", "pid": 42 }));
        time += 0.1;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert_eq!(*BELOW.lock().unwrap(), vec![500], "本物の app が前に来ても Shibubu を本物（500）の真下へ置かない = 本物だけが他の app の上に浮く");
        assert!(pages.panes["notes"].direct.is_some(), "前に来た本物を直接触る間にしない（帯が Using … directly にならない）");
    }

    /// L5-STICK ①（DOGFOOD-X 周 2 回 1）: Shibubu が起こした app が、起動の自分の activate で前に来た（しまった後・人の手でない）→ 直接触る間にせず、
    /// Shibubu を本物（500）の真上へ（本物は Shibubu の後ろ）。起こしてから 1 回だけ・2 回目（人が Cmd+Tab で前に出した）は今までどおり Shibubu を本物の真下へ。
    /// 偽の bridge は Shibubu が既に本物より手前（keep_front は order しない = order_above は起動の腕の物だけ）。
    /// 負の対照: `own` を常に false に → 1 回目で Shibubu を本物の真下へ置いて赤
    #[test]
    fn l5_stick_the_app_shibubu_just_started_coming_front_stays_behind_shibubu() {
        static ABOVE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        static BELOW: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let above: fn(u32) -> Result<(), String> = |cg| {
            ABOVE.lock().unwrap().push(cg);
            Ok(())
        };
        let below: fn(u32) -> Result<(), String> = |cg| {
            BELOW.lock().unwrap().push(cg);
            Ok(())
        };
        let host = std::process::id().to_string();
        let (bridge, log) = g91_bridge("l5-launched", &[("FAKE_HOST_ABOVE", "1"), ("FAKE_HOST_PID", host.as_str())]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, above, below), egui::Context::default(), ui5m_spec(true), 0.0);
        pages.pointer = || (Some([200.0, 200.0]), false);
        let checks = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).filter(|d| d.life == crate::surface::Lifecycle::Active).map_or(0, |d| d.metrics.z_order_checks);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| checks(p) > 0), "前提: しまわない");
        assert!(ABOVE.lock().unwrap().is_empty(), "前提: Shibubu が既に手前なのに keep_front が order した");
        // Shibubu が起こした（launched）本物の app（pid 42）が起動の activate で前に来た
        pages.panes.get_mut("notes").unwrap().launched = Some(crate::surface::now_ms());
        l5_event(&mut pages, serde_json::json!({ "event": "app_activated", "pid": 42 }));
        time += 0.1;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert!(BELOW.lock().unwrap().is_empty() && pages.panes["notes"].direct.is_none(), "起こした app が自分で前に来ただけで直接触る間にした（Shibubu を本物の真下へ = 本物が前に残る）");
        assert_eq!(*ABOVE.lock().unwrap(), vec![500], "起こした app が前に来ても Shibubu を本物（500）の真上へ戻さない（本物が Shibubu を覆ったまま）");
        assert!(g91_sent(&log, "raise").is_empty(), "起こした app の窓を AXRaise した");
        // 2 回目（人が前に出した）→ 今までどおり直接触る間に
        l5_event(&mut pages, serde_json::json!({ "event": "app_activated", "pid": 42 }));
        time += 0.1;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, false, vec![]);
        assert_eq!(*BELOW.lock().unwrap(), vec![500], "2 回目も起動の activate と数えた（人が前に出した本物を後ろへ戻す）");
        assert!(pages.panes["notes"].direct.is_some());
    }

    /// pane-G91 の試験 4: 本物の最小の大きさが枠より大きい（set_frame が枠の大きさにならない）→ 映像は縦横比のまま枠の内側・どちらかの辺が枠に接する。
    #[test]
    fn g91_d_a_window_larger_than_the_frame_is_fitted_keeping_its_shape() {
        let video = Rect::from_min_size(pos2(20.0, 48.0), Vec2::new(400.0, 272.0));
        for (w, h) in [(1000, 800), (900, 300), (400, 272), (300, 900)] {
            let meta = MirrorFrameMeta { seq: 1, width: w, height: h, captured_ms: 0.0, content: [0.0; 4], factor: 2.0 };
            let r = fit(video, &meta);
            assert!(video.expand(0.01).contains_rect(r), "{w}×{h}: 映像が枠の外へ出た {r:?}");
            assert!((r.width() - video.width()).abs() < 0.01 || (r.height() - video.height()).abs() < 0.01, "{w}×{h}: どの辺も枠に接していない {r:?}");
            assert!((r.width() / r.height() - w as f32 / h as f32).abs() < 0.01, "{w}×{h}: 縦横比が替わった {r:?}");
        }
    }

    /// MIRROR-FIT-1101 ②（owner 198 の 8「枠の方が app の表示の大きさに合うように」）: 本物の窓をしまう set_frame は位置だけ動かし大きさは本物の物
    /// （I31 から既定で枠の大きさへ頼むのは `show` の prefit・合わせた後に本物が自分で替えた大きさは頼み直さない）。本物が自分で大きさを替えた（位置は同じ・
    /// 送った枠の echo でない resized）→ 追うのを止めず、枠が本物の大きさになる。
    /// 外から動かされた（位置が違う moved）→ 今まで通り追うのを止め、`REFIT_GAP_S` 空けて位置だけ合わせ直す。
    /// 負の対照（commit message）: `Dock::follow_size` を立てない → resized で Degraded になって赤・`slot` の大きさを映像の矩形に戻す → 2 回目の set_frame が 400 幅で赤
    #[test]
    fn mirror_fit_a_source_that_resizes_itself_stays_docked_and_the_frame_follows() {
        let (bridge, log) = g91_bridge("fit-follow", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let life = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).map(|d| d.life);
        let docked = |p: &MirrorPages| life(p) == Some(Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let set = |log: &Path| g91_sent(log, "set_frame").iter().map(|v| [v["x"].as_f64().unwrap(), v["y"].as_f64().unwrap(), v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]).collect::<Vec<_>>();
        let event = |pages: &mut MirrorPages, kind: &str, f: [f64; 4]| {
            let (Some(Ok(b)), Some(p)) = (pages.bridge.as_mut(), pages.panes.get_mut("notes")) else { panic!("前提: bridge と面") };
            let d = p.dock.as_mut().expect("前提: Dock");
            let handle = d.window.as_ref().expect("前提: 窓").handle.clone();
            d.on_msg(b, Msg::Event(serde_json::json!({ "event": kind, "handle": handle, "frame": { "x": f[0], "y": f[1], "w": f[2], "h": f[3] } })), Instant::now());
        };
        // I31: 枠の大きさの本物は頼まずに合わせた事にする（Dock が付いた後の 1 frame）
        time += 0.05;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![]);
        assert_eq!(pages.panes["notes"].prefit, Some(Vec2::new(400.0, 272.0)), "前提: 枠の大きさの本物を合わせた事にしない");
        // 本物が自分で半分に（同じ位置）→ 追い続け、枠は 200×136 の映像
        event(&mut pages, "resized", [120.0, 98.0, 200.0, 136.0]);
        assert_eq!(life(&pages), Some(Lifecycle::Active), "本物が自分で大きさを替えただけで追うのを止めた");
        assert_eq!(pages.frame_size("notes", Vec2::new(800.0, 600.0)), Vec2::new(200.0, 136.0 + BAND_H), "枠が本物の大きさに合わない");
        // Shibubu の窓が右へ 30 動く → 位置だけ追い、大きさは本物のまま（枠の 400×272 へ戻さない）
        let moved = pos2(G91_HOST.x + 30.0, G91_HOST.y);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, moved, true, |_| set(&log).iter().any(|f| f[0] == 150.0)), "Shibubu の窓が動いても本物を追わない: {:?}", set(&log));
        let last = *set(&log).last().unwrap();
        assert_eq!((last[2], last[3]), (200.0, 136.0), "しまう set_frame が本物の大きさを枠に替えた: {last:?}");
        assert!(pages.misfit("notes", Rect::from_min_size(pos2(20.0, 48.0), Vec2::new(400.0, 272.0))).is_none(), "本物より大きい映像を縮めたと言った");
        // 外から動かされた（位置が違う）→ 追うのを止め、間を置いて位置だけ戻す
        event(&mut pages, "moved", [600.0, 300.0, 200.0, 136.0]);
        assert_eq!(life(&pages), Some(Lifecycle::Degraded), "前提: 外から動かされた窓で追うのを止めない");
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, moved, true, docked), "外から動かされた本物を枠の位置へ戻さない");
        let last = *set(&log).last().unwrap();
        assert_eq!(last, [150.0, 98.0, 200.0, 136.0], "合わせ直しが位置だけでない: {last:?}");
    }

    /// MIRROR-FIT-1101（operator 21:1x「NW / NE / SW の隅を引いて離すと枠が左上に寄る」）: 右下以外の隅を引いて離しても、引いた隅の反対の隅は Page の上で動かない
    /// （枠の左上を Page の並びの所からずらす = `shift`）。本物が引いた大きさを受けた後も同じ・本物が別の大きさを返したら反対の隅に合わせる。並びの所より左・上へは出さない。
    /// 負の対照（未実行）: `shift` を `Vec2::ZERO` に戻す → (50, 40) でなく 0 で赤（interface の commit 0c91ccc8 の形）・離した時に `pin` を置かない → 赤
    #[test]
    fn mirror_fit_the_corner_opposite_the_dragged_one_stays_put() {
        let (bridge, log) = g91_bridge("fit-pin", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        pages.panes.get_mut("notes").unwrap().selected = true;
        // 面 (20, 20) 400×300 の左上の隅 (20, 20) を (70, 60) へ = 右下 (420, 320) は止まったまま・枠は 350×260（映像 350×232）
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let (corner, to) = (pos2(20.0, 20.0), pos2(70.0, 60.0));
        for evs in [vec![Event::PointerMoved(corner), btn(corner, true)], vec![Event::PointerMoved(pos2(45.0, 40.0))], vec![Event::PointerMoved(to)], vec![btn(to, false)]] {
            time += 0.016;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        let size = Vec2::new(350.0, 260.0);
        assert_eq!(pages.shift("notes", size, Rect::from_min_max(Pos2::ZERO, pos2(800.0, f32::INFINITY))), Vec2::new(50.0, 40.0), "左上の隅を引いて離すと右下の隅が動く（枠が並びの左上へ寄る）");
        let set = |log: &Path| g91_sent(log, "set_frame").iter().map(|v| [v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]).collect::<Vec<_>>();
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |_| set(&log).last() == Some(&[350.0, 232.0])), "前提: 本物を引いた大きさに: {:?}", set(&log));
        // 本物が別の大きさ（枠 300×228）を返した → 右下の隅に合わせる
        assert_eq!(pages.shift("notes", Vec2::new(300.0, 228.0), Rect::from_min_max(Pos2::ZERO, pos2(800.0, f32::INFINITY))), Vec2::new(100.0, 72.0), "本物の大きさが替わると反対の隅が動く");
        // 反対の隅より大きい枠は並びの所（0）から（前の物に被せない）・置ける幅からはみ出さない
        assert_eq!(pages.shift("notes", Vec2::new(500.0, 400.0), Rect::from_min_max(Pos2::ZERO, pos2(800.0, f32::INFINITY))), Vec2::ZERO, "並びの所より左・上へ出した");
        assert_eq!(pages.shift("notes", size, Rect::from_min_max(Pos2::ZERO, pos2(380.0, f32::INFINITY))).x, 30.0, "置ける幅からはみ出した");
    }

    /// MIRROR-FIT-1101（owner 200）× 追記 218 の 3: 枠の右下の隅を引いて離す（選んでいなくても）→ 本物をその映像の大きさにする（set_frame の大きさ）。隅の押しは
    /// 触る押しにならない（本物を前に出さない）。MIRROR-HYBRID-1101 追記 240 の 2: 本物が断った（最小の幅 300）→ 枠は引いた幅のまま（跳ね戻さない・本物を比のまま
    /// 縮めて収める）・最小の幅を覚えて次に引く間はそこで止まる・帯に理由の 1 行。
    /// 負の対照（commit message）: 離した時に `resize` を置かない → set_frame が来ず赤・`follow_size` の位置だけの比べを外す → 断られて Degraded のまま赤・
    /// `frame_size` の選んだ大きさの置ける所を外す → 300 幅へ跳ね戻って赤・`least` を `dragged` に渡さない → 2 回目が 200 で赤
    #[test]
    fn mirror_fit_dragging_a_corner_resizes_the_source_and_a_refused_size_comes_back() {
        let (bridge, log) = g91_bridge("fit-corner", &[("FAKE_MIN_W", "300")]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let set = |log: &Path| g91_sent(log, "set_frame").iter().map(|v| [v["w"].as_f64().unwrap(), v["h"].as_f64().unwrap()]).collect::<Vec<_>>();
        // 面 (20, 20) 400×300 の右下の隅 = (420, 320)
        let drag = |pages: &mut MirrorPages, time: &mut f64, to: Pos2| {
            let corner = pos2(420.0, 320.0);
            let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            for evs in [vec![Event::PointerMoved(corner), btn(corner, true)], vec![Event::PointerMoved(pos2((corner.x + to.x) / 2.0, (corner.y + to.y) / 2.0))], vec![Event::PointerMoved(to)], vec![btn(to, false)]] {
                *time += 0.016;
                g91_frame(&ctx, pages, &spec, *time, G91_HOST, true, evs);
            }
        };
        // 追記 218 の 3: 選んでいない枠の隅も引ける（隅の押しは取っ手が取る = HYBRID の触る押しにならない）
        assert!(!pages.panes["notes"].selected, "前提: 選んでいない枠");
        drag(&mut pages, &mut time, pos2(470.0, 360.0));
        assert!(pages.panes["notes"].direct.is_none(), "隅の押しを触る押しにした（本物を前に出した）");
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |_| set(&log).last() == Some(&[450.0, 312.0])), "隅を離しても本物を引いた大きさにしない: {:?}", set(&log));
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| p.frame_size("notes", Vec2::new(800.0, 600.0)) == Vec2::new(450.0, 312.0 + BAND_H)), "引いた大きさが枠に残らない");
        // 断る大きさ（幅 200 < 最小 300）→ 枠は本物の 300 幅に戻る・追うのは止めない・帯に理由
        let to = pos2(220.0, 200.0);
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        // 呼ぶ側の矩形は試験の 400×300 のまま（g91_frame は frame_size を使わない）= 隅は (420, 320) のまま
        for evs in [vec![Event::PointerMoved(pos2(420.0, 320.0)), btn(pos2(420.0, 320.0), true)], vec![Event::PointerMoved(to)], vec![btn(to, false)]] {
            time += 0.016;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, |p| p.panes["notes"].resize.is_none()), "前提: 断られた大きさの答えが来ない");
        assert_eq!(pages.frame_size("notes", Vec2::new(800.0, 600.0)).x, 200.0, "断られた大きさの後に枠が本物の大きさへ跳ね戻った");
        assert_eq!(pages.panes["notes"].least.x, 300.0, "断られた最小の幅を覚えない");
        assert!(docked(&pages), "断られた大きさで追うのを止めた");
        assert!(pages.panes["notes"].band_line.as_ref().is_some_and(|(_, l, _)| l == "TextEdit's window can't be that small · showing it scaled"), "断られた理由が帯に無い: {:?}", pages.panes["notes"].band_line);
        // 次に同じ所まで引くと、引いている間に最小の幅で止まる（離した後に断られない）
        for evs in [vec![Event::PointerMoved(pos2(420.0, 320.0)), btn(pos2(420.0, 320.0), true)], vec![Event::PointerMoved(to)]] {
            time += 0.016;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        assert_eq!(pages.panes["notes"].sized.map(|s| s.x), Some(300.0), "覚えた最小の幅で止まらない");
    }

    /// MIRROR-HYBRID-1101 追記 246 の 3: 隅を引き終わった枠は 1 回だけ Project へ出る（引いている間は出ない）。面 (20, 20) 400×300 の右下を (470, 360) へ =
    /// 並びの所からのずれ 0・枠 450×340（映像 450×312）。
    /// 負の対照（本体の後に未実行）: 離した時に `frames` へ積まない → 空で赤・引いている間に積む → 途中の assert で赤
    #[test]
    fn hybrid_246_a_released_corner_is_saved_to_the_project_once() {
        let (bridge, _log) = g91_bridge("h246-save", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let (corner, to) = (pos2(420.0, 320.0), pos2(470.0, 360.0));
        for evs in [vec![Event::PointerMoved(corner), btn(corner, true)], vec![Event::PointerMoved(pos2(445.0, 340.0))], vec![Event::PointerMoved(to)]] {
            time += 0.016;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        assert!(pages.take_frames().is_empty(), "引いている間に Project へ出した（離す前に revision が積もる）");
        time += 0.016;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![btn(to, false)]);
        assert_eq!(pages.take_frames(), vec![("notes".to_string(), SurfaceFrame { x: 0.0, y: 0.0, w: 450.0, h: 340.0 })], "離した枠が Project へ出ない");
        assert!(pages.take_frames().is_empty(), "取った後も残る");
    }

    /// MIRROR-HYBRID-1101 追記 246（owner「斜め矢印で拡大縮小した時にすでに作ってあるUIrecordなどが邪魔でうねうねして戻る」）: 人が選んだ大きさは、後ろの部品の分を
    /// 残す高さ（room.y）で縮めない（後ろの部品が下へどく）・本物がそれより大きければ比のまま収める・引いている間は引いた大きさそのまま・再起動の後（引いた隅を知らない）は
    /// Project の枠のずれ。
    /// 負の対照（未実行）: `frame_size` の選んだ大きさの置ける所を外す → 100 の高さへ縮めて赤・引いている間の早抜けを外す → 本物の比 (225, 150) で赤
    #[test]
    fn hybrid_246_a_chosen_size_is_not_squeezed_by_the_parts_below() {
        let mut pages = MirrorPages::new(PathBuf::from("/nonexistent-helper"), [], None);
        let mut pane = Pane::new(Err(MirrorError::WindowGone));
        let mut d = Dock::new(AppIdentity { bundle_id: "com.apple.TextEdit".into(), name: "TextEdit".into(), ..AppIdentity::default() }, 1);
        d.window = Some(crate::surface::WindowInfo { frame: Some(Frame { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }), ..Default::default() });
        pane.dock = Some(d);
        pane.sized = Some(Vec2::new(300.0, 200.0));
        pages.panes.insert("s".into(), pane);
        assert_eq!(pages.frame_size("s", Vec2::new(1000.0, 100.0)), Vec2::new(300.0, 200.0 + BAND_H), "後ろの部品の分の高さで選んだ枠を縮めた（引くと押し戻される）");
        let p = pages.panes.get_mut("s").unwrap();
        (p.grab, p.sized) = (Some(Corner::Se), Some(Vec2::new(500.0, 150.0)));
        assert_eq!(pages.frame_size("s", Vec2::new(1000.0, 100.0)), Vec2::new(500.0, 150.0 + BAND_H), "引いている間の枠が引いた所に付いて来ない");
        let p = pages.panes.get_mut("s").unwrap();
        (p.grab, p.sized, p.frame_seen) = (None, None, Some(SurfaceFrame { x: 30.0, y: 12.0, w: 328.0, h: 228.0 }));
        assert_eq!(pages.frame_size("s", Vec2::new(1000.0, 200.0 + BAND_H)), Vec2::new(1000.0, 200.0 + BAND_H), "選んでいない面が置ける所の全部でない（I31・選んだ大きさで縛らない）");
        assert_eq!(pages.shift("s", Vec2::new(328.0, 228.0), Rect::from_min_max(Pos2::ZERO, pos2(800.0, f32::INFINITY))), Vec2::new(30.0, 12.0), "再起動の後に Project の枠のずれで置かない");
    }

    /// MIRROR-HYBRID-1101 追記 240 の 1: 四隅を引いている間の絵は、中身（本物の窓）を枠の映像に縦横それぞれの倍率で重ねる（比に寄せない = 引いた分だけ伸びる）。
    /// 負の対照（未実行）: 倍率を縦横の小さい方にそろえる（`shown_rect` と同じ）→ 中身の高さが 160 で赤
    #[test]
    fn hybrid_240_the_picture_is_stretched_to_the_frame_while_a_corner_is_dragged() {
        let meta = MirrorFrameMeta { seq: 1, width: 400, height: 200, captured_ms: 0.0, content: [10.0, 20.0, 180.0, 80.0], factor: 2.0 };
        let video = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(360.0, 320.0));
        let r = stretched(video, &meta);
        // 中身 = frame の (20, 40) から 360×160 pixel
        let (kx, ky) = (r.width() / 400.0, r.height() / 200.0);
        assert_eq!(Rect::from_min_size(r.min + Vec2::new(20.0 * kx, 40.0 * ky), Vec2::new(360.0 * kx, 160.0 * ky)), video, "中身が映像に重ならない: {r:?}");
        assert_eq!(stretched(video, &MirrorFrameMeta { content: [0.0; 4], ..meta }), video, "中身の無い frame を枠に伸ばさない");
    }

    /// MIRROR-HYBRID-1101 追記 240 の 3（owner 242）: 映像の縁から EDGE の内で止まった pointer では本物を前に出さない（Shibubu の境目を掴みに行く所）・中で止まれば出す。
    /// 負の対照（未実行）: dwell の `inner` の条件を外す → 縁で raise して赤
    #[test]
    fn hybrid_240_resting_on_the_edge_of_the_frame_does_not_bring_the_real_window_forward() {
        let (bridge, log) = g91_bridge("h240-edge", &[]);
        let ((mut pages, _dir), ctx, spec, mut time) = (g91_pages(bridge, |_| Ok(()), |_| Ok(())), egui::Context::default(), ui5m_spec(true), 0.0);
        let docked = |p: &MirrorPages| p.panes.get("notes").and_then(|p| p.dock.as_ref()).is_some_and(|d| d.life == Lifecycle::Active);
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, docked), "前提: しまわない");
        let raises = || g91_sent(&log, "raise").len();
        // 映像 (20, 48)〜(420, 320) の右の縁から 3（右 sidebar の境目を掴みに行く所）
        for (dt, evs) in [(0.016, vec![Event::PointerMoved(pos2(417.0, 180.0))]), (0.1, vec![]), (0.1, vec![]), (0.1, vec![])] {
            time += dt;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(raises(), 0, "縁で止まった pointer で本物を前に出した（境目の上に本物の窓が出る）");
        // 中で止まる → 出す（正の対照）
        for (dt, evs) in [(0.016, vec![Event::PointerMoved(pos2(300.0, 180.0))]), (0.1, vec![]), (0.1, vec![])] {
            time += dt;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        assert!(g91_wait(|| raises() == 1), "中で止まっても本物を前に出さない（raise {}）", raises());
    }

    /// MIRROR-FIT-1101 追記 218 の 3（owner「mirrorがまだ斜め矢印で拡大縮小できないんだけど」= 17 回目は取っ手を選んだ後・Dock の在る面だけに描いた）: 選んでいない
    /// 映っている枠の隅に pointer を載せると斜めの矢印・引ける。本物の窓をしまう口（Dock）の無い面（bridge が起きない）は Page の上の枠の大きさだけ替え、帯に理由の 1 行。
    /// 負の対照（commit message）: handles を呼ぶ条件を前の `selected && Dock の窓` に戻す → 矢印が出ず sized が None で赤・Dock の無い枝で `sized` を置かない →
    /// frame_size が元のままで赤
    #[test]
    fn mirror_fit_218_corners_resize_without_selecting_and_without_a_dock() {
        let (sh, _dir) = ui5m_helper();
        let mut pages = MirrorPages::new(sh, [ui5m_consent()], None);
        let (ctx, spec, mut time) = (egui::Context::default(), ui5m_spec(true), 0.0);
        let shows = |p: &MirrorPages| p.panes.get("notes").is_some_and(|p| p.texture.is_some());
        assert!(g91_until(&ctx, &mut pages, &spec, &mut time, G91_HOST, true, shows), "前提: 映らない");
        assert!(!pages.panes["notes"].selected && pages.panes["notes"].dock.is_none(), "前提: 選んでいない・Dock が無い");
        // 面 (20, 20) 400×300 の右下の隅 = (420, 320)
        let corner = pos2(420.0, 320.0);
        time += 0.016;
        g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![Event::PointerMoved(corner)]);
        time += 0.016;
        let out = g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, vec![]);
        assert_eq!(out.platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe, "選んでいない枠の隅で斜めの矢印が出ない");
        let btn = |p: Pos2, pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let to = pos2(470.0, 360.0);
        for evs in [vec![btn(corner, true)], vec![Event::PointerMoved(pos2(445.0, 340.0))], vec![Event::PointerMoved(to)], vec![btn(to, false)]] {
            time += 0.016;
            g91_frame(&ctx, &mut pages, &spec, time, G91_HOST, true, evs);
        }
        assert_eq!(pages.panes["notes"].sized, Some(Vec2::new(450.0, 312.0)), "Dock の無い面の隅を引いても枠の大きさが替わらない");
        assert_eq!(pages.frame_size("notes", Vec2::new(800.0, 600.0)), Vec2::new(450.0, 312.0 + BAND_H), "引いた大きさが枠に残らない");
        assert!(pages.panes["notes"].band_line.as_ref().is_some_and(|(_, l, _)| l == "TextEdit's window keeps its size · only the frame changed"), "枠だけ替えた理由が帯に無い: {:?}", pages.panes["notes"].band_line);
    }

    /// G87: 入っていない app の面は `Reaper isn't installed on this Mac` と押せる次の一手（AX の Button `Find again` = もう一度 launcher）。
    /// 探し直しをしない（入れるまで同じ答え・helper の list を回さない）。
    #[test]
    fn g87_a_missing_app_says_it_is_not_installed_with_find_again() {
        static CALLS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
        let (mut pages, dir, _) = g82_pages("echo 1 >> \"$(dirname \"$0\")/list.log\"; echo '[]'");
        pages.launch = |_, _| {
            *CALLS.lock().unwrap() += 1;
            Err(MirrorError::NotInstalled { app_name: "Reaper".into() })
        };
        let (spec, ctx) = (g87_spec(), g87_ctx());
        let (st, out) = g87_settle(&ctx, &mut pages, &spec);
        assert_eq!((st, *CALLS.lock().unwrap()), (PortalState::Undiscovered, 1), "{}", g87_label(&out));
        assert_eq!(g87_label(&out), "Not found · Reaper isn't installed on this Mac");
        // 2 秒を過ぎても探し直さない
        pages.panes.get_mut("reaper").unwrap().tried_ms -= RETRY_MS;
        g87_frame(&ctx, &mut pages, &spec, vec![]);
        assert!(pages.panes["reaper"].pending.is_none(), "入っていない app の面を 2 秒ごとに探し直した");
        assert!(!dir.join("list.log").exists(), "入っていない app の窓を helper の list で探した");
        let (_, out) = g87_frame(&ctx, &mut pages, &spec, vec![]);
        let at = g87_button(&out, "Find again").expect("入っていない app の面に押せる次の一手（Find again）が無い");
        g87_click(&ctx, &mut pages, &spec, at);
        g87_settle(&ctx, &mut pages, &spec);
        assert_eq!(*CALLS.lock().unwrap(), 2, "Find again を押しても確かめ直さない");
    }

    /// G88a（h_primary_one・operator 10-27 の決定）: 1 Page に面が 3 つ（どれも窓の無い app = 次の一手の button が 3 つ）でも、塗りの Primary は 1 画面に
    /// ちょうど 1 つ（読む順で最初の面の `Open REAPER`）。残りは Ghost で、名前と AX の Button は同じ。
    /// 負の対照（commit message）: 次の一手を全部 Primary に戻す → 3 つで赤
    #[test]
    fn g88a_three_faces_on_one_page_have_exactly_one_primary() {
        use crate::domain::project::{SurfaceBackendKind, SurfaceTarget};
        let (mut pages, _dir, _) = g82_pages("echo '[]'");
        // 3 つとも動いていて窓が無い（起こさない）
        pages.launch = |id, _| Ok((id.rsplit('.').next().unwrap_or(id).to_uppercase(), false));
        let spec = |id: &str, bundle: &str| SurfaceSpec { id: id.into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: bundle.into(), file: None }, keep_alive: true, frame: None, browser: false };
        let specs = [spec("reaper", "com.cockos.reaper"), spec("music", "com.apple.Music"), spec("notes", "com.apple.Notes")];
        let ctx = g87_ctx();
        let frame = |pages: &mut MirrorPages| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    for (i, s) in specs.iter().enumerate() {
                        pages.show(ui, Rect::from_min_size(pos2(20.0, 20.0 + 290.0 * i as f32), Vec2::new(400.0, 270.0)), s);
                    }
                });
            });
            pages.end_frame();
            out
        };
        // 3 つの面の窓探しが答えるまで（探すのは 1 本ずつ）
        let t0 = std::time::Instant::now();
        let mut out = frame(&mut pages);
        while !specs.iter().all(|s| pages.panes.get(&s.id).is_some_and(|p| p.answered && p.pending.is_none())) && t0.elapsed() < Duration::from_secs(9) {
            std::thread::sleep(Duration::from_millis(50));
            out = frame(&mut pages);
        }
        let out = { let _ = out; frame(&mut pages) };
        let names = ["Open REAPER", "Open MUSIC", "Open NOTES"];
        let at: Vec<Rect> = names.iter().map(|n| {
            let (_, node) = out.platform_output.accesskit_update.iter().flat_map(|u| &u.nodes).find(|(_, x)| x.role() == accesskit::Role::Button && x.label() == Some(*n)).unwrap_or_else(|| panic!("「{n}」が AX の Button に無い"));
            let b = node.bounds().unwrap();
            Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32))
        }).collect();
        let ink = theme::tokens(&ctx).text_primary;
        let primary: Vec<&str> = names.iter().zip(&at).filter(|(_, r)| out.shapes.iter().any(|c| matches!(&c.shape, egui::epaint::Shape::Rect(s) if s.fill == ink && (s.rect.center() - r.center()).length() < 2.0))).map(|(n, _)| *n).collect();
        assert_eq!(primary, ["Open REAPER"], "1 画面の次の一手の Primary がちょうど 1 つ（読む順で最初）でない");
    }

    /// MIRROR-HYBRID-1101 追記 245 の 3: Shibubu が起こした app の窓が隠れていた（隠して起こした窓ができた）→ `Show REAPER` を待たずに 1 回だけ launcher を
    /// if_stopped = false で呼ぶ（前に出さずに隠しを解く）・その間は `Starting REAPER…`。2 回目の隠れ・Shibubu が起こしていない app の隠れは `Show REAPER` を待つ。
    /// 負の対照（未実行・cargo なし）: `lost` の隠しを解く枝を外す → launcher が呼ばれず赤・`unhid` を見ない → 2 回目も解いて赤
    #[test]
    fn hybrid_245_a_window_shibubu_started_hidden_is_shown_behind_without_a_button() {
        static CALLS: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());
        let (mut pages, _dir, _) = g82_pages("echo '[]'");
        pages.launch = |_, if_stopped| {
            CALLS.lock().unwrap().push(if_stopped);
            Ok(("REAPER".into(), true))
        };
        let (spec, ctx) = (g87_spec(), g87_ctx());
        g87_settle(&ctx, &mut pages, &spec);
        CALLS.lock().unwrap().clear();
        let hidden = || MirrorError::WindowHidden { app_name: "REAPER".into() };
        let now = crate::surface::now_ms();
        let p = pages.panes.get_mut("reaper").unwrap();
        p.launched = Some(now - 3000.0);
        p.lost("reaper", hidden(), now);
        let (st, out) = g87_settle(&ctx, &mut pages, &spec);
        assert_eq!(*CALLS.lock().unwrap(), vec![false], "隠して起こした窓の隠しを解かない（launcher を if_stopped = false で 1 回）");
        assert_eq!(st, PortalState::Attaching, "{}", g87_label(&out));
        assert!(g87_label(&out).contains("Starting REAPER…") && g87_button(&out, "Show REAPER").is_none(), "隠しを解く間に button を出した: {}", g87_label(&out));
        // 2 回目の隠れは解かない（繰り返し open しない）
        pages.panes.get_mut("reaper").unwrap().lost("reaper", hidden(), crate::surface::now_ms());
        let (_, out) = g87_frame(&ctx, &mut pages, &spec, vec![]);
        assert!(g87_button(&out, "Show REAPER").is_some(), "2 回目の隠れも勝手に解いた: {}", g87_label(&out));
        // Shibubu が起こしていない app の隠れは解かない
        let p = pages.panes.get_mut("reaper").unwrap();
        (p.launched, p.unhid) = (None, false);
        p.lost("reaper", hidden(), crate::surface::now_ms());
        let (_, out) = g87_frame(&ctx, &mut pages, &spec, vec![]);
        assert!(g87_button(&out, "Show REAPER").is_some(), "人が隠した app の窓を勝手に出した: {}", g87_label(&out));
        assert_eq!(*CALLS.lock().unwrap(), vec![false], "解くのは 1 回だけ");
    }

    /// MIRROR-HYBRID-1101 追記 245 の 2: 窓の無い書類の面は、面の真ん中に `Open TextEdit · notes.md`（Project の repo の書類の名前つき）の button。
    /// 負の対照（未実行・cargo なし）: 状態の塊を左上の child に戻す → 真ん中に無くて赤・書類の名前を外す → button が無くて赤
    #[test]
    fn hybrid_245_open_app_sits_in_the_middle_with_the_document_name() {
        use crate::domain::project::SurfaceTarget;
        let (mut pages, dir, _) = g82_pages("echo '[]'");
        std::fs::write(dir.join("notes.md"), "").unwrap();
        pages.workspace = Some(dir.clone());
        let spec = SurfaceSpec { target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("notes.md".into()) }, ..g87_spec() };
        let ctx = g87_ctx();
        g87_settle(&ctx, &mut pages, &spec);
        // 最初の答え（窓が無い）の次の frame で書類を後ろで開き（試験の open は何もしない）、もう 1 回探した答えまで
        let (_, out) = g87_settle(&ctx, &mut pages, &spec);
        let at = g87_button(&out, "Open TextEdit · notes.md").unwrap_or_else(|| panic!("書類の名前つきの Open の button が無い: {}", g87_label(&out)));
        // 面 (20, 20) 400×300 の映像 = 題の帯の下
        let video = Rect::from_min_max(pos2(20.0, 20.0 + BAND_H), pos2(420.0, 320.0));
        assert!((at.x - video.center().x).abs() < 2.0 && (at.y - video.center().y).abs() < video.height() / 4.0, "button が面の真ん中に無い: {at:?}");
    }

    /// G90 (3)（owner の qa 10: Music の `Mirror here` の後に `Not connected · The app's window closed`・窓は在って画面に出ていなかった）: 隠れた窓の面は
    /// `REAPER's window is hidden` と AX の Button `Show REAPER`。押すと launcher を if_stopped = false で呼ぶ（`open -g -b` = 前に出さずに出す）。
    /// 書類の面でも呼ぶ（窓は在るので空の窓も書類を選ぶ panel も出ない）。
    /// 負の対照: 次の一手の WindowHidden の腕を外す → button が無くて赤・launch の filter の `|| hidden` を外す → 書類の面で赤
    #[test]
    fn g90c_a_hidden_window_says_so_and_show_unhides_it() {
        use crate::domain::project::SurfaceTarget;
        static CALLS: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());
        for file in [None, Some("notes.md".to_string())] {
            let (mut pages, _dir, _) = g82_pages("echo '[]'");
            pages.launch = |_, if_stopped| {
                CALLS.lock().unwrap().push(if_stopped);
                Ok(("REAPER".into(), false))
            };
            let spec = SurfaceSpec { target: SurfaceTarget::App { bundle_id: "com.cockos.reaper".into(), file: file.clone() }, ..g87_spec() };
            let ctx = g87_ctx();
            g87_settle(&ctx, &mut pages, &spec);
            CALLS.lock().unwrap().clear();
            let p = pages.panes.get_mut("reaper").unwrap();
            (p.surface, p.tried_ms) = (Err(MirrorError::WindowHidden { app_name: "REAPER".into() }), crate::surface::now_ms());
            let (st, out) = g87_frame(&ctx, &mut pages, &spec, vec![]);
            assert_eq!(st, PortalState::SourceUnavailable, "{}", g87_label(&out));
            let label = g87_label(&out);
            assert!(label.ends_with("REAPER's window is hidden") && !label.contains("closed"), "隠れた窓を閉じたと言う（{file:?}）: {label}");
            let at = g87_button(&out, "Show REAPER").unwrap_or_else(|| panic!("隠れた窓の面に Show REAPER の button が無い（{file:?}）"));
            g87_click(&ctx, &mut pages, &spec, at);
            g87_settle(&ctx, &mut pages, &spec);
            assert_eq!(*CALLS.lock().unwrap(), vec![false], "Show REAPER で前に出さずに出す（open -g -b）を呼ばない（{file:?}）");
        }
    }

    /// MIRROR-FRAME-1101 の空の枠（bundle id が空の Mirror の面・640×400）。
    fn mf_blank() -> SurfaceSpec {
        use crate::domain::project::{SurfaceFrame, SurfaceTarget};
        SurfaceSpec { target: SurfaceTarget::App { bundle_id: String::new(), file: None }, frame: Some(SurfaceFrame { x: 0.0, y: 0.0, w: 640.0, h: 400.0 }), ..g87_spec() }
    }

    /// MIRROR-FRAME-1101 interface（caller の形・本体の前は ignore）: 空の枠は窓を探さず app を起こさず、AX の枠の名前 `Mirror frame` と Button `Choose a window`・
    /// 枠は Project の 640×400。負の対照: `show` の空の枠の早い return を外す → launcher が呼ばれて赤（`find(helper, "")` の `Open ` の button も出る）。
    #[test]
    fn mirror_frame_1101_a_blank_frame_offers_choose_a_window_and_starts_nothing() {
        static CALLS: std::sync::Mutex<u32> = std::sync::Mutex::new(0);
        let (mut pages, _dir, _) = g82_pages("echo '[]'");
        pages.launch = |_, _| {
            *CALLS.lock().unwrap() += 1;
            Ok(("X".into(), false))
        };
        let spec = mf_blank();
        let ctx = g87_ctx();
        g87_frame(&ctx, &mut pages, &spec, vec![]);
        let (st, out) = g87_frame(&ctx, &mut pages, &spec, vec![]);
        assert!(g87_button(&out, "Choose a window").is_some(), "空の枠に Choose a window が無い");
        assert_eq!(g87_label(&out), "Mirror frame");
        assert!(pages.panes.get("reaper").is_none_or(|p| p.pending.is_none()), "空の枠が窓を探した");
        assert_eq!((*CALLS.lock().unwrap(), st), (0, PortalState::Undiscovered), "空の枠が app を起こした");
        assert_eq!(pages.frame_size("reaper", Vec2::new(800.0, 600.0)), Vec2::new(640.0, 400.0), "枠が Project の 640×400 でない");
    }

    /// MIRROR-FRAME-1101 interface: 題の語（`/` を含む題を書類の path に読ませない）。
    #[test]
    fn mirror_frame_1101_title_word_keeps_slashes_out() {
        assert_eq!(title_word("notes.md"), Some("notes.md".into()));
        assert_eq!(title_word(" owner/repo: Fix it "), Some("repo: Fix it".into()));
        assert_eq!(title_word("   "), None);
        assert!(title_word("a/bb/c").is_some_and(|w| !w.contains('/')));
    }

    /// MIRROR-FRAME-1101 interface (b): 引いた窓 = pointer の下の一番前の窓で、題の帯の中を持って離し、その下が Shibubu。
    /// 負の対照: 「次が Shibubu」を外す → 被った枠の行が赤・題の帯の上限を外す → 本文の行が赤。
    #[test]
    fn mirror_frame_1101_the_window_dragged_by_its_title_onto_shibubu_is_the_one() {
        let w = |pid: i64, id: u64, f: [f64; 4]| (MirrorTarget { window_id: id, pid, bundle_id: format!("app.{pid}"), app_name: format!("App {pid}") }, format!("Window {id}"), f);
        let me = 1;
        // 前から: 引いた GarageBand・Shibubu・後ろの Finder
        let list = vec![w(7, 70, [100.0, 100.0, 500.0, 400.0]), w(me, 10, [0.0, 0.0, 1200.0, 800.0]), w(9, 90, [50.0, 50.0, 900.0, 700.0])];
        assert_eq!(dropped_window(&list, [300.0, 120.0], me).map(|(t, title)| (t.window_id, title)), Some((70, "Window 70".into())));
        assert_eq!(dropped_window(&list, [300.0, 300.0], me), None, "本文の中の drag を入れた");
        assert_eq!(dropped_window(&list[1..], [300.0, 120.0], me), None, "Shibubu が前なのに入れた");
        let covered = vec![list[0].clone(), list[2].clone(), list[1].clone()];
        assert_eq!(dropped_window(&covered, [300.0, 120.0], me), None, "別の app の窓に隠れた枠に落としたのに入れた");
    }

    /// MIRROR-FRAME-1101 (b) × W2b-AX: 前でない間の見張りは押している間と離した時だけ起こす（押していない間・空の枠を描かなくなった後は 0・button も読まない）。
    /// 負の対照（未測）: `pass <= armed` を `pass <= armed + 1` にする → 描かなくなった後の行が赤・`|| held` を外す → 離した時の行が赤。
    #[test]
    fn mirror_frame_1101_b_the_drag_watch_wakes_only_while_a_button_is_down() {
        let unread = || -> bool { panic!("空の枠を描いていないのに button を読んだ") };
        // 空の枠をまだ描いていない（印 0）
        assert_eq!(drag_step(0, 5, unread, false), (false, false));
        // 描いた pass（5 → 印 6）の後: 押していない = 起こさない（idle の frame 0）・押した = 起こす・押している間も起こす
        assert_eq!(drag_step(6, 6, || false, false), (false, false), "押していない間に起こした");
        assert_eq!(drag_step(6, 6, || true, false), (true, true));
        assert_eq!(drag_step(6, 6, || true, true), (true, true));
        // 離した時に 1 回だけ起こす（離した所で `dropped_window` を引く frame）・その次は起こさない
        assert_eq!(drag_step(6, 6, || false, true), (true, false), "離した時に起こさない");
        assert_eq!(drag_step(6, 6, || false, false), (false, false));
        // 空の枠を描かずに 1 pass 進んだ（Page を替えた・Shibubu が前に来た）→ button を読まず起こさない
        assert_eq!(drag_step(6, 7, unread, false), (false, false), "描かなくなった後も起こした");
        // 押している間に描かなくなった → 最後に 1 回だけ起こして止まる（held を落とす）
        assert_eq!(drag_step(6, 7, unread, true), (true, false));
    }

    /// MIRROR-PIN-1102（interface・本体の前は ignore）: `shown` は helper の stream が付いた面だけ（偽の helper の TextEdit の窓 7・pid 42）。
    /// 触る前の時刻は NEG_INFINITY・映像の押し（bridge の無い面でも）の後は押した時刻・止まった pointer だけでは打たない。映る前・知らない面は None。
    /// 負の対照（cargo の束で取る）: surface の Ok を見ずに探している窓（`target`）から返す → 映る前の行が赤・枠を選ぶ押しで打たない → 押した後の行が赤。
    #[test]
    fn mirror_pin_1102_shown_is_the_streamed_window_only() {
        let (sh, dir) = ui5m_helper();
        let mut pages = MirrorPages::new(sh, [ui5m_consent()], None);
        pages.relay_log = dir.join("relay.jsonl");
        let (ctx, spec, mut time) = (egui::Context::default(), ui5m_spec(true), 0.0);
        assert_eq!(pages.shown("notes"), None, "1 frame も描く前に窓を返した");
        assert!(ui5m_live(&ctx, &mut pages, &spec, &mut time), "前提: 偽の helper の frame が 5 秒で映らない");
        let (t, touched) = pages.shown("notes").expect("映っている面の窓を返さない");
        assert_eq!((t.window_id, t.pid, t.bundle_id.as_str()), (7, 42, "com.apple.TextEdit"));
        assert_eq!(touched, f64::NEG_INFINITY, "触っていないのに触った時刻が在る");
        ui5m_click(&ctx, &mut pages, &spec, &mut time, pos2(220.0, 184.0));
        assert!(pages.shown("notes").is_some_and(|(_, at)| at.is_finite()), "映像を押した後に触った時刻が無い");
        assert_eq!(pages.shown("docs"), None, "描いていない面の窓を返した");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// MIRROR-FRAME-1101 interface: 全部の app の画面に出ている窓（画面に無い窓は入れない・frame を読む・Shibubu も返す = 外すのは呼ぶ側）。
    #[test]
    fn mirror_frame_1101_windows_lists_every_apps_on_screen_windows() {
        let me = std::process::id();
        let list = format!(
            r#"echo '[{{"window_id":70,"pid":7,"bundle_id":"com.google.Chrome","app":"Google Chrome","title":"Docs","frame":{{"x":100,"y":100,"w":500,"h":400}},"on_screen":true,"layer":0}},{{"window_id":71,"pid":7,"bundle_id":"com.google.Chrome","app":"Google Chrome","title":"Hidden","frame":{{"x":0,"y":0,"w":500,"h":400}},"on_screen":false,"layer":0}},{{"window_id":10,"pid":{me},"bundle_id":"app.shibubu","app":"Shibubu","title":"Shibubu","frame":{{"x":0,"y":0,"w":1200,"h":800}},"on_screen":true,"layer":0}}]'"#
        );
        let (pages, _dir, _) = g82_pages(&list);
        let got = MirrorTarget::windows(&pages.helper).expect("一覧");
        let ids: Vec<u64> = got.iter().map(|(t, _, _)| t.window_id).collect();
        assert!(ids.contains(&70) && ids.contains(&10) && !ids.contains(&71), "画面に無い窓を入れた・出ている窓を落とした: {ids:?}");
        let (t, title, frame) = got.iter().find(|(t, _, _)| t.window_id == 70).unwrap();
        assert_eq!((t.app_name.as_str(), t.bundle_id.as_str(), title.as_str(), *frame), ("Google Chrome", "com.google.Chrome", "Docs", [100.0, 100.0, 500.0, 400.0]));
    }
}
