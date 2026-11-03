//! W3: 一言 → 複数 Page の Project（YC_WAVES §5.1 の W3・YC_ACCEPTANCE の I1）。
//! 決定的（時刻・乱数・LLM・bridge を使わない）。app があるかの確かめは呼ぶ側（`Generated::create_project` が
//! `project::readback` を surface ごとに呼ぶ）で、ここは起こし方（`SurfaceSpec`）を書くだけ。
//! 一言を読んでいるか（I3）は W13 で合否に入れる。W3 では主デモの一言を名前の表で読んでよい。
//! W13: model（利用者の LLM の CLI）が在る時は `plan_project` / `edit_project_by_model` が一言を model に読ませ、名前の表は引かない。
//! model が無い・答えない時だけ、今の `compose_project` / `edit_project`（名前の表）で続ける（呼ぶ側 = `generated.rs` が決め、理由を 1 行出す）。
//!
//! composer は tests/ から `#[path]` で取り込むので `crate::` を書かない（`super::domain` だけ）。
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::domain::app::{
    AgentPolicy, AppDefinitionV3, AppReadiness, AppState, AutonomyLevel, Composition,
    ExecutionMode, Goal, SchemaVersion, ViewMode,
};
use super::domain::binding::{DataBinding, Source, SourceReadiness};
use super::domain::patch::{AppPatch, AppRevision, PatchOp};
use super::domain::project::{
    resource_source_id, validate_project, PageDefinition, ProjectDefinitionV1, ProjectPatchOp,
    ProjectResource, SurfaceBackendKind, SurfaceSpec, SurfaceTarget, TerminalSessionSpec,
};
use super::domain::ui::{canvas_fault, CanvasProps, PartTrigger, UiNode, TRIGGER_ACTS};
use super::layout::Slot;
use super::patch::EditError;
use super::{ComposeContext, ComposeError, ModelProvider};

/// 生成器が本物の app の席（`compose_project` の Docs の TextEdit・`plan_project` の app の席）に置く backend（owner 24 通目の続き＝M3）。
/// feature `mirror` の時は Mirror（枠の中の app は Mirror だけ・native_direct は撤回）、無い時は NativeDirect（W15 の .app は「外で開く」のまま）。
/// 「<file> を Mirror で見る Page を足して」（`mirror_page_ops`）は feature に関わらず Mirror のまま。
/// W13b: `plan_project` の app の席の面と、1 問の答え「代わりに <app>」の面（`Generated` の `missing_app_ops`）が同じここから取る（既定を替える片はここだけ）。
/// Wave: W16M-3, W13b
pub const APP_SEAT_BACKEND: SurfaceBackendKind =
    if cfg!(feature = "mirror") { SurfaceBackendKind::Mirror } else { SurfaceBackendKind::NativeDirect };

/// 一言から作った Project と、その Page が `app_ref` で指す殻の app（1 つ）。
/// Page は全部この app を指す（`Generated` の Project の id の決め方 `project.{app.id}`・`draw_page` の描き方をそのまま使うため）。
/// 殻の app は action・source を持たず、Page の木は `source_surface`（`props.surface_ref`）と見出し・説明の文字だけ。
/// Wave: W3
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectPlan {
    pub project: ProjectDefinitionV1,
    pub app: AppDefinitionV3,
    /// W13: 誰が一言を読んだか。`"model"`（`plan_project`）か `"rule_based"`（`compose_project` の名前の表）。`BuildPlan::interpreter` と同じ語。
    /// 殻の app の `metadata.composer.interpreter` にも同じ値を書く（DB の apps の行に残り、I3 の tally が model 無しの生成を I3 の合格に数えない）
    pub interpreter: String,
    /// TALK-1101: model が一言と同じ言葉で書いた案の説明（`Preview plan` の節・Build now の `I picked` の 1 行）。名前の表（`rule_based`）は None。
    /// 保存しない（Project の定義に入れない = DB の形を変えない）。読み手 = `generated::plan_lines` と `Generated::save_plan`。
    pub preview: Option<PlanPreview>,
}

/// TALK-1101（owner 190 通目「previewplanも押すと よくわからないもっと詳細に。ただしエンジニア用語により過ぎずに」・191 通目 Build now）: 案の Preview に出す、
/// 使う人の目の説明。model が一言と同じ言葉で書く（`plan_schema` の `preview`・席ごとの見える物は `SeatV1::shows`）。`plan_project` が、席に無い app の名前を
/// 言う行を落とし（`names_other_app`）、Page ごとの見える物（`screens`）を足して `ProjectPlan::preview` に渡す。Mac の中で触る物と聞く許可は model に
/// 書かせない（`plan_lines` が Page の木と面から導く = 案に無い許可を言わない）。前の答え・試験の写しに無い欄は空。
/// 新しい型の理由: 4 つの欄を `PlanV1` に並べると `ProjectPlan` へ運ぶ時にばらける（Preview の 1 塊として運ぶ）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPreview {
    /// 何が出来るか（1〜2 文）
    #[serde(default)]
    pub summary: String,
    /// 使う流れ（3〜6 手・番号は `plan_lines` が付ける）
    #[serde(default)]
    pub steps: Vec<String>,
    /// まだ決まっていない事・出来ない事（答えずに Build now を押した時に選ぶ方を添える）
    #[serde(default)]
    pub open: Vec<String>,
    /// Build now で、答えていない問い（`ComposeContext::unanswered`）に選んだ答え（短く）。問いが無ければ空
    #[serde(default)]
    pub picked: Vec<String>,
    /// Page ごとの見える物と押せる物（`project.pages` と同じ順）。`plan_project` が席の `shows` を Page の束で集める（model の答えには無い）
    #[serde(skip)]
    pub screens: Vec<Vec<String>>,
}

/// W13: model に見せる「この Mac に入っている app」の 1 件（`project::installed_apps` が作る・I3 の held-out と同じ Spotlight の一覧）。
/// 名前の表ではない: run ごと・機械ごとに中身が変わり、repo に置かない。
/// W13b: 代わりの app（`SeatV1::instead`）としても使い、Page の木の 1 問（`SeatV1::asked_in`）に入って保存されるので serde を足した（未知の欄は拒否）。
/// Wave: W13, W13b
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownApp {
    /// 表示名（`kMDItemDisplayName`。末尾の `.app` は落とす）
    pub name: String,
    pub bundle_id: String,
}

/// W13: model の答えの保存形式（`plan_schema` の JSON をそのまま読む・未知の欄は拒否）。
/// Wave: W13
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanV1 {
    pub shape: PlanShape,
    /// Project の短い名前（利用者の言葉で）
    pub name: String,
    /// G88a: 利用者が Page を分けて・別々の Page に・each on its own page と明示した。false（既定）なら外部の service が
    /// `ONE_PAGE_MAX_SERVICES` までは全席を 1 Page に（席の `page` を見ない）。schema では required（前の答え・試験の写しに無ければ false）
    #[serde(default)]
    pub pages_requested: bool,
    /// Page の席（一言に出た順）。どの Page のどの区画に置くかは席の `page`・`slot` と `plan_project` の数の決まり
    pub seats: Vec<SeatV1>,
    /// TALK-1101: 使う人の目の説明（一言と同じ言葉）。前の答え・試験の写しに無ければ空
    #[serde(default)]
    pub preview: PlanPreview,
}

/// G88a: 1 Page に置く外部の service（app と web の席。terminal は数えない）の上限（owner 10-27「基本的に作るのは 1 page・
/// 明示された場合と 3 つ以上の外部 service を入れる場合は 2 page 以降」・完成予想 = REAPER・Music・YouTube の 3 つが 1 枚）。
/// 明示が無く外部がこれ以下 → 全席を Page 0。明示が在るかこれを超える → model の `page` で束ね、1 Page の外部がこれを超えたら順に次の Page へ。
/// Wave: M2
pub const ONE_PAGE_MAX_SERVICES: usize = 3;

/// `project` = 面（Terminal・本物の Desktop App・Web）を並べる Project。`single_app` = 1 Page の app で足りる（読書記録・タイマー）＝ 今の `compose`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanShape {
    Project,
    SingleApp,
}

/// 1 席。kind ごとに使う欄: terminal = 無し・app = `app_name`（と任意の `bundle_id`・`file`）・web = `url`。使わない欄は null。
/// W13b: app の席で、その app がこの Mac の一覧（`KnownApp`）に無い時だけ `install_url` と `instead` を使う（無い app を聞く 1 問の中身）。
/// その時は `plan_project` が面を作らず、この席そのもの（`install_url` は https だけ・`instead` は一覧に在る物だけに絞った写し）を Page の木の 1 問として保存する。
/// Wave: W13, W13b
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeatV1 {
    /// Page の title（利用者の言葉で短く）
    pub title: String,
    pub kind: SeatKind,
    /// 利用者が名指しした app の名前（例: 一言に出た表示名のまま）
    pub app_name: Option<String>,
    /// model の当て推量。`KnownApp` の一覧で `app_name` が引ければそちらが先
    pub bundle_id: Option<String>,
    /// app で開く file（repo の中の相対 path）
    pub file: Option<String>,
    /// web の席の URL（https）
    pub url: Option<String>,
    /// W13b: app の席でその app が一覧に無い時だけ: その app を入れる公式の頁（https）。他は null。`plan_project` は https でなければ捨てる
    pub install_url: Option<String>,
    /// W13b: app の席でその app が一覧に無い時だけ: 同じ用に使える、一覧に在る app（多くて 3）。他は空。
    /// `plan_project` は一覧に無い bundle id・名指しの app 自身・4 つ目から後を捨て、名前は一覧の名前に替える（model の答えを信用しない）
    pub instead: Vec<KnownApp>,
    /// G88a: 置く Page（0 から・負は 0）。`pages_requested` が false で外部が `ONE_PAGE_MAX_SERVICES` 以下なら見ない。前の答え・1 問の写しに無ければ 0。
    /// i64 = model が負の数を返しても答え全体を読めなくしない（schema の部分集合に minimum が無い）
    #[serde(default)]
    pub page: i64,
    /// G88a: Page の中の区画（main = 主に手を動かす物 1 つ・side = 見ながら参照する物・bottom = 録音・端末・一時の道具）。schema は main / side / bottom だけ。
    /// 読めない・無い → 今の並び（`layout::place` の型）
    #[serde(default)]
    pub slot: Option<Slot>,
    /// G88a: その置き場が触りやすい理由の短い 1 行（model に置き方を考えさせる欄・Page の木の node に残す）
    #[serde(default)]
    pub why: String,
    /// G88b: recorder の席だけ: 録った take を入れる app の席の title（無ければ null）。`plan_project` はその席の面の id にする（app の席でなければ null）
    #[serde(default)]
    pub feeds: Option<String>,
    /// TALK-1101: この席で見える物と押せる物（2〜4 個・一言と同じ言葉）。`plan_project` が Page ごとに集めて `PlanPreview::screens` へ。前の答えに無ければ空
    #[serde(default)]
    pub shows: Vec<String>,
}

/// W13b: Page の木の 1 問の置き場（`text` の node の props の key）。`SeatV1::asked_in` と `plan_project`・案の読むだけの行（`generated::plan_lines`）が使う。
pub(crate) const ASKED_PROP: &str = "missing_app";

impl SeatV1 {
    /// W13b: この node に置いた 1 問（一言が名指した app がこの Mac の一覧に無い席）を読む。`plan_project` が面の代わりに置いた `text` の node の
    /// `props["missing_app"]` に、この席の写し（`install_url` は https だけ・`instead` は一覧に在る物だけ）が在る時だけ Some。
    /// 例: `page.root.nodes().into_iter().filter_map(|n| SeatV1::asked_in(n).map(|s| (n.key.clone(), s)))`（`verifier::project_seats` と `Generated::page_intent`）
    /// 保証する: key が無い・`SeatV1` として読めない・kind が app でない → None（その node は今まで通り `props.text` の 1 行の文）。
    /// 保証しない: 読んだ席の app が今もこの Mac に無い事（一覧は作った時の物・入れた後は一言を送り直す）。
    /// Wave: W13b
    pub fn asked_in(node: &UiNode) -> Option<SeatV1> {
        let mut s: SeatV1 = serde_json::from_value(node.props.get(ASKED_PROP)?.clone()).ok()?;
        if s.kind != SeatKind::App {
            return None;
        }
        // 保存した後に書き換えられた写しでも、https でない頁を link にしない
        s.install_url = s.install_url.filter(|u| https(u));
        Some(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    Terminal,
    App,
    Web,
    /// G88b: 録音の帯（一言が録音・鼻歌・歌う・声を入れる事を頼んだ時だけ）。外部の service に数えない
    Recorder,
}

/// W13: `ModelProvider::complete_json` の schema の title（= `JsonRequest::name`）。
pub const PLAN_NAME: &str = "shibubu_project_plan";

/// W13: model に返させる Plan の JSON Schema（`PlanV1` と同じ形）。`llm` の両 provider の部分集合に収まる（全 property を required・
/// 任意の値は type に null・keyword は type / properties / required / additionalProperties / items / enum / title / description だけ・
/// type の配列 4 箇所）。`claude -p --json-schema` にもこのまま渡す。数（席は 1〜8）と中身の整合は `plan_project` が返った値で確かめる。
pub fn plan_schema() -> Value {
    let opt = |d: &str| json!({ "type": ["string", "null"], "description": d });
    let list = |d: &str| json!({ "type": "array", "items": { "type": "string" }, "description": d });
    json!({
        "title": PLAN_NAME,
        "type": "object",
        "additionalProperties": false,
        "required": ["shape", "name", "pages_requested", "seats", "preview"],
        "properties": {
            "shape": {
                "type": "string",
                "enum": ["project", "single_app"],
                "description": "project = 一言が Terminal・Desktop の app・Web を並べる作業場を頼んでいる。single_app = 記録・管理の 1 画面で足りる"
            },
            "name": { "type": "string", "description": "Project の短い名前（利用者の言葉で）" },
            "pages_requested": {
                "type": "boolean",
                "description": "利用者が Page を分けて・別々の Page に・each on its own page / separate pages と明示した時だけ true。言っていなければ false（作業場は 1 Page）"
            },
            "seats": {
                "type": "array",
                "description": "一言が頼んだ物だけを一言に出た順に（頼んでいない物を足さない）",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "kind", "app_name", "bundle_id", "file", "url", "install_url", "instead", "page", "slot", "why", "feeds", "shows"],
                    "properties": {
                        "title": { "type": "string", "description": "席の title（短く）" },
                        "kind": { "type": "string", "enum": ["terminal", "app", "web", "recorder"] },
                        "app_name": opt("kind = app の時: 一言が名指しした app の名前。他は null"),
                        "bundle_id": opt("kind = app の時: 候補の一覧に在ればその bundle id。分からなければ null"),
                        "file": opt("kind = app の時: 開く file（repo の中の相対 path）。無ければ null"),
                        "url": opt("kind = web の時: 開く頁の https の URL。他は null"),
                        "install_url": opt("kind = app でその app が候補の一覧に無い時: その app を入れる公式の頁の https の URL。分からない・一覧に在る・他の kind は null"),
                        "instead": {
                            "type": "array",
                            "description": "kind = app でその app が候補の一覧に無い時: 同じ用に使える、一覧に在る app（多くて 3）。一覧に在る・他の kind は空",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["name", "bundle_id"],
                                "properties": {
                                    "name": { "type": "string", "description": "一覧の名前のまま" },
                                    "bundle_id": { "type": "string", "description": "一覧の bundle id のまま" }
                                }
                            }
                        },
                        "page": { "type": "integer", "description": "置く Page の番号（0 から）。pages_requested が false なら 0" },
                        "slot": {
                            "type": "string",
                            "enum": ["main", "side", "bottom"],
                            "description": "Page の中の置き場。main = 主に手を動かす物（1 Page に 1 つ）・side = 見ながら参照する物（資料・音源・動画。縦に積む）・bottom = 録音・端末・一時の道具"
                        },
                        "why": { "type": "string", "description": "その置き場が触りやすい理由の短い 1 行" },
                        "feeds": opt("kind = recorder の時: 録った take を入れる app の席の title。無ければ・他の kind は null"),
                        "shows": list("この席で見える物と押せる物を 2〜4 個（短く・一言と同じ言葉）")
                    }
                }
            },
            "preview": {
                "type": "object",
                "additionalProperties": false,
                "required": ["summary", "steps", "open", "picked"],
                "description": "使う人に見せる案の説明（一言と同じ言葉・エンジニアの語を使わない・seats に無い物を足さない）",
                "properties": {
                    "summary": { "type": "string", "description": "何が出来るか（1〜2 文）" },
                    "steps": list("使う流れ（3〜6 手・番号は付けない）"),
                    "open": list("まだ決まっていない事・出来ない事（それぞれ、答えずに Build now を押した時に選ぶ方を添える）。無ければ空"),
                    "picked": list("答えていない問いが在る時: あなたが選んだ答え（短く）。無ければ空")
                }
            }
        }
    })
}

