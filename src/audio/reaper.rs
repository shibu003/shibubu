//! 起動中の REAPER に wav を入れる・その insert を undo で 1 回戻す。経路は `REAPER -nonewinst <script.lua>`（REAPER の usage 文:
//! "-nonewinst can be used to add media files and/or run scripts in an already-running instance of REAPER"、
//! whatsnew v6.80 で macOS も対応）。script は REAPER の中で 1 本で「同じ take が既に在るか → InsertMedia → 読み戻し」を回し、
//! 結果を done file に書いて返す。同じ main thread の 1 本なので、timeout（EffectUnknown）の後にもう一度 insert しても二重に入らない。
//! insert は `Undo_BeginBlock2` / `Undo_EndBlock2(0, "Shibubu: add humming <名前> <GUID>", -1)` で包み、undo の口は
//! `Undo_CanUndo2` がその名前で、その GUID の item がこの wav の時だけ `Undo_DoUndo2` を 1 回呼ぶ（他人の操作を戻さない）。
//! 出典: https://www.reaper.fm/whatsnew.txt（v6.80）・https://www.reaper.fm/sdk/reascript/reascripthelp.html
//! （InsertMedia / Undo_BeginBlock2 / Undo_EndBlock2 / Undo_CanUndo2 "returns string of last action,if able,NULL if not" /
//! Undo_DoUndo2 "nonzero if success" / GetMediaSourceFileName / TimeMap2_GetDividedBpmAtTime）・
//! https://www.reaper.fm/sdk/plugin/reaper_plugin.h（UNDO_STATE_ALL 0xFFFFFFFF = -1）。他の口との比較は docs/internal/connectors/reaper.md。
//! path は Lua の長文字列 `[==[ … ]==]` に埋めるので、`]==` や制御文字（改行など）を含む path は `lua_safe` で拒否する（埋めると Lua が走る）。

use super::record::inspect;
use super::surface;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 混んだ機械では REAPER が script を回すまで 30 秒以上かかる事がある（load 427 で実測 34 秒）ので長めに。`--timeout` で変更可
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// REAPER の undo 履歴に出る名前の頭。undo の口はこれで始まる操作しか戻さない
pub const UNDO_PREFIX: &str = "Shibubu: add humming ";

/// 読み戻した長さと wav の長さのずれの許容（秒）
const LENGTH_SLACK: f64 = 0.01;


/// REAPER の中の 1 item（script が読み戻した値）。
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub guid: String,
    /// 入っている track の名前（無名なら ""）
    pub track: String,
    /// 位置（秒）
    pub position: f64,
    /// 長さ（秒）
    pub length: f64,
    /// active take の source の file（REAPER が持っている path）
    pub source: String,
    /// 選択 track が無く、新しい track を作って入れたか
    pub new_track: bool,
    /// item の位置での project の実効 tempo（TimeMap2_GetDividedBpmAtTime）
    pub project_bpm: f64,
}

/// insert の結果（DATA_CONTRACTS §2.1 の Result の形のうち、insert が取る物）。
#[derive(Debug, Clone, PartialEq)]
pub enum Receipt {
    /// 新しい item を読み戻し、source がこの wav で長さが合うのを確かめた
    Verified(Item),
    /// 同じ take（source がこの wav の item）が既に在った。何も入れていない
    Duplicate(Item),
    /// item は増えたが、読み戻しが合わない
    Unverified { item: Item, reason: String },
    /// timeout。入ったかどうか分からない（自動で再送しない。もう一度 insert すると先に Duplicate を確かめる）
    EffectUnknown { reason: String },
}

impl Receipt {
    pub fn kind(&self) -> &'static str {
        match self {
            Receipt::Verified(_) => "verified",
            Receipt::Duplicate(_) => "duplicate",
            Receipt::Unverified { .. } => "unverified",
            Receipt::EffectUnknown { .. } => "effect_unknown",
        }
    }
}

