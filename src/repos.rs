use crate::{github::{repo_api,valid_component_path,valid_repo},model::RepoFeature};
use super::domain::run::{ErrorCode,Outcome};
use std::{fs,io::Read,path::{Path,PathBuf},process::{Child,Command,ExitStatus,Stdio},sync::{Arc,Mutex},thread,time::{Duration,Instant}};

#[derive(Debug,Clone)]
pub struct RepoInspection {
    pub owner:String,pub repo:String,pub subdir:String,pub sha:String,
    pub license:String,pub license_url:String,pub license_path:String,pub description:String,
}

pub fn inspect(repo:&str,subdir:&str)->Result<RepoInspection,String>{
    if !valid_repo(repo) || !valid_component_path(subdir){return Err("Give owner/repo and the subfolder to bring in".into())}
    let metadata=repo_api(repo,"")?;
    let branch=metadata["default_branch"].as_str().ok_or("Can't get the default branch")?;
    if !branch.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'-'|b'_'|b'.'|b'/')){return Err("The branch name isn't valid".into())}
    let commit=repo_api(repo,&format!("/commits/{branch}"))?;
    let sha=commit["sha"].as_str().ok_or("Can't get the commit")?.to_string();
    if sha.len()!=40 || !sha.bytes().all(|c|c.is_ascii_hexdigit()){return Err("The commit isn't valid".into())}
    let entry=repo_api(repo,&format!("/contents/{subdir}?ref={sha}"))?;
    if !entry.is_array(){return Err("The given path isn't a folder".into())}
    let license=metadata["license"]["spdx_id"].as_str().unwrap_or("UNKNOWN").to_string();
    let license_url=metadata["license"]["url"].as_str().unwrap_or("").to_string();
    let license_entry=repo_api(repo,"/license")?;
    let license_path=license_entry["path"].as_str().ok_or("Can't confirm where the license file is")?.to_string();
    if !valid_component_path(&license_path){return Err("The license path isn't valid".into())}
    let (owner,project)=repo.split_once('/').ok_or("The repository name isn't valid")?;
    Ok(RepoInspection{owner:owner.into(),repo:project.into(),subdir:subdir.into(),sha,
        license,license_url,license_path,description:metadata["description"].as_str().unwrap_or("").chars().take(180).collect()})
}