/// W13: 一言を model に読ませて Project を組む（I3: 一言専用の固定でない・YC_ACCEPTANCE の I3）。名前の表（`wants_dev_project`）は引かない。
///
/// 例（`Generated::create_project`）:
/// `let (apps, on_mac) = project::installed_apps(t)?; match composer::project::plan_project(goal, &model, &apps, &on_mac, &ctx, ws) {
///   Ok(Some(plan)) => /* create_project と同じく書く */, Ok(None) => /* 今の `create` */,
///   Err(ComposeError::Model(why)) => /* compose_project（名前の表）で続け、why を 1 行出す */, Err(e) => /* Home に 1 行 */ }`
///
/// 保証する:
/// - 空の一言は model を呼ばずに `Err(Empty)`。model を呼ぶのは 1 回だけ（`PLAN_NAME`・`plan_schema()`・一言と `apps` を見せる prompt）
/// - model の Err・`PlanV1` として読めない・席が 0 か 9 以上・title が空・kind と欄が合わない → `Err(ComposeError::Model(理由の 1 行))`（何も作らない）
/// - `shape = single_app` → `Ok(None)`（呼ぶ側は今の `compose`）
/// - `shape = project` → `Ok(Some)`: `interpreter == "model"`・`project.goal` は一言（trim だけ・nonce を落とさない）・id `project.{app}`・revision 1・
///   **G88a の Page の決まり（model の答えを信じない）:** 外部の service（app と web の席）を数え、`pages_requested == false` かつ ≤ `ONE_PAGE_MAX_SERVICES`
///   → 全席を Page 0。それ以外 → 席の `page`（負は 0）で束ね、番号は詰める（並びは番号順・同じ番号の中は席の順）。どの Page も外部が上限を超えたら、
///   超えた席から後ろを新しい Page に移す（その束の後ろ・空の Page は作らない）。
///   Page: 席が 1 つ → 今まで通り id `page.{app}.{slug}`・route `/{slug}`・title = 席の title（slug = 席の slug）。席が 2 つ以上 → Page が 1 枚だけなら
///   title = Project の名前、他は席の title を ` · ` で繋ぐ。slug は title から（席の slug と重ならない）。木は席の node を区画の順（main → side → bottom・
///   同じ区画は席の順）に並べ、各席の node の props に `layout::SEAT_PROP` = `{slot, why}`（main は 1 Page に 1 つ・2 つ目からは side・main が無ければ
///   最初の外部の席・外部も無ければ最初の席。top / drawer は side・slot の無い席が在る Page は置き場を書かない = 今の型の並び）。
///   面の id と node の key は席の slug（slug は title の ASCII か空き番号 `p{n}`・Project の中で重ならない）。
///   app の席: `app_name` を `apps` の名前（大小文字と末尾の .app を見ない）で引けた bundle id、引けなければ model の `bundle_id` で
///   `SurfaceSpec{id: slug, APP_SEAT_BACKEND, App{bundle_id, file}, keep_alive}`。
///   **W13b:** model の `bundle_id` はこの Mac に在る時（`apps` か `on_mac` に在る）だけ使う（両方が空 = 一覧を読めなかった時だけ今まで通り bundle id の形で信じる）。
///   **反証の直し:** `apps`（model に見せる一覧）で名前も当て推量も引けない時は、`on_mac`（この Mac の app の全部・`installed_apps` の 2 つ目・
///   Finder など置き場の外の app も入る・model には見せない）の名前で引く。署名を変えた理由: 一覧に無いだけでこの Mac に在る app を 1 問にしないため
///   （prompt の一覧は広げない）。試験は `on_mac` に `&[]` を渡せば一覧だけで決まる。
///   どこにも無い app の席は面を作らず、その Page に 1 問を置く: key `<slug>-next` の `text` の node・props は `{text: "<名前> がこの Mac に見つかりません…"
///   の 1 行（前の版はこれを描く・D-e）, style: "helper", "missing_app": その席の写し}`（`install_url` は https の時だけ・`instead` は `apps` に在る bundle id だけを
///   重ねず model の順に多くて 3・名前は一覧の名前・一覧に無い当て推量の `bundle_id` は null）。
///   黙って落とさない: 席ごとに node が 1 つ（G88a: 1 Page に席が複数でも）・その席の node は `SeatV1::asked_in` が Some（`verifier::project_seats` が
///   `SeatKind::MissingApp` の席にし、使える検査の 1 行が問いになる）。呼び出しは今と同じ 1 回（model の同じ答えに 2 欄を足しただけ）。
///   web の席: https の URL なら `SurfaceSpec{id: slug, WebView, Url{url}, keep_alive}`、違えば面を作らず 1 行。
///   terminal の席: `workspace` が Some なら `terminal_specs` に `{id: "<app>.<slug>", working_directory: <abs>, shell: "/bin/zsh", keep_alive_on_page_switch: true}`
///   （`terminal_sessions` は id だけで引くので Project をまたいで重ならない id）と その Page に `terminal` node（`terminal_ref` = その id）。None なら W3 と同じ「repo の場所が渡されていません」の 1 行。`resources` の repo は
///   `workspace` が Some なら席に関わらず 1 つ（`compose_project` と同じ・app の席の相対 path の基点）
/// - 返す前に `validate_project` と `AppRevision::new` を通す（通らなければ `ComposeError::Invalid`）
///
/// 保証しない: model の答えが一言に合っている事（I3 の合否は試験の側が DB の `surface_specs` を一言の名指しと比べる）・同じ一言で毎回同じ Page の title・
/// 主デモで Dev / Docs / Diagram の名前になる事（試験の道具は Page を title でなく面の target で探す）・面の app が今使える事（`SurfaceReceipt`）。
/// W13b の 1 問は model の席（この関数）だけ: 名前の表（`compose_project`）の席と、後から app を消した・他の Mac で開いた Project の面は今まで通り
/// readback の `NOT_INSTALLED` →「入っていません」の 1 行（W14b の `Need::Install`）で、問いは出ない。
/// 未測: `claude -p` の答えの速さと、一言の言い換えで target の集合が揃う率（I3 の run で測る）。
/// Wave: W13, W13b
pub fn plan_project(
    goal: &str,
    model: &dyn ModelProvider,
    apps: &[KnownApp],
    on_mac: &[KnownApp],
    ctx: &ComposeContext,
    workspace: Option<&Path>,
) -> Result<Option<ProjectPlan>, ComposeError> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err(ComposeError::Empty);
    }
    let bad = |why: String| ComposeError::Model(why);
    let reply = model
        .complete_json(&plan_prompt(goal, apps, ctx.unanswered.as_deref()), &plan_schema())
        .map_err(bad)?;
    let plan: PlanV1 = serde_json::from_value(reply)
        .map_err(|e| bad(format!("The AI's answer isn't in the shape of a plan ({e})")))?;
    if plan.shape == PlanShape::SingleApp {
        return Ok(None);
    }
    if plan.seats.is_empty() || plan.seats.len() > 8 {
        return Err(bad(format!(
            "The AI returned {} pages. A project has 1 to 8",
            plan.seats.len()
        )));
    }
    for (i, seat) in plan.seats.iter().enumerate() {
        let some = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
        let fits = match seat.kind {
            SeatKind::Terminal => !some(&seat.app_name) && !some(&seat.url),
            SeatKind::App => some(&seat.app_name) && !some(&seat.url),
            SeatKind::Web => some(&seat.url) && !some(&seat.app_name),
            SeatKind::Recorder => !some(&seat.app_name) && !some(&seat.url),
        };
        if seat.title.trim().is_empty() || !fits {
            return Err(bad(format!(
                "Page {} in the AI's answer is missing a field or has the wrong shape",
                i + 1
            )));
        }
    }
    let app_id = ctx
        .app_id
        .clone()
        .unwrap_or_else(|| "generated.project".into());
    let id = format!("project.{app_id}");
    let name = match plan.name.trim() {
        "" => plan.seats[0].title.trim().to_string(),
        n => n.to_string(),
    };
    let ws = workspace.map(|p| p.display().to_string());
    let mut taken: Vec<String> = Vec::new();
    let (mut pages, mut surface_specs, mut terminal_specs) = (Vec::new(), Vec::new(), Vec::new());
    // 席ごとの (slug, node)。Page は席を全部見てから組む（G88a）
    let mut built: Vec<(String, Vec<UiNode>)> = Vec::new();
    // 席の slug は先に全部決める（G88b: recorder の `feeds` が後ろの席の面の id を指せる）
    let slugs: Vec<String> = plan
        .seats
        .iter()
        .map(|seat| {
            let slug = free_slug(seat.title.trim(), |s| taken.iter().any(|t| t == s));
            taken.push(slug.clone());
            slug
        })
        .collect();
    for (seat, slug) in plan.seats.iter().zip(slugs.iter().cloned()) {
        let children = match seat.kind {
            SeatKind::Terminal => match &ws {
                Some(dir) => {
                    // session の記録（terminal_sessions）は id だけで引くので、Project をまたいで重ならない id（app の id は生成ごとに新しい）
                    let terminal_id = format!("{app_id}.{slug}");
                    terminal_specs.push(TerminalSessionSpec {
                        id: terminal_id.clone(),
                        project_id: id.clone(),
                        shell: "/bin/zsh".into(),
                        working_directory: dir.clone(),
                        keep_alive_on_page_switch: true, command: None,
                    });
                    let mut t = node(&format!("{slug}-terminal"), "terminal", None, json!({}), vec![]);
                    t.props.insert("terminal_ref".into(), json!(terminal_id));
                    vec![t]
                }
                None => vec![text(&format!("{slug}-next"), NO_FOLDER, "helper")],
            },
            SeatKind::App => {
                let said = seat.app_name.as_deref().unwrap_or("").trim();
                let listed = |b: &str| apps.iter().find(|a| a.bundle_id.eq_ignore_ascii_case(b));
                let guessed = seat.bundle_id.as_deref().map(str::trim).filter(|b| bundle_like(b));
                let file = seat.file.as_deref().map(str::trim).filter(|f| !f.is_empty()).map(str::to_string);
                // W13b: 名指しの名前（見せた一覧）→ 当て推量の bundle id → 名指しの名前（この Mac の全部）の順に引く。反証の直し: 一覧の外に在る app
                // （Finder など）を 1 問にしない。一覧も全部も空 = 読めなかった時だけ、bundle id の形の当て推量を今まで通り信じる
                let named = |list: &[KnownApp]| list.iter().find(|a| app_key(&a.name) == app_key(said)).map(|a| a.bundle_id.clone());
                let here = |b: &str| apps.iter().chain(on_mac).find(|a| a.bundle_id.eq_ignore_ascii_case(b)).map(|a| a.bundle_id.clone());
                let unread = apps.is_empty() && on_mac.is_empty();
                let bundle = named(apps)
                    .or_else(|| guessed.and_then(|b| here(b)))
                    .or_else(|| named(on_mac))
                    .or_else(|| guessed.filter(|_| unread).map(str::to_string));
                match bundle {
                    Some(bundle_id) => {
                        surface_specs.push(SurfaceSpec {
                            id: slug.clone(),
                            backend: APP_SEAT_BACKEND,
                            target: SurfaceTarget::App { bundle_id, file },
                            keep_alive: true,
                            frame: None,
                            browser: false,
                        });
                        vec![node(&format!("{slug}-surface"), "source_surface", Some(said), json!({ "surface_ref": slug }), vec![])]
                    }
                    None => {
                        // W13b: 面を黙って落とさず、この Page に 1 問（席の写し: 入れる頁は https だけ・代わりは一覧に在る物だけを重ねず
                        // model の順に多くて 3・名前は一覧の名前・一覧に無い当て推量の bundle id は残さない）
                        let mut instead: Vec<KnownApp> = Vec::new();
                        for a in seat.instead.iter().filter_map(|a| listed(a.bundle_id.trim())) {
                            if instead.len() < 3 && !instead.contains(a) {
                                instead.push(a.clone());
                            }
                        }
                        let asked = SeatV1 {
                            app_name: Some(said.into()),
                            bundle_id: None,
                            file,
                            install_url: seat.install_url.as_deref().map(str::trim).filter(|u| https(u)).map(str::to_string),
                            instead,
                            ..seat.clone()
                        };
                        let mut t = text(
                            &format!("{slug}-next"),
                            &format!("{said} isn't on this Mac. Install it, then send your request again"),
                            "helper",
                        );
                        t.props.insert(ASKED_PROP.into(), json!(asked));
                        vec![t]
                    }
                }
            }
            // G88b: `recorder` の node（label `Record humming`・max_seconds 60・recorder_ref = 席の slug・feeds = 入れる先の席の面の id か null）
            SeatKind::Recorder => {
                // 入れる先 = 名前（title か app_name）が `feeds` と同じ app の席の面の id。web・terminal の席・見つからない → null
                let named = |s: &SeatV1| seat.feeds.as_deref().map(str::trim).is_some_and(|f| s.title.trim().eq_ignore_ascii_case(f) || s.app_name.as_deref().is_some_and(|a| a.trim().eq_ignore_ascii_case(f)));
                let feeds = plan.seats.iter().zip(&slugs).find(|(s, _)| s.kind == SeatKind::App && named(s)).map(|(_, slug)| json!(slug)).unwrap_or(Value::Null);
                vec![node(
                    &format!("{slug}-recorder"),
                    "recorder",
                    None,
                    json!({ "label": "Record humming", "max_seconds": 60, "recorder_ref": slug, "feeds": feeds }),
                    vec![],
                )]
            }
            SeatKind::Web => {
                let url = seat.url.as_deref().unwrap_or("").trim();
                if https(url) {
                    surface_specs.push(SurfaceSpec {
                        id: slug.clone(),
                        backend: SurfaceBackendKind::WebView,
                        target: SurfaceTarget::Url { url: url.into() },
                        keep_alive: true,
                        frame: None,
                        browser: false,
                    });
                    vec![node(&format!("{slug}-surface"), "source_surface", Some(url), json!({ "surface_ref": slug }), vec![])]
                } else {
                    vec![text(
                        &format!("{slug}-next"),
                        &format!("“{url}” isn't an https page, so it won't open. Write an https URL in your request and send it again"),
                        "helper",
                    )]
                }
            }
        };
        built.push((slug, children));
    }
    // G93: tab の題は app の本当の表示名（`REAPER`）で始める（model は `Reaper（…` と綴る）
    let titles: Vec<String> = plan.seats.iter().map(|s| display_title(s, apps, on_mac)).collect();
    // G88a: Page の束ね（model の答えを信じない）。外部 = app と web の席
    let external = |s: &SeatV1| matches!(s.kind, SeatKind::App | SeatKind::Web);
    let services = plan.seats.iter().filter(|s| external(s)).count();
    let groups: Vec<Vec<usize>> = if !plan.pages_requested && services <= ONE_PAGE_MAX_SERVICES {
        vec![(0..plan.seats.len()).collect()]
    } else {
        let mut numbers: Vec<i64> = plan.seats.iter().map(|s| s.page.max(0)).collect();
        numbers.sort_unstable();
        numbers.dedup();
        numbers.iter().map(|n| (0..plan.seats.len()).filter(|&i| plan.seats[i].page.max(0) == *n).collect()).collect()
    };
    // 1 枚の外部が上限を超えたら、超えた席から後ろをその束の後ろの新しい Page へ（空の Page は作らない）
    let mut split: Vec<Vec<usize>> = Vec::new();
    for g in groups {
        let (mut cur, mut n) = (Vec::new(), 0);
        for i in g {
            if external(&plan.seats[i]) {
                if n == ONE_PAGE_MAX_SERVICES {
                    split.push(std::mem::take(&mut cur));
                    n = 0;
                }
                n += 1;
            }
            cur.push(i);
        }
        split.push(cur);
    }
    let alone = split.len() == 1;
    // TALK-1101: Preview の説明。席に無い app の名前を言う行は出さない（Page ごとの見える物は `split` の束と同じ順）
    let mut in_plan: Vec<String> = plan.seats.iter().filter(|s| s.kind == SeatKind::App).filter_map(|s| s.app_name.as_deref().map(app_key)).collect();
    for spec in &surface_specs {
        if let SurfaceTarget::App { bundle_id, .. } = &spec.target {
            in_plan.extend(apps.iter().chain(on_mac).filter(|a| a.bundle_id.eq_ignore_ascii_case(bundle_id)).map(|a| app_key(&a.name)));
        }
    }
    if plan.seats.iter().any(|s| s.kind == SeatKind::Terminal) {
        in_plan.push("terminal".into());
    }
    in_plan.push("shibubu".into());
    let known: Vec<&str> = apps.iter().chain(on_mac).map(|a| a.name.as_str()).collect();
    let keep = |lines: &[String]| -> Vec<String> {
        lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !names_other_app(l, &known, &in_plan)).map(str::to_string).collect()
    };
    let said = &plan.preview;
    let preview = PlanPreview {
        summary: Some(said.summary.trim()).filter(|l| !names_other_app(l, &known, &in_plan)).unwrap_or_default().to_string(),
        steps: keep(&said.steps),
        open: keep(&said.open),
        picked: keep(&said.picked),
        screens: split.iter().map(|g| g.iter().flat_map(|&i| keep(&plan.seats[i].shows)).collect()).collect(),
    };
    for g in &split {
        let (title, slug, children) = match g[..] {
            // 席が 1 つ: 今まで通り（Page の slug = 席の slug・置き場は今の型）
            [i] => (titles[i].clone(), built[i].0.clone(), built[i].1.clone()),
            _ => {
                let title = if alone { name.clone() } else { g.iter().map(|&i| titles[i].as_str()).collect::<Vec<_>>().join(" · ") };
                let slug = free_slug(&title, |s| taken.iter().any(|t| t == s));
                taken.push(slug.clone());
                // 区画: top / drawer は side・main は 1 つ（2 つ目から side・無ければ最初の外部の席・外部も無ければ最初の席）・slot の無い席が在れば書かない
                let mut slots: Option<Vec<Slot>> = g.iter().map(|&i| plan.seats[i].slot.map(|s| if matches!(s, Slot::Main | Slot::Bottom) { s } else { Slot::Side })).collect();
                if let Some(sl) = slots.as_mut() {
                    let mains: Vec<usize> = (0..sl.len()).filter(|&k| sl[k] == Slot::Main).collect();
                    for &k in mains.iter().skip(1) {
                        sl[k] = Slot::Side;
                    }
                    if mains.is_empty() {
                        sl[(0..g.len()).find(|&k| external(&plan.seats[g[k]])).unwrap_or(0)] = Slot::Main;
                    }
                }
                let mut order: Vec<usize> = (0..g.len()).collect();
                if let Some(sl) = &slots {
                    order.sort_by_key(|&k| match sl[k] { Slot::Main => 0, Slot::Side => 1, _ => 2 });
                }
                let children: Vec<UiNode> = order.iter().flat_map(|&k| {
                    let mut nodes = built[g[k]].1.clone();
                    if let (Some(sl), Some(n)) = (&slots, nodes.first_mut()) {
                        n.props.insert(super::layout::SEAT_PROP.into(), json!({ "slot": sl[k].name(), "why": plan.seats[g[k]].why.trim() }));
                    }
                    nodes
                }).collect();
                (title, slug, children)
            }
        };
        pages.push(PageDefinition {
            id: format!("page.{app_id}.{slug}"),
            route: format!("/{slug}"),
            title,
            app_ref: Some(app_id.clone()),
            root: node(&format!("{slug}-root"), "column", None, json!({}), children),
            revision: 1,
        });
    }
    // repo は Terminal の作業 folder と、app の席の相対 path の file の基点（compose_project と同じく workspace が在れば必ず持つ）
    let resources: Vec<ProjectResource> = ws
        .iter()
        .map(|dir| ProjectResource {
            id: "repo".into(),
            kind: "repo".into(),
            uri: Some(format!("file://{dir}")),
        })
        .collect();
    let outcome = format!("Open {} with {} pages", name, pages.len());
    let app = shell_app(&app_id, goal, &name, &outcome, "model");
    let project = ProjectDefinitionV1 {
        id,
        name,
        goal: goal.into(),
        app_refs: vec![app_id],
        pages,
        resources,
        terminal_specs,
        surface_specs,
        revision: 1,
    };
    AppRevision::new(app.clone())
        .map_err(|errs| ComposeError::Invalid(errs.iter().map(ToString::to_string).collect()))?;
    validate_project(&project, &[&app]).map_err(ComposeError::Invalid)?;
    Ok(Some(ProjectPlan {
        project,
        app,
        interpreter: "model".into(),
        preview: Some(preview),
    }))
}

/// TALK-1101: AI が書いた Preview の行が、案に無い app（この Mac の一覧に在って `in_plan` = 席の app の `app_key` に無い名前）を言っていれば true
/// （その行は出さない・spec「plan に無い事は書かない。無い app を足さない」）。名前は大文字小文字を見て、前後が英数字でない所だけ（`Notes` は引くが
/// `notes`・`Keynotes` は引かない）・3 文字未満の名前は見ない。
/// ponytail: 名前の一致だけ・無い手（席に無い操作）は見ない（prompt が「seats に無い物を足さない」と頼むだけ）。
fn names_other_app(line: &str, known: &[&str], in_plan: &[String]) -> bool {
    known.iter().filter(|n| n.chars().count() >= 3 && !in_plan.contains(&app_key(n))).any(|n| {
        line.match_indices(*n).any(|(i, _)| {
            let edge = |c: Option<char>| !c.is_some_and(|c| c.is_ascii_alphanumeric());
            edge(line[..i].chars().next_back()) && edge(line[i + n.len()..].chars().next())
        })
    })
}

/// model に渡す 1 つの文（一言と、この Mac の app の一覧）。特定の app・site の名前を書かない（I3 の負の対照「主デモの Plan を返す固定の生成器」に当たる）。
/// TALK-1101: 言葉は一言と同じ（日本語の一言なら日本語・English なら English・名前はそのまま）・`preview` と席の `shows` を使う人の目で・
/// `unanswered`（Home の会話で答えずに残った問い）が在れば自分で決めて `preview.picked` に書く。
fn plan_prompt(goal: &str, apps: &[KnownApp], unanswered: Option<&str>) -> String {
    let list: Vec<String> = apps
        .iter()
        .map(|a| format!("{}: {}", a.name, a.bundle_id))
        .collect();
    format!(
        "利用者の一言を読み、Shibubu の Project の Plan を JSON Schema の形の JSON だけで返す。\n\
         - shape: 一言が Terminal・この Mac の app・Web の頁を並べる作業場を頼んでいれば project。記録・管理・計測の 1 画面で足りれば single_app（seats は空）。\n\
         - seats: 一言が頼んだ物だけを 1 席ずつ、一言に出た順に（頼んでいない Page を足さない）。repo・開発・コードの作業を頼んでいれば terminal の席を 1 つ。\n\
         - app の席: app_name は一言に出た app の名前のまま。下の一覧に在ればその bundle_id、無ければ null。file は一言が名指しした file（repo の中の相対 path）か null。\n\
         - app の席でその app が下の一覧に無い時だけ: install_url はその app を入れる公式の頁の https の URL（分からなければ null）、instead は同じ用に使える下の一覧の app を多くて 3 つ（一覧の名前と bundle id のまま）。一覧に在る app と他の席は install_url を null・instead を空に。\n\
         - web の席: url は https の URL（site の名前だけなら、その site の公式の頁の URL）。\n\
         - title は利用者の言葉で短く。使わない欄は null。\n\
         - pages_requested: 利用者が Page を分けて・別々の Page に・each on its own page / separate pages と明示した時だけ true。true の例: 「…Put each on its own page.」。言っていなければ false（作業場は 1 Page）。\n\
         - page: その席を置く Page の番号（0 から）。pages_requested が false なら全席 0。\n\
         - recorder の席: 一言が録音・鼻歌・歌う・声を入れる事を頼んだ時だけ 1 つ（slot は bottom）。feeds は録った take を入れる app の席の title（無ければ null）。\n\
         - slot: 触りやすさで置く。main = 主に手を動かす物（1 Page に 1 つ・大きく）、side = 見ながら参照する物（資料・音源・動画。縦に積む）、bottom = 録音・端末・一時の道具。why にその理由を短く 1 行。\n\
         - 言葉: name・title・why・shows・preview は一言と同じ言葉で書く（日本語の一言なら日本語、English なら English）。app・file・人の名前は一言のまま。\n\
         - shows: その席で見える物と押せる物を 2〜4 個、短く（例: 録音の button・app の窓・頁）。\n\
         - preview: 初めて使う人に見せる説明。summary = 何が出来るか 1〜2 文。steps = 使う流れ 3〜6 手。open = まだ決まっていない事・出来ない事（それぞれ、答えずに作った時に選ぶ方を添える・無ければ空）。\n\
           seats に在る物と、Shibubu が本当にする事だけを書く（seats に無い app・button・手を足さない）。エンジニアの語（schema・JSON・API・adapter・bundle・WebView・PTY・intent・provider 等）は使わず言い換える。許可の話は書かない（Shibubu が別に言う）。\n\
         {}一言: {goal}\n\
         この Mac の app（名前: bundle id）:\n{}",
        unanswered.map(str::trim).filter(|q| !q.is_empty()).map_or_else(
            || "- picked: 空。\n".to_string(),
            |q| format!("- 利用者は次の問いに答えずに作る事を選んだ: 「{q}」。一番ふつうの答えをあなたが選んで、その答えで seats を組み、選んだ答えを picked に短く書く。\n")
        ),
        list.join("\n")
    )
}

/// https の URL（scheme の後に中身が在る）。web の席の頁と、無い app を入れる頁（W13b）の両方。
fn https(url: &str) -> bool {
    url.starts_with("https://") && url.len() > "https://".len()
}

/// app の名前の照合の形（大小文字と末尾の .app を見ない）。
/// 画面に出す path（W15）: `home` の下は `~/…`、外はそのまま。保存する値・Terminal の cwd は変えない（表示だけ）。
/// `/Users/a` の `home` で `/Users/ab/x` は置換しない（区切りまで見る）。home が空・`/` の時は何もしない。
pub fn tilde_home(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some("") if !home.is_empty() => "~".into(),
        Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => path.into(),
    }
}

/// `tilde_home` を環境の `$HOME` で（画面に path を描く所はこちら）。
pub fn display_path(path: &str) -> String {
    tilde_home(path, &std::env::var("HOME").unwrap_or_default())
}

