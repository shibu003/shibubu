//! Web JSON: 設定した 1 つの URL の JSON を読む = external_read。経路は official API（その service の HTTP API）。
//! HTTP と分類は src/github.rs の web_read_at（https か loopback だけ・userinfo / fragment は拒否・256KB まで）。
use super::super::domain::{action::Effect, binding::Source, capability::CapabilityReadiness, run::{Observation, Outcome}};
use super::super::github::{url_host, web_read_at, web_url_ok};
use super::sdk::{now, read_result, source, Adapter, AdapterKind, Route, SourceSync, Spec};
use serde_json::Value;

const SPECS: &[Spec] = &[
    Spec { id: "web.json.read", effect: Effect::ExternalRead, approval: false, scopes: &["web.read"], input: r#"{"type":"object"}"#, verifier: "read_back" },
];

pub struct WebJson { url: Option<String>, token: Option<String>, sync: SourceSync }
impl WebJson {
    pub fn new(url: Option<String>, token: Option<String>) -> Self { Self { url, token, sync: SourceSync::default() } }
    fn configured(&self) -> bool { self.url.as_deref().is_some_and(web_url_ok) }
    fn read(&self, t: u64) -> (Outcome, Option<Observation>) {
        let Some(url) = self.url.as_deref() else { return (Outcome::Rejected { reason: super::super::github::WEB_DATA_OFF.into() }, None) };
        // 根拠には host だけを書く（query に鍵を入れる service がある。path の無い `https://host?key=…` も query の前で切る）
        read_result(web_read_at(url, self.token.as_deref()).map_err(|f| f.code()), "web.json", &format!("GET {}", url_host(url)), t)
    }
}
impl Adapter for WebJson {
    fn id(&self) -> &'static str { "web_json" }
    fn kind(&self) -> AdapterKind { AdapterKind::Api }
    fn route(&self) -> Route { Route::OfficialApi }
    fn provider(&self) -> &'static str { "web" }
    fn specs(&self) -> &'static [Spec] { SPECS }
    fn readiness(&self, _action: &str) -> CapabilityReadiness { self.sync.capability(self.configured()) }
    fn sources(&self) -> Vec<Source> { vec![source("web.json", AdapterKind::Api, "web", &self.sync, self.configured())] }
    /// 同じ GET で疎通を見る。読んだ中身は捨てるので鮮度は進めない
    fn check(&mut self) { let t = now(); let (o, _) = self.read(t); self.sync.record_check(t, &o); }
    fn invoke(&mut self, _action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
        let t = now();
        let (outcome, observation) = self.read(t);
        self.sync.record(t, &outcome, observation.as_ref());
        (outcome, observation)
    }
    fn cleanup(&mut self) { self.url = None; self.token = None; self.sync = SourceSync::default(); }
}