pub fn import_feature(review:&RepoInspection,base:&Path,license_accepted:bool)->Result<RepoFeature,String>{
    if !license_accepted{return Err("Approve the license and dependency review".into())}
    if !valid_repo(&format!("{}/{}",review.owner,review.repo)) || !valid_component_path(&review.subdir)
      || review.sha.len()!=40 || !review.sha.bytes().all(|b|b.is_ascii_hexdigit()) {
        return Err("The review information isn't valid".into());
    }
    if review.license=="UNKNOWN" || review.license=="NOASSERTION" || review.license.is_empty(){
        return Err("The license isn't confirmed. Confirm an explicit license, then bring it in".into());
    }
    fs::create_dir_all(base).map_err(|e|e.to_string())?;
    let target=base.join(format!("{}-{}-{}",review.owner,review.repo,&review.sha[..12]));
    if target.exists(){return Err("This commit has already been brought in".into())}
    let remote=format!("https://github.com/{}/{}.git",review.owner,review.repo);
    let t=target.to_string_lossy().to_string();
    run_git(&["clone","--filter=blob:none","--sparse","--depth=1","--single-branch","--no-checkout",&remote,&t],None)?;
    let outcome=(||{
        // Checkout only the chosen subtree plus its license notice.
        let directory_pattern=format!("/{}/**",review.subdir);
        let license_pattern=format!("/{}",review.license_path);
        run_git(&["sparse-checkout","set","--no-cone","--",&directory_pattern,&license_pattern],Some(&target))?;
        run_git(&["checkout","--detach","HEAD"],Some(&target))?;
        let head=git_output(&["rev-parse","HEAD"],Some(&target))?;
        if !head.trim().eq_ignore_ascii_case(&review.sha){return Err("The upstream commit changed after the check. Bringing it in was stopped for safety".into())}
        let component=target.join(&review.subdir);
        if !component.is_dir(){return Err("The subfolder for that feature couldn't be fetched".into())}
        let root=target.canonicalize().map_err(|e|e.to_string())?;
        let canonical=component.canonicalize().map_err(|e|e.to_string())?;
        if !canonical.starts_with(&root){return Err("A fetched file was outside the repository folder. Bringing it in was stopped for safety".into())}
        if !target.join(&review.license_path).is_file(){return Err("Can't confirm the license file was saved".into())}
        Ok(())
    })();
    if let Err(e)=outcome{let _=fs::remove_dir_all(&target);return Err(e)}
    Ok(RepoFeature{id:crate::model::new_id(),owner:review.owner.clone(),repo:review.repo.clone(),
        subdir:review.subdir.clone(),commit:review.sha.clone(),license:review.license.clone(),
        local_path:target.to_string_lossy().to_string(),cli_path:String::new(),cli_args:vec![],enabled:false,cli_pin:String::new()})
}
fn run_git(args:&[&str],cwd:Option<&Path>)->Result<(),String>{
    let output=git_process(args,cwd)?;
    if !output.status.success(){return Err(format!("Git failed: {}",String::from_utf8_lossy(&output.stderr).chars().take(400).collect::<String>()))}
    Ok(())
}
fn git_output(args:&[&str],cwd:Option<&Path>)->Result<String,String>{
    let output=git_process(args,cwd)?;
    if !output.status.success(){return Err("git rev-parse failed".into())}
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
fn git_process(args:&[&str],cwd:Option<&Path>)->Result<std::process::Output,String>{
    let mut command=Command::new("git");command.arg("-c").arg("core.hooksPath=/dev/null").args(args)
        .env("GIT_TERMINAL_PROMPT","0").env("GIT_CONFIG_NOSYSTEM","1")
        .env("GIT_CONFIG_GLOBAL","/dev/null")
        .env("GIT_LFS_SKIP_SMUDGE","1")
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir)=cwd{command.current_dir(dir);}
    let child=command.spawn().map_err(|e|format!("Can't start git: {e}"))?;
    match wait_bounded(child,Duration::from_secs(90)).map_err(|e|e.to_string())?{
        Some((status,stdout,stderr))=>Ok(std::process::Output{status,stdout,stderr}),
        None=>Err("Git timed out (90 s)".into()),
    }
}
/// 子が書く pipe を別 thread で読む（1MB+1 byte まで残し、残りは捨てる）。take は EOF を until まで待ち、来なければその時までに読めた分を返す
struct Pipe(Arc<Mutex<Vec<u8>>>,thread::JoinHandle<()>);
impl Pipe {
    fn read<R:Read+Send+'static>(r:Option<R>)->Self{
        let buf=Arc::new(Mutex::new(Vec::new()));
        let b=Arc::clone(&buf);
        Pipe(buf,thread::spawn(move||{
            let Some(mut r)=r else{return};
            let mut chunk=[0u8;8192];
            loop{match r.read(&mut chunk){
                Ok(0)=>break,
                Ok(n)=>{let mut g=b.lock().unwrap_or_else(|e|e.into_inner());let room=1_048_577usize.saturating_sub(g.len());g.extend_from_slice(&chunk[..n.min(room)]);}
                Err(e) if e.kind()==std::io::ErrorKind::Interrupted=>{}
                Err(_)=>break,
            }}
        }))
    }
    fn take(self,until:Instant)->Vec<u8>{
        while !self.1.is_finished()&&Instant::now()<until{thread::sleep(Duration::from_millis(10))}
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e|e.into_inner()))
    }
}
/// 子 process の終わりを limit まで待つ（信頼済 CLI・git の共通の口。spawn は呼ぶ側 = 起動の失敗と待つ間の失敗を分ける）。
/// stdout / stderr は待つ間に読む（64KB を超える出力で子が書けずに止まらない）。limit を超えたら kill して None。
/// 子が終わった後の読み終わり（EOF）も limit まで（最低 2 秒）しか待たない: 孫（`nohup daemon &`）が pipe を継いで握ると EOF は来ないので、
/// その時までに読めた分を返す（AD-R2-2）。短い猶予にしないのは、混んだ機械で読む thread が間に合わずに普通の出力を切らないため
// ponytail: 孫が pipe を握り続けると、読む thread は孫が閉じるまで残る（1 回に 2 本）。止めるなら process group ごと kill する（孫も止まる）
pub fn wait_bounded(mut child:Child,limit:Duration)->std::io::Result<Option<(ExitStatus,Vec<u8>,Vec<u8>)>>{
    let (out,err)=(Pipe::read(child.stdout.take()),Pipe::read(child.stderr.take()));
    let until=Instant::now()+limit;
    let status=loop{
        match child.try_wait(){
            Ok(Some(s))=>break s,
            Ok(None) if Instant::now()>until=>{let _=child.kill();let _=child.wait();return Ok(None)}
            Ok(None)=>thread::sleep(Duration::from_millis(40)),
            Err(e)=>{let _=child.kill();let _=child.wait();return Err(e)}
        }
    };
    let until=until.max(Instant::now()+Duration::from_secs(2));
    Ok(Some((status,out.take(until),err.take(until))))
}
/// 信頼済 CLI を 1 回走らせた結果を domain の Outcome で返す（批評 safety #1・CD-19・ADR-002 (a)）。
/// 終了コード 0 は CLI の自己申告なので Unverified。時間切れの kill は途中まで走った可能性があるので EffectUnknown
/// 信頼済 CLI の指紋: 実行ファイルの実 path（symlink を解く）・固定引数・実行ファイルの中身の sha256（ADR-002 硬化 (b)）。
/// 登録の時に RepoFeature::cli_pin に固定し、実行の直前に読み直して比べる。読めなければ None
// ponytail: 実行のたびに実行ファイルを丸ごと読む（大きな binary でも 1 回の sha256）。重くなったら mtime と大きさで先に絞る
pub fn cli_pin(feature:&RepoFeature)->Option<String>{
    let path=Path::new(&feature.cli_path).canonicalize().ok()?;
    let body=fs::read(&path).ok()?;
    let id=serde_json::json!([path.display().to_string(),feature.cli_args,crate::crypto::sha256_hex(&body)]);
    Some(crate::crypto::sha256_hex(id.to_string().as_bytes()))
}
/// RQ4（RQ-017・Connections › Trusted commands の `Choose program…`）: 利用者が macOS の choose file で選んだ program（絶対 path・打たせない）と、1 つの欄で打った
/// 固定の引数から Trusted command を 1 つ作る（取り込みの欄 `owner`・`repo`・`subdir`・`commit`・`license`・`local_path` は空・`enabled = true`・`cli_pin` = `cli_pin` の指紋）。
/// 呼ぶ側（本体の app.rs）は足す前に行内の確認（`Shell::connection_confirm`）で program の名前・引数・指紋の頭 8 文字と `Shibubu asks before every run` を見せ、
/// `Allow` の後だけ `store.features` に足す。例: `let f = repos::trusted_command(new_id(), Path::new(&chosen), args)?;`
/// 保証する: 相対 path・在らない file・dir・取り込んだ code の dir（`features/` の下）の file は Err の 1 行（英語）・引数は空白で分けた語のまま固定（後から替えるには消して足し直す）。
/// 保証しない: program が安全な事（走らせる度に承認・`run_trusted_feature_outcome` が直前に指紋を読み直す）・Engine の CLI の手（`RecipeCall::Cli`）をこの list で縛る事（WF の段）。
/// Wave: RQ4
pub fn trusted_command(id:u64,program:&Path,args:Vec<String>)->Result<RepoFeature,String>{
    if !program.is_absolute(){return Err("Choose the program with Choose program… (Shibubu needs its full place on this Mac)".into())}
    if !program.is_file(){return Err("That isn't a program file (it's missing or a folder) · Choose program… again".into())}
    let real=program.canonicalize().map_err(|_|"Can't check the program · Choose program… again".to_string())?;
    // 取り込んだ code（data dir の features/ の下）は走らせない（run_trusted_feature_outcome と同じ門を足す前にも）
    if real.ancestors().any(|a|a.file_name().is_some_and(|n|n=="features")&&a.parent().is_some_and(|p|p.join("sessions.json").is_file())){
        return Err("Code that was brought in can't be run (only programs a person installed)".into())
    }
    // 実行できる file だけ（/etc/hosts のような走らせられない file を Trusted command に足させない）
    {use std::os::unix::fs::PermissionsExt;
        if real.metadata().map(|m|m.permissions().mode()&0o111==0).unwrap_or(true){return Err("That file isn't a program (it can't be run) · Choose program… again".into())}}
    let mut f=RepoFeature{id,owner:String::new(),repo:String::new(),subdir:String::new(),commit:String::new(),license:String::new(),local_path:String::new(),
        cli_path:program.display().to_string(),cli_args:args,enabled:true,cli_pin:String::new()};
    f.cli_pin=cli_pin(&f).ok_or("Can't read the program to take its fingerprint · Choose program… again")?;
    Ok(f)
}
pub fn run_trusted_feature_outcome(feature:&RepoFeature)->Outcome{
    let reject=|m:&str|Outcome::Rejected{reason:m.into()};
    if !feature.enabled{return reject("The command-line connection isn't turned on")}
    let p=PathBuf::from(&feature.cli_path);
    if !p.is_absolute() || !p.is_file(){return reject("The program must be an absolute path that exists")}
    // 取り込んだ code（features dir = local_path の親）の中の file は実行しない。symlink は canonicalize で解いてから比べる
    if !feature.local_path.is_empty(){
        let Ok(cli)=p.canonicalize() else {return reject("Can't check the program")};
        if let Some(Ok(dir))=Path::new(&feature.local_path).parent().map(Path::canonicalize){
            if cli.starts_with(&dir){return reject("Code that was brought in can't be run (only programs a person installed)")}
        }
    }
    // 登録した時（承認 card が見せる物）の実行ファイル・引数・中身と同じ物だけを走らせる。実行の直前に読み直す（ADR-002 硬化 (b)・AD-R3-5）
    if feature.cli_pin.is_empty() || cli_pin(feature).as_deref()!=Some(feature.cli_pin.as_str()){
        return reject("The program or arguments differ from when it was registered (the contents may have changed). Register the connection again")
    }
    // Explicitly configured installed executable, never guessed from downloaded GitHub source. stdin は渡さない（対話で止まらせない）
    let child=match Command::new(&p).args(&feature.cli_args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn(){
        Ok(c)=>c,Err(e)=>return Outcome::Rejected{reason:format!("Couldn't start the program: {e}")}};
    let (status,stdout)=match wait_bounded(child,Duration::from_secs(25)){
        Ok(Some((status,stdout,_)))=>(status,stdout),
        Ok(None)=>return Outcome::EffectUnknown{reason:"Stopped the program after 25 s. It may have run partway".into()},
        Err(e)=>return Outcome::EffectUnknown{reason:format!("Can't confirm the program ended ({e}). It may have run partway")},
    };
    if !status.success(){return Outcome::Failed{error:ErrorCode::DriverRefused}}
    Outcome::Unverified{reason:format!("CLIは終了コード0で終わりました（自己申告）。業務上の成功は未検証。出力: {}",String::from_utf8_lossy(&stdout).chars().take(3000).collect::<String>())}
}
/// 旧来の文字列の口（app.rs が使う）。中身は run_trusted_feature_outcome の薄い wrapper
pub fn run_trusted_feature(feature:&RepoFeature)->Result<String,String>{
    match run_trusted_feature_outcome(feature){
        Outcome::Verified{evidence}=>Ok(evidence.join(" / ")),
        Outcome::Unverified{reason}=>Ok(reason),
        Outcome::Rejected{reason}|Outcome::EffectUnknown{reason}=>Err(reason),
        Outcome::Failed{..}=>Err("The program ended with an error".into()),
    }
}
#[cfg(test)]
mod tests {
 use super::*;
 #[test] fn cannot_import_without_consent(){let i=RepoInspection{owner:"a".into(),repo:"b".into(),subdir:"src".into(),sha:"a".repeat(40),license:"MIT".into(),license_url:"".into(),license_path:"LICENSE".into(),description:"".into()};assert!(import_feature(&i,Path::new("/tmp/kangae-test-invalid"),false).is_err());}
 /// RQ4 interface（caller の形・本体の前は todo!() で赤）: 選んだ program は絶対 path の file だけ・指紋を持ち・すぐ走れる（取り込みの欄は空）。相対 path と dir は断る。
 /// 負の対照（本体の commit）: `cli_pin` を空のまま返す → `run_trusted_feature` が断って赤。
 #[test] fn rq4_trusted_command_pins_the_chosen_program_and_refuses_relative_paths(){
     let f=trusted_command(7,Path::new("/bin/echo"),vec!["hi".into()]).unwrap();
     assert!(f.enabled&&f.owner.is_empty()&&!f.cli_pin.is_empty()&&f.cli_args==["hi"],"{f:?}");
     assert!(run_trusted_feature(&f).is_ok(),"足した直後に走れない");
     assert!(trusted_command(10,Path::new("/etc/hosts"),vec![]).is_err(),"走らせられない file が足せた");
     assert!(trusted_command(8,Path::new("Cargo.toml"),vec![]).is_err()/* 在る file の相対 path: 絶対 path の門だけが断る（無い名前は is_file でも断るので武装しない） */&&trusted_command(9,Path::new("/bin"),vec![]).is_err());
 }
 #[test] fn unapproved_command_denied(){let f=RepoFeature{id:1,owner:"a".into(),repo:"b".into(),subdir:"src".into(),commit:"a".repeat(40),license:"MIT".into(),local_path:"".into(),cli_path:"/bin/echo".into(),cli_args:vec![],enabled:false,cli_pin:String::new()};assert!(run_trusted_feature(&f).is_err());}
}
