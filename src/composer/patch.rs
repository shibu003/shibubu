//! 自然言語の編集 → AppPatch（GEN-011 / GEN-014 / TEST-007）。
//!
//! 「検索を追加」「この表を並び替え可能に」「この欄を消して」を、指した subtree だけに当たる op 列にする。
//! 名前で指した時は文に含まれる一番長い label の node（「一覧」「表」の語より先）。固定した部品（消す subtree の中も）と、action に必ず要る欄は消さない。
//! app を作り直さない: 変わるのは対象の node と、その親の子の並びだけ（data・他の node・action・source は触らない）。
//! apply は domain::patch::apply（原子的・version + 1）の後に validate_generated を通す。inverse を一緒に返すので、
//! undo は前の revision に戻す（CD-18）か inverse を当てる。AppPatch は composition.root しか変えられないので、
//! 使える action は定義に在る物だけ（planner が local の collection には filter / search を必ず宣言している）。
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::domain::app::AppDefinitionV3;
use super::domain::binding::DataBinding;
use super::domain::patch::{apply, AppPatch, AppRevision, PatchError, PatchOp};
use super::domain::ui::UiNode;
use super::domain::validate::{validate_generated, ValidationError};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edit {
    pub summary: String,
    /// 変わる subtree の根
    pub target_key: String,
    pub patches: Vec<AppPatch>,
    /// 元に戻す op 列
    pub inverse: Vec<AppPatch>,
    /// 中身が変わる node（足した・消した・props を変えた node と、子の並びが変わる親）
    pub affected_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    /// 言葉から編集の種類が読めない
    Unsupported(String),
    /// 対象の node が決まらない
    NoTarget(String),
    /// 必要な action が定義に無い（AppPatch では足せない）
    MissingAction { node: String, wanted: Vec<String> },
    /// 利用者が固定した部品（DESIGN §0-9: 手修正は AI の再生成より優先）。消す subtree の中に在っても止める
    Locked(String),
    /// form の欄が、その form の action に必ず要る（input_schema.required）。外すと送れなくなる
    RequiredField { node: String, field: String, action: String },
    RootImmovable(String),
    AlreadyApplied(String),
    Patch(PatchError),
    Invalid(Vec<ValidationError>),
    /// W13: model が答えなかった・答えが編集の決まりに合わない（`project::edit_project_by_model`）。String は画面に出せる 1 行の理由。
    /// ops 0・何も書いていない。呼ぶ側は名前の表（`project::edit_project`）で読み直してよい
    Model(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(s) => write!(f, "この編集は読めない: '{s}'（検索を追加 / 並び替え可能に / 消して）"),
            Self::NoTarget(s) => write!(f, "対象が決まらない: {s}"),
            Self::MissingAction { node, wanted } => write!(f, "node '{node}' に使える action が定義に無い（候補 {wanted:?}）"),
            Self::Locked(k) => write!(f, "'{k}' は固定されている（固定を外してから）"),
            Self::RequiredField { node, field, action } => write!(f, "欄「{field}」は '{node}' の操作 {action} に必ず要る（外すと送れなくなる）"),
            Self::RootImmovable(k) => write!(f, "root '{k}' は消せない"),
            Self::AlreadyApplied(k) => write!(f, "'{k}' は既にある"),
            Self::Patch(e) => write!(f, "{e}"),
            Self::Invalid(errs) => write!(f, "編集後の定義が規則に落ちる: {}", errs.iter().map(ToString::to_string).collect::<Vec<_>>().join(" | ")),
            Self::Model(why) => write!(f, "AI を使えなかった: {why}"),
        }
    }
}
impl std::error::Error for EditError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    AddSearch,
    MakeSortable,
    Remove,
}

fn classify(text: &str) -> Option<Kind> {
    let t = text.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    let removing = has(&["消して", "削除", "外して", "取り除", "いらない", "remove", "delete"]);
    if has(&["並び替え", "並べ替え", "ソート", "sort"]) && !removing {
        return Some(Kind::MakeSortable);
    }
    if has(&["検索", "絞り込", "絞込", "search", "filter"]) && !removing {
        return Some(Kind::AddSearch);
    }
    removing.then_some(Kind::Remove)
}