/// G93: app の席の題の頭が app の名前（model の綴り・または表示名）なら、`apps` / `on_mac` の表示名（`kMDItemDisplayName`）に置き換える。
/// 後ろの利用者の言葉（`（鼻歌で…`）は残す。引けない・頭が名前でない・名前の続きが英数字（`Reaperish`）なら trim しただけ。
/// 例: `display_title(&seat("Reaper（鼻歌）", "app", Some("Reaper")), &[reaper], &[])` = `REAPER（鼻歌）`。
fn display_title(seat: &SeatV1, apps: &[KnownApp], on_mac: &[KnownApp]) -> String {
    let title = seat.title.trim();
    let said = seat.app_name.as_deref().unwrap_or("").trim();
    let Some(display) = (seat.kind == SeatKind::App).then(|| apps.iter().chain(on_mac).find(|a| app_key(&a.name) == app_key(said)).map(|a| a.name.as_str())).flatten() else {
        return title.to_string();
    };
    [said, display]
        .into_iter()
        .filter(|n| !n.is_empty())
        .find_map(|n| {
            let rest = title.get(n.len()..).filter(|_| title.get(..n.len()).is_some_and(|h| h.eq_ignore_ascii_case(n)))?;
            (!rest.starts_with(|c: char| c.is_alphanumeric())).then(|| format!("{display}{rest}"))
        })
        .unwrap_or_else(|| title.to_string())
}

fn app_key(name: &str) -> String {
    let n = name.trim().to_lowercase();
    n.strip_suffix(".app").unwrap_or(&n).trim().to_string()
}

/// bundle id の形（2 つ以上の区切り・英数字と - _ だけ）。model の当て推量をそのまま信用しない。
fn bundle_like(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// 名前から読める ASCII の slug（無ければ空き番号 `p{n}`）で、`taken` に無い物。
fn free_slug(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = name
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    (1..)
        .map(|n| match (base.is_empty(), n) {
            (true, n) => format!("p{n}"),
            (false, 1) => base.clone(),
            (false, n) => format!("{base}-{n}"),
        })
        .find(|s| !taken(s))
        .expect("slug は有限個")
}

/// W13: 自然言語の編集の答えの保存形式（`edit_schema` の JSON）。
/// Wave: W13
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditPlanV1 {
    /// 頼まれた順。1 つも無い = 読めない
    pub steps: Vec<EditStepV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditStepV1 {
    pub verb: EditVerb,
    /// add_page = 足す Page の名前・add_mirror = Mirror で見る file・move_right = 動かす面の名前（`surface_specs` の名前）・unsupported = 読めなかった文・
    /// add_part = 部品の見出し
    pub name: String,
    /// AI-COMPOSE-1101: add_part だけ = 置く Page の title（きっかけが在ればその手の部品の Page が先）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub page: String,
    /// AI-COMPOSE-1101: add_part だけ = 部品の HTML（`CanvasProps::html`）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub html: String,
    /// ENGINE-GENERIC-1101: add_part だけ = 部品の手が起きた後に Engine に頼むきっかけ（`CanvasProps::on`・利用者の一言から会話の AI が書く）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<PartTrigger>,
}

/// `edit_project` が読む形の種類（W7 の 2 つ + W16M-3 の Mirror + UI-2 の英語の形で増えた 3 つ）。それ以外は unsupported。
/// model の語（`edit_schema` の enum・`EditPlanV1` の serde）は AddPage・AddMirror・MoveRight・Unsupported と AI-COMPOSE-1101 の AddPart の 5 つ
/// （AddPart は model だけが書く = 名前の表は読まない・`example` は None）。
/// UI-2 の 3 つ（MoveLeft・RenamePage・RunTests）は名前の表だけの形で `#[serde(skip)]`（model の答えに来れば今まで通り読めない JSON =
/// `EditError::Model` → 名前の表で読み直す）。呼ぶ側は `EditVerb::read` が model の語に無い形を返したら model に送らない（`Generated::start_edit`）。
/// 保証しない: model に名前替え・左へ・走らせるを読ませる事（rename は名前が 2 つ要り `EditStepV1` の 1 欄に入らない）。
/// Wave: UI-2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditVerb {
    AddPage,
    AddMirror,
    MoveRight,
    Unsupported,
    /// AI-COMPOSE-1101（owner 206）: AI が HTML で書いた部品（canvas）を Page に 1 つ置く（`EditStepV1` の page・html・on）
    AddPart,
    /// UI-2: `move <surface> left`（→ `MoveSurface` を 1 つ左の Page へ）
    #[serde(skip)]
    MoveLeft,
    /// UI-2: `rename <page> to <name>`（→ `ProjectPatchOp::RenamePage`）
    #[serde(skip)]
    RenamePage,
    /// UI-2: `run (the )?tests?`（→ Project の差分ではなく `PageIntent::RunTests`・`edit_project` は ops を作らない）
    #[serde(skip)]
    RunTests,
}

impl EditVerb {
    /// UI-2: 名前の表で文の形を読む（日本語の 3 形 + 英語の 4 形・`edit_project` が ops を作る前と同じ読み方）。None = 名前の表では読めない。
    /// 例: `if EditVerb::read(&text) == Some(EditVerb::RunTests) { return self.page_intent_inner(id, PageIntent::RunTests { command: "cargo test".into() }); }`
    /// 保証する: 決定的（model・時刻・bridge を使わない）・`edit_project` が `Unsupported` を返す文には None・英語は大小文字を見ない・
    /// 末尾の `.`・`!`・`。`・`！` と前後の空白（前後が両方引用符ならそれも）を落としてから読む。保証しない: 読んだ名前の Page / 面が在る事（`edit_project` の NoTarget）。
    /// Wave: UI-2
    pub fn read(instruction: &str) -> Option<EditVerb> {
        parse_edit(instruction).map(|(verb, _, _)| verb)
    }

    /// UI-2: その形の英語の例（打てば `EditVerb::read` がこの variant を返す文）。救いの 1 行（`rescue_line`）と Project の欄の placeholder
    /// （`Change this project, e.g. {AddPage の例}`）が引く表。AddPage `add a Tests page`・MoveRight `move Excalidraw right`・
    /// MoveLeft `move Excalidraw left`・RenamePage `rename Docs to Notes`・RunTests `run the tests`。
    /// 例: `format!("Change this project, e.g. {}", EditVerb::AddPage.example().unwrap_or_default())`
    /// 保証する: Some の文は打つ形の英語（小文字で始まる・固有名は大文字・句点なし = copy.en.md の例と同じ字）で、`EditVerb::read(例) == Some(self)`。保証しない: 今の Project で ops になる事
    /// （主デモの生成直後の `move Excalidraw right` は右に Page が無く NoTarget）。Unsupported は None。AddMirror は `add a page that mirrors notes.md`（W15-c1: 救いの 1 行が案内する形を名前の表が読む）。
    /// Wave: UI-2
    pub fn example(self) -> Option<&'static str> {
        match self {
            EditVerb::AddPage => Some("add a Tests page"),
            EditVerb::MoveRight => Some("move Excalidraw right"),
            EditVerb::MoveLeft => Some("move Excalidraw left"),
            EditVerb::RenamePage => Some("rename Docs to Notes"),
            EditVerb::RunTests => Some("run the tests"),
            EditVerb::AddMirror => Some("add a page that mirrors notes.md"),
            EditVerb::Unsupported | EditVerb::AddPart => None,
        }
    }
}

/// UI-2: 名前の表で読めなかった文の救いの 1 行（copy.en.md の `Not sure what to change. Try “{form 1}” or “{form 2}”.`）。
/// 2 つの例は `EditVerb::AddPage.example()` と `EditVerb::MoveRight.example()`（表から引く・打てば必ず `EditVerb::read` が読む）。
/// 例（`Generated::finish_edit`）: `EditError::Unsupported(_) => composer::project::rescue_line()`
/// 保証する: 英語・利用者の打った文を中に写さない（文は欄に残る）・内部の語（op・verb・surface_ref）を出さない。保証しない: 例が今の Project で ops になる事。
/// Wave: UI-2
pub fn rescue_line() -> String {
    let ex = |v: EditVerb| v.example().unwrap_or_default();
    format!("Not sure what to change. Try “{}” or “{}”.", ex(EditVerb::AddPage), ex(EditVerb::MoveRight))
}

/// `EditVerb::read` と `edit_project` の共通の読み（同じ文を同じ形に読む = Unsupported の文には read も None）。
/// -> (形, 名前〈Page・面・Mirror の file・rename の元の Page〉, rename の新しい名前〈他は空〉)。None = 読めない。
/// 英語は ASCII の小文字の写しで形を見て、名前は元の文から同じ byte の幅で切る（`to_ascii_lowercase` は byte 長を変えない）。
fn parse_edit(instruction: &str) -> Option<(EditVerb, String, String)> {
    let said = instruction
        .trim()
        .trim_end_matches(['。', '.', '!', '！'])
        .trim();
    // 救いの 1 行の例を引用符ごと写して打つ（`“add a Tests page”`・review-UI-2 B4）: 前後が両方引用符の時だけ落とす（`rename Docs to “Notes”` の名前の引用符は残す）
    let said = said
        .strip_prefix(['“', '"', '\''])
        .and_then(|r| r.strip_suffix(['”', '"', '\'']))
        .map_or(said, |r| r.trim().trim_end_matches(['。', '.', '!', '！']).trim());
    let one = |verb, name: &str| Some((verb, name.trim().to_string(), String::new()));
    if let Some(file) = mirror_page_file(said) {
        return Some((EditVerb::AddMirror, file, String::new()));
    }
    if let Some(name) = said.strip_suffix("を足して") {
        let name = name.trim();
        let name = ["Page", "page", "ページ"]
            .iter()
            .find_map(|w| name.strip_suffix(w))
            .unwrap_or(name)
            .trim();
        return if name.is_empty() { None } else { one(EditVerb::AddPage, name) };
    }
    if let Some(name) = said.strip_suffix("を右へ") {
        return one(EditVerb::MoveRight, name);
    }
    // 英語の 4 形（UI-2）: 空白は 1 つに寄せてから読む
    let s = said.split_whitespace().collect::<Vec<_>>().join(" ");
    let l = s.to_ascii_lowercase();
    let cut = |from: usize, len: usize| s[from..from + len].trim().to_string();
    if matches!(l.as_str(), "run tests" | "run the tests" | "run test" | "run the test") {
        return Some((EditVerb::RunTests, String::new(), String::new()));
    }
    if let Some(mid) = l.strip_prefix("add ").and_then(|r| r.strip_suffix(" page")) {
        let name = cut(4, mid.len());
        let lower = name.to_ascii_lowercase();
        let name = ["a ", "an "]
            .iter()
            .find(|a| lower.starts_with(*a))
            .map_or(name.as_str(), |a| name[a.len()..].trim());
        return if name.is_empty() || matches!(lower.as_str(), "a" | "an") { None } else { one(EditVerb::AddPage, name) };
    }
    if let Some(rest) = l.strip_prefix("move ") {
        for (end, verb) in [(" right", EditVerb::MoveRight), (" left", EditVerb::MoveLeft)] {
            if let Some(mid) = rest.strip_suffix(end) {
                // `move Excalidraw to the right` も同じ（「to the」は面の名前ではない）・`move the Excalidraw right` の頭の `the ` も名前ではない
                let mid = mid.strip_suffix(" to the").unwrap_or(mid);
                let the = if mid.starts_with("the ") { 4 } else { 0 };
                let name = cut(5 + the, mid.len() - the);
                return if name.is_empty() { None } else { one(verb, &name) };
            }
        }
    }
    // 表の EN の 1 行（rules.md の空の Page の 1 行）が「<面の名前> to the right」と書くので、その形も move ... right と同じに読む
    if let Some(mid) = l.strip_suffix(" to the right") {
        let name = cut(0, mid.len());
        return if name.is_empty() { None } else { one(EditVerb::MoveRight, &name) };
    }
    if let Some(rest) = l.strip_prefix("rename ") {
        // ponytail: 最初の " to " で切る。名前に " to " を含む Page は替えられない（要る時は引用符で囲む形を足す）
        let (from, _) = rest.split_once(" to ")?;
        let title = s[7 + from.len() + 4..].trim().to_string();
        // `rename the Docs page to Notes` の元の名前は `Docs`（頭の `the ` と末尾の ` page` は名前ではない）
        let the = if from.starts_with("the ") { 4 } else { 0 };
        let bare = from[the..].strip_suffix(" page").unwrap_or(&from[the..]);
        let page = cut(7 + the, bare.len());
        return if page.is_empty() || title.is_empty() { None } else { Some((EditVerb::RenamePage, page, title)) };
    }
    None
}

/// W13: `ModelProvider::complete_json` の schema の title（= `JsonRequest::name`）。
pub const EDIT_NAME: &str = "shibubu_project_edit";

/// W13: model に返させる編集の JSON Schema（`EditPlanV1` と同じ形・`plan_schema` と同じ部分集合）。
pub fn edit_schema() -> Value {
    json!({
        "title": EDIT_NAME,
        "type": "object",
        "additionalProperties": false,
        "required": ["steps"],
        "properties": {
            "steps": {
                "type": "array",
                "description": "頼まれた編集を順に。読めない時は unsupported を 1 つ",
                "items": { "anyOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["verb", "name"],
                        "properties": {
                            "verb": { "type": "string", "enum": ["add_page", "add_mirror", "move_right", "unsupported"] },
                            "name": { "type": "string", "description": "add_page = 足す Page の名前・add_mirror = Mirror で見る file・move_right = 右の Page へ動かす面の名前・unsupported = 読めなかった文" }
                        }
                    },
                    // AI-COMPOSE-1101: 部品を書く形は欄が多いので別の形（前の 4 つの答えはそのまま通る）
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["verb", "name", "page", "html", "on"],
                        "properties": {
                            "verb": { "type": "string", "enum": ["add_part"] },
                            "name": { "type": "string", "description": "部品の見出し（短く・利用者の言葉）" },
                            "page": { "type": "string", "description": "置く Page の title（今の Page の一覧のどれか）" },
                            "html": { "type": "string", "description": "部品の 1 枚で閉じた HTML（script 可・外の URL と file は読めない）" },
                            "on": {
                                "type": "array",
                                "description": "下の「部品の手」が起きた後に Engine に頼む一言（利用者がそう頼んだ時だけ）。要らなければ空",
                                "items": {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["act", "key", "intent"],
                                    "properties": {
                                        "act": { "type": "string", "description": "下の「部品の手」のどれかをそのまま" },
                                        "key": { "type": "string", "description": "一言に入れる shibubu.set の値の key。値を使わなければ空" },
                                        "intent": { "type": "string", "description": "利用者の言葉から書く Engine への一言。key が在れば {value} が選んだ値に替わる" }
                                    }
                                }
                            }
                        }
                    }
                ] }
            }
        }
    })
}

/// W13: 自然言語の編集を model に読ませて Project の差分にする（W7 の `edit_project` の名前の表を model に替える・I2 の言い換え）。
///
/// 例（`PageIntent::Edit`）: `let ops = match composer::project::edit_project_by_model(&p, &text, &model) {
///   Err(EditError::Model(why)) => { line = Some(why); composer::project::edit_project(&p, &text)? } r => r? };`
///
/// 保証する: model を呼ぶのは 1 回（`EDIT_NAME`・`edit_schema()`・今の Page の title と面の名前を見せる prompt）。各 step は `edit_project` と同じ op と
/// 同じ拒否（add_page → `AddPage` 1 つ・既に在る名前は `AlreadyApplied`・add_mirror → `AddPage` 1 つ + Mirror の `AddSurface` 1 つ・同じ target は `AlreadyApplied`・
/// move_right → `MoveSurface` 1 つ・面や右の Page が無ければ `NoTarget`）。
/// step が 2 つ以上なら、前の op を当てた写し（app とは照合しない・`domain::project::apply_unchecked`）の上で次を作る（全部か無し・
/// 写しに当たらなければ `NoTarget`）。全部の op を本物の app と照合するのは呼ぶ側（保存の前の `apply`）。steps が空か unsupported を含む → `Unsupported(文)`。
/// model の Err・`EditPlanV1` として読めない → `EditError::Model(理由の 1 行)`。どの Err も ops 0 で何も書かない。
/// AI-COMPOSE-1101: add_part → `ProjectPatchOp::Page`（canvas の Insert）1 つ・保存できない部品は `NoTarget`（`part_op`）。
/// 保証しない: 上の種類以外の編集（名前替え・消す・決まった部品を足す）・add_part の html が app を正しく読む事（中身は AI が書く）。
/// Wave: W13
pub fn edit_project_by_model(
    project: &ProjectDefinitionV1,
    instruction: &str,
    model: &dyn ModelProvider,
) -> Result<Vec<ProjectPatchOp>, EditError> {
    let said = instruction.trim();
    let pages: Vec<&str> = project.pages.iter().map(|p| p.title.as_str()).collect();
    let surfaces = surface_names(project);
    // ENGINE-GENERIC-1101: add_part の on が指せる部品の手（部品の種類と手の名前の表 `TRIGGER_ACTS` から一般に・決まった機能の例を並べない）
    let acts: Vec<String> = project
        .pages
        .iter()
        .flat_map(|pg| {
            pg.root.nodes().into_iter().flat_map(move |n| {
                let name = n.label.clone().unwrap_or_else(|| n.key.clone());
                TRIGGER_ACTS.iter().filter(move |(c, _)| *c == n.component).map(move |(c, a)| format!("{}.{a}（{} の Page の {c}「{name}」）", n.key, pg.title))
            })
        })
        .collect();
    let prompt = format!(
        "利用者の文を、Shibubu の Project への編集の手順として JSON Schema の形の JSON だけで返す。\n\
         - add_page: Page を末尾に足す。name は足す Page の名前（利用者の言葉・英語の名前が出ていればそのまま）。\n\
         - add_mirror: file を TextEdit の Mirror で見る Page を足す。name は file（docs/ が無ければ足す。例 notes.md）。\n\
         - move_right: 面を 1 つ右の Page へ動かす。name は下の「面の名前」のどれかをそのまま。\n\
         - add_part: Page の上で選ぶ・見る・押す物が欲しい頼み（例: app の中の候補から選べるように・app の今の様子を見えるように）は、その部品を HTML で書く。\
         html の script が使える口は shibubu.values（前に残した値）・shibubu.read(app, what)（app を読んだ答えの Promise。what に欲しい物と形を書く。\
         例 'guitar sound names as a JSON array of strings'。失敗は Error の message を部品の中に出す）・shibubu.set(key, value)（選んだ値を残す）・\
         shibubu.act(intent, value)（app への頼み）。\n\
         - 利用者の文が「〜したら〜して」と部品の手（下の「部品の手」）の後に何かを頼む時は、add_part の on に 1 つ（act = その手をそのまま・\
         intent = 利用者の文の頼みを Engine への一言に書いた物〈app と物は利用者の言葉のまま・利用者が言っていない手を足さない〉・\
         部品の選んだ値を一言に入れる時は key = set の key で intent に {{value}}・入れない時は key を空）。\n\
         - 開いている app の中で今 1 回だけする操作（track を足す・channel を開く・button を押す・文字を打つ）は Project の編集ではない = unsupported（Page を足さない）。\n\
         - それ以外の頼み・読めない文は unsupported を 1 つ（name は利用者の文）。頼まれていない手順を足さない。\n\
         利用者の文: {said}\n\
         今の Page（左から）: {}\n\
         面の名前: {}\n\
         部品の手: {}",
        pages.join(", "),
        if surfaces.is_empty() { "(none)".to_string() } else { surfaces.join(", ") },
        if acts.is_empty() { "(none)".to_string() } else { acts.join(", ") },
    );
    let reply = model
        .complete_json(&prompt, &edit_schema())
        .map_err(EditError::Model)?;
    let plan: EditPlanV1 = serde_json::from_value(reply)
        .map_err(|e| EditError::Model(format!("The AI's answer isn't in the shape of edits ({e})")))?;
    if plan.steps.is_empty() || plan.steps.iter().any(|s| s.verb == EditVerb::Unsupported) {
        return Err(EditError::Unsupported(said.into()));
    }
    // 前の op を当てた写しの上で次を作る。途中の写しは app と照合しない（Page の action・source は本物の app にしか無い・反証の直し）。
    // 全部の op は呼ぶ側が保存の前に `apply` で本物の app と照合する（`Generated::apply_project`）
    let mut cur = project.clone();
    let mut ops = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        let name = step.name.trim();
        let next = match step.verb {
            EditVerb::AddPage => add_page_op(&cur, name)?,
            EditVerb::AddMirror => mirror_page_ops(&cur, "com.apple.TextEdit", name)?,
            EditVerb::MoveRight => move_op(&cur, name, true)?,
            EditVerb::AddPart => part_op(&cur, step)?,
            EditVerb::Unsupported => unreachable!("上で断った"),
            // UI-2: `#[serde(skip)]` なので model の答えからは作れない（来れば serde の Err = 上の EditError::Model）
            EditVerb::MoveLeft | EditVerb::RenamePage | EditVerb::RunTests => unreachable!("名前の表だけの形"),
        };
        if i + 1 < plan.steps.len() {
            // ここで落ちるのは op どうしの食い違い（model の答えは読めている）: AI のせいにせず・内部の id を出さず、1 つずつ頼む次の一手
            cur = super::domain::project::apply_unchecked(&cur, &next).map_err(|_| {
                EditError::NoTarget(
                    "Couldn't build those edits in one go. Ask for them one at a time".into(),
                )
            })?;
        }
        ops.extend(next);
    }
    Ok(ops)
}

