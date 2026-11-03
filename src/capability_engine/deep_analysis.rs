//! W19-3: Deep Software Analysis（REA などの Analysis Backend の結果を Capability の候補にする）
use super::*;

/// 解析の report を Capability の候補にする（finding 1 つ → 候補 0〜1 つ）。
/// Wave: W19-3
/// 例: `let report = backend.analyze(&req)?; let cands = capability_engine::candidates_from(&report); found.extend(cands);`
/// 保証する: どの候補も `Origin::Analysis { backend, report: report.id }`・`Verification::Unverified`（readiness は決して Ready にならない・調査しただけの能力を
/// Ready にしない 19 通目 §5）・`versions` に対象の版・`evidence` に finding の根拠・`constraints` に report の制約。`Callable` より前（Analyzed / Inferred）の finding は
/// constraints に「呼べるか未確認」を書く。保証しない: 候補が実際に使える事（Verification の段）・安全で許された連携が在る事（無ければ report の constraints を Unsupported の理由にする）。
pub fn candidates_from(report: &AnalysisReport) -> Vec<CapabilityRecord> {
    let app = report.target.bundle_id.clone().unwrap_or_else(|| report.target.path.clone());
    let head = format!("{ANALYSIS_ACTION}.{}", slug(&report.id));
    let base = |id: String, verification: Verification, evidence: Vec<String>, constraints: Vec<String>| CapabilityRecord {
        reuse: Reuse::AppSpecific { app: app.clone() },
        origin: Origin::Analysis { backend: report.backend.clone(), report: report.id.clone() },
        verification,
        versions: report.target.version.iter().cloned().collect(),
        evidence,
        constraints,
        ..base_record(id, AdapterKind::Undecided)
    };
    if report.findings.is_empty() {
        // 安全で許された連携が見つからない: その制約を、使えない候補 1 件として残す（解析しなかった事にしない・W19-3 の合格の条件）
        let reason = if report.constraints.is_empty() { "Analysis found nothing Shibubu can call from outside the app".to_string() } else { report.constraints.join("・") };
        let seen = format!("Analysis of {} found 0 things (target {} · version {})", report.backend, report.target.path, report.target.version.as_deref().unwrap_or("Unknown"));
        return vec![base(format!("{head}.none"), Verification::Unsupported { reason }, vec![seen], report.constraints.clone())];
    }
    report
        .findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut constraints = report.constraints.clone();
            if f.status < FindingStatus::Callable {
                constraints.push(NOT_CALLED.into());
            }
            if f.status == FindingStatus::Inferred {
                constraints.push(INFERRED.into());
            }
            let mut evidence = vec![format!("{} ({})", f.claim, status_word(f.status))];
            evidence.extend(f.evidence.iter().cloned());
            // Backend が何と言っても候補は Unverified（確かめるのは Verification の段で、元の app の状態で）
            base(format!("{head}.{}", i + 1), Verification::Unverified, evidence, constraints)
        })
        .collect()
}

/// 解析で見つけただけの候補の constraints の 1 行（`FindingStatus::Callable` より前）。
pub(super) const NOT_CALLED: &str = "Found by analysis only · not tried yet. Shibubu tries it on a copy and checks the result in the app before using it";
/// 推測の候補の constraints の 1 行。
pub(super) const INFERRED: &str = "A guess from what analysis found (not something that was read)";

fn status_word(s: FindingStatus) -> &'static str {
    match s {
        FindingStatus::Analyzed => "Facts read by analysis",
        FindingStatus::Inferred => "Guess",
        FindingStatus::Callable => "Confirmed callable · result not verified",
        FindingStatus::Verified => "The analysis tool says it is verified · Shibubu hasn't checked",
    }
}

