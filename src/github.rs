use super::domain::run::{ErrorCode, Outcome};
use serde_json::{json, Value};
use std::{env, io::Read, path::{Path, PathBuf}, process::{Command, Stdio}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(12)).redirects(0).build()
}
pub fn valid_repo(repo:&str)->bool {
    let parts:Vec<&str>=repo.split('/').collect();
    parts.len()==2 && parts.iter().all(|part|{
        !part.is_empty() && part.len()<=100 && !part.starts_with('.') && !part.ends_with('.') &&
        part.bytes().all(|c|c.is_ascii_alphanumeric()||matches!(c,b'_'|b'-'|b'.'))
    })
}
pub fn valid_component_path(path:&str)->bool{
    !path.is_empty() && path.len()<=180 && !path.starts_with('/') && !path.contains('\\') &&
    path.split('/').all(|s|!s.is_empty() && s!="." && s!=".." && s.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'.'|b'-')))
}
const API_BASE:&str="https://api.github.com";
/// 試験の偽 server・手元の service の口。host が `127.0.0.1` で、続きは数字だけの `:port` と `/path`・`?query`・`#…` だけ
/// （`http://127.0.0.1.evil.example` も、`@` の前を userinfo にして host を差し替える `http://127.0.0.1:@evil.example` も通さない）
pub fn loopback(url:&str)->bool{
    url.strip_prefix("http://127.0.0.1").is_some_and(|rest|{
        let port=&rest[..rest.find(['/','?','#']).unwrap_or(rest.len())];
        port.is_empty()||port.strip_prefix(':').is_some_and(|p|!p.is_empty()&&p.bytes().all(|b|b.is_ascii_digit()))
    })
}
// 試験用の口: KANGAE_GITHUB_API_BASE は loopback の時だけ採用（それ以外は無視して既定の https://api.github.com）
fn api_base()->String{
    match env::var("KANGAE_GITHUB_API_BASE"){Ok(b) if loopback(&b)=>b.trim_end_matches('/').to_string(),_=>API_BASE.to_string()}
}
fn rate_limited(code:u16,resp:&ureq::Response)->bool{
    (code==403||code==429) && (resp.header("X-RateLimit-Remaining")==Some("0")||resp.header("Retry-After").is_some())
}
fn status_message(code:u16,resp:&ureq::Response)->String{
    if rate_limited(code,resp){
        let reset=resp.header("X-RateLimit-Reset").and_then(|v|v.parse::<u64>().ok());
        let when=match reset{
            Some(t)=>{let now=SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_secs()).unwrap_or(0);
                format!("in about {} min (UNIX time {t})",t.saturating_sub(now).div_ceil(60).max(1))}
            None=>"after a while".into()};
        return format!("GitHub's API limit was reached. Try again {when}, or sign in to GitHub (run gh auth login, then Check again in Connections)")
    }
    match code{
        // agent は redirects(0)（token を転送先へ送らない）。転送は「その名前には無い」= 何もしていない
        300..=399=>format!("GitHub answered with a redirect (HTTP {code}). The repository may have been renamed or moved. Check the new owner/repo (Shibubu doesn't follow redirects)"),
        404=>"The repository wasn't found. Check the spelling of owner/repo (a private repository needs a GitHub sign-in: run gh auth login, then Check again in Connections)".into(),
        401=>"GitHub didn't accept the saved sign-in. Run gh auth login, then Check again in Connections".into(),
        403=>"GitHub refused access. For a private repository, check the permissions of your GitHub sign-in".into(),
        500..=599=>format!("GitHub is having trouble (HTTP {code}). Wait a moment and try again"),
        _=>format!("GitHub gave an unexpected answer (HTTP {code}). Try again")}
}
/// HTTP status → domain の ErrorCode。5xx は再試行できる一時障害として ConnectionLost に寄せる（ErrorCode に server error は無い）
fn status_kind(code:u16,resp:&ureq::Response)->ErrorCode{
    if rate_limited(code,resp){return ErrorCode::RateLimited}
    match code{401|403=>ErrorCode::PermissionDenied,300..=399|404=>ErrorCode::TargetNotFound,422=>ErrorCode::InvalidInput,429=>ErrorCode::RateLimited,500..=599=>ErrorCode::ConnectionLost,_=>ErrorCode::Internal}
}