/// Lua の長文字列 `[==[ … ]==]` に埋めても script にならない path なら、その文字列。
/// `]==` を含む（`]==]` で閉じる・末尾の `]==` は閉じと重なって閉じる）・制御文字（改行・tab など）・UTF-8 でない path は拒否。
pub fn lua_safe(path: &Path) -> Result<&str, String> {
    let s = path
        .to_str()
        .ok_or_else(|| format!("The path isn't UTF-8, so it isn't passed to REAPER: {}", path.display()))?;
    if s.contains("]==") || s.chars().any(char::is_control) {
        return Err(format!(
            "The path has characters that can't go into a REAPER script (`]==`, a line break, a tab or another control character), so it wasn't passed (nothing was done): {s:?}. Use the name Shibubu gave it (kangae-hum-date-time.wav), then run it again"
        ));
    }
    Ok(s)
}

/// 送る前の確認: REAPER が起動している（起動はしない）。送る先の実行 file は `KANGAE_REAPER_BIN` → 起動中の process の実行 file
/// （`~/Applications` などに入れた REAPER も）→ `/Applications/REAPER.app`。
fn ready() -> Result<PathBuf, String> {
    let Some(pid) = surface::pid_of(surface::REAPER.process) else {
        return Err(match surface::find_app(&surface::REAPER) {
            Some(_) => "REAPER isn't running. Open REAPER (kangae-surface reaper --open), then run it again".to_string(),
            None => format!("REAPER wasn't found. Install it from {}, open it, then run it again", surface::REAPER.download),
        });
    };
    let bin = std::env::var_os("KANGAE_REAPER_BIN")
        .map(PathBuf::from)
        .or_else(|| surface::exe_of(pid))
        .unwrap_or_else(|| PathBuf::from("/Applications/REAPER.app/Contents/MacOS/REAPER"));
    if !bin.is_file() {
        return Err(format!("REAPER's program file wasn't found: {} (set the location with KANGAE_REAPER_BIN)", bin.display()));
    }
    Ok(bin)
}

/// wav の canonical な path（REAPER に渡す形）と、その長さ（秒）。埋め込めない path はここで止める。
fn take_path(wav: &Path) -> Result<(String, f64), String> {
    let wav = wav.canonicalize().map_err(|_| format!("The wav file wasn't found: {}", wav.display()))?;
    let s = lua_safe(&wav)?.to_string();
    let seconds = inspect(&wav)?.seconds() as f64;
    Ok((s, seconds))
}

/// wav を起動中の REAPER の選択 track の cursor 位置へ入れる（選択が無ければ新しい track）。REAPER は起動しない。
/// 同じ take が既に在れば入れずに Duplicate。timeout は EffectUnknown（自動で再送しない）。
pub fn insert(wav: &Path, timeout: Duration) -> Result<Receipt, String> {
    let (wav, seconds) = take_path(wav)?;
    let bin = ready()?;
    match send(&bin, |done, cancel| script(&wav, done, cancel, INSERT), timeout)? {
        None => Ok(Receipt::EffectUnknown {
            reason: format!(
                "REAPER didn't answer within {} s. Can't tell whether it went in (it won't be sent again on its own). If REAPER has a dialog open, close it and run --insert again (it first checks whether the same take is already in, and does nothing if it is). On a busy Mac, raise the seconds with --timeout",
                timeout.as_secs()
            ),
        }),
        Some(text) => parse_insert(&text, &wav, seconds),
    }
}

/// この wav の insert を REAPER の undo で 1 回戻す。REAPER の最後の操作がこの wav の Kangae の insert の時だけ
/// `Undo_DoUndo2` を呼び、その item が消えたのを読み戻す。戻り値は戻した操作の名前。
pub fn undo(wav: &Path, timeout: Duration) -> Result<String, String> {
    let (wav, _) = take_path(wav)?;
    let bin = ready()?;
    match send(&bin, |done, cancel| script(&wav, done, cancel, UNDO), timeout)? {
        None => Err(format!(
            "REAPER didn't answer within {} s. Can't tell whether it was undone (it won't be called again on its own). Check in REAPER's Edit › Undo History (running --undo again does nothing unless the last action is this take's insert)",
            timeout.as_secs()
        )),
        Some(text) => parse_undo(&text),
    }
}