/// 解析の承認の action の頭（`analysis.<backend>`・`PendingApproval::action` と `ConsentReceipt` の action_ref）。画面（`engine_view`）はこの頭で解析の 1 行にする。
/// Wave: W19-3
pub const ANALYSIS_ACTION: &str = "analysis";
/// 解析の承認で許す範囲（承認の input に入れて digest で縛り、`AnalysisConsent::scope` にも書く）。確かめている事だけを書く: 使うのは道具の名前の allowlist の中で、
/// 道具が壊さないと申告した tool だけ（`analysis_backends`・申告は道具の物で、Shibubu が確かめた物ではない）・呼ぶ tool は承認の input の `tools`・対象は読むだけ・
/// Shibubu は対象の app を起動も install もしない。REA 6.7.0 の 5 つの tool が network に出ない事は source を読み、通信を観測して確かめた（docs/evidence/W19-3/rea-facts.md）。
const ANALYSIS_SCOPE: &str = "Reads once, using only tools that say they change nothing (that claim is the tool's, not Shibubu's). Shibubu doesn't launch or install the app. Anything found is tried on a copy and needs your approval again before it's used for real";
/// 解析の承認を待つ時の `Denial::why`（人の言葉・path や識別子を入れない）。「何も送っていません」は Shibubu の事（通常の探索は試験の写しの前で止まった）。
pub(super) const ANALYSIS_WAIT: &str = "Existing connections found no way, so Shibubu is waiting for permission to analyze (nothing has been analyzed or sent yet)";
/// 解析 1 回の待ちの上限の既定（対象を開く・読む。道具は `availability` で起こした session を使う）。`analysis_timeout` が env と probe の実測で伸ばす。
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(120);
/// 解析 1 回の待ちの上限を替える env（秒・既定 `ANALYSIS_TIMEOUT`・W19-3-fix）。
pub(super) const ANALYSIS_TIMEOUT_ENV: &str = "SHIBUBU_ANALYSIS_TIMEOUT_S";
/// 対象の app を探す・Info.plist を読む待ちの上限。
const LOCATE_TIMEOUT: Duration = Duration::from_secs(10);
/// この process で走っている解析（この Mac では解析を同時に 1 本・19 通目 §4）。
// ponytail: process の中だけを見る。別の process の build・試験の負荷は見ない（見るなら load average を読む口を足す）
pub(super) static ANALYZING: AtomicBool = AtomicBool::new(false);

/// `ANALYZING` を持っている間だけ在る印（drop で離す・解析が誤りで返っても離す）。
pub(super) struct Busy;
impl Busy {
    pub(super) fn take() -> Option<Busy> {
        ANALYZING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Busy)
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        ANALYZING.store(false, Ordering::SeqCst);
    }
}

/// `deep_analysis` の結果。
/// Wave: W19-3
#[derive(Debug, Clone, PartialEq)]
pub struct DeepAnalysis {
    /// `analyses/<id>.json` に残した物（id と backend は Engine が付け直した後）
    pub report: AnalysisReport,
    /// `candidates_from(&report)`（`capabilities/` に残した物・どれも Ready でない）
    pub candidates: Vec<CapabilityRecord>,
}

