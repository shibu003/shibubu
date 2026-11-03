//! W10: VT（Terminal の画面の状態）。PTY の byte を食わせ、`TermScreen` の写しを返す。PTY は持たない（PTY の持ち主は `TerminalManager` だけ）。
//! crate は `alacritty_terminal` 0.26（`docs/evidence/W10/vt.md` の 4 観点で vt100 と比べて選んだ・owner 4 通目: 第一候補・`vte` 単体は不可）。
//! 使うのは `Term`（grid・mode・返事）と `vte::ansi::Processor` だけで、crate の `tty`・`event_loop`（PTY）は呼ばない（PTY は portable-pty）。
//! 署名に crate の型を出さない（`TermScreen` に写す）。`TerminalManager` は session ごとに 1 つ持ち、reader の thread が食わせる。PTY 無しの試験はこれを直に使う。

use super::{TermSize, TerminalError};
use crate::domain::terminal::{TermColor, TermCursor, TermRun, TermScreen, TermStyle};

/// TERM-RESTORE-1101: 線の行の印（列 0 の cell に付ける幅 0 の 2 文字 = ZWSP と WORD JOINER・`seed` が書き、`runs` は字にせず `screen` が `rule` に写す）。
/// 2 文字の並びにして、出力に偶然 1 つ混ざった ZWSP を線と読まない。
const RULE_MARK: [char; 2] = ['\u{200B}', '\u{2060}'];
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, Processor};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 1 session の VT。
///
/// 使い方: `let mut vt = Vt::new(TermSize::default(), 10_000); let reply = vt.feed(b"\x1b[31mred\x1b[0m\r\n"); let s = vt.screen(0);`
/// 保証する: byte をどこで切って食わせても同じ画面になる（切れた escape と UTF-8 は次の `feed` へ持ち越す）。壊れた・知らない escape で panic しない。
/// 保証しない: 画面の外への出力（bell・clipboard の OSC 52 は捨てる・題名は `title` で読むだけ）・mouse の報告・sixel などの画像・色の問い合わせ（OSC 10 / 11）への返事。
/// Wave: W10
pub struct Vt {
    term: Term<Replies>,
    parser: Processor,
    replies: Replies,
}

/// `Term` が PTY へ返す byte（DA・DSR の返事）を溜める。`feed` が取り出す。TITLEBAR-ICONS-1101 の 4: 2 つ目 = 最後の題（OSC 0 / 2・ResetTitle で None・`title` が読む）。
#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<u8>>>, Arc<Mutex<Option<String>>>);

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(s) => self.0.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(s.as_bytes()),
            Event::Title(t) => *self.1.lock().unwrap_or_else(|e| e.into_inner()) = Some(t),
            Event::ResetTitle => *self.1.lock().unwrap_or_else(|e| e.into_inner()) = None,
            _ => {}
        }
    }
}

/// `Term` に渡す大きさ（scrollback は `Config` が持つ）。
struct Dims(TermSize);

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
    fn screen_lines(&self) -> usize {
        self.0.rows as usize
    }
    fn columns(&self) -> usize {
        self.0.cols as usize
    }
}