fn parent_of<'a>(root: &'a UiNode, key: &str) -> Option<(&'a UiNode, usize)> {
    root.nodes().into_iter().find_map(|n| n.children.iter().position(|c| c.key == key).map(|i| (n, i)))
}

const LISTS: &[&str] = &["table", "list", "timeline", "calendar"];

/// 欄の英語の名前の日本語の言い方（FX-032 で欄の名前を英語にした。日本語の編集の文「題名の欄を消して」も同じ欄に当てる・保存済みの定義の日本語の名前は label のまま当たる）。
const FIELD_NAMES_JA: &[(&str, &str)] = &[
    ("Title", "題名"), ("Title", "件名"), ("Author", "著者"), ("Amount", "金額"), ("Date", "日付"), ("Category", "分類"), ("Note", "メモ"), ("Name", "名前"),
    ("Status", "状態"), ("Quantity", "数量"), ("Body", "本文"), ("Minutes", "時間"), ("Thoughts", "感想"), ("Due", "期限"), ("Place", "場所"),
    ("Word", "単語"), ("Meaning", "意味"), ("Learned", "覚えた"), ("Finished on", "読み終えた日"), ("Watched on", "見た日"), ("Exercise", "種目"),
    ("Weight (kg)", "重さ"), ("Reps", "回数"), ("Sets", "セット数"), ("Kind", "種類"), ("Distance (km)", "距離"), ("Value", "値"), ("Time", "時刻"),
    ("Starts", "開始"), ("Ends", "終了"), ("Severity", "重要度"), ("Group", "所属"), ("Contact", "連絡先"), ("Last contacted", "最後に連絡した日"),
    ("Recipe", "料理名"), ("Ingredients", "材料"), ("Steps", "作り方"), ("Habit", "習慣"), ("Done", "やった"), ("Added", "追加日時"),
    ("Updated", "更新日時"), ("Reported", "報告日時"), ("Question", "問題"), ("Answer", "答え"), ("Entry", "内容"), ("Plan", "予定"), ("Item", "項目"),
    ("Place or activity", "やる事"), ("Details and steps to reproduce", "再現手順"),
];

/// 文が label を指しているか（大文字小文字を見ない・英語の欄の名前は日本語の言い方でも）。
fn names(label: &str, instruction: &str) -> bool {
    instruction.to_lowercase().contains(&label.to_lowercase()) || FIELD_NAMES_JA.iter().any(|(en, ja)| *en == label && instruction.contains(ja))
}

/// 文に含まれる label のうち一番長い物（「選んだ在庫」と「在庫」が両方当たれば「選んだ在庫」）。同じ長さなら先の物。
fn longest_label<'s, T>(cands: impl IntoIterator<Item = (Option<&'s str>, T)>, instruction: &str) -> Option<T> {
    let mut best: Option<(usize, T)> = None;
    for (label, it) in cands {
        let Some(l) = label.filter(|l| !l.is_empty() && names(l, instruction)) else { continue };
        let n = l.chars().count();
        if best.as_ref().map_or(true, |(b, _)| n > *b) {
            best = Some((n, it));
        }
    }
    best.map(|(_, it)| it)
}

/// 「表」を部品の表と読まない語（「一覧の表示」は一覧を指す）。
const NOT_TABLE: &[&str] = &["表示", "発表", "公表", "代表", "表現", "表紙", "表記"];