/// 利用者が Shibubu の MCP の設定（`mcp::load_engine_config`）に入れた解析の道具を Backend にする（今は REA だけ: key `rea`）。無ければ空（解析しない・他の段は同じに動く）。
/// owner 10-24 A: REA は任意・既定 off（mcp.json に無ければ解析の段は Skipped）・demo の依存にしない。
/// Wave: W19-3
/// 例: `let mut analysis = capability_engine::analysis_backends(&mcp); let mut cx = EngineContext { analysis: analysis.as_mut_slice(), .. };`
/// 保証する: 作るだけでは process を起こさない（`availability` の時に 1 回起こして tools/list を読み、その session を同じ run の解析に使う = 1 run で REA を 1 回だけ
/// 起こす・W19-3-fix）・REA 専用の client を書かない（汎用の `mcp::McpSession`）・解析 1 本 = session 1 つで、`open_binary` で開いた target に続けて読み、終わったら閉じる
/// （解析しなかった run の session は Backend の drop で閉じる・2 本目の解析は開き直す）・道具には壊さないと申告した tool
/// （`McpToolHints::not_destructive`）の名前だけを渡す（申告の無い tool は選ばれない・Backend は更に名前の allowlist で絞る）。
/// 保証しない: REA が入っている事（`availability` が Missing を返す）。
pub fn analysis_backends(cfg: &McpConfig) -> Vec<Box<dyn AnalysisBackend>> {
    // ponytail: Backend は key が rea の server だけ。別の key で入れた REA は mcp_records が tool の名前で見分けて候補と生成から外すが、解析には使わない
    // （使うなら server ごとに道具を確かめる口を足す・run ごとに server の数だけ余分に起こす）
    let Some(server) = cfg.mcp_servers.get(REA_SERVER) else { return Vec::new() };
    // 版と tool を読んだ session の置き場（Backend 1 つ = 1 run・解析 1 本が取る）。REA の起動は 20〜40 秒なので 1 run で 2 回起こさない（W19-3-real の実測）
    let parked: std::rc::Rc<std::cell::RefCell<Option<mcp::McpSession>>> = Default::default();
    let (open, probe, kept) = (server.clone(), server.clone(), parked.clone());
    vec![Box::new(ReaBackend::with_sessions(
        Box::new(move |timeout| {
            let mut s = match parked.borrow_mut().take() {
                Some(s) => s,
                None => mcp::McpSession::open(&open, timeout).map_err(|e| e.to_string())?,
            };
            Ok(Box::new(move |tool: &str, args: &Value, t: Duration| s.call_tool(tool, args, t).map_err(|e| e.to_string())) as analysis::ToolCaller)
        }),
        Box::new(move || {
            let mut s = mcp::McpSession::open(&probe, analysis::REA_PROBE_TIMEOUT).map_err(|e| e.to_string())?;
            let tools = s.tools().map_err(|e| e.to_string())?;
            let version = s.version().to_string();
            *kept.borrow_mut() = Some(s);
            Ok(ToolProbe { version, tools: tools.into_iter().filter(|(_, h)| h.not_destructive).map(|(t, _)| t.name).collect() })
        }),
    ))]
}

/// Deep Software Analysis の 1 回: 使える Backend（最初の Ready）で対象を解析し、report を `<data dir>/analyses/` に・候補を `capabilities/` に残す。
/// Wave: W19-3
/// 例: `let d = capability_engine::deep_analysis(&req, &mut cx)?; found.extend(d.candidates);`
/// 保証する: 承認（`req.consent`）の無い依頼は Backend が `NotPermitted`・この Mac では解析を同時に 1 本（走っていれば `Overloaded` で何もしない）・頼んだ対象と違う
/// 物の結果と `Verified` を名乗る finding を持つ結果は `Backend` で何も残さない・report の id と backend は Engine が付け直す（同じ Backend・対象・版なら同じ id
/// = 次の run は解析し直さずに読む）・候補は全部 `candidates_from`（Unverified・Ready にしない）。保証しない: 候補が使える事（Verification の段）。
pub fn deep_analysis(req: &AnalysisRequest, cx: &mut EngineContext) -> Result<DeepAnalysis, AnalysisError> {
    let Some(b) = cx.analysis.iter_mut().find_map(|b| matches!(b.availability(), Availability::Ready { .. }).then_some(b)) else {
        return Err(AnalysisError::Unavailable("No analysis tool is available".into()));
    };
    let _busy = Busy::take().ok_or_else(|| AnalysisError::Overloaded("Another analysis is running (only one at a time on this Mac)".into()))?;
    let mut report = b.analyze(req)?;
    if report.target != req.target {
        return Err(AnalysisError::Backend("A result came back for something other than what was asked".into()));
    }
    if report.findings.iter().any(|f| f.status == FindingStatus::Verified) {
        return Err(AnalysisError::Backend("The analysis tool called itself “verified” (Shibubu checks results in the app itself)".into()));
    }
    (report.id, report.backend) = (analysis_id(b.id(), &req.target), b.id().to_string());
    analysis::save_report(&cx.data_dir.join(ANALYSES_DIR), &report)?;
    let candidates = candidates_from(&report);
    for c in &candidates {
        save_record(&cx.data_dir.join(CAPABILITIES_DIR), c).map_err(|e| AnalysisError::Store(e.to_string()))?;
    }
    Ok(DeepAnalysis { report, candidates })
}