impl Vt {
    /// `size` の空の画面（cursor は左上）。`scrollback_lines` は通常の画面の上に残す行の上限（alt screen は持たない）。
    /// 0 の大きさは 1 に寄せる（拒むのは `TerminalManager::spawn` と `resize`）。
    pub fn new(size: TermSize, scrollback_lines: usize) -> Self {
        let size = TermSize {
            rows: size.rows.max(1),
            cols: size.cols.max(1),
        };
        let config = Config {
            scrolling_history: scrollback_lines,
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let replies = Replies::default();
        Vt {
            term: Term::new(config, &Dims(size), replies.clone()),
            parser: Processor::new(),
            replies,
        }
    }

    /// PTY から読んだ byte を食わせ、端末から PTY へ返す byte（DA1 の `ESC [ c` と DSR の `ESC [ 6 n` への返事）を返す。返事が無ければ空。
    /// 保証する: DA1 と cursor 位置の DSR には必ず返事を返す（tmux・vim が起動の時に問い合わせる・`Term` が返す）。
    /// 同期更新（`ESC [?2026h` 〜 `l`・Claude Code の Ink が使う）の間は描かずに溜め、150ms 待っても終わらなければ溜めた分を描く。
    /// 空の `bytes` は、期限の切れた同期更新を描くだけ（`TerminalManager::screen` が毎 frame 呼ぶ）。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self
            .parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|t| t <= Instant::now())
        {
            self.parser.stop_sync(&mut self.term);
        }
        self.parser.advance(&mut self.term, bytes);
        std::mem::take(&mut *self.replies.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// TITLEBAR-ICONS-1101 の 4: 中の program が付けた最後の題（OSC 0 / 2）。付けていない・ResetTitle の後は None。
    /// 例: `vt.feed(b"\x1b]0;build\x07"); vt.title() // Some("build")`
    pub fn title(&self) -> Option<String> {
        self.replies.1.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 画面の大きさを変える（PTY の大きさは `TerminalManager::resize` が同じ呼び出しで変える・ここは VT だけ）。
    /// rows か cols が 0 なら `InvalidSize` で、何も変えない。折り返した行は新しい幅で折り返し直す（`Term::resize`）。
    pub fn resize(&mut self, size: TermSize) -> Result<(), TerminalError> {
        if size.rows == 0 || size.cols == 0 {
            return Err(TerminalError::InvalidSize(size));
        }
        self.term.resize(Dims(size));
        Ok(())
    }

    /// TERM-RESTORE-1101: 前の起動の画面（`ansi` が作った byte）を食わせ、その下に線の行を 1 つ置く（列 0 の cell に幅 0 の印 `RULE_MARK`・字は描かない）。
    /// 子の出力より先に呼ぶ（`TerminalManager::spawn_restorable` が reader の thread を立てる前）。返事は捨てる（保存した byte に問い合わせは入れない）。
    /// 例: `let mut vt = Vt::new(saved.size, 10_000); vt.seed(saved.ansi.as_bytes()); vt.screen(0).rule // Some(線の行)`
    /// Wave: TERM-RESTORE-1101
    pub fn seed(&mut self, saved: &[u8]) {
        if saved.is_empty() {
            return;
        }
        let _ = self.feed(saved);
        // 前の画面の最後の行の後（`ansi` は行ごとに SGR を戻して改行する）: 空白 1 つに印を付けた行 → 改行 = 新しい shell は次の行から
        let mut rule = String::from("\x1b[0m\r ");
        rule.extend(RULE_MARK);
        rule.push_str("\r\n");
        let _ = self.feed(rule.as_bytes());
    }

    /// 今の画面の写し。`history` は scrollback から上に足す行の数の上限（0 = 今の画面だけ・alt screen の間は常に 0 行）。
    pub fn screen(&self, history: usize) -> TermScreen {
        let grid = self.term.grid();
        let (rows, cols) = (grid.screen_lines(), grid.columns());
        // alt screen の grid は scrollback 0 で作られる（`Term::new` の inactive_grid）ので、ここで 0 になる
        let history = history.min(grid.history_size());
        let mode = *self.term.mode();
        let at = grid.cursor.point;
        let shown = -(history as i32)..rows as i32;
        TermScreen {
            rows: rows as u16,
            cols: cols as u16,
            rule: shown.clone().position(|l| is_rule(&grid[Line(l)])),
            lines: shown.map(|l| runs(&grid[Line(l)], cols)).collect(),
            history,
            cursor: mode.contains(TermMode::SHOW_CURSOR).then_some(TermCursor {
                row: at.line.0 as u16,
                col: at.column.0 as u16,
            }),
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        }
    }
}

/// TERM-RESTORE-1101: 画面の行（`lines`・scrollback を含む）を、VT に食わせると同じ字と色の行になる byte にする（行ごとに run の列へ CHA・SGR・字・`\x1b[0m\r\n`）。
/// 末尾の空の行は落とす・線の行は空の行になる（前の線は残さない = `seed` が 1 本だけ置く）。cursor の位置・mode・alt screen の切り替えは入れない。
/// 例: `let bytes = ansi(&vt.screen(RESTORE_LINES)); let mut back = Vt::new(size, n); back.seed(bytes.as_bytes());`
/// Wave: TERM-RESTORE-1101
pub fn ansi(screen: &TermScreen) -> String {
    let end = screen.lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    let mut out = String::new();
    for line in &screen.lines[..end] {
        for r in line {
            out.push_str(&format!("\x1b[{}G{}{}", u32::from(r.col) + 1, sgr(&r.style), r.text));
        }
        out.push_str("\x1b[0m\r\n");
    }
    out
}

/// TERM-XTERM-1101: 動いている session を xterm.js の頁へ初めて出す時（頁を開いた・頁を読み直した）の 1 回分の byte。
/// `screen`（`TerminalManager::screen(key, HISTORY_SHOWN)`）の全行を字・色のまま書き（最後の行の後に改行を書かない = 頁の今の画面が `screen` の今の画面と同じ行に揃う）、
/// cursor の位置（隠れていれば `ESC [?25l`）と、打鍵の形を決める mode（`app_cursor` = `ESC [?1h`・`bracketed_paste` = `ESC [?2004h`）を足す。
/// alt screen（vim・tmux・claude の全画面）の間は `ESC [?1049h` の後に今の画面の rows 行だけ。線の行（`rule`）は薄い `─` の列。
/// 例（`Generated::term_sync` の `TermMsg::Ready`）: `view.write(vt::seed(&mgr.screen(&key, HISTORY_SHOWN)?).as_bytes())` の後に `read_since(key, 今の end)` を流す。
/// 保証しない: mouse の報告・scroll の範囲・窓の題など `TermScreen` に無い mode（次の出力で app が出し直すまで既定）。
/// Wave: TERM-XTERM-1101
pub fn seed(screen: &TermScreen) -> String {
    let mut out = String::new();
    let lines = if screen.alt_screen {
        out.push_str("\x1b[?1049h\x1b[H");
        &screen.lines[screen.history.min(screen.lines.len())..]
    } else {
        &screen.lines[..]
    };
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str("\x1b[0m\r\n");
        }
        if !screen.alt_screen && screen.rule == Some(i) {
            out.push_str(&format!("\x1b[0;2m{}", "─".repeat(usize::from(screen.cols))));
            continue;
        }
        for r in line {
            out.push_str(&format!("\x1b[{}G{}{}", u32::from(r.col) + 1, sgr(&r.style), r.text));
        }
    }
    out.push_str("\x1b[0m");
    match screen.cursor {
        Some(c) => out.push_str(&format!("\x1b[{};{}H", u32::from(c.row) + 1, u32::from(c.col) + 1)),
        None => out.push_str("\x1b[?25l"),
    }
    if screen.app_cursor {
        out.push_str("\x1b[?1h");
    }
    if screen.bracketed_paste {
        out.push_str("\x1b[?2004h");
    }
    out
}

