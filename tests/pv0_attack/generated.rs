// PV0 の反証（review・yc/review-pv）: `src/generated.rs` の `mod tests` の中の `mod pv0_attack` が include! で取り込む（PV の試験と file を分ける）。
// 撃つ所 1 = 承認の前に入る道（review を飛ばす・読んでいる間の承認・二度押し・Cancel の後の承認と再試行）。偽の brew だけ・この Mac に何も入れない。

/// review が開いていない時の Approve / Check（Start の前・読んでいる間・Cancel の後）は何も入れない・review の間の Check は brew を叩かない・
/// Cancel の後の再試行は読み直す（info が 2 回目・入れない）・Approve の二度押しで入れるのは 1 回だけ。
/// 負の対照: `install_act` の Cancel の腕が review を消さない → Cancel の後の Approve が入れて赤。
#[test]
fn pv0x_nothing_installs_without_an_open_review_and_only_once() {
    use crate::project::acquire::pv0_tests::calls;
    let mut g = fresh();
    let (project, d) = pv0_project(&mut g, "x-approve", "ok");
    let installs = |d: &std::path::Path| calls(d).iter().filter(|c| c.starts_with("install")).count();
    let infos = |d: &std::path::Path| calls(d).iter().filter(|c| c.starts_with("info")).count();
    // review を飛ばす
    assert!(g.page_intent(&project, pv0_start("game", InstallAct::Approve)).is_err(), "Start の前の Approve が通った");
    assert!(g.page_intent(&project, pv0_start("game", InstallAct::Check)).is_err(), "Start の前の Check が通った");
    assert!(g.installs.is_empty() && calls(&d).is_empty(), "review の前に brew を叩いた: {:?}", calls(&d));
    // 読んでいる間（poll の前 = Resolving）の Approve は通らない
    g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
    assert!(g.page_intent(&project, pv0_start("game", InstallAct::Approve)).is_err(), "読んでいる間の Approve が通った");
    pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Review(_))));
    // review の間の Check は brew を叩かない
    assert!(g.page_intent(&project, pv0_start("game", InstallAct::Check)).is_err(), "review の間の Check が通った");
    // Cancel の後の Approve は閉じた review を承認にしない
    g.page_intent(&project, pv0_start("game", InstallAct::Cancel)).unwrap();
    assert!(g.page_intent(&project, pv0_start("game", InstallAct::Approve)).is_err(), "Cancel の後の Approve が通った");
    assert_eq!(installs(&d), 0, "Cancel の後に入れた");
    // Cancel の後の再試行は読み直す
    g.page_intent(&project, pv0_start("game", InstallAct::Start)).unwrap();
    pv0_wait(&mut g, &project, "game", |i| matches!(i, Some(Install::Review(_))));
    assert_eq!((infos(&d), installs(&d)), (2, 0), "{:?}", calls(&d));
    // 二度押し
    g.page_intent(&project, pv0_start("game", InstallAct::Approve)).unwrap();
    assert!(g.page_intent(&project, pv0_start("game", InstallAct::Approve)).is_err(), "入れている間の 2 回目の Approve が通った");
    pv0_wait(&mut g, &project, "game", |i| i.is_none());
    assert_eq!(installs(&d), 1, "二度押しで 2 回入れた");
}