/// 一言が「複数の面（Terminal・本物の Desktop App・Web）を並べる Project」かを読み、そうなら Project を組む。
///
/// 例: `match composer::project::compose_project(text, &ComposeContext { app_id: Some(id), ..Default::default() }, workspace)? {
///   Some(plan) => /* projects と revision 1（origin = generate）を書く */, None => /* 今の `compose`（読書記録などの 1 Page）*/ }`
///
/// 保証する（Some の時）:
/// - `project.revision == 1`・`project.goal` は一言をそのまま（trim だけ。nonce を落とさない）・`project.id == format!("project.{}", app.id)`
/// - Page は 3 枚・この順・route と title は固定: `/dev`「Dev」・`/docs`「Docs」・`/diagram`「Diagram」（Tests の Page は作らない＝W7 の I2 が足す）
/// - `surface_specs` は 2 つ: `docs`（`APP_SEAT_BACKEND`＝feature `mirror` なら Mirror・`App{bundle_id: "com.apple.TextEdit", file: Some("README.md")}`・keep_alive）と
///   `diagram`（WebView・`Url{url: "https://excalidraw.com"}`・keep_alive）。Docs と Diagram の Page の木にそれぞれを指す `source_surface` が 1 つ
/// - `workspace` が Some（絶対 path）の時だけ `resources` に `ProjectResource{id: "repo", kind: "repo", uri: Some("file://<abs>")}` と
///   `terminal_specs` に `{id: "<app>.dev", working_directory: <abs>, shell: "/bin/zsh", keep_alive_on_page_switch: true}`。None なら両方空で、Dev の Page は
///   「repo の場所が渡されていません」と次の一手の 1 行を出す木になる（偽の Terminal を描かない・D-e）
/// - `validate_project(&plan.project, &[&plan.app])` と `AppRevision::new(plan.app)` を通る（通らなければ `ComposeError::Invalid`）
/// - W12a（未決 7-7 = Git の欄は Dev の Page の中）: `workspace` が Some の時だけ、Dev の Page の木の末尾に Git の欄の 4 node:
///   `dev-git-title`（text・heading「Git」）・`dev-git-branch`（text・label「branch」・binding `{source_ref: resource_source_id("repo"), path: "head.branch"}`・
///   空の文「branch に居ません（detached HEAD）」）・`dev-git-sha`（text・mono・label「SHA」・path `head.sha`・40 桁のまま・空の文「まだ commit がありません」）・
///   `dev-git-changes`（list・label「変更した file」・path `changes`・行の名前 = `path`・補足 = `status`・空の文「変更した file はありません」）。
///   殻の app の `sources` に `Source{id: "resource.repo", kind: "local", readiness: Local}`（Page の binding の行き先を validate_project が照合する）。
///   中身の形は `adapters::git::RepoStatus`（composer は path の名前だけを知る）。None の時は欄も source も足さない（読む repo が無い）。Page の数・順・route は変えない。
///
/// 保証しない: TextEdit・excalidraw.com が今使える事（`SurfaceReceipt` の読み戻しで見る）・`workspace` が git の repo である事・
/// 主デモの一言以外の言い回しで Project を組む事（W13 の I3）。None は「1 Page の app で足りる」（読書記録・タイマーは今の `compose` のまま）。
/// Err は今の `compose` と同じ種類（Empty = 空の一言）。
/// W13: これは model が無い・答えない時の道（`interpreter == "rule_based"`）。model が在る時は `plan_project`（I3 はこの道では通らない＝負の対照）。
/// 契約（UI-2 から・型と署名は変えない）: 画面の文は `docs/ui-copy/` の表の EN（Project の name `Dev environment`・app の purpose
/// `Open the repo in Dev, Docs and Diagram pages`・Git の欄の空の文 `Not on a branch` / `No commits yet` / `No changes`・面の label は app 名と file）。
/// Page の数・順・route・title（Dev・Docs・Diagram）・id・node の key と数と順・binding・`surface_specs`・`terminal_specs` は変えない（layout と page_hash の試験・
/// 主デモの試験の期待はそのまま）。表が `(removed; …)` の文の node（`dev-repo`・`dev-next`）は消さず、表に在る事実の EN か path だけを置く（その行の事実が無い repo の有る Dev の `dev-next` は空の文）。
/// 保証しない: 一言（利用者の言葉）を英語にする事・`goal` は打った文のまま。
/// Wave: W3, UI-2
pub fn compose_project(
    goal: &str,
    ctx: &ComposeContext,
    workspace: Option<&Path>,
) -> Result<Option<ProjectPlan>, ComposeError> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err(ComposeError::Empty);
    }
    if !wants_dev_project(goal) {
        return Ok(None);
    }
    let app_id = ctx
        .app_id
        .clone()
        .unwrap_or_else(|| "generated.dev_project".into());
    let mut app = shell_app(
        &app_id,
        goal,
        "Dev environment",
        "Open the repo in Dev, Docs and Diagram pages",
        "rule_based",
    );
    if workspace.is_some() {
        // W12a: Dev の Page の Git の欄の binding の行き先（validate_project が照合する・中身は read_resources が書く）
        app.sources.push(Source {
            id: resource_source_id(REPO),
            kind: "local".into(),
            readiness: SourceReadiness::Local,
            scope: None,
            last_observed_at: None,
            provider: None,
        });
    }
    let id = format!("project.{app_id}");
    let ws = workspace.map(|p| p.display().to_string());
    let page = |slug: &str, title: &str, children: Vec<UiNode>| PageDefinition {
        id: format!("page.{app_id}.{slug}"),
        route: format!("/{slug}"),
        title: title.into(),
        app_ref: Some(app_id.clone()),
        root: node(&format!("{slug}-root"), "column", None, json!({}), children),
        revision: 1,
    };
    let dev = match &ws {
        Some(dir) => vec![
            text("dev-repo", &format!("repo: {}", display_path(dir)), "mono"),
            // 表（copy.en.md:545）は `(removed; the page has its terminal)`: 文は空・node は木の形（layout・page_hash）のため残す（UI-2 反証の直し:
            // 別の行の `No terminal on this page` を置くと、Terminal の在る Dev に「Terminal が無い」と言う）
            text("dev-next", "", "helper"),
            // W12a: Git の欄（中身は Generated::read_resources が ui_state の resource.repo に書いた物だけ・composer は path の名前だけを知る）
            text("dev-git-title", "Git", "heading"),
            repo_bound("dev-git-branch", "text", "branch", "head.branch", "Not on a branch", json!({})),
            repo_bound("dev-git-sha", "text", "SHA", "head.sha", "No commits yet", json!({ "style": "mono" })),
            repo_bound(
                "dev-git-changes",
                "list",
                "changed",
                "changes",
                "No changes",
                json!({ "columns": ["path", "status"] }),
            ),
        ],
        None => vec![text("dev-next", NO_FOLDER, "helper")],
    };
    let surface = |key: &str, surface_ref: &str, label: &str| {
        node(
            key,
            "source_surface",
            Some(label),
            json!({ "surface_ref": surface_ref }),
            vec![],
        )
    };
    let project = ProjectDefinitionV1 {
        id: id.clone(),
        name: "Dev environment".into(),
        goal: goal.into(),
        app_refs: vec![app_id.clone()],
        pages: vec![
            page("dev", "Dev", dev),
            page(
                "docs",
                "Docs",
                // 面の label は file と app 名（holdout H6: 日本語の雛形の題を英語に・additions.md の Project builder）
                vec![surface("docs-surface", "docs", "README.md in TextEdit")],
            ),
            page(
                "diagram",
                "Diagram",
                vec![surface(
                    "diagram-surface",
                    "diagram",
                    "Diagram in Excalidraw",
                )],
            ),
        ],
        resources: ws
            .iter()
            .map(|dir| ProjectResource {
                id: REPO.into(),
                kind: "repo".into(),
                uri: Some(format!("file://{dir}")),
            })
            .collect(),
        terminal_specs: ws
            .iter()
            .map(|dir| TerminalSessionSpec {
                // Project をまたいで重ならない id（terminal_sessions は id だけで引く・plan_project と同じ）
                id: format!("{app_id}.dev"),
                project_id: id.clone(),
                shell: "/bin/zsh".into(),
                working_directory: dir.clone(),
                keep_alive_on_page_switch: true, command: None,
            })
            .collect(),
        surface_specs: vec![
            SurfaceSpec {
                id: "docs".into(),
                backend: APP_SEAT_BACKEND,
                target: SurfaceTarget::App {
                    bundle_id: "com.apple.TextEdit".into(),
                    file: Some("README.md".into()),
                },
                keep_alive: true,
                frame: None,
                browser: false,
            },
            SurfaceSpec {
                id: "diagram".into(),
                backend: SurfaceBackendKind::WebView,
                target: SurfaceTarget::Url {
                    url: "https://excalidraw.com".into(),
                },
                keep_alive: true,
                frame: None,
                browser: false,
            },
        ],
        revision: 1,
    };
    AppRevision::new(app.clone())
        .map_err(|errs| ComposeError::Invalid(errs.iter().map(ToString::to_string).collect()))?;
    validate_project(&project, &[&app]).map_err(ComposeError::Invalid)?;
    Ok(Some(ProjectPlan {
        project,
        app,
        interpreter: "rule_based".into(),
        preview: None,
    }))
}

/// W7: 自然言語の編集 → Project の差分（YC_WAVES §5.1 の W7・YC_ACCEPTANCE の I2）。決定的（時刻・乱数・LLM・bridge を使わない）。
/// 読むのは 3 種類だけ（名前の表。W13 から model が在る時は `edit_project_by_model` が読み、これは model 無しの道）:
/// - 「<file> を Mirror で見る Page を足して」→ `AddPage` 1 つ + Mirror の `AddSurface` 1 つ（`mirror_page_ops`。target は TextEdit と file。file に `/` が無ければ `docs/` を付ける）。
/// - 「<名前> Page を足して」→ `AddPage` 1 つ（末尾・`index: None`）。Page は id `page.{app}.{slug}`（`app` = `project.app_refs[0]`・W3 と同じ形）・
///   route `/{slug}`・title `<名前>`・`app_ref: Some(app)`・root は `{slug}-root` の column に D-e の 1 行（偽の結果を描かない）。
///   「Tests」なら slug `tests`・1 行は「「走らせる」で走らせた結果がここに出ます」（RunReceipt は W11 の口だけが書く）。`terminal_specs` は足さない。
/// - 「<面の名前> を右へ」→ `MoveSurface` 1 つ: その面を指す `source_surface`（`props.surface_ref` = `surface_specs` の id）を、
///   今その node が在る Page の **1 つ右の Page** の root の末尾へ（`to_page` = 右の Page の id・`parent` = その root の key・`index: None`）。
///   面の名前は `surface_name`（`Url` は host の先頭の label〈excalidraw〉・`App` は backend に関わらず bundle id の末尾〈TextEdit〉・大小文字は見ない）・
///   「mirror」は一番後ろに足した Mirror の面（`MIRROR_NAME`）。
///   W16-3「TextEdit を右へ」・W16M-3「Mirror を右へ」も同じ口。
///
/// 例: `let ops = composer::project::edit_project(&p, "Tests Page を足して")?;
///      generated.apply_project(&app, &hash, &ops, RevisionOrigin::NlEdit)?;`（画面からは `PageIntent::Edit` → `Generated::page_intent`）
///
/// 保証する: Ok の ops は空でなく、頼んだ物だけ（AddPage / MoveSurface は 1 つ・Mirror は AddPage + AddSurface）。AddPage / MoveSurface は `surface_specs` を変えない。
/// Err（ops 0・何も書かない）: 同じ名前の Page / 同じ Mirror の target が既に在る → `EditError::AlreadyApplied`（二重に足さない・I2 の過剰側）・
/// 面が無い / 右に Page が無い（主デモの生成直後は Diagram が末尾なので「Excalidraw を右へ」は Tests を足した後で通る）→ `EditError::NoTarget(次の一手の 1 行)`・
/// 読めない文 → `EditError::Unsupported(文)`。
/// 保証しない: 上の 3 種類以外の言い回し（W13 の I3）・Page の中の app の部品の編集（`composer::patch::edit` への委譲はしない）・面が今使える事（`SurfaceReceipt`）。
///
/// 契約（UI-2 から・署名は変えない・日本語の 3 形は今のまま読む）: 英語の 4 形も読む（大小文字を見ない・`<name>` は打った大小文字のまま・形は `EditVerb::read` と同じ）:
/// - `add (a|an)? <name> page`（`add a Tests page`・`add tests page`）→ 「<名前> Page を足して」と同じ `AddPage` 1 つ（同じ名前は `AlreadyApplied`）。
/// - `move <surface> right` → 「<面の名前> を右へ」と同じ `MoveSurface`・`move <surface> left` → 同じ形で **1 つ左の Page** の root の末尾へ
///   （左端の Page に在れば `NoTarget(次の一手の 1 行)`）。
/// - `rename <page> to <name>` → `ProjectPatchOp::RenamePage { page: その Page の id, title: <name> }` 1 つ（`<page>` は今の Page の title・大小文字を見ない。
///   無ければ `NoTarget`〈在る Page の名前を並べる〉・`<name>` が既に別の Page の title なら `AlreadyApplied`・同じ名前なら `AlreadyApplied`）。route と id は変えない。
/// - `run (the )?tests?` → ops を作らず走らせもしない: `Err(EditError::NoTarget(表の EN の 1 行・表に無ければ sendback))`。呼ぶ側（`Generated` の `PageIntent::Edit`）は
///   先に `EditVerb::read` で `RunTests` を見て `PageIntent::RunTests` へ回すので、この Err は他の呼び方の時だけ出る（model に送らない・engine に渡さない）。
/// UI-2 から画面の文は英語（`AlreadyApplied` の名前・`NoTarget` の 1 行・足した Page の 1 行は `docs/ui-copy/` の表の EN）。Page の id・route・slug の規則は変えない。
/// Wave: W7, W16M-3, UI-2
pub fn edit_project(
    project: &ProjectDefinitionV1,
    instruction: &str,
) -> Result<Vec<ProjectPatchOp>, EditError> {
    let Some((verb, name, title)) = parse_edit(instruction) else {
        let said = instruction
            .trim()
            .trim_end_matches(['。', '.', '!', '！'])
            .trim();
        return Err(EditError::Unsupported(said.into()));
    };
    match verb {
        EditVerb::AddMirror => mirror_page_ops(project, "com.apple.TextEdit", &name),
        EditVerb::AddPage => add_page_op(project, &name),
        EditVerb::MoveRight => move_op(project, &name, true),
        EditVerb::MoveLeft => move_op(project, &name, false),
        EditVerb::RenamePage => rename_page_op(project, &name, &title),
        // 走らせるのは呼ぶ側（PageIntent::RunTests）。ここでは ops を作らない（sendback 37・EN は additions.md・review-UI-2 C5）
        EditVerb::RunTests => Err(EditError::NoTarget(
            "Run tests from the Tests page".into(),
        )),
        EditVerb::Unsupported | EditVerb::AddPart => unreachable!("parse_edit は Unsupported と AddPart を返さない"),
    }
}

/// `rename <page> to <name>`（UI-2）: `ProjectPatchOp::RenamePage` 1 つ。`page` は今の Page の title（大小文字を見ない）。
fn rename_page_op(p: &ProjectDefinitionV1, page: &str, title: &str) -> Result<Vec<ProjectPatchOp>, EditError> {
    let Some(pg) = p.pages.iter().find(|pg| pg.title.to_lowercase() == page.to_lowercase()) else {
        let names: Vec<&str> = p.pages.iter().map(|pg| pg.title.as_str()).collect();
        return Err(EditError::NoTarget(format!(
            "There's no “{page}” page in this project. Pages: {}",
            names.join(", ")
        )));
    };
    // 名前は引用符で囲む（`a {title} page` は `a Inbox page` になる・review-UI-2 B6）
    if p.pages.iter().any(|other| other.title.to_lowercase() == title.to_lowercase()) {
        return Err(EditError::AlreadyApplied(format!("a page named “{title}”")));
    }
    Ok(vec![ProjectPatchOp::RenamePage {
        page_id: pg.id.clone(),
        title: title.into(),
    }])
}

/// AI-COMPOSE-1101（owner 206）: AI が書いた部品（canvas）を Page の root の末尾に 1 つ（`ProjectPatchOp::Page` の Insert）。置く Page = きっかけの手の部品を持つ Page
/// （きっかけは同じ Page の手しか指せない）→ title が `page` の Page → 先頭の Page。key は Project の中で空いた `part-<名前>`。値は空から（利用者が選ぶ）。
/// 保存できない部品（`canvas_fault`）は ops 0 で、もう一度頼む 1 行（AI の答えの中身は出さない）。
fn part_op(p: &ProjectDefinitionV1, step: &EditStepV1) -> Result<Vec<ProjectPatchOp>, EditError> {
    let first_act = step.on.first().and_then(|t| t.act.rsplit_once('.')).map(|(k, _)| k);
    let page = first_act
        .and_then(|k| p.pages.iter().find(|pg| pg.root.find(k).is_some()))
        .or_else(|| p.pages.iter().find(|pg| pg.title.eq_ignore_ascii_case(step.page.trim())))
        .or(p.pages.first())
        .ok_or_else(|| EditError::NoTarget("This project has no pages yet. Add a page first".into()))?;
    let taken: std::collections::HashSet<String> = p.pages.iter().flat_map(|pg| pg.root.nodes()).map(|n| n.key.clone()).collect();
    let key = format!("part-{}", free_slug(&step.name, |s| taken.contains(&format!("part-{s}"))));
    let props = CanvasProps { html: step.html.clone(), values: Default::default(), on: step.on.clone() };
    let name = step.name.trim();
    let part = node(&key, "canvas", (!name.is_empty()).then_some(name), serde_json::to_value(props).unwrap_or_default(), vec![]);
    if canvas_fault(&part, &page.root).is_some() {
        return Err(EditError::NoTarget("The AI's part couldn't be placed. Ask again in other words".into()));
    }
    let value = serde_json::to_value(&part).map_err(|e| EditError::Model(e.to_string()))?;
    Ok(vec![ProjectPatchOp::Page {
        page_id: page.id.clone(),
        patch: AppPatch { op: PatchOp::Insert, target_key: page.root.key.clone(), value: Some(value) },
    }])
}

fn add_page_op(p: &ProjectDefinitionV1, name: &str) -> Result<Vec<ProjectPatchOp>, EditError> {
    let same = name.to_lowercase();
    if p.pages.iter().any(|pg| pg.title.to_lowercase() == same) {
        return Err(EditError::AlreadyApplied(format!("a page named “{name}”")));
    }
    let app = p.app_refs.first().ok_or_else(|| {
        EditError::NoTarget(
            "This project can't draw its pages. Create it again from Home".into(),
        )
    })?;
    // route が埋まっているのは重なりではない（名前替えで route は残る・消した後の番号）: 空いた slug を探す。
    // ponytail: ASCII を含まない名前は p1, p2… の空き番号
    let taken = |s: &str| {
        p.pages
            .iter()
            .any(|pg| pg.route == format!("/{s}") || pg.id == format!("page.{app}.{s}"))
    };
    let slug = free_slug(name, taken);
    let route = format!("/{slug}");
    let line = if free_slug(name, |_| false) == "tests" {
        "No runs yet".to_owned()
    } else {
        format!("The {name} page is empty. Type “<screen name> to the right” in the box above to move a screen here from the page on the left")
    };
    let page = PageDefinition {
        id: format!("page.{app}.{slug}"),
        route,
        title: name.into(),
        app_ref: Some(app.clone()),
        root: node(
            &format!("{slug}-root"),
            "column",
            None,
            json!({}),
            vec![text(&format!("{slug}-next"), &line, "helper")],
        ),
        revision: 1,
    };
    Ok(vec![ProjectPatchOp::AddPage { page, index: None }])
}

/// 「Mirror を右へ」の名前。指すのは `surface_specs` の一番後ろの Mirror の面（利用者が「<file> を Mirror で見る Page を足して」で足した面は
/// 生成器の席より後ろに並ぶ。M3: feature `mirror` では Docs の TextEdit の席も Mirror なので、backend だけでは「Mirror」が 2 つの面を指す）。
/// ponytail: 並びの順で引く。Mirror の面を 2 つ足した後に前の方を名前で引く口は無い（file の名前で引く口を足す時に直す）
const MIRROR_NAME: &str = "mirror";

/// 面の名前: `Url` が host の先頭の label（https://excalidraw.com → excalidraw）・`App` が backend に関わらず bundle id の末尾
/// （com.apple.TextEdit → textedit。M3 で Mirror になった Docs の TextEdit の席も「TextEdit を右へ」のまま）。同じ名前の面が 2 つ在れば前の方。
fn surface_name(spec: &SurfaceSpec) -> String {
    match &spec.target {
        SurfaceTarget::Url { url } => {
            let host = url.split("://").nth(1).unwrap_or(url);
            let host = host.split(['/', ':', '?']).next().unwrap_or(host);
            let host = host.strip_prefix("www.").unwrap_or(host);
            host.split('.').next().unwrap_or(host).to_lowercase()
        }
        SurfaceTarget::App { bundle_id, .. } => bundle_id
            .rsplit('.')
            .next()
            .unwrap_or(bundle_id)
            .to_lowercase(),
    }
}

/// 利用者が打てる面の名前（重なりを除いた `surface_name` の並びと、Mirror の面が在れば `MIRROR_NAME`）。
fn surface_names(p: &ProjectDefinitionV1) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // MIRROR-FRAME-1101: 中身の無い Mirror の枠（名前が空）は一言の候補に出さない
    for n in p.surface_specs.iter().filter(|s| !s.blank_frame()).map(surface_name) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    if p.surface_specs.iter().any(|s| s.backend == SurfaceBackendKind::Mirror) {
        names.push(MIRROR_NAME.into());
    }
    names
}