/// TERM-RESTORE-1101: run の見た目の SGR（0 から書き直す = 前の run の見た目を持ち越さない）。16 色は 30〜37 / 90〜97（背景は 40〜47 / 100〜107）・他は 38;5 / 38;2。
fn sgr(s: &TermStyle) -> String {
    let mut p = vec!["0".to_owned()];
    for (on, code) in [(s.bold, "1"), (s.dim, "2"), (s.italic, "3"), (s.underline, "4"), (s.inverse, "7"), (s.strike, "9")] {
        if on {
            p.push(code.to_owned());
        }
    }
    let color = |c: TermColor, base: u8| match c {
        TermColor::Default => None,
        TermColor::Indexed(n) if n < 8 => Some((base + n).to_string()),
        TermColor::Indexed(n) if n < 16 => Some((base + 60 + n - 8).to_string()),
        TermColor::Indexed(n) => Some(format!("{};5;{n}", base + 8)),
        TermColor::Rgb(r, g, b) => Some(format!("{};2;{r};{g};{b}", base + 8)),
    };
    p.extend(color(s.fg, 30));
    p.extend(color(s.bg, 40));
    format!("\x1b[{}m", p.join(";"))
}

/// TERM-RESTORE-1101: `seed` が置いた線の行か（列 0 の cell の幅 0 の字がちょうど `RULE_MARK`）
fn is_rule(row: &Row<Cell>) -> bool {
    row[Column(0)].zerowidth() == Some(&RULE_MARK[..])
}