/// 1 回の request の失敗。文言は従来のまま、分類は ErrorCode と「request が相手に出たか」
#[derive(Debug,Clone,PartialEq)]
pub enum Fail {
    /// 送る前に止めた（入力・設定が不正）
    Invalid(String),
    /// 相手が 2xx 以外の status で答えた（3xx / 4xx / 5xx。3xx は redirects(0) で追わないので失敗）
    Status{code:u16,kind:ErrorCode,message:String},
    /// 接続できなかった（DNS / TCP / TLS）。request は出ていない（ureq 2.12 の ErrorKind::Dns / ConnectionFailed は接続の段でだけ出る）
    Unsent(String),
    /// request を出した後に切れた・時間切れ。相手に届いた可能性がある
    Lost(String),
    /// 2xx だが本文を読めない（大きすぎる・JSON でない）
    Body(String),
}
impl Fail {
    pub fn message(&self)->&str{match self{Self::Invalid(m)|Self::Unsent(m)|Self::Lost(m)|Self::Body(m)|Self::Status{message:m,..}=>m}}
    pub fn code(&self)->ErrorCode{
        match self{Self::Invalid(_)=>ErrorCode::InvalidInput,Self::Status{kind,..}=>*kind,Self::Unsent(_)|Self::Lost(_)=>ErrorCode::ConnectionLost,Self::Body(_)=>ErrorCode::Internal}
    }
}
fn transport(t:&ureq::Transport,message:String)->Fail{
    if matches!(t.kind(),ureq::ErrorKind::Dns|ureq::ErrorKind::ConnectionFailed){Fail::Unsent(message)}else{Fail::Lost(message)}
}