/// 面を 1 つ右（`right`）か左の Page の root の末尾へ（W7 の「を右へ」・UI-2 の `move <surface> right|left`）。
fn move_op(p: &ProjectDefinitionV1, name: &str, right: bool) -> Result<Vec<ProjectPatchOp>, EditError> {
    let want = name.to_lowercase();
    let found = if want == MIRROR_NAME {
        p.surface_specs.iter().rev().find(|s| s.backend == SurfaceBackendKind::Mirror)
    } else {
        p.surface_specs.iter().find(|s| !want.is_empty() && surface_name(s) == want)
    };
    let Some(spec) = found else {
        let names = surface_names(p);
        return Err(EditError::NoTarget(if names.is_empty() {
            format!("There's no “{name}” screen in this project (nothing can be moved)")
        } else {
            format!(
                "There's no “{name}” screen in this project. Screens you can move: {}",
                names.join(", ")
            )
        }));
    };
    let found = p.pages.iter().enumerate().find_map(|(i, pg)| {
        pg.root
            .nodes()
            .into_iter()
            .find(|n| {
                n.component == "source_surface"
                    && n.props.get("surface_ref").and_then(|v| v.as_str()) == Some(spec.id.as_str())
            })
            .map(|n| (i, n.key.clone()))
    });
    let Some((at, key)) = found else {
        return Err(EditError::NoTarget(format!(
            "“{name}” isn't on any page"
        )));
    };
    let to = if right { p.pages.get(at + 1) } else { at.checked_sub(1).and_then(|i| p.pages.get(i)) };
    let Some(to) = to else {
        return Err(EditError::NoTarget(if right {
            format!(
                "“{name}” is on “{}”, the last page, and there's no page to its right. Add a page first, for example “add a Tests page”",
                p.pages[at].title
            )
        } else {
            // EN は additions.md（右端の行と対・review-UI-2 A5）
            format!("“{name}” is on “{}”, the first page, so it can't move left", p.pages[at].title)
        }));
    };
    Ok(vec![ProjectPatchOp::MoveSurface {
        surface_key: key,
        to_page: to.id.clone(),
        parent: to.root.key.clone(),
        index: None,
    }])
}

/// W3 は主デモの一言だけを名前の表で読む（I3 = 一言を読んでいるかは W13）。
/// ponytail: 名前の表。W13 から model が在る時は `plan_project` が読み、これは model 無しの道だけ（I3 の負の対照はこの表を赤にする）
fn wants_dev_project(goal: &str) -> bool {
    let g = goal.to_lowercase();
    g.contains("開発環境")
        || (g.contains("repo") && (g.contains("textedit") || g.contains("excalidraw")))
}

/// Page が `app_ref` で指す殻の app: action・source を持たない（Page の木は文字と `source_surface` だけ）。
/// `status_message` は持たない（Apps の行に成功の語を出さない）。`metadata.composer.interpreter` に誰が一言を読んだか（`ProjectPlan::interpreter`）。
fn shell_app(
    id: &str,
    goal: &str,
    title: &str,
    outcome: &str,
    interpreter: &str,
) -> AppDefinitionV3 {
    let mut meta = serde_json::Map::new();
    meta.insert("composer".into(), json!({ "interpreter": interpreter }));
    AppDefinitionV3 {
        schema_version: SchemaVersion::V3,
        id: id.into(),
        version: 1,
        title: Some(title.into()).filter(|t: &String| !t.is_empty()),
        goal: Goal {
            literal_goal: goal.into(),
            outcomes: vec![outcome.into()]
                .into_iter()
                .filter(|o: &String| !o.is_empty())
                .collect(),
            assumptions: vec![],
            constraints: vec![],
        },
        composition: Composition {
            root: node(
                "shell-root",
                "column",
                None,
                json!({}),
                vec![text(
                    "shell-pages",
                    "This app holds the pages. Switch between them with the page tabs above",
                    "helper",
                )],
            ),
            view_modes: vec![ViewMode::Use],
        },
        capabilities: vec![],
        actions: vec![],
        sources: vec![],
        workflows: vec![],
        agent: AgentPolicy {
            suggestions_enabled: false,
            autonomy_level: AutonomyLevel::SuggestOnly,
            entrypoint: "agent_rail".into(),
            policy_scope: vec!["local".into()],
        },
        state: AppState {
            readiness: AppReadiness::Preview,
            execution_mode: ExecutionMode::LocalReady,
            status_message: None,
        },
        metadata: Some(meta),
    }
}

fn node(
    key: &str,
    component: &str,
    label: Option<&str>,
    props: serde_json::Value,
    children: Vec<UiNode>,
) -> UiNode {
    UiNode {
        key: key.into(),
        component: component.into(),
        label: label.map(Into::into),
        props: props.as_object().cloned().unwrap_or_default(),
        children,
        ..UiNode::default()
    }
}

fn text(key: &str, text: &str, style: &str) -> UiNode {
    node(
        key,
        "text",
        None,
        json!({ "text": text, "style": style }),
        vec![],
    )
}

/// G99c: repo の場所が無い Terminal の席の 1 行（`Generated` が同じ文の node を探して `Choose folder…` を添え、folder を当てると Terminal に替える）。
pub const NO_FOLDER: &str = "No folder";

/// Project の repo の resource の id（`resources[0].id`・binding の行き先は `resource_source_id(REPO)` = "resource.repo"）。
const REPO: &str = "repo";

/// W12a: repo を読んだ中身（`ui_state` の `resource.repo`）の `path` に bind した node。`empty` は読めた上で値が無い時の 1 行
/// （読めていない時は read_resources が region を Error「読めていません」にするので、この文は出ない）。
fn repo_bound(
    key: &str,
    component: &str,
    label: &str,
    path: &str,
    empty: &str,
    props: serde_json::Value,
) -> UiNode {
    UiNode {
        binding: Some(DataBinding {
            source_ref: resource_source_id(REPO),
            path: path.into(),
            extra: Default::default(),
        }),
        states: json!({ "empty": empty }).as_object().cloned(),
        ..node(key, component, Some(label), props, vec![])
    }
}

/// W16M-3: 「<file> を Mirror で見る Page を足して」の差分 = AddPage 1 つ + Mirror の AddSurface 1 つ（YC_WAVES の W16M-3 の行・I2 と同じ検査）。
/// 名前の表の道（`edit_project`）と model の道（`edit_project_by_model`）の両方が、読んだ app と file をここに渡して ops を作る。
/// Wave: W16M-3
/// 例: `mirror_page_ops(&project, "com.apple.TextEdit", "docs/notes.md")?` → `[AddPage{..}, AddSurface{ spec: backend Mirror・target App{ bundle_id, file } }]`
/// 保証する: 同じ target（bundle id と file）の Mirror の面が既に在れば `EditError::AlreadyApplied`（「既に在ります」の 1 行・ops 0）・
/// file は Project の working dir からの相対だけ（絶対 path・`..` は `EditError::NoTarget`）・決定的（時刻・乱数・bridge を使わない）。
/// 保証しない: 一言から bundle id と file を読む事（読むのは呼ぶ側。`edit_project` は TextEdit と file を渡す）。
pub fn mirror_page_ops(
    project: &ProjectDefinitionV1,
    bundle_id: &str,
    file: &str,
) -> Result<Vec<ProjectPatchOp>, EditError> {
    let file = mirror_file(file)?;
    let same = project.surface_specs.iter().any(|s| {
        s.backend == SurfaceBackendKind::Mirror
            && matches!(
                &s.target,
                SurfaceTarget::App { bundle_id: b, file: Some(f) } if b == bundle_id && f == &file
            )
    });
    if same {
        let shown = Path::new(&file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&file);
        return Err(EditError::AlreadyApplied(format!(
            "Page that mirrors {shown}"
        )));
    }
    let stem = Path::new(&file)
        .file_stem()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("mirror");
    let mut ops = add_page_op(project, stem)?;
    // 足す Page には Mirror の面を置くので、add_page_op の「まだ空です」の 1 行は嘘になる（BG の下見で映像の上に出ていた・D-e）。外す
    if let Some(ProjectPatchOp::AddPage { page, .. }) = ops.first_mut() {
        page.root.children.clear();
    }
    let page = match ops.first() {
        Some(ProjectPatchOp::AddPage { page, .. }) => page,
        _ => unreachable!("add_page_op は AddPage 1 つ"),
    };
    let taken = |id: &str| project.surface_specs.iter().any(|s| s.id == id);
    let id = free_slug(stem, taken);
    ops.push(ProjectPatchOp::AddSurface {
        spec: SurfaceSpec {
            id: id.clone(),
            backend: SurfaceBackendKind::Mirror,
            target: SurfaceTarget::App {
                bundle_id: bundle_id.into(),
                file: Some(file),
            },
            keep_alive: true,
            frame: None,
            browser: false,
        },
        page_id: page.id.clone(),
        parent: page.root.key.clone(),
        node_key: format!("surface.{id}"),
        index: None,
    });
    Ok(ops)
}

/// 「<file> を Mirror で見る Page を足して」（Page / page / ページは省略可）。file が空なら None。
fn mirror_page_file(said: &str) -> Option<String> {
    // 英語の形（救いの 1 行が案内する `add a page that mirrors <file>`）: 大小文字を見ない・空白は 1 つに寄せる（末尾の `.` は呼ぶ側が落とす）
    const EN: &str = "add a page that mirrors ";
    let s = said.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(file) = s.get(EN.len()..).filter(|_| s.get(..EN.len()).is_some_and(|h| h.eq_ignore_ascii_case(EN))) {
        return Some(file.trim().to_string()).filter(|f| !f.is_empty());
    }
    let name = said.strip_suffix("を足して")?.trim();
    let name = ["Page", "page", "ページ"]
        .iter()
        .find_map(|w| name.strip_suffix(w))
        .unwrap_or(name)
        .trim();
    for sep in ["を Mirror で見る", "をミラーで見る", "を mirror で見る"] {
        if let Some(file) = name.strip_suffix(sep) {
            let file = file.trim();
            if !file.is_empty() {
                return Some(file.to_string());
            }
        }
    }
    None
}