/// script を temp dir に書いて `bin -nonewinst` で送り、done file を待つ。timeout なら Ok(None)
/// （遅れて走った script が何もしないよう cancel file を置き、temp dir は残す）。script(done, cancel) は Lua の本文。
pub fn send(bin: &Path, script: impl Fn(&str, &str) -> String, timeout: Duration) -> Result<Option<String>, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("kangae-rec-{}-{nanos}", std::process::id()));
    let (done, cancel) = (dir.join("done.txt"), dir.join("cancel"));
    // temp dir（TMPDIR）も script に埋めるので同じ検査を通す
    let text = script(lua_safe(&done)?, lua_safe(&cancel)?);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    let file = dir.join("kangae.lua");
    std::fs::write(&file, text).map_err(|e| format!("Can't write {}: {e}", file.display()))?;
    let mut child = Command::new(bin)
        .arg("-nonewinst")
        .arg(&file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start {}: {e}", bin.display()))?;
    let started = Instant::now();
    let text = loop {
        if let Ok(t) = std::fs::read_to_string(&done) {
            break t;
        }
        if started.elapsed() > timeout {
            // temp dir は残す（消すと遅れた script が REAPER の中で error になる）
            let _ = std::fs::write(&cancel, "timeout");
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // -nonewinst の process は渡し終えると終わる。終わらなければ 2 秒で畳む（zombie を残さない）
    let t = Instant::now();
    while matches!(child.try_wait(), Ok(None)) && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(100));
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(Some(text.trim_end_matches(['\n', '\r']).to_string()))
}

/// done file の 1 行: `ok|dup \t mode \t GUID \t 位置 \t 長さ \t bpm \t source \t track 名`（track 名は最後なので tab を含んでよい）
/// か `err \t 原文`。ok は source と長さを wav と照らして Verified / Unverified に分ける。
pub fn parse_insert(text: &str, wav: &str, seconds: f64) -> Result<Receipt, String> {
    let f: Vec<&str> = text.splitn(8, '\t').collect();
    let bad = || format!("Couldn't read REAPER's reply: {text}");
    match f[0] {
        "err" => return Err(format!("Failed inside REAPER (most likely nothing was added): {}", f.get(1).unwrap_or(&""))),
        "ok" | "dup" if f.len() == 8 => {}
        _ => return Err(bad()),
    }
    let num = |i: usize| f[i].parse::<f64>().map_err(|_| bad());
    let item = Item {
        guid: f[2].to_string(),
        track: f[7].to_string(),
        position: num(3)?,
        length: num(4)?,
        source: f[6].to_string(),
        new_track: f[0] == "ok" && num(1)? == 1.0,
        project_bpm: num(5)?,
    };
    if f[0] == "dup" {
        return Ok(Receipt::Duplicate(item));
    }
    let reason = if item.source != wav {
        format!("The source of the item that went in was {} (what was added was {wav})", item.source)
    } else if (item.length - seconds).abs() > LENGTH_SLACK {
        format!("The item that went in is {:.3} s long (the wav is {seconds:.3} s)", item.length)
    } else {
        return Ok(Receipt::Verified(item));
    };
    Ok(Receipt::Unverified { item, reason })
}

/// undo の done file: `ok \t 名前 \t redo の名前` / `unverified \t 名前` / `refused \t other|another \t 最後の操作の名前` / `err \t 原文`。
pub fn parse_undo(text: &str) -> Result<String, String> {
    let f: Vec<&str> = text.splitn(3, '\t').collect();
    let at = |i: usize| f.get(i).copied().unwrap_or("");
    match (f[0], at(1)) {
        ("ok", desc) => Ok(desc.to_string()),
        ("unverified", desc) => Err(format!("REAPER's undo ran, but the item “{desc}” is still there. Check in REAPER's Edit › Undo History")),
        ("refused", "another") => Err(format!(
            "REAPER's last action is the insert of a different take (“{}”), so nothing is undone (nothing was done)",
            at(2)
        )),
        ("refused", _) => Err(format!(
            "REAPER's last action isn't the insert of this take ({}), so nothing is undone (nothing was done). To go back, use Edit › Undo in REAPER",
            if at(2).is_empty() { "There's nothing that can be undone".to_string() } else { format!("“{}”", at(2)) }
        )),
        ("err", why) => Err(format!("Failed inside REAPER: {why}")),
        _ => Err(format!("Couldn't read REAPER's reply: {text}")),
    }
}

/// REAPER の中で走る Lua: path 3 つ（`lua_safe` を通った物だけ）+ 共通の部品 + 本文（INSERT / UNDO）。
pub fn script(wav: &str, done: &str, cancel: &str, body: &str) -> String {
    format!("local wav = [==[{wav}]==]\nlocal done = [==[{done}]==]\nlocal cancel = [==[{cancel}]==]\nlocal prefix = \"{UNDO_PREFIX}\"\n{PRELUDE}{body}")
}

const PRELUDE: &str = r#"local function report(text)  -- 失敗しても REAPER に error dialog を出さない（pcall）。tmp に書いて rename（読みかけを掴ませない）
  pcall(function()
    local f = assert(io.open(done .. ".tmp", "w"))
    f:write(text)
    f:close()
    os.rename(done .. ".tmp", done)
  end)
end
local c = io.open(cancel, "r")
if c then c:close(); report("err\ttimeout で取り消し済み"); return end
local function guid(item) local _, g = reaper.GetSetMediaItemInfo_String(item, "GUID", "", false); return g end
local function source(take)
  if not take then return "" end
  local s = reaper.GetMediaItemTake_Source(take)
  return s and reaper.GetMediaSourceFileName(s) or ""
end
local function find(g)
  for i = 0, reaper.CountMediaItems(0) - 1 do
    local item = reaper.GetMediaItem(0, i)
    if guid(item) == g then return item end
  end
end
local function line(kind, mode, item)
  local at = reaper.GetMediaItemInfo_Value(item, "D_POSITION")
  local tr = reaper.GetMediaItem_Track(item)
  local name = ""
  if tr then local _, n = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false); name = n end
  return table.concat({kind, mode, guid(item), at, reaper.GetMediaItemInfo_Value(item, "D_LENGTH"),
    reaper.TimeMap2_GetDividedBpmAtTime(0, at), source(reaper.GetActiveTake(item)), name}, "\t")