/// Deep Analysis の段の、操作 1 つの決め（`run` の中だけ）。
pub(super) enum AnalysisPlan {
    Skip(String),
    /// 前に解析した結果を読んだ（解析し直さない・承認は要らない）
    Saved(String, Vec<CapabilityRecord>),
    /// 承認が在る（通常の探索で足りず、利用者が承認した）: 解析する（report の id・依頼）
    Run(String, AnalysisRequest),
    /// 承認がまだ無い: 解析しない。生成の model が手段無しと答えた時だけ、この対象の解析の承認を求める（道具はその時に確かめる・`analysis_ask`）
    IfShort(AnalysisTarget),
}

/// 解析は通常の探索で足りない時だけ（19 通目 §3 の 3・§4）: ここでは前の結果を読むか、承認の在る対象を解析するだけ。その app 専用の確かめ済みの能力が
/// 在るかでは決めない（別の目的で手段が無い時もある・手段が足りるかは生成の model が決める）。道具を起こすのは承認が在る時だけ。
pub(super) fn analysis_plan(goal: &str, p: &Plan, saved: &[Workflow], cx: &mut EngineContext) -> AnalysisPlan {
    use AnalysisPlan::Skip;
    let Some(target) = p.op.target.as_deref() else { return Skip("Doesn't analyze an action whose target app isn't decided".into()) };
    let wf = workflow_id(goal, &p.op);
    if saved.iter().any(|w| w.id == wf && usable(w, &p.found, cx.registry)) || load_trial(cx.data_dir, &wf).is_some() {
        return Skip("There's a saved procedure for this purpose (not analyzing)".into());
    }
    if cx.analysis.is_empty() {
        return Skip("No analysis backend (it is not required)".into());
    }
    let Some(t) = analysis_target(target) else { return Skip("Can't tell where the target app is (not analyzing)".into()) };
    for b in cx.analysis.iter_mut() {
        let input = analysis_input(b.as_ref(), &t);
        let id = analysis_id(b.id(), &t);
        if let Ok(r) = analysis::load_report(&cx.data_dir.join(ANALYSES_DIR), &id) {
            return AnalysisPlan::Saved("Used the earlier analysis result (didn't analyze again)".into(), candidates_from(&r));
        }
        let action = analysis_action(b.id());
        let Some(c) = consent_for(cx.consents, &action, &input) else { continue };
        // ponytail: 解析の承認の使用済みは数えない（同じ対象・版は report を読むので 2 回目は解析しない）。版が変われば input が変わり承認し直す
        if crate::adapters::approval_gap(Some(c), cx.caller, &action, &[], &input, &HashSet::new()).is_some() {
            continue;
        }
        return match b.availability() {
            Availability::Ready { probe_ms, .. } => AnalysisPlan::Run(
                id,
                AnalysisRequest {
                    target: t,
                    question: p.op.want.clone(),
                    consent: AnalysisConsent { granted_by: format!("You (approval {})", c.id), at: c.decided_at.clone(), scope: ANALYSIS_SCOPE.into() },
                    placement: Placement::Local,
                    timeout_ms: analysis_timeout(std::env::var(ANALYSIS_TIMEOUT_ENV).ok().as_deref(), Duration::from_millis(probe_ms)).as_millis() as u64,
                },
            ),
            Availability::Missing { why } => Skip(format!("The analysis tool can't be used ({why}) · carrying on the existing way without analysis")),
        };
    }
    AnalysisPlan::IfShort(t)
}

/// 解析 1 回の待ちの上限: env `SHIBUBU_ANALYSIS_TIMEOUT_S`（秒・読めない・0 は既定 `ANALYSIS_TIMEOUT`）と、道具を確かめた実測（`Availability::Ready::probe_ms`）× 3 の
/// 大きい方（混んだ Mac では起こすのが遅い分だけ読むのも遅い・W19-3-fix）。
pub(super) fn analysis_timeout(env: Option<&str>, probe: Duration) -> Duration {
    // 1 日で頭打ち（Instant に足して溢れない）
    let base = env.and_then(|s| s.trim().parse::<u64>().ok()).filter(|&s| s > 0).map_or(ANALYSIS_TIMEOUT, |s| Duration::from_secs(s.min(86_400)));
    base.max(probe.saturating_mul(3))
}