/// 一覧の対象: selected（その下の最初の一覧）→ 文に含まれる一番長い label → 言葉の「表 / 一覧」→ role primary の一覧 → 最初の一覧。
/// local の source に bind した物だけ。label を語より先に見る（「取り込んだ資料の一覧」は「一覧」の語ではなく名前で指している）。
fn pick_collection<'a>(def: &'a AppDefinitionV3, instruction: &str, selected: Option<&str>, kinds: &[&str]) -> Result<&'a UiNode, EditError> {
    let root = &def.composition.root;
    let local = |n: &&UiNode| n.binding.as_ref().map(|b| b.source_ref.starts_with("local.")).unwrap_or(false);
    if let Some(k) = selected {
        let sel = root.find(k).ok_or_else(|| EditError::NoTarget(format!("選んだ '{k}' が無い")))?;
        return sel.nodes().into_iter().filter(local).find(|n| kinds.contains(&n.component.as_str())).ok_or_else(|| EditError::NoTarget(format!("選んだ '{k}' の中に一覧が無い")));
    }
    let lists: Vec<&UiNode> = root.nodes().into_iter().filter(local).filter(|n| kinds.contains(&n.component.as_str())).collect();
    let t = instruction.to_lowercase();
    let says_table = t.contains("table") || NOT_TABLE.iter().fold(t.clone(), |s, w| s.replace(w, "")).contains('表');
    let says_list = t.contains("一覧") || t.contains("リスト") || t.contains("list");
    let of_kind = |c: &str| lists.iter().find(|n| n.component == c).copied();
    longest_label(lists.iter().map(|n| (n.label.as_deref(), *n)), instruction)
        .or_else(|| says_table.then(|| of_kind("table")).flatten())
        .or_else(|| says_list.then(|| of_kind("list")).flatten())
        .or_else(|| lists.iter().find(|n| n.role.as_deref() == Some("primary")).copied())
        .or_else(|| lists.first().copied())
        .ok_or_else(|| EditError::NoTarget("端末内の一覧（table / list / timeline）が無い".into()))
}

/// 新しい部品を置く所: 対象から上へ、縦に積む容器（column / stack / scroll）が見つかるまで登る（split / tabs / row の比率を崩さない）。
fn insertion_point<'a>(root: &'a UiNode, key: &str) -> Option<(&'a UiNode, usize)> {
    let mut k = key.to_string();
    loop {
        let (parent, index) = parent_of(root, &k)?;
        if matches!(parent.component.as_str(), "column" | "stack" | "scroll") {
            return Some((parent, index));
        }
        k = parent.key.clone();
    }
}

fn node_value(n: &UiNode) -> Value {
    serde_json::to_value(n).unwrap_or(Value::Null)
}