/// 1 行の cell を、同じ見た目の続きの run にする。全角の続きの cell（`WIDE_CHAR_SPACER`）は入れず、行末の既定の見た目の空白は落とす。
/// tab の cell は空白 1 つ（alacritty は tab の始まりの cell に `\t` を残す・後ろの cell は空白。`\t` のままだと renderer の列がずれる）。
// ponytail: SGR 8（`HIDDEN`）と下線の種類（二重・波線）は持たない（文字のまま・ただの下線）。要る app が出たら TermStyle に足す
fn runs(row: &Row<Cell>, cols: usize) -> Vec<TermRun> {
    // TERM-RESTORE-1101: 線の行は字を持たない（印も空白も run にしない = 写しても選んでも空の行）
    if is_rule(row) {
        return Vec::new();
    }
    let blank = |c: &Cell| {
        matches!(c.c, ' ' | '\t') && c.zerowidth().is_none() && style(c) == TermStyle::default()
    };
    let end = (0..cols)
        .rev()
        .find(|&x| !blank(&row[Column(x)]))
        .map_or(0, |x| x + 1);
    let mut out: Vec<TermRun> = Vec::new();
    for x in 0..end {
        let cell = &row[Column(x)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let st = style(cell);
        // 行末で折り返した全角の前の空き（LEADING_WIDE_CHAR_SPACER）と tab の cell は空白 1 つ
        let ch = if cell.c == '\t' || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            ' '
        } else {
            cell.c
        };
        match out.last_mut() {
            Some(r) if r.style == st => r.text.push(ch),
            _ => out.push(TermRun {
                col: x as u16,
                text: ch.to_string(),
                style: st,
            }),
        }
        if let (Some(zw), Some(r)) = (cell.zerowidth(), out.last_mut()) {
            r.text.extend(zw); // 結合文字（濁点・異体字の選択子）は前の文字の後ろ
        }
    }
    out
}

fn style(c: &Cell) -> TermStyle {
    let f = c.flags;
    TermStyle {
        fg: color(c.fg),
        bg: color(c.bg),
        bold: f.contains(Flags::BOLD),
        dim: f.contains(Flags::DIM),
        italic: f.contains(Flags::ITALIC),
        underline: f.intersects(Flags::ALL_UNDERLINES),
        inverse: f.contains(Flags::INVERSE),
        strike: f.contains(Flags::STRIKEOUT),
    }
}