/// 生成の model が手段無しと答えた操作の、解析の承認（使える解析の道具〈最初の Ready〉が在る時だけ・道具はここで初めて確かめる）。無ければ None（解析せず手段無しのまま）。
pub(super) fn analysis_ask(op: &Operation, t: &AnalysisTarget, analysis: &mut [Box<dyn AnalysisBackend>]) -> Option<PendingApproval> {
    let b = analysis.iter_mut().find_map(|b| matches!(b.availability(), Availability::Ready { .. }).then_some(b))?;
    let (action, input) = (analysis_action(b.id()), analysis_input(b.as_ref(), t));
    Some(PendingApproval { step: op.id.clone(), digest: sdk::payload_digest(&action, &input), action, input, scopes: vec![] })
}

/// 解析の承認の action（`analysis.<backend>`）。
pub(super) fn analysis_action(backend: &str) -> String {
    format!("{ANALYSIS_ACTION}.{}", slug(backend))
}

/// 解析の承認の exact payload（app の名前・path・版・範囲・道具の名前・呼ぶ tool。求める時と照らす時で同じ物・道具を起こさずに決まる）。
pub(super) fn analysis_input(b: &dyn AnalysisBackend, t: &AnalysisTarget) -> Value {
    json!({ "app_name": app_name(t), "target": t.path, "version": t.version, "scope": ANALYSIS_SCOPE, "by": b.id().to_uppercase(), "tools": b.tools_for(t) })
}

/// 操作の対象（bundle id か app の絶対 path）→ 解析する物（path・bundle id・版）。見つからなければ None（解析しない）。対象の app を起動しない。
pub(super) fn analysis_target(target: &str) -> Option<AnalysisTarget> {
    let t = target.trim();
    if t.starts_with('/') {
        let p = Path::new(t);
        if !p.exists() || p.components().any(|c| c == Component::ParentDir) {
            return None;
        }
        let plist = crate::intel::read_plist(&p.join("Contents/Info.plist"), LOCATE_TIMEOUT);
        let key = |k: &str| plist.as_ref().and_then(|v| v[k].as_str()).map(str::to_string);
        return Some(AnalysisTarget { bundle_id: key("CFBundleIdentifier"), path: t.to_string(), version: key("CFBundleShortVersionString") });
    }
    if !crate::intel::is_bundle_id(t) {
        return None;
    }
    let found = crate::intel::investigate(t, LOCATE_TIMEOUT).ok()?;
    let app = found.app.found()?;
    Some(AnalysisTarget { bundle_id: app.bundle_id.clone(), path: app.path.clone(), version: app.version.clone() })
}

/// 承認の 1 行に出す app の名前（path の最後の名前から .app を落とした物）。
pub(super) fn app_name(t: &AnalysisTarget) -> String {
    Path::new(&t.path).file_stem().map_or_else(|| "Target app".into(), |s| s.to_string_lossy().into_owned())
}

/// 解析の結果の id（同じ Backend・対象・版なら同じ = 2 回目は解析し直さずに読む・版が変われば解析し直す）。
pub(super) fn analysis_id(backend: &str, t: &AnalysisTarget) -> String {
    let key = format!("{backend}\n{}\n{}", t.path, t.version.as_deref().unwrap_or(""));
    format!("an-{}-{}", slug(&app_name(t)), &crate::crypto::sha256_hex(key.as_bytes())[..16])
}

/// Deep Analysis の段の記録の 1 行。
pub(super) fn analyzed_note(d: &DeepAnalysis) -> String {
    let plural = crate::domain::plural;
    format!(
        "Analyzed {} with {} (version {}): {} found · {} (not used until checked) · {}",
        app_name(&d.report.target),
        d.report.backend,
        d.report.target.version.as_deref().unwrap_or("Unknown"),
        plural(d.report.findings.len(), "thing"),
        plural(d.candidates.len(), "candidate"),
        plural(d.report.constraints.len(), "limit")
    )
}