/// GitHub REST の口（base と token）。base は https://api.github.com か loopback だけ（token を他所へ送らない）
#[derive(Debug,Clone)]
pub struct Api{pub base:String,pub token:Option<String>}
impl Api {
    pub fn new(base:&str,token:Option<String>)->Self{
        let base=base.trim_end_matches('/');
        Self{base:if base==API_BASE||loopback(base){base.to_string()}else{API_BASE.to_string()},token:token.filter(|t|!t.trim().is_empty())}
    }
    /// token は app が Connections で決めた物が先（`use_connections`・決めた後は env も gh も読まない・切った GitHub は token なし）。
    /// 決めていなければ環境変数、無ければ gh の login（`gh auth token`・本物の api.github.com に送る時だけ = 試験の loopback の偽 server に本物の token を送らない）。
    pub fn from_env()->Self{
        let base=api_base();
        if let Some((gh,_))=chosen(){
            // PASSWORD-1101 の 2: Connections の token が password manager の項目の参照なら、ここ（裏の thread）で値へ
            return Self::new(&base,gh.and_then(crate::domain::vault::resolve));
        }
        let token=env::var("GITHUB_TOKEN").ok().filter(|t|!t.trim().is_empty()).or_else(||if base==API_BASE{gh_path().and_then(|bin|gh_token_at(&bin))}else{None});
        Self::new(&base,token)
    }
    fn send(&self,req:ureq::Request,body:Option<&Value>)->Result<Value,Fail>{
        let mut req=req.set("User-Agent","Kangae-Rust-MVP/0.3").set("Accept","application/vnd.github+json");
        if let Some(token)=&self.token{req=req.set("Authorization",&format!("Bearer {token}"));}
        let resp=match body{Some(b)=>req.send_json(b),None=>req.call()}.map_err(|e|match e{
            ureq::Error::Status(code,r)=>Fail::Status{code,kind:status_kind(code,&r),message:status_message(code,&r)},
            ureq::Error::Transport(t)=>{let m=format!("Can't connect to GitHub ({t}). Check your network connection and try again");transport(&t,m)}})?;
        // ureq 2.12 は redirects(0) の時 3xx を Ok で返す（Error::Status は 400 以上だけ）。2xx 以外を成功と読まない
        let code=resp.status();
        if !(200..300).contains(&code){return Err(Fail::Status{code,kind:status_kind(code,&resp),message:status_message(code,&resp)})}
        let mut bytes=Vec::new();let mut reader=resp.into_reader().take(1024*1024+1);
        reader.read_to_end(&mut bytes).map_err(|e|Fail::Body(format!("Couldn't read GitHub's answer ({e}). Try again")))?;
        if bytes.len()>1024*1024{return Err(Fail::Body("GitHub's answer is too large".into()))}
        serde_json::from_slice(&bytes).map_err(|_|Fail::Body("GitHub's answer was in an unexpected form (a proxy or a temporary fault, perhaps). Wait a moment and try again".into()))
    }
    pub fn repo(&self,repo:&str,endpoint:&str)->Result<Value,Fail>{
        if !valid_repo(repo){return Err(Fail::Invalid("Give the repository as owner/repo".into()))}
        if (!endpoint.is_empty() && !endpoint.starts_with('/')) || endpoint.contains("..") {return Err(Fail::Invalid("The API path isn't valid (start it with / and don't use ..)".into()))}
        self.send(agent().get(&format!("{}/repos/{repo}{endpoint}",self.base)),None)
    }
    pub fn issues(&self,repo:&str)->Result<Value,Fail>{
        let raw=self.repo(repo,"/issues?state=open&per_page=20")?;
        let arr=raw.as_array().ok_or_else(||Fail::Body("GitHub's list of issues was in an unexpected form. Try again".into()))?;
        Ok(json!({"repo":repo,"items":arr.iter().filter(|v|v.get("pull_request").is_none()).take(12)
            .map(|v|json!({"number":v["number"],"title":v["title"],"url":v["html_url"]})).collect::<Vec<_>>()}))
    }
    pub fn pulls(&self,repo:&str)->Result<Value,Fail>{
        let raw=self.repo(repo,"/pulls?state=open&per_page=12")?;
        let arr=raw.as_array().ok_or_else(||Fail::Body("GitHub's list of pull requests was in an unexpected form. Try again".into()))?;
        Ok(json!({"repo":repo,"items":arr.iter().take(12)
            .map(|v|json!({"number":v["number"],"title":v["title"],"url":v["html_url"]})).collect::<Vec<_>>()}))
    }
    /// 疎通と token の確認。`/rate_limit` は利用枠を消費しない
    pub fn rate_limit(&self)->Result<Value,Fail>{self.send(agent().get(&format!("{}/rate_limit",self.base)),None)}
    /// Issue 作成 1 回の結果を domain の Outcome で返す（批評 safety #1）。
    /// 作成済みの番号と URL が返れば Verified（source の受領）。送った後に切れた・5xx は EffectUnknown（作られた可能性 = 自動で再送しない）
    pub fn create_issue_outcome(&self,repo:&str,title:&str,body:&str)->Outcome{
        let reject=|m:&str|Outcome::Rejected{reason:m.into()};
        if !valid_repo(repo){return reject("The repository name isn't valid. Give it as owner/repo")}
        if title.trim().is_empty()||title.len()>200||body.len()>3000{return reject("The title must be 1 to 200 characters and the text up to 3000 characters")}
        if self.token.is_none(){return reject("Sign in to GitHub to create an issue (run gh auth login, then Check again in Connections)")}
        let maybe=|m:&str|Outcome::EffectUnknown{reason:format!("{m} (the issue may have been created. Check on GitHub, then run it again)")};
        match self.send(agent().post(&format!("{}/repos/{repo}/issues",self.base)),Some(&json!({"title":title,"body":body}))){
            Ok(v)=>match (v["number"].as_u64(),v["html_url"].as_str()){
                (Some(n),Some(url))=>Outcome::Verified{evidence:vec![format!("#{n}"),url.to_string()]},
                _=>Outcome::Unverified{reason:"GitHubは作成を受理しましたが、応答にIssue番号がありません。業務上の成功は未検証。".into()},
            },
            Err(Fail::Body(m))=>Outcome::Unverified{reason:format!("GitHubは作成を受理しましたが、応答を読めません（{m}）。業務上の成功は未検証。")},
            Err(Fail::Status{code:500..=599,message,..})|Err(Fail::Lost(message))=>maybe(&message),
            Err(Fail::Status{kind,..})=>Outcome::Failed{error:kind},
            Err(f)=>Outcome::Failed{error:f.code()},
        }
    }
}