/// 名前つきの 16 色は `Indexed(0..16)`、既定の前景・背景（と Term の中だけの名前の色）は `Default`。
fn color(c: Color) -> TermColor {
    match c {
        Color::Named(n) if (n as usize) < 16 => TermColor::Indexed(n as u8),
        Color::Named(_) => TermColor::Default,
        Color::Indexed(i) => TermColor::Indexed(i),
        Color::Spec(rgb) => TermColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::terminal::{TermColor, TermCursor};

    /// W10 holdout（interface の commit b1cc7fe では本体が todo!() で赤）: `git diff` の色・全角の cursor・切れ目に依らない画面・
    /// scrollback・mode と alt screen・問い合わせへの返事・0 の大きさの拒否。PTY 無しで VT だけを見る（tmux・vim の通しは manager の試験）。
    #[test]
    fn w10_vt_screen_has_colors_cursor_modes_and_answers_queries() {
        let size = TermSize { rows: 4, cols: 20 };
        let diff = "\x1b[31m-old\x1b[m\r\n\x1b[32m+new\x1b[m\r\n全角";
        let mut vt = Vt::new(size, 100);
        assert!(
            vt.feed(diff.as_bytes()).is_empty(),
            "色と文字だけの byte に返事は無い"
        );
        let s = vt.screen(0);
        assert_eq!((s.rows, s.cols, s.history), (4, 20, 0));
        assert_eq!(s.visible_rows(), vec!["-old", "+new", "全角", ""]);
        assert_eq!(
            (s.lines[0][0].style.fg, s.lines[1][0].style.fg),
            (TermColor::Indexed(1), TermColor::Indexed(2))
        );
        assert_eq!(
            s.cursor,
            Some(TermCursor { row: 2, col: 4 }),
            "全角 2 文字 = 4 列"
        );
        let mut one = Vt::new(size, 100);
        for b in diff.as_bytes() {
            one.feed(std::slice::from_ref(b));
        }
        assert_eq!(
            one.screen(0),
            s,
            "1 byte ずつ食わせても同じ画面（切れた escape と UTF-8 を持ち越す）"
        );

        let mut h = Vt::new(TermSize { rows: 2, cols: 10 }, 100);
        h.feed(b"1\r\n2\r\n3\r\n4");
        let s = h.screen(10);
        assert_eq!(
            (s.history, s.visible_rows()),
            (2, vec!["3".to_owned(), "4".to_owned()])
        );
        assert_eq!(s.text(), "1\n2\n3\n4");
        assert_eq!(h.screen(1).history, 1, "history は上限まで");

        vt.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1049h\x1b[?25l");
        let a = vt.screen(100);
        assert!(
            a.alt_screen && a.app_cursor && a.bracketed_paste && a.cursor.is_none(),
            "{a:?}"
        );
        assert_eq!(
            (a.history, a.visible_rows()),
            (0, vec![String::new(); 4]),
            "alt screen は scrollback を持たず空で始まる"
        );
        assert!(vt.feed(b"\x1b[c").starts_with(b"\x1b[?"), "DA1 に返事");
        vt.feed(b"\x1b[2;3H");
        assert_eq!(
            vt.feed(b"\x1b[6n"),
            b"\x1b[2;3R".to_vec(),
            "DSR の cursor 位置（1 始まり）"
        );
        vt.feed(b"\x1b[?1049l");
        assert!(!vt.screen(0).alt_screen);

        assert!(matches!(
            vt.resize(TermSize { rows: 0, cols: 80 }),
            Err(TerminalError::InvalidSize(_))
        ));
        vt.resize(TermSize { rows: 10, cols: 40 }).unwrap();
        let r = vt.screen(0);
        assert_eq!((r.rows, r.cols, r.lines.len()), (10, 40, 10));
    }

    /// 同期更新（Claude Code の Ink が使う `ESC [?2026h` 〜 `l`）: 終わるまで画面は前のまま、終われば一度に出る。
    /// 終わりが来ない時は 150ms の後の `feed(&[])`（manager の `screen` が毎 frame 呼ぶ）で出る。
    #[test]
    fn w10_sync_update_draws_at_the_end_or_after_the_timeout() {
        let mut vt = Vt::new(TermSize { rows: 2, cols: 10 }, 0);
        vt.feed(b"\x1b[?2026hframe");
        assert_eq!(
            vt.screen(0).visible_rows(),
            vec!["", ""],
            "同期更新の途中は描かない"
        );
        vt.feed(b"\x1b[?2026l");
        assert_eq!(vt.screen(0).visible_rows(), vec!["frame", ""]);
        vt.feed(b"\x1b[?2026h\r\nlost");
        vt.feed(&[]);
        assert_eq!(vt.screen(0).visible_rows()[1], "", "期限の前は描かない");
        std::thread::sleep(std::time::Duration::from_millis(200));
        vt.feed(&[]);
        assert_eq!(
            vt.screen(0).visible_rows(),
            vec!["frame", "lost"],
            "ESU の来ない app でも 150ms の後に出る"
        );
    }

    /// TERM-RESTORE-1101: 保存した byte（`ansi`）を新しい VT に `seed` すると同じ字と色の行が並び、その下に字の無い線の行が 1 つ・その下に新しい shell の出力。
    /// 線の印（幅 0 の字）は run の字に出ない・幅を変えても線の行は残る。
    /// 変異: `seed` の線の行を書かない → rule が None で赤・`runs` で印を落とさない → 字に U+200B が入って赤・`ansi` の SGR を書かない → 色が落ちて赤。
    #[test]
    fn term_restore_seed_draws_the_saved_lines_and_one_rule_line() {
        let size = TermSize { rows: 3, cols: 20 };
        let mut before = Vt::new(size, 100);
        before.feed(b"\x1b[31mred\x1b[m plain\r\n\x1b[1;44m bold \x1b[m\r\n% ");
        let saved = ansi(&before.screen(100));
        let mut after = Vt::new(size, 100);
        after.seed(saved.as_bytes());
        after.feed(b"$ ");
        let s = after.screen(100);
        let texts: Vec<String> = s.lines.iter().map(|l| TermScreen::line_text(l)).collect();
        assert_eq!(texts, vec!["red plain", " bold", "%", "", "$"], "前の画面 → 線の行 → 新しい prompt");
        assert_eq!(s.rule, Some(3), "線の行の位置");
        assert_eq!(s.lines[0][0].style.fg, TermColor::Indexed(1), "色が戻らない");
        assert!(s.lines[1][0].style.bold && s.lines[1][0].style.bg == TermColor::Indexed(4), "{:?}", s.lines[1]);
        assert!(s.lines.iter().flatten().all(|r| !r.text.contains(['\u{200B}', '\u{2060}'])), "線の印が字に出た: {:?}", s.lines);
        after.resize(TermSize { rows: 3, cols: 10 }).unwrap();
        assert!(after.screen(100).rule.is_some(), "幅を変えたら線が消えた");
        assert_eq!(ansi(&after.screen(100)).matches("\r\n").count(), 5, "線の行は空の行として保存（2 本目の線を作らない）");
    }

    /// TERM-XTERM-1101: `seed` を新しい VT（= 頁の xterm と同じ立場）に食わせると、同じ画面・cursor・打鍵の mode になる（alt screen も）
    #[test]
    fn term_xterm_1101_seed_puts_the_same_screen_cursor_and_modes_into_a_fresh_terminal() {
        let size = TermSize { rows: 3, cols: 20 };
        let mut before = Vt::new(size, 100);
        before.feed(b"one\r\n\x1b[32mtwo\x1b[m\r\nthree\r\nfour\x1b[?2004h\x1b[?1h\x1b[2;3H");
        let s = before.screen(100);
        let mut after = Vt::new(size, 100);
        after.feed(seed(&s).as_bytes());
        let t = after.screen(100);
        assert_eq!(t.visible_rows(), s.visible_rows(), "今の画面の行がずれた");
        assert_eq!(t.cursor, Some(TermCursor { row: 1, col: 2 }));
        assert!(t.bracketed_paste && t.app_cursor, "打鍵の mode が渡らない");
        assert_eq!(t.lines[t.history][0].style.fg, TermColor::Indexed(2), "色が渡らない");
        let mut vim = Vt::new(size, 100);
        vim.feed(b"shell\r\n\x1b[?1049h\x1b[H~ vim\x1b[?25l");
        let v = vim.screen(100);
        let mut a = Vt::new(size, 100);
        a.feed(seed(&v).as_bytes());
        let w = a.screen(100);
        assert!(w.alt_screen && w.cursor.is_none(), "alt screen と隠れた cursor");
        assert_eq!(w.visible_rows(), v.visible_rows());
    }

    /// 行末の既定の空白は run に入れず、色のある空白は入れる。結合文字は前の文字に付く。行末で折り返した全角は次の行へ。
    #[test]
    fn w10_runs_keep_colored_blanks_and_combining_marks() {
        let mut vt = Vt::new(TermSize { rows: 3, cols: 5 }, 0);
        vt.feed("ab   \r\n\x1b[41m  \x1b[m\r\nか\u{3099}".as_bytes());
        let s = vt.screen(0);
        assert_eq!(
            s.lines[0],
            vec![TermRun {
                col: 0,
                text: "ab".into(),
                style: TermStyle::default()
            }]
        );
        assert_eq!(s.lines[1].len(), 1);
        assert_eq!(
            (s.lines[1][0].text.as_str(), s.lines[1][0].style.bg),
            ("  ", TermColor::Indexed(1)),
            "色のある空白は残す"
        );
        assert_eq!(
            s.lines[2],
            vec![TermRun {
                col: 0,
                text: "か\u{3099}".into(),
                style: TermStyle::default()
            }],
            "結合文字は前の文字の run に"
        );
        let mut w = Vt::new(TermSize { rows: 2, cols: 3 }, 0);
        w.feed("ab全".as_bytes());
        assert_eq!(
            w.screen(0).visible_rows(),
            vec!["ab", "全"],
            "3 列目に全角は入らないので次の行へ"
        );
    }

    /// W10 反証: tab の後の文字は tab stop（8 列ごと）の列に在る。alacritty は tab の始まりの cell に `\t` を残すので、写しでは空白 1 つにする
    /// （`\t` が run に入ると renderer は幅 0 と数え、epaint は 4 cell で描いて、後ろの文字が cursor と背景からずれる）。行末の tab は空白として落ちる。
    #[test]
    fn w10_tab_is_blank_cells_up_to_the_tab_stop() {
        let mut vt = Vt::new(TermSize { rows: 4, cols: 20 }, 0);
        vt.feed(b"a\tb\r\n\tx\r\n+\tfoo\r\nz\t");
        let s = vt.screen(0);
        assert_eq!(
            s.visible_rows(),
            vec!["a       b", "        x", "+       foo", "z"]
        );
        let plain = |text: &str| TermRun {
            col: 0,
            text: text.into(),
            style: TermStyle::default(),
        };
        assert_eq!(
            s.lines[0],
            vec![plain("a       b")],
            "b は 8 列目（1 cell = 1 文字）"
        );
        assert_eq!(
            s.lines[3],
            vec![plain("z")],
            "行末の tab は既定の空白として落とす"
        );
        assert!(
            s.lines.iter().flatten().all(|r| !r.text.contains('\t')),
            "{:?}",
            s.lines
        );
    }
}