/// Godot（表の app）の席に model が偽の `install_url` と偽の bundle id を書いた Project（偽の brew の `g`・`fake_brew` の ok）。
/// app の席の `url` は composer が plan ごと拒む（`Page 2 … wrong shape`）ので、model の URL が 1 問に届く口は `install_url` だけ。
fn pv0x_evil_project(g: &mut Generated, name: &str) -> String {
    use crate::project::acquire::pv0_tests::{fake_brew, listing, temp, URL};
    use composer::project::KnownApp;
    let d = temp(name);
    g.homebrew = crate::project::acquire::Homebrew::at(fake_brew(&d, "ok", &listing("4.7.2", URL)), d.join("apps"));
    let apps = [KnownApp { name: "Notes Maker".into(), bundle_id: "org.example.notes".into() }, KnownApp { name: "Brushwork".into(), bundle_id: "org.example.brushwork".into() }];
    let plan = serde_json::json!({ "shape": "project", "name": "Game", "pages_requested": true, "seats": [
        { "title": "Notes", "kind": "app", "app_name": "Notes Maker", "bundle_id": null, "file": null, "url": null, "install_url": null, "instead": [], "page": 0, "slot": "main", "why": "", "feeds": null },
        { "title": "Game", "kind": "app", "app_name": "Godot", "bundle_id": "com.evil.godot", "file": null, "url": null, "install_url": "https://evil.example/godot.dmg", "instead": [{ "name": "Brushwork", "bundle_id": "org.example.brushwork" }], "page": 1, "slot": "main", "why": "", "feeds": null }
    ]});
    let plan = composer::project::plan_project(&format!("Make a 2D game in Godot pv0x-{name}"), &PlanOf(plan), &apps, &[], &new_ctx(), None).unwrap().expect("project の形");
    let View::Project { project, .. } = g.save_plan(plan, None).unwrap() else { panic!() };
    project
}

/// 1 問の外（面・Page の node）に model の URL が入っていない（1 問の席の写しは model の言った事の記録なので見ない）。
fn pv0x_assert_no_model_url(g: &Generated, project: &str, when: &str) {
    let (_, p, _) = g.project(project).unwrap();
    for s in &p.surface_specs {
        if let project::SurfaceTarget::Url { url } = &s.target {
            assert!(!url.contains("evil.example"), "{when}: model の URL を面にした: {url}");
        }
    }
    for pg in &p.pages {
        for n in pg.root.nodes().into_iter().filter(|n| composer::project::SeatV1::asked_in(n).is_none()) {
            let seen = format!("{:?} {}", n.label, serde_json::Value::Object(n.props.clone()));
            assert!(!seen.contains("evil.example"), "{when}: model の URL が Page の node（{}）に入った: {seen}", n.key);
        }
    }
}

/// operator（PV1 の前の頼み）: 表の app の 1 問で、model の出した URL は link にも WebView の面にも Page の node にもならない（URL は表からだけ）。
/// 答えの並びに依らない形: 出ている答えを 1 つずつ別の Project で押し（`Install & continue` の導入の手は上の試験・link は押さない）、押した後の Project を見る。
/// PV1 の `Use Godot in the browser (…)`（`InstallAct::UseWeb`）も押す。負の対照: UseWeb の腕で model の install_url を先に置く → 面の URL で赤。
#[test]
fn pv0x_a_model_url_never_becomes_a_link_or_a_web_surface_for_a_catalog_app() {
    let mut g = fresh();
    let probe = pv0x_evil_project(&mut g, "x-web");
    pv0x_assert_no_model_url(&g, &probe, "作った直後");
    let (_, answers) = pv0_answers(&g, &probe, "game");
    for (label, url, _) in &answers {
        assert!(!label.contains("evil.example") && !url.as_deref().unwrap_or_default().contains("evil.example"), "答えに model の URL: {label} {url:?}");
    }
    let mut pressed = 0;
    for (i, (label, _, intent)) in answers.iter().enumerate() {
        let Some(intent) = intent.clone().filter(|i| !matches!(i, PageIntent::Install { act: InstallAct::Start, .. })) else { continue };
        let project = pv0x_evil_project(&mut g, &format!("x-web-{i}"));
        g.page_intent(&project, intent).unwrap_or_else(|e| panic!("{label}: {e}"));
        pv0x_assert_no_model_url(&g, &project, label);
        pressed += 1;
    }
    assert!(pressed >= 2, "押して確かめた答えが {pressed} 個（{:?}）", answers.iter().map(|a| &a.0).collect::<Vec<_>>());
}