end
"#;

/// 同じ take が在れば dup → 無ければ undo block の中で InsertMedia → GUID の差で新しい item を探して読み戻す。
pub const INSERT: &str = r#"local ok, err = pcall(function()
  local seen = {}  -- InsertMedia は既存の選択を外さないので、新しい item は GUID の差で探す
  for i = 0, reaper.CountMediaItems(0) - 1 do
    local item = reaper.GetMediaItem(0, i)
    for t = 0, reaper.CountTakes(item) - 1 do
      if source(reaper.GetTake(item, t)) == wav then report(line("dup", 0, item)); return end
    end
    seen[guid(item)] = true
  end
  local mode = reaper.CountSelectedTracks(0) > 0 and 0 or 1  -- 0: 選択 track の cursor 位置 / 1: 新しい track
  local placed
  reaper.Undo_BeginBlock2(0)
  local ok2, err2 = pcall(function()
    reaper.InsertMedia(wav, mode)
    for i = 0, reaper.CountMediaItems(0) - 1 do
      local item = reaper.GetMediaItem(0, i)
      if not seen[guid(item)] then placed = item; break end
    end
  end)
  -- block は失敗しても閉じる。名前に GUID を入れ、undo の口はこの名前と GUID で自分の insert だけを戻す
  reaper.Undo_EndBlock2(0, prefix .. (wav:match("[^/]+$") or wav) .. (placed and (" " .. guid(placed)) or ""), -1)
  if not ok2 then error(err2) end
  if not placed then error("item が増えませんでした") end
  reaper.UpdateArrange()
  report(line("ok", mode, placed))
end)
if not ok then report("err\t" .. tostring(err)) end
"#;

/// 最後の操作が「この wav の Kangae の insert」の時だけ Undo_DoUndo2 を 1 回 → その GUID の item が消えたかを読み戻す。
pub const UNDO: &str = r#"local ok, err = pcall(function()
  local desc = reaper.Undo_CanUndo2(0) or ""
  local g = desc:sub(1, #prefix) == prefix and desc:match("(%b{})$")
  if not g then report("refused\tother\t" .. desc); return end
  local item = find(g)
  if not item or source(reaper.GetActiveTake(item)) ~= wav then report("refused\tanother\t" .. desc); return end
  if reaper.Undo_DoUndo2(0) == 0 then error("Undo_DoUndo2 が 0 を返しました（undo できませんでした）") end
  reaper.UpdateArrange()
  report((find(g) and "unverified" or "ok") .. "\t" .. desc .. "\t" .. (reaper.Undo_CanRedo2(0) or ""))
end)
if not ok then report("err\t" .. tostring(err)) end
"#;
