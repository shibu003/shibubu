// WF-E の反証 9: crate の module（main.rs の `#[cfg(test)] mod app_wfe_attack`）。
// include だけだと src/*.rs が main.rs の mod に無く、門の lint が赤になる。
// 試験の殻 `Win` は `app::tests` の pub(crate)。本物の AI は使わない（model は無し・偽の Adapter の `attk.a.set` だけ）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::app::tests::Win;
use crate::domain::permission::AutomationMode;
use crate::generated::{EngineAnswer, EngineRequest, EngineSetup, PageEngine};
use crate::ui::engine_view::EngineView;
use crate::ui::shell::View;

/// 本物の `CoreEngine` に、Page の宛先（Project の Page の app と書類）と model を外して渡す（試験の Adapter は app を持たない・本物の AI を使わない）
struct Bare(crate::engine_page::CoreEngine);
impl PageEngine for Bare {
    fn run(&self, req: EngineRequest) -> EngineAnswer {
        self.0.run(EngineRequest { target: None, document: None, document_file: None, llm: None, ..req })
    }
}

/// Settings › Automation の行が書く所（`UiSettings::automation`・RQ1）に `mode` を置き、1 frame の後に同じ一言を頼んで、終わるまで frame を回す。
/// 答え = (この run で送った `attk.a.set` の数, Page の 1 行)
fn ask(w: &mut Win, project: &str, goal: &str, mode: AutomationMode, calls: &Mutex<Vec<String>>) -> (usize, String) {
    let sent = || calls.lock().unwrap().iter().filter(|c| *c == "attk.a.set").count();
    w.app.test_set_automation(mode);
    w.step(0.05, true, vec![]);
    let before = sent();
    assert!(w.app.test_generated().start_engine(project, goal, vec![], None), "{mode:?}: 依頼が始まらない");
    let t0 = Instant::now();
    loop {
        w.step(0.05, true, vec![]);
        match w.app.test_generated().engine_view(project) {
            Some(EngineView::Line { line, .. }) => return (sent() - before, line.clone()),
            Some(EngineView::Running { .. }) => {}
            other => panic!("{mode:?}: 1 行の答えでない: {other:?}"),
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "{mode:?}: run が終わらない");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 撃つ所 9（RQ-023・段 1b の Automation の写し）: Settings で Ask before write → Suggest に替えた直後の run は、本物の門で 1 手も送らない（Page は Suggest の 1 行）。
/// 前提 = 替える前の run は送る・戻した直後の run はまた送る（門が値に従っている・Suggest の後で Engine が壊れたのではない）。
/// 負の対照（束で取る）: app.rs の update の写し（`self.generated.automation=self.store.ui.automation`）を外す → 替えた直後の run が送って赤。
#[test]
fn wfe_x9_a_run_right_after_switching_to_suggest_sends_nothing_through_the_real_engine() {
    let goal = "Set a";
    let mut w = Win::open("wfe-x9");
    w.app.test_clear_llm_rx();
    w.app.test_generated().set_llm(Err(crate::llm::LlmError::NoCli { detail: "wfe-x9".into() }));
    let data = w.dir.join("engine");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let registry = crate::capability_engine::wfe_attack::nine(&data, goal, calls.clone());
    let core = crate::engine_page::CoreEngine {
        registry,
        data_dir: data.clone(),
        mcp: Default::default(),
        env: Default::default(),
        readback: Duration::ZERO,
        act_consents: w.dir.join("act-consents.json"),
    };
    w.app.test_generated().set_engine(EngineSetup { engine: Arc::new(Bare(core)), data_dir: data });
    let Some(View::Project { project, .. }) = w.app.test_generated().create_project(crate::ui::home::EXAMPLES[0].1, None, None).expect("Project を作れない") else {
        panic!("主デモの一言は Project")
    };
    w.step(0.05, true, vec![]);

    let (n, line) = ask(&mut w, &project, goal, AutomationMode::AskBeforeWrite, &calls);
    assert!(n == 1 && !line.contains("Suggest"), "Ask before write の run は送る（前提）: 送った手 {n}・{line}・{:?}", calls.lock().unwrap());
    let (n, line) = ask(&mut w, &project, goal, AutomationMode::Suggest, &calls);
    assert!(n == 0 && line.contains("Suggest"), "Settings で Suggest に替えた直後の run が古い門を通った: 送った手 {n}・Page の 1 行 = {line}");
    let (n, line) = ask(&mut w, &project, goal, AutomationMode::AskBeforeWrite, &calls);
    assert!(n == 1 && !line.contains("Suggest"), "Ask before write に戻した直後の run が送らない（門が値に従っていない）: 送った手 {n}・{line}");
}
