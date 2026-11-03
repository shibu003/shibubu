//! GitHub: issues / pulls の一覧 = external_read、Issue 作成 = external_write（承認つき）。経路は official API（REST）。
//! HTTP と結果の分類は src/github.rs の Api（作成は create_issue_outcome）。id は CD-04 の表。
//! source は読む物ごとに 2 つ（github.issues / github.pulls）。鮮度はその読み取りだけが進める（CON-010）。接続の状態（token・疎通）は 1 つを共有する。
use super::super::domain::{action::Effect, binding::Source, capability::CapabilityReadiness, run::{Observation, Outcome}};
use super::super::github::{valid_repo, Api};
use super::sdk::{now, read_result, source, Adapter, AdapterKind, Route, SourceSync, Spec};
use serde_json::Value;

const REPO: &str = r#"{"type":"object","required":["repo"],"properties":{"repo":{"type":"string"}}}"#;
const SPECS: &[Spec] = &[
    Spec { id: "github.issues.list", effect: Effect::ExternalRead, approval: false, scopes: &["github.issues.read"], input: REPO, verifier: "read_back" },
    Spec { id: "github.pulls.list", effect: Effect::ExternalRead, approval: false, scopes: &["github.pulls.read"], input: REPO, verifier: "read_back" },
    Spec { id: "github.issues.create", effect: Effect::ExternalWrite, approval: true, scopes: &["github.issues.write"],
        input: r#"{"type":"object","required":["repo","title"],"properties":{"repo":{"type":"string"},"title":{"type":"string"},"body":{"type":"string"}}}"#,
        verifier: "source_receipt" },
];

pub struct GitHub { api: Api, sync: SourceSync }
impl GitHub {
    pub fn new(api: Api) -> Self { Self { api, sync: SourceSync::default() } }
}
impl Adapter for GitHub {
    fn id(&self) -> &'static str { "github" }
    fn kind(&self) -> AdapterKind { AdapterKind::Api }
    fn route(&self) -> Route { Route::OfficialApi }
    fn provider(&self) -> &'static str { "github" }
    fn specs(&self) -> &'static [Spec] { SPECS }
    /// 公開 repo は token 無しでも読める。書き込みだけ token が要る
    fn readiness(&self, action: &str) -> CapabilityReadiness { self.sync.capability(action != "github.issues.create" || self.api.token.is_some()) }
    fn sources(&self) -> Vec<Source> {
        ["github.issues", "github.pulls"].iter().map(|id| source(id, AdapterKind::Api, "github", &self.sync, true)).collect()
    }
    fn check(&mut self) {
        let o = match self.api.rate_limit() { Ok(_) => Outcome::Verified { evidence: vec!["GET /rate_limit".into()] }, Err(f) => Outcome::Failed { error: f.code() } };
        self.sync.record_check(now(), &o);
    }
    fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
        let repo = input["repo"].as_str().unwrap_or("");
        if !valid_repo(repo) { return (Outcome::Rejected { reason: "repo は owner/repo で指定してください".into() }, None) }
        let t = now();
        let (outcome, observation) = match action {
            "github.issues.create" => (self.api.create_issue_outcome(repo, input["title"].as_str().unwrap_or(""), input["body"].as_str().unwrap_or("")), None),
            "github.pulls.list" => read_result(self.api.pulls(repo).map_err(|f| f.code()), "github.pulls", &format!("GET /repos/{repo}/pulls"), t),
            _ => read_result(self.api.issues(repo).map_err(|f| f.code()), "github.issues", &format!("GET /repos/{repo}/issues"), t),
        };
        self.sync.record(t, &outcome, observation.as_ref());
        (outcome, observation)
    }
    fn cleanup(&mut self) { self.api.token = None; self.sync = SourceSync::default(); }
}