/// working dir からの相対。`/` が無ければ `docs/` を付ける。絶対 path と `..` は NoTarget。
fn mirror_file(file: &str) -> Result<String, EditError> {
    let file = file.trim();
    if file.is_empty() {
        return Err(EditError::NoTarget(
            "Name a file to mirror, for example notes.md".into(),
        ));
    }
    let path = Path::new(file);
    if path.is_absolute() || file.starts_with('/') {
        return Err(EditError::NoTarget(
            "Write the mirrored file's path relative to the project folder".into(),
        ));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(EditError::NoTarget(
            "A mirrored file's path can't contain ..".into(),
        ));
    }
    if file.contains('/') {
        Ok(file.to_string())
    } else {
        Ok(format!("docs/{file}"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn w15_tilde_home_only_under_home_with_separator() {
        use super::tilde_home as t;
        assert_eq!(t("/Users/a/repo", "/Users/a"), "~/repo");
        assert_eq!(t("/Users/a", "/Users/a"), "~");
        assert_eq!(t("/Users/a/repo", "/Users/a/"), "~/repo", "home の末尾の / は無視");
        assert_eq!(t("/opt/x", "/Users/a"), "/opt/x", "HOME の外はそのまま");
        assert_eq!(t("/Users/ab/x", "/Users/a"), "/Users/ab/x", "別の user の path を置換しない");
        assert_eq!(t("/x", ""), "/x");
        assert_eq!(t("/x", "/"), "/x", "home が / でも何も置換しない");
    }

    use super::*;

    fn ctx() -> ComposeContext<'static> {
        ComposeContext {
            app_id: Some("app.t".into()),
            ..ComposeContext::default()
        }
    }
    const DEMO: &str = "この repo の開発環境を作って。README は TextEdit で直して、構成図は Excalidraw で描いて repo に保存したい";

    #[test]
    fn w3_demo_goal_makes_three_pages_and_two_surfaces() {
        let ws = Path::new("/tmp/shibubu-yc-demo");
        let plan = compose_project(&format!("  {DEMO} n-1 "), &ctx(), Some(ws))
            .unwrap()
            .expect("主デモの一言は Project");
        let p = &plan.project;
        assert_eq!((p.id.as_str(), p.revision), ("project.app.t", 1));
        assert_eq!(
            p.goal,
            format!("{DEMO} n-1"),
            "goal は trim だけ（nonce を残す）"
        );
        let pages: Vec<_> = p
            .pages
            .iter()
            .map(|pg| (pg.route.as_str(), pg.title.as_str()))
            .collect();
        assert_eq!(
            pages,
            [("/dev", "Dev"), ("/docs", "Docs"), ("/diagram", "Diagram")]
        );
        assert!(p
            .pages
            .iter()
            .all(|pg| pg.app_ref.as_deref() == Some("app.t")));
        let specs: Vec<_> = p
            .surface_specs
            .iter()
            .map(|s| (s.id.as_str(), s.backend, &s.target))
            .collect();
        assert_eq!(specs[0].0, "docs");
        assert_eq!(specs[0].1, APP_SEAT_BACKEND);
        assert_eq!(
            specs[0].2,
            &SurfaceTarget::App {
                bundle_id: "com.apple.TextEdit".into(),
                file: Some("README.md".into())
            }
        );
        assert_eq!(
            (specs[1].0, specs[1].1),
            ("diagram", SurfaceBackendKind::WebView)
        );
        assert_eq!(
            specs[1].2,
            &SurfaceTarget::Url {
                url: "https://excalidraw.com".into()
            }
        );
        // Docs と Diagram の Page にそれぞれを指す source_surface が 1 つ
        for (i, sid) in [(1, "docs"), (2, "diagram")] {
            let refs: Vec<_> = p.pages[i]
                .root
                .nodes()
                .into_iter()
                .filter(|n| n.component == "source_surface")
                .filter_map(|n| n.props.get("surface_ref")?.as_str())
                .collect();
            assert_eq!(refs, [sid]);
        }
        assert_eq!(
            p.resources[0].uri.as_deref(),
            Some("file:///tmp/shibubu-yc-demo")
        );
        assert_eq!(
            (
                p.terminal_specs[0].working_directory.as_str(),
                p.terminal_specs[0].shell.as_str()
            ),
            ("/tmp/shibubu-yc-demo", "/bin/zsh")
        );
        assert!(plan.app.actions.is_empty() && plan.app.state.status_message.is_none());
        // W12a: 殻の app の source は Git の欄の行き先 1 つだけ（local・接続の要らない data）
        assert_eq!(
            plan.app
                .sources
                .iter()
                .map(|s| (s.id.as_str(), s.kind.as_str(), s.readiness))
                .collect::<Vec<_>>(),
            [("resource.repo", "local", SourceReadiness::Local)]
        );
    }

    /// W12a: workspace が在る時だけ Dev の Page の末尾に Git の欄（見出し・branch・SHA・変更 file）。値は binding だけで、文字を作らない。
    #[test]
    fn w12a_dev_page_ends_with_the_git_fields_bound_to_the_repo() {
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/shibubu-yc-demo")))
            .unwrap()
            .unwrap();
        let dev = &plan.project.pages[0];
        let tail: Vec<_> = dev.root.children[dev.root.children.len() - 4..]
            .iter()
            .map(|n| {
                (
                    n.key.as_str(),
                    n.component.as_str(),
                    n.binding
                        .as_ref()
                        .map(|b| (b.source_ref.as_str(), b.path.as_str())),
                )
            })
            .collect();
        assert_eq!(
            tail,
            [
                ("dev-git-title", "text", None),
                (
                    "dev-git-branch",
                    "text",
                    Some(("resource.repo", "head.branch"))
                ),
                ("dev-git-sha", "text", Some(("resource.repo", "head.sha"))),
                (
                    "dev-git-changes",
                    "list",
                    Some(("resource.repo", "changes"))
                ),
            ]
        );
        let changes = dev.root.find("dev-git-changes").unwrap();
        assert_eq!(
            changes.props["columns"],
            json!(["path", "status"]),
            "行の名前 = path・補足 = status"
        );
        for key in ["dev-git-branch", "dev-git-sha", "dev-git-changes"] {
            let n = dev.root.find(key).unwrap();
            assert!(
                n.props.get("text").is_none(),
                "{key}: 値を文字で持たない（data からだけ）"
            );
            assert!(
                n.states.as_ref().and_then(|s| s.get("empty")).is_some(),
                "{key}: 読めた上で空の時の 1 行"
            );
        }
        // Docs と Diagram の Page の木は W3 のまま（Git の欄は Dev の中だけ）
        assert!(plan.project.pages[1..].iter().all(|pg| pg
            .root
            .nodes()
            .iter()
            .all(|n| n.binding.is_none())));
        // workspace が無い時は欄も source も無い（読む repo が無い）
        let none = compose_project(DEMO, &ctx(), None).unwrap().unwrap();
        assert!(none.app.sources.is_empty());
        assert!(none.project.pages[0]
            .root
            .nodes()
            .iter()
            .all(|n| !n.key.starts_with("dev-git")));
    }

    #[test]
    fn w3_without_workspace_dev_page_says_so_and_holds_no_repo() {
        let plan = compose_project(DEMO, &ctx(), None).unwrap().unwrap();
        assert!(plan.project.resources.is_empty() && plan.project.terminal_specs.is_empty());
        let dev: Vec<_> = plan.project.pages[0]
            .root
            .nodes()
            .into_iter()
            .filter_map(|n| n.props.get("text")?.as_str())
            .collect();
        assert!(
            dev.iter()
                .any(|t| t.contains("No folder")),
            "{dev:?}"
        );
        assert!(
            dev.iter().all(|t| !t.contains("SHIBUBU_")),
            "内部の環境変数の名前を画面に出さない: {dev:?}"
        );
        // UI-2: 表（copy.en.md:550）の EN は `No folder` だけ。次の一手（`Choose folder…`）の button は G99c から Generated が同じ Page に添える（sendback 47）
        assert!(
            dev.iter().any(|t| *t == "No folder"),
            "表の EN の 1 行: {dev:?}"
        );
    }

    #[test]
    fn w3_one_page_goals_and_empty_are_not_projects() {
        assert_eq!(
            compose_project("読書記録をつけたい", &ctx(), None),
            Ok(None)
        );
        assert_eq!(
            compose_project("25 分のタイマーを作って", &ctx(), None),
            Ok(None)
        );
        assert_eq!(
            compose_project("   ", &ctx(), None),
            Err(ComposeError::Empty)
        );
    }

    /// W7 holdout（I2 の composer の側）: 「Tests Page を足して」→ AddPage 1 つ・「Excalidraw を右へ」→ MoveSurface 1 つ。
    /// 前提: 生成直後に Tests は無い（在れば W3 が作った事になり I2 の前提崩れ）。過剰側: 2 回目は ops 0 の AlreadyApplied。
    /// 右に Page が無い（生成直後は Diagram が末尾）は NoTarget。読めない文は Unsupported。
    #[test]
    fn w7_edit_project_reads_add_tests_and_move_right() {
        use super::super::domain::project::apply;
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/w7")))
            .unwrap()
            .unwrap();
        let p0 = plan.project;
        assert!(
            p0.pages.iter().all(|pg| pg.title != "Tests"),
            "前提: 生成は Tests を作らない"
        );
        assert!(
            matches!(
                edit_project(&p0, "Excalidraw を右へ"),
                Err(EditError::NoTarget(_))
            ),
            "Diagram は末尾"
        );
        let ops = edit_project(&p0, "Tests Page を足して").unwrap();
        let [ProjectPatchOp::AddPage { page, index: None }] = ops.as_slice() else {
            panic!("AddPage 1 つだけ: {ops:?}")
        };
        assert_eq!(
            (page.id.as_str(), page.route.as_str(), page.title.as_str()),
            ("page.app.t.tests", "/tests", "Tests")
        );
        assert_eq!(page.app_ref.as_deref(), Some("app.t"));
        let helper = page.root.children.iter().find_map(|n| n.props.get("text").and_then(|v| v.as_str()));
        assert_eq!(helper, Some("No runs yet"));
        assert!(!helper.unwrap().contains("exit"), "内部語 exit を画面に出さない");
        assert!(!helper.unwrap().contains("cargo test を打つ"), "Terminal の cargo test を自動では出さない");
        let p1 = apply(&p0, &ops, &[&plan.app]).unwrap();
        assert!(
            matches!(
                edit_project(&p1, "Tests Page を足して"),
                Err(EditError::AlreadyApplied(_))
            ),
            "2 回目は ops 0"
        );
        let ops = edit_project(&p1, "Excalidraw を右へ").unwrap();
        assert_eq!(
            ops,
            [ProjectPatchOp::MoveSurface {
                surface_key: "diagram-surface".into(),
                to_page: "page.app.t.tests".into(),
                parent: "tests-root".into(),
                index: None,
            }]
        );
        let p2 = apply(&p1, &ops, &[&plan.app]).unwrap();
        assert_eq!(p2.surface_specs, p1.surface_specs, "面の定義は変えない");
        assert!(matches!(
            edit_project(&p2, "天気を教えて"),
            Err(EditError::Unsupported(_))
        ));
    }

    // ---- UI-2: 英語の編集の形 4 つ（Done 3・5）。負の対照は各試験の doc と commit message ----

    /// UI-2 Done 3 ①: `add a Tests page` は「Tests Page を足して」と同じ ops（冠詞・大小文字・句点は見ない）。2 回目は AlreadyApplied。
    /// 名前の無い `add a page` は読めない（Unsupported・read も None）。負の対照: parse_edit の `add ... page` の腕を外す → 赤。
    #[test]
    fn ui2_add_a_tests_page_is_the_japanese_add_page() {
        use super::super::domain::project::apply;
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/ui2"))).unwrap().unwrap();
        let p0 = plan.project;
        let ja = edit_project(&p0, "Tests Page を足して").unwrap();
        for said in ["add a Tests page", "Add a Tests page.", "add  an Tests  page", "ADD A Tests PAGE", "“add a Tests page”.", "\"add a Tests page\""] {
            assert_eq!(edit_project(&p0, said), Ok(ja.clone()), "{said}");
            assert_eq!(EditVerb::read(said), Some(EditVerb::AddPage), "{said}");
        }
        let ops = edit_project(&p0, "add tests page").unwrap();
        let [ProjectPatchOp::AddPage { page, .. }] = ops.as_slice() else { panic!("{ops:?}") };
        assert_eq!(page.title, "tests", "冠詞の無い形・名前は打った大小文字のまま");
        let p1 = apply(&p0, &ja, &[&plan.app]).unwrap();
        assert!(matches!(edit_project(&p1, "add a tests page"), Err(EditError::AlreadyApplied(_))), "同じ名前は二重に足さない");
        for said in ["add a page", "add page", "add an page"] {
            assert!(matches!(edit_project(&p0, said), Err(EditError::Unsupported(_))), "{said}");
            assert_eq!(EditVerb::read(said), None, "{said}");
        }
    }

    /// UI-2 Done 3 ②: `move Excalidraw right` は「Excalidraw を右へ」と同じ ops・`move <面> left` は 1 つ左の Page の root の末尾へ・左端は NoTarget。
    /// 負の対照: move_op の左の `checked_sub(1)` を `+ 1` にする / parse_edit の ` left` の腕を外す → 赤。
    #[test]
    fn ui2_move_excalidraw_right_and_left() {
        use super::super::domain::project::apply;
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/ui2"))).unwrap().unwrap();
        let p0 = plan.project;
        assert!(matches!(edit_project(&p0, "move Excalidraw right"), Err(EditError::NoTarget(_))), "生成直後は Diagram が末尾");
        let p1 = apply(&p0, &edit_project(&p0, "add a Tests page").unwrap(), &[&plan.app]).unwrap();
        let ja = edit_project(&p1, "Excalidraw を右へ").unwrap();
        for said in ["move Excalidraw right", "Move excalidraw right.", "move Excalidraw to the right", "Excalidraw to the right", "move the Excalidraw right", "“move Excalidraw right”"] {
            assert_eq!(edit_project(&p1, said), Ok(ja.clone()), "{said}");
            assert_eq!(EditVerb::read(said), Some(EditVerb::MoveRight), "{said}");
        }
        let p2 = apply(&p1, &ja, &[&plan.app]).unwrap();
        let left = edit_project(&p2, "move Excalidraw left").unwrap();
        assert_eq!(EditVerb::read("move Excalidraw left"), Some(EditVerb::MoveLeft));
        assert_eq!(
            left,
            [ProjectPatchOp::MoveSurface {
                surface_key: "diagram-surface".into(),
                to_page: "page.app.t.diagram".into(),
                parent: "diagram-root".into(),
                index: None,
            }],
            "Tests から 1 つ左の Diagram へ戻す"
        );
        let docs = edit_project(&p0, "move TextEdit left").unwrap();
        assert_eq!(
            docs,
            [ProjectPatchOp::MoveSurface { surface_key: "docs-surface".into(), to_page: "page.app.t.dev".into(), parent: "dev-root".into(), index: None }],
            "Docs の TextEdit を左の Dev へ"
        );
        let p3 = apply(&p0, &docs, &[&plan.app]).unwrap();
        // holdout H2 の直し: 左端の 1 行も英語（additions.md の右端の行と対）
        assert_eq!(
            edit_project(&p3, "move TextEdit left"),
            Err(EditError::NoTarget("“TextEdit” is on “Dev”, the first page, so it can't move left".into())),
            "左端の Dev に在る面は左へ動かせない"
        );
        assert!(matches!(edit_project(&p0, "move Nothing left"), Err(EditError::NoTarget(_))), "無い面");
        assert_eq!(EditVerb::read("move right"), None, "面の名前の無い形は読めない");
    }

    /// UI-2 Done 3 ③: `rename Docs to Notes` → 既存の `ProjectPatchOp::RenamePage` 1 つ（id と route は変えない）。Page の名前は大小文字を見ない・
    /// 無い Page は NoTarget・新しい名前が在る Page と同じなら AlreadyApplied。負の対照: parse_edit の `rename` の腕を外す → 赤。
    #[test]
    fn ui2_rename_docs_to_notes() {
        use super::super::domain::project::apply;
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/ui2"))).unwrap().unwrap();
        let p0 = plan.project;
        let want = vec![ProjectPatchOp::RenamePage { page_id: "page.app.t.docs".into(), title: "Notes".into() }];
        for said in ["rename Docs to Notes", "Rename docs to Notes.", "rename  DOCS  to Notes", "rename the Docs page to Notes"] {
            assert_eq!(edit_project(&p0, said), Ok(want.clone()), "{said}");
            assert_eq!(EditVerb::read(said), Some(EditVerb::RenamePage), "{said}");
        }
        let p1 = apply(&p0, &want, &[&plan.app]).unwrap();
        let notes = p1.pages.iter().find(|pg| pg.title == "Notes").expect("名前が替わる");
        assert_eq!((notes.id.as_str(), notes.route.as_str()), ("page.app.t.docs", "/docs"), "id と route は変えない");
        // holdout H3 の直し: 無い Page の 1 行も英語（在る Page を並べる）
        assert_eq!(
            edit_project(&p1, "rename Docs to Readme"),
            Err(EditError::NoTarget("There's no “Docs” page in this project. Pages: Dev, Notes, Diagram".into())),
            "もう Docs は無い"
        );
        assert!(matches!(edit_project(&p1, "rename Notes to dev"), Err(EditError::AlreadyApplied(_))), "在る名前にはしない");
        assert!(matches!(edit_project(&p1, "rename Notes to Notes"), Err(EditError::AlreadyApplied(_))));
        for said in ["rename Docs", "rename to Notes", "rename Docs to "] {
            assert_eq!(EditVerb::read(said), None, "{said}");
        }
    }

    /// UI-2 Done 3 ④: `run the tests` は RunTests と読む（呼ぶ側が PageIntent::RunTests へ回す）・edit_project は ops を作らない。
    /// 負の対照: parse_edit の `run ...` の腕を外す → read が None で赤。
    #[test]
    fn ui2_run_the_tests_is_read_but_makes_no_ops() {
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/ui2"))).unwrap().unwrap();
        for said in ["run the tests", "Run tests.", "run the test", "run test", "RUN THE TESTS!"] {
            assert_eq!(EditVerb::read(said), Some(EditVerb::RunTests), "{said}");
            // holdout H4 の直し: NoTarget の 1 行も英語
            assert_eq!(edit_project(&plan.project, said), Err(EditError::NoTarget("Run tests from the Tests page".into())), "{said}: ops を作らない");
        }
        assert_eq!(EditVerb::read("run the tests now"), None);
    }

    /// holdout H6 と反証の直し（UI-2）: 主デモの一言で組んだ Page の画面の文（node の label と text）に日本語が無い（repo の有る時も無い時も・
    /// 一言〈goal〉は利用者の言葉なので見ない）。repo の有る Dev の `dev-next` は空（表は `(removed; the page has its terminal)`・node は木の形のため残す）で、
    /// Terminal が在る Page に「No terminal」と言わない。負の対照: 面の label を `README.md（TextEdit）` に戻す / `dev-next` に `No terminal on this page` を戻す → 赤。
    #[test]
    fn ui2_composed_pages_speak_english_and_dev_has_no_false_terminal_line() {
        let ja = |s: &str| s.chars().any(|c| matches!(c, '\u{3040}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}' | '\u{FF00}'..='\u{FFEF}'));
        for ws in [Some(Path::new("/tmp/ui2")), None] {
            let plan = compose_project(DEMO, &ctx(), ws).unwrap().unwrap();
            for pg in &plan.project.pages {
                for n in pg.root.nodes() {
                    let shown = n.label.iter().map(String::as_str).chain(n.props.get("text").and_then(|t| t.as_str()));
                    for s in shown {
                        assert!(!ja(s), "ws={}: {}.{} = {s:?}", ws.is_some(), pg.title, n.key);
                    }
                }
            }
            let labels: Vec<_> = plan.project.pages[1..].iter().map(|pg| pg.root.children[0].label.clone().unwrap_or_default()).collect();
            assert_eq!(labels, ["README.md in TextEdit", "Diagram in Excalidraw"], "面の label は file と app 名");
        }
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/ui2"))).unwrap().unwrap();
        assert!(!plan.project.terminal_specs.is_empty(), "正の対照: repo の有る Project は Dev の Terminal を持つ");
        let next = plan.project.pages[0].root.find("dev-next").expect("node は残す");
        assert_eq!(next.props.get("text").and_then(|t| t.as_str()), Some(""), "Terminal の在る Dev に「No terminal」と言わない");
    }

    /// UI-2 Done 5: 例の表の文は read が同じ形を返し、救いの 1 行の 2 つの例は edit_project が変更として読む（順に当てると両方 Ok）。
    /// 日本語の 3 形も read が読み、Unsupported の文には None。負の対照: example の AddPage を表に無い文（`add the page Tests`）にする → 赤。
    #[test]
    fn ui2_examples_read_back_and_the_rescue_line_examples_edit() {
        use super::super::domain::project::apply;
        for verb in [EditVerb::AddPage, EditVerb::MoveRight, EditVerb::MoveLeft, EditVerb::RenamePage, EditVerb::RunTests, EditVerb::AddMirror] {
            let ex = verb.example().unwrap_or_else(|| panic!("{verb:?} の例"));
            assert_eq!(EditVerb::read(ex), Some(verb), "{ex}");
            assert!(ex.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && !ex.ends_with('.'), "打つ形: {ex}");
        }
        assert_eq!(EditVerb::Unsupported.example(), None);
        let line = rescue_line();
        let (a, m) = (EditVerb::AddPage.example().unwrap(), EditVerb::MoveRight.example().unwrap());
        assert_eq!(line, format!("Not sure what to change. Try “{a}” or “{m}”."));
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/ui2"))).unwrap().unwrap();
        let p1 = apply(&plan.project, &edit_project(&plan.project, a).expect("例 1 は変更"), &[&plan.app]).unwrap();
        assert!(edit_project(&p1, m).is_ok_and(|ops| !ops.is_empty()), "例 2 は Page を足した後の変更");
        assert_eq!(EditVerb::read("Tests Page を足して"), Some(EditVerb::AddPage));
        assert_eq!(EditVerb::read("Excalidraw を右へ"), Some(EditVerb::MoveRight));
        assert_eq!(EditVerb::read("notes.md を Mirror で見る Page を足して"), Some(EditVerb::AddMirror));
        for said in ["天気を教えて", "", "  ", "add", "move", "rename", "Page を足して"] {
            assert_eq!(EditVerb::read(said), None, "{said:?}");
            assert!(matches!(edit_project(&plan.project, said), Err(EditError::Unsupported(_))), "{said:?}");
        }
    }

    /// W15-c1（台本 #21・救いの 1 行が案内する形）: 英語の Mirror の一言を名前の表が読む（model に落とさない）。大小文字・余分な空白・末尾の `.`・前後の引用符を許す。
    /// file の無い形は読まない。負の対照: `mirror_page_file` の英語の腕を外す → 1 行目が赤（Unsupported になる）。
    #[test]
    fn w15c1_english_mirror_phrase_reads_in_the_name_table() {
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/w15c1"))).unwrap().unwrap();
        let ja = edit_project(&plan.project, "notes.md を Mirror で見る Page を足して").unwrap();
        for said in ["add a page that mirrors notes.md", "Add a page that mirrors notes.md.", "  add  a page that   MIRRORS notes.md ", "“add a page that mirrors notes.md”"] {
            assert_eq!(EditVerb::read(said), Some(EditVerb::AddMirror), "{said}");
            assert_eq!(edit_project(&plan.project, said).unwrap(), ja, "日本語の形と同じ ops: {said}");
        }
        assert_eq!(parse_edit("add a page that mirrors docs/notes.md").map(|(_, f, _)| f).as_deref(), Some("docs/notes.md"));
        for said in ["add a page that mirrors", "add a page that mirrors ", "add a page that mirror notes.md"] {
            assert_eq!(EditVerb::read(said), None, "{said:?}");
        }
    }

    /// W16M-3: 「notes.md を Mirror で見る Page を足して」→ AddPage + Mirror の AddSurface（TextEdit・docs/notes.md・keep_alive）。
    /// 同じ target の 2 回目は AlreadyApplied。絶対 path と `..` は NoTarget。Tests を足した後の「Mirror を右へ」は足した Mirror の MoveSurface・
    /// 「TextEdit を右へ」は Docs の TextEdit の席（M3: feature `mirror` では席も Mirror だが、「Mirror」は足した方・「TextEdit」は席を指す）。
    #[test]
    fn w16m3_edit_project_adds_mirror_page_and_moves_it_right() {
        use super::super::domain::project::apply;
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/w16m3")))
            .unwrap()
            .unwrap();
        let p0 = plan.project;
        let ops = edit_project(&p0, "notes.md を Mirror で見る Page を足して").unwrap();
        let [ProjectPatchOp::AddPage { page, index: None }, ProjectPatchOp::AddSurface { spec, page_id, parent, node_key, index: None }] =
            ops.as_slice()
        else {
            panic!("AddPage + AddSurface: {ops:?}")
        };
        assert_eq!(
            (page.title.as_str(), page.route.as_str()),
            ("notes", "/notes")
        );
        // Mirror の面を置く Page に「まだ空です」の 1 行を置かない（面の他に node が無い）
        assert!(page.root.children.is_empty(), "Mirror の Page に空の 1 行: {:?}", page.root.children);
        assert_eq!(
            (
                spec.id.as_str(),
                spec.backend,
                spec.keep_alive,
                spec.target.clone()
            ),
            (
                "notes",
                SurfaceBackendKind::Mirror,
                true,
                SurfaceTarget::App {
                    bundle_id: "com.apple.TextEdit".into(),
                    file: Some("docs/notes.md".into())
                }
            )
        );
        assert_eq!(
            (page_id.as_str(), parent.as_str(), node_key.as_str()),
            (page.id.as_str(), page.root.key.as_str(), "surface.notes")
        );
        let p1 = apply(&p0, &ops, &[&plan.app]).unwrap();
        assert!(matches!(
            edit_project(&p1, "notes.md を Mirror で見る Page を足して"),
            Err(EditError::AlreadyApplied(_))
        ));
        assert!(matches!(
            edit_project(&p0, "/tmp/notes.md を Mirror で見る Page を足して"),
            Err(EditError::NoTarget(_))
        ));
        assert!(matches!(
            edit_project(&p0, "../notes.md を Mirror で見る Page を足して"),
            Err(EditError::NoTarget(_))
        ));
        let p2 = apply(
            &p1,
            &edit_project(&p1, "Tests Page を足して").unwrap(),
            &[&plan.app],
        )
        .unwrap();
        let ops = edit_project(&p2, "Mirror を右へ").unwrap();
        assert_eq!(
            ops,
            [ProjectPatchOp::MoveSurface {
                surface_key: "surface.notes".into(),
                to_page: p2.pages.last().unwrap().id.clone(),
                parent: p2.pages.last().unwrap().root.key.clone(),
                index: None,
            }]
        );
        assert_eq!(
            edit_project(&p2, "TextEdit を右へ").unwrap(),
            [ProjectPatchOp::MoveSurface {
                surface_key: "docs-surface".into(),
                to_page: p2.pages[2].id.clone(),
                parent: p2.pages[2].root.key.clone(),
                index: None,
            }],
            "TextEdit は Docs の席（Diagram へ）"
        );
        let m = Canned::new(vec![Ok(
            json!({ "steps": [{ "verb": "add_mirror", "name": "notes.md" }] }),
        )]);
        assert_eq!(
            edit_project_by_model(&p0, "notes.md を Mirror で見る Page を足して", &m).unwrap(),
            edit_project(&p0, "notes.md を Mirror で見る Page を足して").unwrap()
        );
    }

    /// AI-COMPOSE-1101（owner 206）: add_part は AI の HTML を canvas の node 1 つにし、きっかけの手の部品を持つ Page（model の page より先）の末尾に置く
    /// （保存の前の検査を通る・値は空から）。ENGINE-GENERIC-1101: prompt に部品の手（`TRIGGER_ACTS` から `<録音の key>.recorded`）を見せ、決まった機能の例
    /// （Add take・app の名前）は見せない。html が空・きっかけが Page に無い手・その部品の起こさない手・key と {value} が合わない → ops 0 でもう一度頼む 1 行。
    /// 負の対照: `part_op` の Page の選び方から「きっかけの手の Page」を外す → page_id の assert が赤・`canvas_fault` の component の照合を外す → Page の root の `recorded` が通って赤。
    #[test]
    fn ai_compose_add_part_places_the_canvas_by_its_trigger() {
        use super::super::domain::project::apply;
        use super::super::domain::ui::CanvasProps;
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/w7"))).unwrap().unwrap();
        let mut p = plan.project.clone();
        p.pages[1].root.children.push(node("take", "recorder", None, json!({ "label": "Record humming", "max_seconds": 60, "recorder_ref": "take", "feeds": null }), vec![]));
        let step_with = |html: &str, act: &str, key: &str, intent: &str| {
            json!({ "steps": [{ "verb": "add_part", "name": "Guitar", "page": "Dev", "html": html,
                "on": [{ "act": act, "key": key, "intent": intent }] }] })
        };
        let step = |html: &str, act: &str| step_with(html, act, "guitar", "Set the track's instrument to {value}");
        struct Seen(std::cell::RefCell<String>, Value);
        impl ModelProvider for Seen {
            fn complete_json(&self, prompt: &str, _schema: &Value) -> Result<Value, String> {
                *self.0.borrow_mut() = prompt.to_owned();
                Ok(self.1.clone())
            }
        }
        let m = Seen(Default::default(), step("<select id=g></select>", "take.recorded"));
        let ops = edit_project_by_model(&p, "ギターの種類を選べるようにして", &m).expect("部品の 1 手");
        let prompt = m.0.borrow().clone();
        assert!(prompt.contains("take.recorded"), "prompt に部品の手が無い（model が on の act を写せない）");
        assert!(!prompt.contains(".add") && !prompt.contains("Add take") && !prompt.contains("instrument"), "prompt に決まった機能の例: {prompt}");
        let [ProjectPatchOp::Page { page_id, .. }] = &ops[..] else { panic!("Page の 1 手でない: {ops:?}") };
        assert_eq!(*page_id, p.pages[1].id, "きっかけの手の Page に置かない");
        let next = apply(&p, &ops, &[&plan.app]).expect("保存の前の検査を通る");
        let part = next.pages[1].root.children.last().unwrap();
        assert_eq!((part.component.as_str(), part.label.as_deref()), ("canvas", Some("Guitar")));
        let c = CanvasProps::of(part).unwrap();
        assert!(c.values.is_empty() && c.on.len() == 1 && c.on[0].act == "take.recorded", "{c:?}");
        // 値を使わないきっかけ（key が空・{value} 無し）も置ける
        let plain = Canned::new(vec![Ok(step_with("<p>x</p>", "take.recorded", "", "Put the take just recorded on the selected track"))]);
        assert!(edit_project_by_model(&p, "x", &plain).is_ok(), "値を使わないきっかけを拒否した");
        // 帯でない部品（Page の root）の recorded・取り下げた add・Page に無い部品・key と {value} の食い違いは置かない
        let root = p.pages[1].root.key.clone();
        for bad in [step("  ", "take.recorded"), step("<p>x</p>", "nothing.recorded"), step("<p>x</p>", "take.add"), step("<p>x</p>", &format!("{root}.recorded")),
            step_with("<p>x</p>", "take.recorded", "", "Use {value}"), step_with("<p>x</p>", "take.recorded", "guitar", "Use it")]
        {
            let m = Canned::new(vec![Ok(bad)]);
            assert!(matches!(edit_project_by_model(&p, "x", &m), Err(EditError::NoTarget(_))), "保存できない部品を ops にした");
        }
    }

    /// 反証の指摘（W7）: route が埋まっているだけでは「既に在ります」と言わない。重なるのは title だけ。
    #[test]
    fn w7_add_page_refuses_only_same_title_and_finds_free_route() {
        use super::super::domain::project::apply;
        let plan = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/w7")))
            .unwrap()
            .unwrap();
        let add = |p: &ProjectDefinitionV1, said: &str| {
            let ops = edit_project(p, said).unwrap_or_else(|e| panic!("{said}: {e:?}"));
            apply(p, &ops, &[&plan.app]).unwrap()
        };
        // (1) ASCII の無い名前の番号 slug が、Page を消した後に既存の /pN と重なる
        let p = add(&plan.project, "テスト Page を足して");
        let docs = p
            .pages
            .iter()
            .find(|pg| pg.title == "Docs")
            .unwrap()
            .id
            .clone();
        let p = apply(
            &p,
            &[ProjectPatchOp::RemovePage { page_id: docs }],
            &[&plan.app],
        )
        .unwrap();
        let p = add(&p, "メモ Page を足して");
        assert!(
            p.pages.iter().any(|pg| pg.title == "メモ"),
            "メモ が足される"
        );
        // (2) 名前替えで route /tests が残っても、Tests は足せる
        let p = add(&p, "Tests Page を足して");
        let tests = p
            .pages
            .iter()
            .find(|pg| pg.title == "Tests")
            .unwrap()
            .id
            .clone();
        let p = apply(
            &p,
            &[ProjectPatchOp::RenamePage {
                page_id: tests,
                title: "Checks".into(),
            }],
            &[&plan.app],
        )
        .unwrap();
        let p = add(&p, "Tests Page を足して");
        let mut routes: Vec<_> = p.pages.iter().map(|pg| pg.route.as_str()).collect();
        let mut ids: Vec<_> = p.pages.iter().map(|pg| pg.id.as_str()).collect();
        let n = routes.len();
        routes.sort();
        routes.dedup();
        ids.sort();
        ids.dedup();
        assert_eq!(
            (routes.len(), ids.len()),
            (n, n),
            "route と id は重ならない: {:?}",
            p.pages
        );
        // 同じ title は今まで通り断る（大文字小文字は見ない）
        assert!(matches!(
            edit_project(&p, "tests Page を足して"),
            Err(EditError::AlreadyApplied(_))
        ));
    }

    // ---- W13: 一言・編集を model に読ませる（Canned = 決めた答えを返す model） ----

    /// 決めた答えを返す model（呼ばれた回数を数える）。主デモの Plan を返し続ける物は I3 の負の対照「固定の生成器」に当たる
    struct Canned(
        std::cell::RefCell<Vec<Result<Value, String>>>,
        std::cell::Cell<usize>,
    );
    impl Canned {
        fn new(replies: Vec<Result<Value, String>>) -> Self {
            Canned(std::cell::RefCell::new(replies), std::cell::Cell::new(0))
        }
    }
    impl ModelProvider for Canned {
        fn complete_json(&self, _prompt: &str, _schema: &Value) -> Result<Value, String> {
            self.1.set(self.1.get() + 1);
            self.0.borrow_mut().remove(0)
        }
    }

    fn seat(
        title: &str,
        kind: &str,
        app: Option<&str>,
        bundle: Option<&str>,
        url: Option<&str>,
    ) -> Value {
        json!({ "title": title, "kind": kind, "app_name": app, "bundle_id": bundle, "file": null, "url": url, "install_url": null, "instead": [], "feeds": null })
    }

    /// G88a: 置く Page と区画と理由のついた席（schema の required が全部在る形）
    fn placed(title: &str, kind: &str, app: Option<&str>, url: Option<&str>, page: i64, slot: &str) -> Value {
        let mut s = seat(title, kind, app, None, url);
        s["page"] = json!(page);
        s["slot"] = json!(slot);
        s["why"] = json!(format!("{title} を{slot}に"));
        s
    }

    /// W13b の試験の Plan を 1 席 1 Page に（利用者が Page を分けると明示した形・席の並び = Page の並び）。G88a の既定（外部 3 つまでは 1 Page）の前の形を試すため
    fn each_on_its_own_page(mut plan: Value) -> Value {
        plan["pages_requested"] = json!(true);
        for (i, s) in plan["seats"].as_array_mut().unwrap().iter_mut().enumerate() {
            s["page"] = json!(i);
        }
        plan
    }

    /// M3（owner 24 通目の続き）: 生成器の app の席の backend は feature `mirror` の時だけ Mirror。Docs の TextEdit（`compose_project`）と model の app の席
    /// （`plan_project`）が同じ値。web の席は WebView のまま。feature なしの build（W15 の .app）では NativeDirect＝「外で開く」のまま。
    #[test]
    fn w16m3_app_seats_are_mirror_only_with_the_feature() {
        let want = if cfg!(feature = "mirror") { SurfaceBackendKind::Mirror } else { SurfaceBackendKind::NativeDirect };
        assert_eq!(APP_SEAT_BACKEND, want);
        let ws = Path::new("/tmp/m3-ws");
        let demo = compose_project(DEMO, &ctx(), Some(ws)).unwrap().expect("主デモは Project");
        let backends: Vec<_> = demo.project.surface_specs.iter().map(|s| (s.id.as_str(), s.backend)).collect();
        assert_eq!(backends, [("docs", want), ("diagram", SurfaceBackendKind::WebView)]);
        let m = Canned::new(vec![Ok(json!({ "shape": "project", "name": "開発", "seats": [
            seat("Docs", "app", Some("TextEdit"), None, None),
            seat("図", "web", None, None, Some("https://excalidraw.com")),
        ]}))]);
        let apps = [KnownApp { name: "TextEdit".into(), bundle_id: "com.apple.TextEdit".into() }];
        let plan = plan_project("README は TextEdit・図は excalidraw で m3", &m, &apps, &[], &ctx(), Some(ws)).unwrap().expect("project の形");
        let backends: Vec<_> = plan.project.surface_specs.iter().map(|s| s.backend).collect();
        assert_eq!(backends, [want, SurfaceBackendKind::WebView]);
    }

    /// schema の required と型の欄が両向きに一致する（schema にだけ在る欄は deny_unknown_fields で、型にだけ在る欄は直列化した key で赤）。
    #[test]
    fn w13_plan_and_edit_schemas_match_their_types() {
        let keys = |v: &Value| -> Vec<String> {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        let required = |s: &Value| -> Vec<String> {
            let mut k: Vec<String> = s["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect();
            k.sort();
            k
        };
        let s = plan_schema();
        let item = &s["properties"]["seats"]["items"];
        let plan = json!({ "shape": "project", "name": "議事録", "pages_requested": false, "seats": [
            placed("Notes", "app", Some("Notes Maker"), None, 0, "main"),
            placed("参考", "web", None, Some("https://example.org/a"), 0, "side"),
            placed("Dev", "terminal", None, None, 0, "bottom"),
        ]});
        let read: PlanV1 =
            serde_json::from_value(plan.clone()).expect("schema の形の答えは PlanV1 として読める");
        let back = serde_json::to_value(&read).unwrap();
        assert_eq!(back, plan, "往復で変わらない");
        assert_eq!(keys(&back), required(&s));
        assert_eq!(keys(&back["seats"][0]), required(item));
        assert_eq!(s["title"], PLAN_NAME);
        let mut extra = plan.clone();
        extra["seats"][0]["frame"] = json!(1);
        assert!(
            serde_json::from_value::<PlanV1>(extra).is_err(),
            "未知の欄は拒否"
        );

        let e = edit_schema();
        // AI-COMPOSE-1101: 部品を書く形（add_part）は別の形（anyOf の 2 つ目）・前の 4 つの答えは 1 つ目のまま
        let edit = json!({ "steps": [{ "verb": "add_page", "name": "Tests" }, { "verb": "move_right", "name": "excalidraw" },
            { "verb": "add_part", "name": "Guitar", "page": "Dev", "html": "<select></select>", "on": [{ "act": "take.recorded", "key": "guitar", "intent": "Use {value}" }] }] });
        let read: EditPlanV1 = serde_json::from_value(edit.clone()).unwrap();
        let back = serde_json::to_value(&read).unwrap();
        assert_eq!(back, edit);
        assert_eq!(keys(&back), required(&e));
        let shapes = &e["properties"]["steps"]["items"]["anyOf"];
        assert_eq!(keys(&back["steps"][0]), required(&shapes[0]));
        assert_eq!(keys(&back["steps"][2]), required(&shapes[1]));
        assert_eq!(e["title"], EDIT_NAME);
    }

    /// I3 の形: 席の target は一言が名指しした app（一覧で引き直した bundle id・model の当て推量より先）と site の集合。名前の表は引かない。
    #[test]
    fn w13_plan_targets_follow_the_named_app_and_site() {
        let m = Canned::new(vec![Ok(
            json!({ "shape": "project", "name": "議事録", "seats": [
                seat("Notes", "app", Some("notes maker.app"), Some("com.example.guess"), None),
                seat("参考", "web", None, None, Some("https://example.org/a")),
            ]}),
        )]);
        let apps = [KnownApp {
            name: "Notes Maker".into(),
            bundle_id: "org.example.notes".into(),
        }];
        let plan = plan_project(
            "議事録を Notes Maker で・参考は example で n-13",
            &m,
            &apps,
            &[],
            &ctx(),
            None,
        )
        .unwrap()
        .expect("project の形");
        assert_eq!((plan.interpreter.as_str(), m.1.get()), ("model", 1));
        assert_eq!(
            plan.project.goal,
            "議事録を Notes Maker で・参考は example で n-13"
        );
        let targets: Vec<SurfaceTarget> = plan
            .project
            .surface_specs
            .iter()
            .map(|s| s.target.clone())
            .collect();
        assert_eq!(
            targets,
            vec![
                SurfaceTarget::App {
                    bundle_id: "org.example.notes".into(),
                    file: None
                },
                SurfaceTarget::Url {
                    url: "https://example.org/a".into()
                },
            ]
        );
        assert!(plan.project.surface_specs.iter().all(|s| s.keep_alive));
    }

    /// W13b 本体（25 通目）: 一言が名指した app がこの Mac の一覧に無い席は、面を黙って落とさず、その Page に 1 問（席の写し）を置く。
    /// 在る app には面を置き問いを出さない・一覧に無い当て推量の bundle id に面を置かない・代わりは一覧に在る物だけ（重ねず・多くて 3・一覧の名前）・
    /// 入れる頁は https だけ・席の数 = Page の数（Page を分けると明示した Plan）。使える検査の席（`verifier::project_seats`）は 1 問ごとに MissingApp（id = 席の slug = 1 席 1 Page では Page の route の名前）。
    /// 負の対照: 当て推量を一覧で絞らない（W13 の `known.or(guessed)` に戻す）→ 面が 2 つになり赤・`instead` を一覧で絞らない → Ghost が残り赤・
    /// 1 問の写しを props に置かない（W13 の 1 行だけに戻す）→ 「1 問だけ」で赤。
    #[test]
    fn w13b_missing_app_is_one_question_in_its_page_and_an_installed_app_docks() {
        let known = |name: &str, bundle_id: &str| KnownApp {
            name: name.into(),
            bundle_id: bundle_id.into(),
        };
        let apps = [
            known("Notes Maker", "org.example.notes"),
            known("Brushwork", "org.example.brushwork"),
            known("Sketchpad", "org.example.sketch"),
            known("Inkwell", "org.example.ink"),
        ];
        let mut zeta = seat("Draw", "app", Some("Zeta Edit"), Some("dev.example.zeta"), None);
        zeta["install_url"] = json!(" https://example.org/zeta ");
        zeta["instead"] = json!([
            { "name": "brush", "bundle_id": "org.example.brushwork" },
            { "name": "Ghost", "bundle_id": "org.example.ghost" },
            { "name": "Brushwork", "bundle_id": "org.example.brushwork" },
            { "name": "Sketchpad", "bundle_id": "org.example.sketch" },
            { "name": "Notes Maker", "bundle_id": "org.example.notes" },
            { "name": "Inkwell", "bundle_id": "org.example.ink" }
        ]);
        let mut paint = seat("Paint", "app", Some("Paint X"), None, None);
        paint["install_url"] = json!("http://example.org/paint");
        let m = Canned::new(vec![Ok(each_on_its_own_page(json!({ "shape": "project", "name": "作業場", "seats": [
            seat("Notes", "app", Some("Notes Maker"), None, None),
            zeta,
            paint,
            seat("Dev", "terminal", None, None, None),
        ]})))]);
        let plan = plan_project("Notes Maker と Zeta Edit と Paint X で作業場 n-13b", &m, &apps, &[], &ctx(), None)
            .unwrap()
            .expect("project の形");
        let p = &plan.project;
        assert_eq!(m.1.get(), 1, "model の呼び出しは 1 回のまま");
        assert_eq!(p.pages.len(), 4, "席の数 = Page の数（黙って落とさない）");
        assert_eq!(
            p.surface_specs.iter().map(|s| s.target.clone()).collect::<Vec<_>>(),
            [SurfaceTarget::App { bundle_id: "org.example.notes".into(), file: None }],
            "面は一覧に在る app だけ（一覧に無い当て推量の bundle id に面を置かない）"
        );
        let asked = |i: usize| -> Vec<SeatV1> { p.pages[i].root.nodes().into_iter().filter_map(SeatV1::asked_in).collect() };
        assert!(asked(0).is_empty() && asked(3).is_empty(), "在る app・Terminal の Page に問いを出した");
        let [z]: [SeatV1; 1] = asked(1).try_into().expect("無い app の Page に 1 問だけ");
        assert_eq!(z.app_name.as_deref(), Some("Zeta Edit"));
        assert_eq!(z.install_url.as_deref(), Some("https://example.org/zeta"));
        assert_eq!(z.bundle_id, None, "一覧に無い当て推量の bundle id を写しに残した");
        assert_eq!(
            z.instead.iter().map(|a| (a.name.as_str(), a.bundle_id.as_str())).collect::<Vec<_>>(),
            [("Brushwork", "org.example.brushwork"), ("Sketchpad", "org.example.sketch"), ("Notes Maker", "org.example.notes")],
            "代わりは一覧に在る物だけ・重ねず・model の順に多くて 3・名前は一覧の名前"
        );
        let [x]: [SeatV1; 1] = asked(2).try_into().expect("無い app の Page に 1 問だけ");
        assert_eq!((x.install_url, x.instead.len()), (None, 0), "https でない入れる頁を残した");
        let line = p.pages[1].root.nodes().into_iter().find(|n| SeatV1::asked_in(n).is_some()).and_then(|n| n.props["text"].as_str()).unwrap_or("");
        assert!(line.contains("Zeta Edit isn't on this Mac"), "前の版が描く 1 行は今のまま: {line}");
        let seats: Vec<(String, String)> = super::super::verifier::project_seats(p)
            .into_iter()
            .filter(|s| s.kind == super::super::verifier::SeatKind::MissingApp)
            .map(|s| (s.id, s.name))
            .collect();
        assert_eq!(
            seats,
            [(p.pages[1].route[1..].to_string(), "Zeta Edit".to_string()), (p.pages[2].route[1..].to_string(), "Paint X".to_string())],
            "1 問ごとに MissingApp の席（id = Page の route の名前・名前 = 名指しの app）"
        );
    }

    /// W13b: 一覧も全部も空（読めなかった）時だけ、bundle id の形の当て推量を今まで通り信じる。一覧に在る bundle id なら名前が違っても面を置く。
    #[test]
    fn w13b_a_guess_docks_only_when_listed_or_when_the_list_was_unreadable() {
        let listed = [KnownApp {
            name: "ZetaEdit Pro".into(),
            bundle_id: "dev.example.zeta".into(),
        }];
        for (apps, why) in [(&[][..], "一覧が空"), (&listed[..], "一覧に在る bundle id")] {
            let m = Canned::new(vec![Ok(json!({ "shape": "project", "name": "作業場", "seats": [
                seat("Draw", "app", Some("Zeta Edit"), Some("dev.example.zeta"), None),
            ]}))]);
            let p = plan_project("Zeta Edit で描きたい n-13b", &m, apps, &[], &ctx(), None).unwrap().unwrap().project;
            assert_eq!(
                p.surface_specs.iter().map(|s| s.target.clone()).collect::<Vec<_>>(),
                [SurfaceTarget::App { bundle_id: "dev.example.zeta".into(), file: None }],
                "{why}"
            );
            assert!(p.pages[0].root.nodes().into_iter().all(|n| SeatV1::asked_in(n).is_none()), "{why}: 面を置いた席に問いを出した");
        }
    }

    /// W13b の反証の直し: model に見せる一覧の外でも、この Mac に在る app（Finder など CoreServices の下の app・`installed_apps` の全部）は 1 問にせず面を置く。
    /// 名指しの名前でも、model の当て推量の bundle id でも引く。どこにも無い app だけが 1 問。代わりの候補は見せた一覧の物だけ（model が見たのはそれだけ）。
    /// 負の対照: 全部（`on_mac`）を引かない（一覧だけで決める）→ Folder Browser と Files の 2 席が 1 問になり赤。
    #[test]
    fn w13b_an_app_on_this_mac_outside_the_list_docks_and_only_an_app_nowhere_is_asked() {
        let known = |name: &str, bundle_id: &str| KnownApp { name: name.into(), bundle_id: bundle_id.into() };
        let shown = [known("Notes Maker", "org.example.notes")];
        let on_mac = [known("Folder Browser", "com.example.finder"), known("Notes Maker", "org.example.notes")];
        let mut ghost = seat("Ghost", "app", Some("Ghost Pad"), Some("org.example.ghost"), None);
        ghost["instead"] = json!([{ "name": "Folder Browser", "bundle_id": "com.example.finder" }, { "name": "Notes Maker", "bundle_id": "org.example.notes" }]);
        let m = Canned::new(vec![Ok(each_on_its_own_page(json!({ "shape": "project", "name": "作業場", "seats": [
            // 一覧に無いので model は bundle id を null にする（prompt のとおり）: 名前で全部から引く
            seat("Folder", "app", Some("folder browser.app"), None, None),
            // 名前は違うが当て推量の bundle id が全部に在る
            seat("Files", "app", Some("Files"), Some("COM.example.finder"), None),
            ghost,
        ]})))]);
        let p = plan_project("Folder Browser と Files と Ghost Pad で作業場 n-13b", &m, &shown, &on_mac, &ctx(), None).unwrap().unwrap().project;
        assert_eq!(
            p.surface_specs.iter().map(|s| (s.id.as_str(), s.target.clone())).collect::<Vec<_>>(),
            [
                ("folder", SurfaceTarget::App { bundle_id: "com.example.finder".into(), file: None }),
                ("files", SurfaceTarget::App { bundle_id: "com.example.finder".into(), file: None }),
            ],
            "この Mac に在る app を 1 問にした"
        );
        let asked: Vec<Vec<SeatV1>> = p.pages.iter().map(|pg| pg.root.nodes().into_iter().filter_map(SeatV1::asked_in).collect()).collect();
        assert_eq!(asked.iter().map(Vec::len).collect::<Vec<_>>(), [0, 0, 1], "どこにも無い app だけが 1 問");
        assert_eq!(
            asked[2][0].instead.iter().map(|a| a.bundle_id.as_str()).collect::<Vec<_>>(),
            ["org.example.notes"],
            "代わりの候補に model に見せていない app を入れた"
        );
    }

    /// W13b: `SeatV1::asked_in` の保証（key が無い・読めない・kind が app でない → None・保存した入れる頁は https だけ）。
    #[test]
    fn w13b_asked_in_reads_only_an_app_seat_and_keeps_only_https() {
        let mut n = text("draw-next", "1 行", "helper");
        assert_eq!(SeatV1::asked_in(&n), None, "key が無い");
        n.props.insert(ASKED_PROP.into(), json!({ "title": "Draw" }));
        assert_eq!(SeatV1::asked_in(&n), None, "読めない");
        n.props.insert(ASKED_PROP.into(), seat("Web", "web", None, None, Some("https://example.org/a")));
        assert_eq!(SeatV1::asked_in(&n), None, "app でない");
        let mut app = seat("Draw", "app", Some("Zeta Edit"), None, None);
        app["install_url"] = json!("javascript:alert(1)");
        n.props.insert(ASKED_PROP.into(), app);
        let s = SeatV1::asked_in(&n).expect("app の席の写し");
        assert_eq!((s.app_name.as_deref(), s.install_url), (Some("Zeta Edit"), None), "https でない入れる頁を link にしない");
    }

    /// 反証の直し（W13）: Terminal の席の id は Project をまたいで重ならない（`terminal_sessions` は id だけで引く）。Page の terminal node はその id を指す。
    #[test]
    fn w13_terminal_ids_do_not_collide_across_projects() {
        let ws = Path::new("/tmp/w13-ws");
        let ids: Vec<(String, String)> = ["app.a", "app.b"]
            .iter()
            .map(|app| {
                let m = Canned::new(vec![Ok(
                    json!({ "shape": "project", "name": "dev", "seats": [seat("Dev", "terminal", None, None, None)] }),
                )]);
                let c = ComposeContext { app_id: Some(app.to_string()), ..ComposeContext::default() };
                let p = plan_project("repo を開いて", &m, &[], &[], &c, Some(ws)).unwrap().unwrap().project;
                let node = p.pages[0].root.nodes().into_iter().find(|n| n.component == "terminal").unwrap().clone();
                (p.terminal_specs[0].id.clone(), node.props["terminal_ref"].as_str().unwrap().to_string())
            })
            .collect();
        assert_eq!(ids[0].0, ids[0].1, "node は spec を指す");
        assert_ne!(ids[0].0, ids[1].0, "Project をまたいで重ならない: {ids:?}");
        let a = compose_project(
            DEMO,
            &ComposeContext {
                app_id: Some("app.a".into()),
                ..ComposeContext::default()
            },
            Some(ws),
        )
        .unwrap()
        .unwrap();
        let b = compose_project(
            DEMO,
            &ComposeContext {
                app_id: Some("app.b".into()),
                ..ComposeContext::default()
            },
            Some(ws),
        )
        .unwrap()
        .unwrap();
        assert_ne!(
            a.project.terminal_specs[0].id, b.project.terminal_specs[0].id,
            "名前の表の道も"
        );
    }

    #[test]
    fn w13_model_failure_and_bad_answers_are_model_errors() {
        let bad = [
            Err("LLM の CLI が答えませんでした".to_string()),
            Ok(json!({ "shape": "project", "name": "x", "seats": [] })),
            Ok(
                json!({ "shape": "project", "name": "x", "seats": [seat("", "web", None, None, Some("https://example.org"))] }),
            ),
            Ok(
                json!({ "shape": "project", "name": "x", "seats": [seat("A", "app", None, None, Some("https://example.org"))] }),
            ),
            Ok(json!({ "nope": true })),
        ];
        for reply in bad {
            let shown = format!("{reply:?}");
            let m = Canned::new(vec![reply]);
            assert!(
                matches!(
                    plan_project("何か作って", &m, &[], &[], &ctx(), None),
                    Err(ComposeError::Model(_))
                ),
                "{shown}"
            );
        }
        let one = Canned::new(vec![Ok(
            json!({ "shape": "single_app", "name": "読書記録", "seats": [] }),
        )]);
        assert_eq!(
            plan_project("読書記録を作って", &one, &[], &[], &ctx(), None),
            Ok(None)
        );
        let none = Canned::new(vec![]);
        assert_eq!(
            plan_project("  ", &none, &[], &[], &ctx(), None),
            Err(ComposeError::Empty)
        );
        assert_eq!(none.1.get(), 0, "空の一言で model を呼ばない");
    }

    #[test]
    fn w13_model_edit_gives_the_same_ops_as_the_name_table() {
        let ws = Path::new("/tmp/shibubu-yc-demo");
        let p = compose_project(DEMO, &ctx(), Some(ws))
            .unwrap()
            .unwrap()
            .project;
        let m = Canned::new(vec![Ok(
            json!({ "steps": [{ "verb": "add_page", "name": "Tests" }] }),
        )]);
        assert_eq!(
            edit_project_by_model(&p, "テスト用のページを追加して", &m).unwrap(),
            edit_project(&p, "Tests Page を足して").unwrap()
        );
        let m = Canned::new(vec![Ok(
            json!({ "steps": [{ "verb": "unsupported", "name": "色を変えて" }] }),
        )]);
        assert!(matches!(
            edit_project_by_model(&p, "色を変えて", &m),
            Err(EditError::Unsupported(_))
        ));
        let m = Canned::new(vec![Err("timeout".into())]);
        assert!(matches!(
            edit_project_by_model(&p, "Tests Page を足して", &m),
            Err(EditError::Model(_))
        ));
    }

    /// M2 G-f: 開いている app の中の操作（REAPER の track を足す・Discord の channel を開く）は Project の編集ではない。名前の表は Page を足さず
    /// （" page" で終わる文だけ）`Unsupported` = Page の欄から Engine へ・`EditVerb::read` も None（model が在れば model が読む）。
    /// model の prompt は「app の中の操作は unsupported」の 1 行を持つ（本物の model がそう読むかは未測）。
    #[test]
    fn m2_in_app_requests_are_not_project_edits() {
        struct Prompted(std::cell::RefCell<String>);
        impl ModelProvider for Prompted {
            fn complete_json(&self, prompt: &str, _schema: &Value) -> Result<Value, String> {
                *self.0.borrow_mut() = prompt.into();
                Ok(json!({ "steps": [{ "verb": "unsupported", "name": "Add a new track" }] }))
            }
        }
        let p = compose_project(DEMO, &ctx(), Some(Path::new("/tmp/shibubu-yc-demo")))
            .unwrap()
            .unwrap()
            .project;
        for said in ["Add a new track", "Open the general channel"] {
            assert!(matches!(edit_project(&p, said), Err(EditError::Unsupported(_))), "{said}");
            assert_eq!(EditVerb::read(said), None, "{said}");
        }
        assert!(edit_project(&p, "Add a Tracks page").is_ok(), "正の対照: \" page\" で終わる文は Page を足す");
        let m = Prompted(Default::default());
        assert!(matches!(edit_project_by_model(&p, "Add a new track", &m), Err(EditError::Unsupported(_))));
        assert!(m.0.borrow().contains("app の中の操作"), "{}", m.0.borrow());
    }

    /// 反証の直し（W13）: 中身を持つ app を包んだ Project（読書記録・form と list に action_ref）でも、2 手の編集は model の答えのまま組め、
    /// 本物の app で照合して当たる（前は途中の写しを action を持たない殻の app で照合し、MissingAction を AI の失敗と言っていた）。
    #[test]
    fn w13_model_edit_of_two_steps_on_a_wrapped_app() {
        let app = super::super::compose("読書記録を作って", &ctx())
            .unwrap()
            .into_app();
        let p = ProjectDefinitionV1::wrap(&app);
        assert!(
            p.pages[0]
                .root
                .nodes()
                .iter()
                .any(|n| n.action_ref.is_some()),
            "中身を持つ app を包んでいる"
        );
        let m = Canned::new(vec![Ok(json!({ "steps": [
            { "verb": "add_page", "name": "Tests" },
            { "verb": "add_page", "name": "Docs" }
        ] }))]);
        let ops =
            edit_project_by_model(&p, "Tests Page と Docs Page を足して", &m).expect("2 手を組む");
        let next = super::super::domain::project::apply(&p, &ops, &[&app])
            .expect("本物の app で照合して当たる");
        let titles: Vec<&str> = next.pages.iter().map(|pg| pg.title.as_str()).collect();
        assert!(titles.ends_with(&["Tests", "Docs"]), "{titles:?}");
    }

    // ---- G88a: 既定は 1 Page（外部の service が ONE_PAGE_MAX_SERVICES まで）・明示か超えた時だけ Page を分ける・置き場は model の slot ----

    /// 席の node の model の置き場（`layout::SEAT_PROP` の slot）
    fn seat_slot(n: &UiNode) -> Option<String> {
        n.props.get(super::super::layout::SEAT_PROP).and_then(|v| v["slot"].as_str()).map(str::to_string)
    }

    /// Page ごとの面の id（source_surface の surface_ref・木の順）
    fn faces(p: &ProjectDefinitionV1) -> Vec<Vec<String>> {
        p.pages.iter().map(|pg| pg.root.nodes().into_iter().filter_map(|n| n.props.get("surface_ref").and_then(Value::as_str).map(str::to_string)).collect()).collect()
    }

    /// G88a（owner 10-27「基本的に作るのは 1 page だが user によって明示された場合と 3 つ以上の外部 service を入れる場合は 2 page 以降に」・完成予想
    /// docs/assets/music-workspace-concept.png）: model が席を別々の page に置いても、明示が無く外部（app と web）が 3 つまでなら全席を Page 0（title = Project の名前）。
    /// terminal は数えない。置き場は model の slot を今の型の形（`layout::seat_template`）にして `place` に渡すと main に REAPER・side に Music と YouTube（並び順）・
    /// bottom に terminal。席の node に slot と why が残る。明示（pages_requested）が在れば model の page で束ねる（番号は詰める）。
    /// 負の対照（commit message）: 外部の数を見ない（明示が無くても model の page に従う）→ Page 3 枚で赤／pages_requested を見ない → 明示の Plan が 1 枚で赤
    #[test]
    fn g88a_three_services_share_one_page_in_the_models_slots_unless_pages_are_asked_for() {
        use super::super::layout::{place, seat_template, SEAT_PROP, SEAT_TEMPLATE};
        let apps = [KnownApp { name: "REAPER".into(), bundle_id: "com.cockos.reaper".into() }, KnownApp { name: "Music".into(), bundle_id: "com.apple.Music".into() }];
        let ws = Path::new("/tmp/g88a-ws");
        let said = "REAPER と Apple Music と YouTube を見ながら EDM を作る workspace";
        let edm = |requested: bool| json!({ "shape": "project", "name": "EDM 音楽制作", "pages_requested": requested, "seats": [
            placed("REAPER", "app", Some("REAPER"), None, 0, "main"),
            placed("Apple Music", "app", Some("Music"), None, 1, "side"),
            placed("YouTube", "web", None, Some("https://www.youtube.com"), 2, "side"),
            placed("Shell", "terminal", None, None, 5, "bottom"),
        ]});
        let p = plan_project(said, &Canned::new(vec![Ok(edm(false))]), &apps, &[], &ctx(), Some(ws)).unwrap().expect("project の形").project;
        assert_eq!(p.pages.iter().map(|pg| pg.title.as_str()).collect::<Vec<_>>(), ["EDM 音楽制作"], "外部 3 つ・明示なしが 1 枚でない");
        assert_eq!(faces(&p), [["reaper", "apple-music", "youtube"]], "面の id は席の slug・1 枚に 3 つ");
        let slots: Vec<Option<String>> = p.pages[0].root.children.iter().map(seat_slot).collect();
        assert_eq!(slots, [Some("main"), Some("side"), Some("side"), Some("bottom")].map(|s| s.map(str::to_string)), "席の node に model の置き場が無い");
        assert_eq!(p.pages[0].root.children[2].props[SEAT_PROP]["why"], json!("YouTube をsideに"), "why を残さない");
        let t = seat_template(&p, None).expect("置き場の在る Page の型");
        assert_eq!(t.id, SEAT_TEMPLATE);
        let shown = place(&p.pages[0], &p, Some(&t));
        let under = |slot: &str| -> Vec<String> { shown.find(&format!("slot.{slot}")).map(|n| n.children.iter().map(|c| c.key.clone()).collect()).unwrap_or_default() };
        assert_eq!(
            (under("main"), under("side"), under("bottom")),
            (vec!["reaper-surface".to_owned()], vec!["apple-music-surface".to_owned(), "youtube-surface".to_owned()], vec!["shell-terminal".to_owned()]),
            "model の置き場どおりに置かない"
        );
        // 明示が在れば model の page で束ねる（0・1・2・5 → 詰めて 4 枚・席が 1 つの Page は席の title で置き場を書かない）
        let q = plan_project(said, &Canned::new(vec![Ok(edm(true))]), &apps, &[], &ctx(), Some(ws)).unwrap().unwrap().project;
        assert_eq!(q.pages.iter().map(|pg| pg.title.as_str()).collect::<Vec<_>>(), ["REAPER", "Apple Music", "YouTube", "Shell"], "明示の Plan を model の page で束ねない");
        assert!(q.pages.iter().all(|pg| pg.root.children.iter().all(|c| seat_slot(c).is_none())), "席が 1 つの Page に置き場を書いた（今の型で置く）");
        assert_eq!(seat_template(&q, None), None, "置き場の無い Project に型を作った");
        assert_eq!(q.pages.iter().map(|pg| pg.route.as_str()).collect::<Vec<_>>(), ["/reaper", "/apple-music", "/youtube", "/shell"], "1 席 1 Page の route が前と違う");
    }

    /// G88a: 明示が無くても外部が 3 つを超えたら Page を分ける（model の page で束ね、1 枚の外部が 3 つを超えたら順に次の Page へ・空の Page を作らない）。
    /// main は 1 Page に 1 つ（2 つ目からは side）・main が無ければ最初の外部の席を main。
    #[test]
    fn g88a_a_fourth_service_spills_to_the_next_page_and_each_page_has_one_main() {
        let web = |t: &str, slot: &str| placed(t, "web", None, Some(&format!("https://{}.example.org", t.to_lowercase())), 0, slot);
        let four = json!({ "shape": "project", "name": "Band", "pages_requested": false, "seats": [web("A", "main"), web("B", "main"), web("C", "side"), web("D", "side")] });
        let p = plan_project("A と B と C と D", &Canned::new(vec![Ok(four)]), &[], &[], &ctx(), None).unwrap().unwrap().project;
        assert_eq!(faces(&p), [vec!["a", "b", "c"], vec!["d"]], "4 つ目の外部が次の Page に溢れない");
        assert_eq!(p.pages.iter().map(|pg| pg.title.as_str()).collect::<Vec<_>>(), ["A · B · C", "D"]);
        // 外部が 3 を超えたら model の page で束ねる（数を見ずに 1 枚に詰めて 3 つずつ切る形にしない）
        let split = |t: &str, page: i64, slot: &str| placed(t, "web", None, Some(&format!("https://{}.example.org", t.to_lowercase())), page, slot);
        let by_model = json!({ "shape": "project", "name": "Band", "pages_requested": false, "seats": [split("A", 0, "main"), split("B", 1, "main"), split("C", 1, "side"), split("D", 1, "side")] });
        let q = plan_project("A と B と C と D", &Canned::new(vec![Ok(by_model)]), &[], &[], &ctx(), None).unwrap().unwrap().project;
        assert_eq!(faces(&q), [vec!["a"], vec!["b", "c", "d"]], "外部 4 つで model の page を見ない");
        let slots: Vec<Option<String>> = p.pages[0].root.children.iter().map(seat_slot).collect();
        assert_eq!(slots, [Some("main"), Some("side"), Some("side")].map(|s| s.map(str::to_string)), "main が 2 つ");
        let none_main = json!({ "shape": "project", "name": "X", "pages_requested": false, "seats": [placed("Dev", "terminal", None, None, 0, "side"), web("A", "side"), web("B", "side")] });
        let p = plan_project("A と B と repo", &Canned::new(vec![Ok(none_main)]), &[], &[], &ctx(), Some(Path::new("/tmp/g88a-ws"))).unwrap().unwrap().project;
        // 木は区画の順（main → side・同じ区画は席の順）
        let slots: Vec<(String, Option<String>)> = p.pages[0].root.children.iter().map(|c| (c.key.clone(), seat_slot(c))).collect();
        let want = [("a-surface", "main"), ("dev-terminal", "side"), ("b-surface", "side")].map(|(k, s)| (k.to_string(), Some(s.to_string())));
        assert_eq!(slots, want, "main の無い Page の最初の外部を main にしない・区画の順に並べない");
    }

    /// G88a（owner 10-27 18:0x「後から4つ以上も1ページに入れることはできるがそれはuser次第」）: `ONE_PAGE_MAX_SERVICES` は作る時（`plan_project`）だけ。
    /// 作った後に利用者が面を動かす・足す（MoveSurface・AddSurface）時は数で止めない: 外部 3 つの Page に 4 つ目を入れても拒否されず 4 つ並ぶ。
    /// 負の対照（commit message）: apply の MoveSurface / AddSurface に上限 3 を入れる → 赤
    #[test]
    fn g88a_a_person_can_put_a_fourth_service_on_a_page_later() {
        use super::super::domain::project::{apply, ProjectPatchOp, SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        let web = |t: &str, page: i64, slot: &str| placed(t, "web", None, Some(&format!("https://{}.example.org", t.to_lowercase())), page, slot);
        let band = json!({ "shape": "project", "name": "Band", "pages_requested": true, "seats": [
            web("A", 0, "main"), web("B", 0, "side"), web("C", 0, "side"), web("D", 1, "main"), placed("Shell", "terminal", None, None, 1, "bottom"),
        ]});
        let plan = plan_project("A と B と C と D", &Canned::new(vec![Ok(band)]), &[], &[], &ctx(), Some(Path::new("/tmp/g88a-ws"))).unwrap().unwrap();
        let p0 = plan.project.clone();
        assert_eq!(faces(&p0)[0], ["a", "b", "c"], "前提: 外部 3 つの Page が無い");
        let (first, root) = (p0.pages[0].id.clone(), p0.pages[0].root.key.clone());
        let moved = apply(&p0, &[ProjectPatchOp::MoveSurface { surface_key: "d-surface".into(), to_page: first.clone(), parent: root.clone(), index: None }], &[&plan.app]).expect("4 つ目を動かすのを拒否した");
        assert_eq!(faces(&moved)[0], ["a", "b", "c", "d"], "動かした 4 つ目が並ばない");
        let spec = SurfaceSpec { id: "notes".into(), backend: SurfaceBackendKind::Mirror, target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("docs/notes.md".into()) }, keep_alive: true, frame: None, browser: false };
        let added = apply(&p0, &[ProjectPatchOp::AddSurface { spec, page_id: first, parent: root, node_key: "notes-surface".into(), index: None }], &[&plan.app]).expect("4 つ目を足すのを拒否した");
        assert_eq!(faces(&added)[0], ["a", "b", "c", "notes"], "足した 4 つ目が並ばない");
    }

    /// G88a: 1 Page に無い app の 1 問が 2 つ在っても、使える検査の席（MissingApp）は 1 問ごと（id = 席の slug・Page の route ではない）。
    /// 1 席 1 Page の時は今まで通り route の名前と同じ（前に保存した Project の席の id が変わらない）。
    #[test]
    fn g88a_two_missing_apps_on_one_page_are_two_seats() {
        let m = Canned::new(vec![Ok(json!({ "shape": "project", "name": "作業場", "pages_requested": false, "seats": [
            placed("Draw", "app", Some("Zeta Edit"), None, 0, "main"),
            placed("Paint", "app", Some("Paint X"), None, 0, "side"),
        ]}))]);
        let p = plan_project("Zeta Edit と Paint X で n-88a", &m, &[KnownApp { name: "Other".into(), bundle_id: "org.example.other".into() }], &[], &ctx(), None).unwrap().unwrap().project;
        assert_eq!(p.pages.len(), 1);
        let seats: Vec<(String, String)> = super::super::verifier::project_seats(&p)
            .into_iter()
            .filter(|s| s.kind == super::super::verifier::SeatKind::MissingApp)
            .map(|s| (s.id, s.name))
            .collect();
        assert_eq!(seats, [("draw".to_string(), "Zeta Edit".to_string()), ("paint".to_string(), "Paint X".to_string())], "1 問ごとの席でない");
    }

    /// G88b（owner の完成予想の下の `Record Humming` の帯）: recorder の席は外部に数えず（REAPER・Music・YouTube ＋ 録音 = 1 Page）、bottom に `recorder` の node
    /// （label `Record humming`・max_seconds 60・recorder_ref = 席の slug・feeds = REAPER の席の面の id）。feeds が app の席でなければ null。
    #[test]
    fn g88b_a_recorder_seat_sits_at_the_bottom_and_feeds_the_named_app() {
        let apps = [KnownApp { name: "REAPER".into(), bundle_id: "com.cockos.reaper".into() }, KnownApp { name: "Music".into(), bundle_id: "com.apple.Music".into() }];
        let mut hum = placed("Hum", "recorder", None, None, 0, "bottom");
        hum["feeds"] = json!("REAPER");
        let m = Canned::new(vec![Ok(json!({ "shape": "project", "name": "EDM", "pages_requested": false, "seats": [
            placed("REAPER", "app", Some("REAPER"), None, 0, "main"),
            placed("Apple Music", "app", Some("Music"), None, 0, "side"),
            placed("YouTube", "web", None, Some("https://www.youtube.com"), 0, "side"),
            hum,
        ]}))]);
        let p = plan_project("REAPER と Music と YouTube を見ながら・鼻歌を REAPER に", &m, &apps, &[], &ctx(), None).unwrap().unwrap().project;
        assert_eq!(p.pages.len(), 1, "recorder を外部に数えた");
        let rec = p.pages[0].root.children.last().expect("席の node").clone();
        assert_eq!((rec.component.as_str(), seat_slot(&rec).as_deref()), ("recorder", Some("bottom")));
        assert_eq!(
            (rec.props["label"].clone(), rec.props["max_seconds"].clone(), rec.props["recorder_ref"].clone(), rec.props["feeds"].clone()),
            (json!("Record humming"), json!(60), json!("hum"), json!("reaper")),
            "recorder の node の props"
        );
        let mut alone = placed("Hum", "recorder", None, None, 0, "main");
        alone["feeds"] = json!("YouTube");
        let m = Canned::new(vec![Ok(json!({ "shape": "project", "name": "Hum", "pages_requested": false, "seats": [placed("YouTube", "web", None, Some("https://www.youtube.com"), 0, "main"), alone] }))]);
        let p = plan_project("YouTube を見ながら鼻歌", &m, &[], &[], &ctx(), None).unwrap().unwrap().project;
        let rec = p.pages[0].root.nodes().into_iter().find(|n| n.component == "recorder").expect("recorder").clone();
        assert_eq!(rec.props["feeds"], Value::Null, "web の席に take を入れる先を書いた");
    }

    /// G88a: prompt は Page の決まりと置き方の考え方を持ち、明示の例は app の名前の無い文（I3: plan の prompt に特定の app・site の名前を書かない）。
    #[test]
    fn g88a_the_prompt_teaches_pages_and_slots_without_naming_apps() {
        let prompt = plan_prompt("何か", &[], None);
        for want in ["pages_requested", "Put each on its own page.", "main", "side", "bottom", "why"] {
            assert!(prompt.contains(want), "prompt に {want} が無い");
        }
        for app in ["TextEdit", "Excalidraw", "REAPER", "YouTube"] {
            assert!(!prompt.to_lowercase().contains(&app.to_lowercase()), "plan の prompt に app の名前 {app}（I3）");
        }
    }

    /// G93: Page の題の頭は app の表示名（`REAPER`）・利用者の言葉は残す。負の対照: `display_title` を `title.trim().to_string()` にすると赤。
    #[test]
    fn g93_page_title_starts_with_the_apps_display_name() {
        let reaper = KnownApp { name: "REAPER".into(), bundle_id: "com.cockos.reaper".into() };
        let app = |title: &str, said: &str| serde_json::from_value::<SeatV1>(placed(title, "app", Some(said), None, 0, "main")).unwrap();
        let t = |title: &str, said: &str| display_title(&app(title, said), &[reaper.clone()], &[]);
        assert_eq!(t("Reaper（鼻歌でメロディ）", "Reaper"), "REAPER（鼻歌でメロディ）");
        assert_eq!(t("reaper", "reaper"), "REAPER");
        assert_eq!(t("鼻歌を Reaper に", "Reaper"), "鼻歌を Reaper に", "頭が名前でない題は触らない");
        assert_eq!(t("Reaperish", "Reaper"), "Reaperish", "名前の続きが英数字の語は別の語");
        assert_eq!(display_title(&app("Reaper", "Nope"), &[reaper], &[]), "Reaper", "引けない app は触らない");
    }
}

/// W13b の署名を compile で縛る（本体は todo!()・署名を変えると compile が赤）。本体の要らない性質だけを走らせる:
/// Plan の席の新しい 2 欄が読めて往復し、保存する 1 問の中身（席の写しと `KnownApp`）は未知の欄を拒む。
#[cfg(test)]
mod w13b_interface {
    use super::*;

    #[test]
    fn w13b_signatures_and_saved_shape() {
        let _: fn(&UiNode) -> Option<SeatV1> = SeatV1::asked_in;
        assert_eq!(
            ASKED_PROP, "missing_app",
            "保存した Page の木の key は変えない"
        );
        let asked = json!({
            "title": "Notes", "kind": "app", "app_name": "Notes Maker", "bundle_id": null, "file": "docs/a.md", "url": null,
            "install_url": "https://example.org/get", "instead": [{ "name": "Other Notes", "bundle_id": "org.example.other" }],
            "page": 0, "slot": null, "why": "", "feeds": null
        });
        let seat: SeatV1 = serde_json::from_value(asked.clone()).unwrap();
        assert_eq!(seat.install_url.as_deref(), Some("https://example.org/get"));
        assert_eq!(
            seat.instead,
            [KnownApp {
                name: "Other Notes".into(),
                bundle_id: "org.example.other".into()
            }]
        );
        assert_eq!(
            serde_json::to_value(&seat).unwrap(),
            asked,
            "往復で変わらない"
        );
        let mut extra = asked;
        extra["instead"][0]["guess"] = json!(1);
        assert!(
            serde_json::from_value::<SeatV1>(extra).is_err(),
            "代わりの app の未知の欄を受けた"
        );
    }
}