/// 唯一の入口。`selected` は画面で選んでいる node の key（「この〜」は選択を指す）。
pub fn edit(def: &AppDefinitionV3, instruction: &str, selected: Option<&str>) -> Result<Edit, EditError> {
    let kind = classify(instruction).ok_or_else(|| EditError::Unsupported(instruction.to_string()))?;
    let root = &def.composition.root;
    match kind {
        Kind::AddSearch => {
            let target = pick_collection(def, instruction, selected, LISTS)?;
            let source = target.binding.as_ref().map(|b| b.source_ref.clone()).unwrap_or_default();
            if let Some(existing) = root.nodes().into_iter().find(|n| n.component == "form" && n.binding.as_ref().map(|b| b.source_ref == source && b.path == "filter").unwrap_or(false)) {
                return Err(EditError::AlreadyApplied(existing.key.clone()));
            }
            let wanted = vec![format!("{source}.filter"), format!("{source}.search")];
            let action = wanted.iter().find(|id| def.action(id).is_some()).cloned().ok_or_else(|| EditError::MissingAction { node: target.key.clone(), wanted: wanted.clone() })?;
            let (parent, index) = insertion_point(root, &target.key).ok_or_else(|| EditError::NoTarget(format!("'{}' の上に置ける縦の容器が無い", target.key)))?;
            let mut new_key = format!("{}-search", target.key);
            while root.find(&new_key).is_some() {
                new_key.push('x');
            }
            let label = target.label.clone().unwrap_or_else(|| "一覧".into());
            let mut form = UiNode { key: new_key.clone(), component: "form".into(), label: Some("絞り込み".into()), ..Default::default() };
            form.props.insert("fields".into(), json!([{ "name": "query", "label": format!("{label}を検索"), "input": "text" }]));
            form.binding = Some(DataBinding { source_ref: source, path: "filter".into(), extra: Map::new() });
            form.action_ref = Some(action.clone());
            Ok(Edit {
                summary: format!("「{label}」の上に検索を足す（action {action}・他の部品は変えない）"),
                target_key: parent.key.clone(),
                patches: vec![
                    AppPatch { op: PatchOp::Insert, target_key: parent.key.clone(), value: Some(node_value(&form)) },
                    AppPatch { op: PatchOp::Move, target_key: new_key.clone(), value: Some(json!({ "parent": parent.key, "index": index })) },
                ],
                inverse: vec![AppPatch { op: PatchOp::Remove, target_key: new_key.clone(), value: None }],
                affected_keys: vec![parent.key.clone(), new_key],
            })
        }
        Kind::MakeSortable => {
            let target = pick_collection(def, instruction, selected, &["table", "list"])?;
            if target.props.get("sortable") == Some(&json!(true)) {
                return Err(EditError::AlreadyApplied(target.key.clone()));
            }
            if target.user_locked {
                return Err(EditError::Locked(target.key.clone()));
            }
            // update は field 単位で置き換わる（props は丸ごと）。元の props を保って sortable を足す
            let mut props = target.props.clone();
            props.insert("sortable".into(), json!(true));
            if let Some(cols) = props.get("columns").cloned() {
                props.insert("sort_columns".into(), cols);
            }
            Ok(Edit {
                summary: format!("「{}」を並び替え可能にする", target.label.clone().unwrap_or_else(|| target.key.clone())),
                target_key: target.key.clone(),
                patches: vec![AppPatch { op: PatchOp::Update, target_key: target.key.clone(), value: Some(json!({ "props": Value::Object(props) })) }],
                inverse: vec![AppPatch { op: PatchOp::Update, target_key: target.key.clone(), value: Some(json!({ "props": Value::Object(target.props.clone()) })) }],
                affected_keys: vec![target.key.clone()],
            })
        }
        Kind::Remove => {
            let t = instruction.to_lowercase();
            let target = match selected {
                Some(k) => root.find(k).ok_or_else(|| EditError::NoTarget(format!("選んだ '{k}' が無い")))?,
                None if t.contains("検索") || t.contains("絞り込") => root
                    .nodes()
                    .into_iter()
                    .find(|n| n.component == "form" && n.binding.as_ref().map(|b| b.path == "filter").unwrap_or(false))
                    .ok_or_else(|| EditError::NoTarget("検索の部品が無い".into()))?,
                // 名前で指した: 文に含まれる一番長い label の node（「選んだ在庫を消して」を「在庫」の一覧と読まない）
                None => longest_label(root.nodes().into_iter().filter(|n| n.key != root.key).map(|n| (n.label.as_deref(), n)), instruction)
                    .ok_or_else(|| EditError::NoTarget("消す部品を選ぶか、名前で指す（例:「絞り込みを消して」）".into()))?,
            };
            if target.key == root.key {
                return Err(EditError::RootImmovable(target.key.clone()));
            }
            // 固定した部品は、それ自身でも、消す subtree の中に在っても消さない
            if let Some(locked) = target.nodes().into_iter().find(|n| n.user_locked) {
                return Err(EditError::Locked(locked.key.clone()));
            }
            // form の欄を名前で指した（「メモの欄を消して」）→ その欄だけ外す。form の action に必ず要る欄（required）は外さない
            if target.component == "form" {
                if let Some(fields) = target.props.get("fields").and_then(Value::as_array) {
                    if fields.len() > 1 {
                        // 「〜の欄を消して」なのにどの欄とも当たらない: form ごと消さない（知らない欄の名前を form 全体への指示と読まない）
                        if instruction.contains('欄') && longest_label(fields.iter().map(|f| (f.get("label").and_then(Value::as_str), ())), instruction).is_none() {
                            return Err(EditError::NoTarget("その名前の欄がこの form に無い（欄の名前で指す）".into()));
                        }
                        if let Some(i) = longest_label(fields.iter().enumerate().map(|(i, f)| (f.get("label").and_then(Value::as_str), i)), instruction) {
                            let name = fields[i].get("name").and_then(Value::as_str).unwrap_or("");
                            let label = fields[i].get("label").and_then(Value::as_str).unwrap_or(name);
                            if let Some(a) = target.action_ref.as_deref().and_then(|id| def.action(id)) {
                                if a.input_schema.get("required").and_then(Value::as_array).map(|r| r.iter().any(|v| v.as_str() == Some(name))).unwrap_or(false) {
                                    return Err(EditError::RequiredField { node: target.key.clone(), field: label.into(), action: a.id.clone() });
                                }
                            }
                            let mut props = target.props.clone();
                            let mut kept = fields.clone();
                            kept.remove(i);
                            props.insert("fields".into(), Value::Array(kept));
                            return Ok(Edit {
                                summary: format!("「{}」から欄「{label}」を外す", target.label.clone().unwrap_or_default()),
                                target_key: target.key.clone(),
                                patches: vec![AppPatch { op: PatchOp::Update, target_key: target.key.clone(), value: Some(json!({ "props": Value::Object(props) })) }],
                                inverse: vec![AppPatch { op: PatchOp::Update, target_key: target.key.clone(), value: Some(json!({ "props": Value::Object(target.props.clone()) })) }],
                                affected_keys: vec![target.key.clone()],
                            });
                        }
                    }
                }
            }
            let (parent, index) = parent_of(root, &target.key).ok_or_else(|| EditError::NoTarget(target.key.clone()))?;
            let mut patches = vec![AppPatch { op: PatchOp::Remove, target_key: target.key.clone(), value: None }];
            let mut inverse = vec![
                AppPatch { op: PatchOp::Insert, target_key: parent.key.clone(), value: Some(node_value(target)) },
                AppPatch { op: PatchOp::Move, target_key: target.key.clone(), value: Some(json!({ "parent": parent.key, "index": index })) },
            ];
            // split の子を外したら比率も同じ割合で詰める（子の数と ratio を合わせる: DESIGN §7）
            if let Some(ratio) = parent.layout.as_ref().and_then(|l| l.get("ratio")).and_then(Value::as_array) {
                let mut vals: Vec<f64> = ratio.iter().filter_map(Value::as_f64).collect();
                if vals.len() == parent.children.len() && vals.len() > 1 {
                    vals.remove(index);
                    let sum: f64 = vals.iter().sum();
                    let vals: Vec<f64> = vals.iter().map(|v| ((v / sum) * 100.0).round() / 100.0).collect();
                    let mut layout = parent.layout.clone().unwrap_or_default();
                    layout.insert("ratio".into(), json!(vals));
                    patches.push(AppPatch { op: PatchOp::Update, target_key: parent.key.clone(), value: Some(json!({ "layout": Value::Object(layout) })) });
                    inverse.push(AppPatch { op: PatchOp::Update, target_key: parent.key.clone(), value: Some(json!({ "layout": parent.layout.clone().map(Value::Object).unwrap_or(Value::Null) })) });
                }
            }
            Ok(Edit {
                summary: format!("「{}」を外す（元に戻せる）", target.label.clone().unwrap_or_else(|| target.key.clone())),
                target_key: parent.key.clone(),
                patches,
                inverse,
                affected_keys: vec![parent.key.clone(), target.key.clone()],
            })
        }
    }
}

/// 編集を当てる: domain の apply（原子的・version + 1）→ 生成物の規則（validate_generated）。落ちたら base は変わらない。
pub fn apply_edit(base: &AppRevision, e: &Edit) -> Result<AppRevision, EditError> {
    let next = apply(base, &e.patches).map_err(EditError::Patch)?;
    validate_generated(next.definition()).map_err(EditError::Invalid)?;
    Ok(next)
}

/// 取り消し: inverse を当てた新しい revision（composition は編集前と同じ・version はさらに + 1）。前の revision を持っていればそれに戻すだけでもよい。
pub fn undo(current: &AppRevision, e: &Edit) -> Result<AppRevision, EditError> {
    let back = apply(current, &e.inverse).map_err(EditError::Patch)?;
    validate_generated(back.definition()).map_err(EditError::Invalid)?;
    Ok(back)
}