/// gh（GitHub CLI）の在りか。`/opt/homebrew/bin`・`/usr/local/bin`・`/usr/bin` の絶対 path だけ（PATH を信じない = adapters/git.rs と同じ）。
// ponytail: G99 の resolver が入ったらそちらに寄せる（FX5 は探し方を増やさない）
pub fn gh_path()->Option<PathBuf>{
    ["/opt/homebrew/bin/gh","/usr/local/bin/gh","/usr/bin/gh"].iter().map(PathBuf::from).find(|p|p.is_file())
}
/// `gh auth token` の値（5 秒で諦める・stderr は捨てる・値は記録も表示もしない）。login していない・失敗は None。
fn gh_token_at(bin:&Path)->Option<String>{
    let mut child=Command::new(bin).args(["auth","token"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let deadline=Instant::now()+Duration::from_secs(5);
    loop{
        match child.try_wait().ok()?{
            Some(st)=>{if !st.success(){return None}break}
            None if Instant::now()>=deadline=>{let _=child.kill();let _=child.wait();return None}
            None=>std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut out=String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let t=out.trim().to_owned();
    (!t.is_empty()).then_some(t)
}
/// Connections の GitHub の行が読み戻した今の状態（account 名だけ持つ・token は持たない）。
#[derive(Debug,Clone,PartialEq)]
pub enum Standing{
    /// token で `GET /user` を読み戻せた
    SignedIn{login:String},
    /// token も gh も無い
    NoCli,
    /// gh は在るが login していない
    NotSignedIn,
    /// token は在るが GitHub が受けなかった・届かなかった（利用者の言葉の 1 文）
    Failed(String),
}
/// token（環境変数 → gh の順）で `GET /user` を 1 回読み戻す。`base` は api.github.com か loopback。
pub fn standing_at(base:&str,env_token:Option<String>,gh:Option<&Path>)->Standing{
    // 環境変数の token → gh の token の順に、GitHub が受けた最初の物で account 名を読み戻す。古い token が先でも gh の login に落ちる
    // （「gh auth login して」と言うのは、どの token も受けられなかった時だけ）
    let env_token=env_token.map(|t|t.trim().to_owned()).filter(|t|!t.is_empty());
    let gh_token=gh.and_then(gh_token_at).filter(|g|Some(g)!=env_token.as_ref());
    let mut refused=None;
    for token in [env_token.clone(),gh_token.clone()].into_iter().flatten(){
        let api=Api::new(base,Some(token));
        match api.send(agent().get(&format!("{}/user",api.base)),None){
            Ok(v)=>return v["login"].as_str().filter(|l|!l.is_empty()).map_or_else(||Standing::Failed("GitHub didn't say who is signed in".into()),|l|Standing::SignedIn{login:l.into()}),
            Err(Fail::Status{code:401,..})=>refused=Some(Standing::Failed("GitHub didn't accept the saved sign-in · Run gh auth login in a terminal".into())),
            Err(f)=>return Standing::Failed(f.message().into()),
        }
    }
    refused.unwrap_or(if gh.is_none(){Standing::NoCli}else{Standing::NotSignedIn})
}
/// 今の環境の `standing_at`（app.rs の裏の thread が呼ぶ）。
pub fn standing()->Standing{standing_at(&api_base(),env::var("GITHUB_TOKEN").ok(),gh_path().as_deref())}
/// 今の時刻 `HH:MM`（この Mac の地域・`/bin/date`）。読めなければ空。
pub fn clock_hhmm()->String{
    Command::new("/bin/date").arg("+%H:%M").output().ok().map(|o|String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
}


/// RQ4（RQ-015・GitHub の行の `Paste token`）: Keychain に置いた GitHub の token（`secret_store::connection_key("github", "token")`）。無い・読めない・空は None。
/// 例（本体の app.rs）: `let token = (!ui.disconnected.iter().any(|k| k == "github")).then(|| github::keychain_token(&keys)).flatten();`
/// 読む順（本体・FX5 の merge の後）: gh の login（FX5 の `gh_token_at`）→ この token → env `GITHUB_TOKEN`（試験の口）。切った GitHub（`UiSettings::disconnected`）は呼ぶ側が呼ばない。
/// 保証する: 値を log・画面・Err に出さない（返すだけ）。保証しない: token が GitHub で通る事（`Check again` の `GET /rate_limit` が言う・利用枠を使わない）。
/// Wave: RQ4
pub fn keychain_token(keys:&crate::storage::secret_store::SecretStore)->Option<String>{
    keys.get(&crate::storage::secret_store::connection_key("github","token")).ok().flatten().filter(|t|!t.trim().is_empty())
}
/// RQ4（RQ-015）: app が Connections で決めた GitHub の token（None = 切った・無い）と Web data の URL と token（None = 未設定か切った）。
/// 外側の None = app が決めていない（試験・CLI）→ env の `GITHUB_TOKEN`・`KANGAE_WEB_SERVICE_URL`・`KANGAE_WEB_SERVICE_TOKEN` を読む（今まで通り）。
static CHOSEN:std::sync::Mutex<Option<(Option<String>,Option<(String,Option<String>)>)>>=std::sync::Mutex::new(None);
/// RQ4（RQ-015）: GitHub と Web data の読みに使う token と URL を決める（app.rs が起動の 1 回と Connections の手の後に呼ぶ）。決めた後は env を読まない
/// （切った GitHub は token を送らず public の読みだけ・切った Web data は読まない）。例: `github::use_connections(keychain_token(&keys), Some((url, None)))`。
/// 保証する: 値を log・画面に出さない。
/// Wave: RQ4
pub fn use_connections(github:Option<String>,web:Option<(String,Option<String>)>){
    *CHOSEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=Some((github.filter(|t|!t.trim().is_empty()),web));
}
fn chosen()->Option<(Option<String>,Option<(String,Option<String>)>)>{CHOSEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()}
/// Web JSON の URL として受けられるか（https か loopback・userinfo / fragment なし）
pub fn web_url_ok(url:&str)->bool{(url.starts_with("https://")||loopback(url))&&!has_userinfo(url)&&!url.contains('#')}
/// 認証部（scheme の後から最初の `/` `?` `#` まで）に `@` が在るか。path や query の中の `@`（`https://mastodon.social/@x.rss`・`?mail=a@b.com`）は userinfo ではない。
fn has_userinfo(url:&str)->bool{
    let rest=url.split_once("://").map_or(url,|(_,r)|r);
    rest.split(['/','?','#']).next().unwrap_or("").contains('@')
}
/// URL の host（:port まで）。userinfo（`u:p@`）・path・query（鍵を入れる service がある）・fragment は落とす。
/// 画面・根拠・失敗文に URL を出す所は全部これを通す（AD-R1-4 / AD-SEC-R2-2 / AD-SEC-R2-3）
pub fn url_host(url:&str)->&str{
    let rest=url.split_once("//").map_or(url,|(_,r)|r);
    let authority=&rest[..rest.find(['/','?','#']).unwrap_or(rest.len())];
    authority.rsplit_once('@').map_or(authority,|(_,host)|host)
}
/// Web JSON を 1 回読む。https か loopback だけ・URL に userinfo / fragment は入れない
pub fn web_read_at(url:&str,token:Option<&str>)->Result<Value,Fail>{
    if !url.starts_with("https://") && !loopback(url) {return Err(Fail::Invalid("The web API supports HTTPS only".into()))}
    if url.contains('@') || url.contains('#'){return Err(Fail::Invalid("The web API URL can't contain a user name, a password or a “#” part".into()))}
    let mut req=agent().get(url).set("Accept","application/json");
    if let Some(token)=token.filter(|t|!t.is_empty()){req=req.set("Authorization",&format!("Bearer {token}"));}
    // 失敗文は host と status / 種類だけで組む（ureq の Display は URL 全体 = query の鍵を前に付ける）
    let host=url_host(url);
    let resp=req.call().map_err(|e|match e{
        ureq::Error::Status(code,r)=>Fail::Status{code,kind:status_kind(code,&r),message:format!("Web API error: {host} returned HTTP {code}")},
        ureq::Error::Transport(t)=>{let m=format!("Web API error: can't connect to {host} ({})",t.kind());transport(&t,m)}
    })?;
    // 3xx は Ok で来る（redirects(0)）。転送先を読んだ事にしない
    let code=resp.status();
    if !(200..300).contains(&code){return Err(Fail::Status{code,kind:status_kind(code,&resp),message:format!("The web API answered with a redirect (HTTP {code}). Change the URL to the new place (Shibubu doesn't follow redirects)")})}
    let mut bytes=Vec::new();resp.into_reader().take(256*1024+1).read_to_end(&mut bytes).map_err(|e|Fail::Body(e.to_string()))?;
    if bytes.len()>256*1024{return Err(Fail::Body("The web API's answer is too large".into()))}
    let data:Value=serde_json::from_slice(&bytes).map_err(|e|Fail::Body(format!("The web API's answer was in an unexpected form: {e}")))?;
    Ok(json!({"items":data}))
}

// ---- 旧来の文字列の口（app.rs / repos.rs が使う）。中身は上の型付きの関数の薄い wrapper ----
pub fn repo_api(repo:&str,endpoint:&str)->Result<Value,String>{Api::from_env().repo(repo,endpoint).map_err(|f|f.message().into())}
pub fn issues(repo:&str)->Result<Value,String>{Api::from_env().issues(repo).map_err(|f|f.message().into())}
pub fn pulls(repo:&str)->Result<Value,String>{Api::from_env().pulls(repo).map_err(|f|f.message().into())}
pub fn create_issue(repo:&str,title:&str,body:&str)->Result<Value,String>{
    match Api::from_env().create_issue_outcome(repo,title,body){
        Outcome::Verified{evidence}=>Ok(json!({"verified":true,"evidence":evidence})),
        // 「業務上の成功は未検証」の語で app.rs は「受理（結果未検証）」と出す（完了にしない）
        Outcome::Unverified{reason}=>Ok(json!({"verified":false,"reason":reason})),
        Outcome::Rejected{reason}|Outcome::EffectUnknown{reason}=>Err(reason),
        Outcome::Failed{error}=>Err(match error{
            ErrorCode::PermissionDenied=>"GitHub refused (sign-in or write permission). Run gh auth login, then Check again in Connections. No issue was created",
            ErrorCode::RateLimited=>"GitHub's API limit was reached. Try again after a while. No issue was created",
            ErrorCode::TargetNotFound=>"The repository wasn't found (for a private one, check the permissions of your GitHub sign-in). No issue was created",
            ErrorCode::InvalidInput=>"GitHub didn't accept the input. Check the title and the text. No issue was created",
            ErrorCode::ConnectionLost=>"Can't connect to GitHub. Check your network connection and try again. No issue was created",
            _=>"Creating the issue failed. No issue was created",
        }.into()),
    }
}
/// Web data は画面から設定できない間（RQ-015 の欄ができるまで）の事実の 1 行。Connections を指さない。
pub const WEB_DATA_OFF:&str="Web data can't be set up in this version";
pub fn web_read()->Result<Value,String>{
    // RQ4: app が Connections で決めた URL と token（決めていなければ env。未設定の文は FX の WEB_DATA_OFF）。
    if let Some((_,web))=chosen(){
        let (url,token)=web.ok_or("Web data isn't set up · Connect it in Connections")?;
        return web_read_at(&url,token.and_then(crate::domain::vault::resolve).as_deref()).map_err(|f|f.message().into());
    }
    let url=env::var("KANGAE_WEB_SERVICE_URL").map_err(|_|WEB_DATA_OFF)?;
    if !url.starts_with("https://") {return Err("The web API supports HTTPS only".into())}
    web_read_at(&url,env::var("KANGAE_WEB_SERVICE_TOKEN").ok().as_deref()).map_err(|f|f.message().into())
}

#[cfg(test)]
mod tests {
 use super::*;
 use std::{io::Write,net::TcpListener,sync::Mutex};
 static ENV:Mutex<()>=Mutex::new(());
 /// RQ5 反証の直し（rq5）: userinfo（認証部の `@`）だけ断る。path / query の `@` は通す。負の対照: `has_userinfo` を `url.contains('@')` に戻す → 赤。
 #[test] fn rq5_web_url_refuses_userinfo_but_not_an_at_sign_in_path_or_query(){
  assert!(web_url_ok("https://mastodon.social/@x.rss")&&web_url_ok("https://api.example.com/items?mail=a@b.com"));
  assert!(!web_url_ok("https://user:pw@api.example.com/items")&&!web_url_ok("https://user@api.example.com/")&&!web_url_ok("https://api.example.com/items#frag"));
 }
 #[test] fn repo_validation(){assert!(valid_repo("rust-lang/rust"));assert!(!valid_repo("evil/repo/extra"));assert!(!valid_repo("hello/../no"));}
 #[test] fn path_validation(){assert!(valid_component_path("crates/small-tool"));assert!(!valid_component_path("../../etc"));assert!(!valid_component_path("/tmp"));}
 // 固定応答を1回返す偽server。戻り値は KANGAE_GITHUB_API_BASE に入れる値
 fn fake(response:&'static str)->String{
    let l=TcpListener::bind("127.0.0.1:0").unwrap();let port=l.local_addr().unwrap().port();
    std::thread::spawn(move||{if let Ok((mut s,_))=l.accept(){let mut buf=[0u8;2048];let _=s.read(&mut buf);let _=s.write_all(response.as_bytes());}});
    format!("http://127.0.0.1:{port}")
 }
 fn run(base:&str,f:fn(&str)->Result<Value,String>)->Result<Value,String>{env::set_var("KANGAE_GITHUB_API_BASE",base);f("o/r")}
 #[test] fn fake_server_cases(){
    let _g=ENV.lock().unwrap_or_else(|e|e.into_inner());
    env::remove_var("GITHUB_TOKEN");
    let ok=run(&fake("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n[{\"number\":1,\"title\":\"t\",\"html_url\":\"u\"},{\"number\":2,\"title\":\"pr\",\"html_url\":\"v\",\"pull_request\":{}}]"),issues).unwrap();
    assert_eq!(ok["items"].as_array().unwrap().len(),1);assert_eq!(ok["items"][0]["number"],1);
    let e=run(&fake("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n{}"),issues).unwrap_err();assert!(e.contains("wasn't found"),"{e}");
    let e=run(&fake("HTTP/1.1 403 Forbidden\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 4102444800\r\nConnection: close\r\n\r\n{}"),pulls).unwrap_err();
    assert!(e.contains("API limit")&&e.contains("4102444800")&&e.contains("Connections")&&!e.contains("GITHUB_TOKEN"),"{e}");
    let e=run(&fake("HTTP/1.1 403 Forbidden\r\nX-RateLimit-Remaining: 40\r\nConnection: close\r\n\r\n{}"),pulls).unwrap_err();assert!(e.contains("refused")&&!e.contains("API limit"),"{e}");
    let e=run(&fake("HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n\r\n"),issues).unwrap_err();assert!(e.contains("having trouble")&&e.contains("503"),"{e}");
    let e=run(&fake("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n<html>nope</html>"),issues).unwrap_err();assert!(e.contains("unexpected form"),"{e}");
    // 不通: 閉じた port
    let closed={let l=TcpListener::bind("127.0.0.1:0").unwrap();format!("http://127.0.0.1:{}",l.local_addr().unwrap().port())};
    let e=run(&closed,issues).unwrap_err();assert!(e.contains("Can't connect"),"{e}");
    // http は loopback の時だけ。それ以外の base は無視され既定の https に戻る
    env::set_var("KANGAE_GITHUB_API_BASE","http://evil.example");assert_eq!(api_base(),API_BASE);
    env::set_var("KANGAE_GITHUB_API_BASE","http://127.0.0.1.evil.example");assert_eq!(api_base(),API_BASE);
    env::remove_var("KANGAE_GITHUB_API_BASE");assert_eq!(api_base(),API_BASE);
 }
 /// FX5（FX-023）: gh の login だけで（環境変数の token 無しで）`Signed in as` の account 名を読み戻す。gh が無い・login していない・GitHub が受けない時は別の状態。
 /// token の値は結果のどこにも出ない。負の対照（commit message）: `standing_at` の gh の腕を外す → 1 つ目が赤。
 #[test] fn fx5_gh_login_alone_is_enough_and_the_token_is_never_shown(){
    use std::os::unix::fs::PermissionsExt;
    let dir=std::env::temp_dir().join(format!("shibubu-fx5-gh-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();
    let script=|name:&str,body:&str|{let p=dir.join(name);std::fs::write(&p,format!("#!/bin/sh\n{body}\n")).unwrap();std::fs::set_permissions(&p,std::fs::Permissions::from_mode(0o755)).unwrap();p};
    let ok=script("gh-ok","echo ghp_fake_token_value");
    let out=standing_at(&fake("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"login\":\"octo\"}"),None,Some(ok.as_path()));
    assert_eq!(out,Standing::SignedIn{login:"octo".into()});
    assert!(!format!("{out:?}").contains("ghp_fake"),"token が結果に出た");
    assert_eq!(standing_at("http://127.0.0.1:9",None,None),Standing::NoCli,"gh も token も無い");
    let out_gh=script("gh-out","exit 1");
    assert_eq!(standing_at("http://127.0.0.1:9",None,Some(out_gh.as_path())),Standing::NotSignedIn,"gh が login していない");
    let refused=standing_at(&fake("HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n{}"),Some("stale".into()),None);
    assert!(matches!(&refused,Standing::Failed(m) if m.contains("didn't accept")&&!m.contains("stale")),"{refused:?}");
    let _=std::fs::remove_dir_all(&dir);
 }
 // 実 HTTPS（token 無し）: cargo test -- --ignored --nocapture
 #[test] #[ignore] fn real_github(){
    let _g=ENV.lock().unwrap_or_else(|e|e.into_inner());
    env::remove_var("GITHUB_TOKEN");env::remove_var("KANGAE_GITHUB_API_BASE");
    for (n,r) in [("issues",issues("rust-lang/rust")),("pulls",pulls("rust-lang/rust")),("404",issues("rust-lang/no-such-repo-kangae"))]{
        match r{Ok(v)=>println!("{n}: {} 件 先頭={}",v["items"].as_array().unwrap().len(),v["items"][0]),Err(e)=>println!("{n}: ERR {e}")}
    }
 }
}
