//! 信頼済 CLI: 人が絶対 path を明示して登録した実行ファイルを 1 回走らせる = external_write ＋ scope exec:<feature_id>（CD-01:290・ADR-002:46・
//! ADR-005 決定 3。毎回 exact payload の承認）。経路は許可された OS automation（local の子 process）。実行と分類は src/repos.rs の
//! run_trusted_feature_outcome（exit 0 = Unverified・features dir の code は拒否・登録時の指紋と違えば拒否・pipe で止まらない・stdin なし）。
//! 1 本への承認で別の CLI は走らない（scope が CLI ごと）。承認 card が見せた物（登録した実行ファイル・引数）と走る物を結ぶのは scope ではなく
//! 登録時の指紋 RepoFeature::cli_pin（path・引数・symlink の先・中身のどれが変わっても走らない・ADR-002 硬化 (b)）。
use super::super::domain::{action::Effect, binding::Source, capability::CapabilityReadiness, run::{Observation, Outcome}};
use super::super::model::RepoFeature;
use super::super::repos::run_trusted_feature_outcome;
use super::sdk::{now, source, Adapter, AdapterKind, Route, SourceSync, Spec};
use serde_json::{json, Value};
use std::path::Path;

const SPECS: &[Spec] = &[
    Spec { id: "cli.trusted.run", effect: Effect::ExternalWrite, approval: true, scopes: &[],
        input: r#"{"type":"object","required":["feature_id"],"properties":{"feature_id":{"type":"integer"}}}"#, verifier: "human_review" },
];

/// 承認が束ねる scope: exec:<feature_id>（CD-01 / ADR-005 の形）。入力の feature_id がそのまま入る（登録の無い id は誰も持たない scope になり、invoke も拒否する）
pub fn exec_scope(feature_id: &Value) -> String {
    format!("exec:{feature_id}")
}

pub struct TrustedCli { features: Vec<RepoFeature>, sync: SourceSync }
impl TrustedCli {
    pub fn new(features: Vec<RepoFeature>) -> Self { Self { features, sync: SourceSync::default() } }
    fn configured(&self) -> bool { self.features.iter().any(|f| f.enabled) }
}
impl Adapter for TrustedCli {
    fn id(&self) -> &'static str { "trusted_cli" }
    fn kind(&self) -> AdapterKind { AdapterKind::Repo }
    fn route(&self) -> Route { Route::OsAutomation }
    fn provider(&self) -> &'static str { "cli" }
    fn specs(&self) -> &'static [Spec] { SPECS }
    fn readiness(&self, _action: &str) -> CapabilityReadiness { self.sync.capability(self.configured()) }
    fn sources(&self) -> Vec<Source> { vec![source("cli.trusted", AdapterKind::Repo, "cli", &self.sync, self.configured())] }
    /// 有効な CLI の実行ファイルが「在る絶対 path」か（走らせはしない）。1 本でも在れば接続は使える: 欠けた 1 本は実行の時に
    /// run_trusted_feature_outcome が拒否するので、他の正常な CLI まで止めない（AD-R3-4）
    fn check(&mut self) {
        let t = now();
        let enabled: Vec<&RepoFeature> = self.features.iter().filter(|f| f.enabled).collect();
        let present = enabled.iter().filter(|f| Path::new(&f.cli_path).is_absolute() && Path::new(&f.cli_path).is_file()).count();
        if present > 0 {
            self.sync.record(t, &Outcome::Verified { evidence: vec![format!("登録済みの CLI {} 件のうち {present} 件の実行ファイルが在る", enabled.len())] }, None);
        } else {
            self.sync.missing(t);
        }
    }
    fn invoke(&mut self, _action: &str, input: &Value) -> (Outcome, Option<Observation>) {
        let id = input["feature_id"].as_u64();
        let Some(feature) = self.features.iter().find(|f| Some(f.id) == id) else { return (Outcome::Rejected { reason: "その CLI は登録されていません".into() }, None) };
        let outcome = run_trusted_feature_outcome(feature);
        self.sync.record(now(), &outcome, None);
        (outcome, None)
    }
    fn cleanup(&mut self) { self.features.clear(); self.sync = SourceSync::default(); }
    /// 宣言は有効な CLI 全部の exec:<id>、実行はその入力の 1 本の exec:<id>
    fn scopes(&self, _spec: &Spec, input: Option<&Value>) -> Vec<String> {
        match input {
            Some(i) => vec![exec_scope(&i["feature_id"])],
            None => self.features.iter().filter(|f| f.enabled).map(|f| exec_scope(&json!(f.id))).collect(),
        }
    }
}
