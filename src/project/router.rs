//! PageRouter — Project の中の Page の移り・戻る・足す・名前替え・消すを決める純関数（YCM-012・013・SPEC v15 §4）。
//! std と `domain::project` だけに依る（egui・SQLite・cua を取り込まない: 画面なしの試験と tests/yc_demo_e2e.rs から直に回す）。
//! Page の形を変える物（create / rename / delete）は `ProjectPatchOp` を返すだけで、当てるのは `Generated::apply_project`（全部か無しか）。
//! Page ID は変えない。誤りは既存の `ProjectPatchError`（PageNotFound / DuplicatePage / LastPage）を使い、新しい誤りの型は作らない。
use crate::domain::project::{
    PageDefinition, ProjectDefinitionV1, ProjectPatchError, ProjectPatchOp,
};
use crate::domain::ui::UiNode;

/// 1 Project 分の居場所と戻りの履歴（保存しない。再起動したら `open` で先頭か指定の Page から始める）。
///
/// 使い方: `let mut r = PageRouter::open(&p, None)?; if let Some(sw) = r.navigate(&p, "page.b")? { save(sw.from); activate(sw.to) }`
/// 保証する: `current` は常に `p.pages` に在る id（`reconcile` を呼んだ後）。`history` に同じ id が続けて並ばない・`current` で終わらない。
/// 保証しない: Project の中身が変わった事に自分では気付かない（apply の後に呼ぶ側が `reconcile` を呼ぶ）。
/// Wave: W2
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRouter {
    current: String,
    /// 戻る先。古い順（末尾が次の `back` の行き先）。
    history: Vec<String>,
}

/// 1 回の移り（SPEC §4 の順: 呼ぶ側は `from` の `save_view_state` → `to` の `activate`）。
///
/// 使い方: `if let Some(PageSwitch { from, to }) = r.back(&p) { g.save_view_state(pid, &from)?; g.activate(pid, &to)?; }`
/// 保証する: `from != to`（同じ Page への移りは `None` で返り、PageSwitch は作らない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSwitch {
    pub from: String,
    pub to: String,
}

impl PageRouter {
    /// Project を開く。`page` が None か、もう無い id なら先頭の Page から始める（古い deep link で止めない）。
    ///
    /// 使い方: `PageRouter::open(&project, Some(&view_page))`
    /// 保証する: Page が 1 枚以上なら Ok。0 枚の Project は `ProjectPatchError::Invalid` で返す（形の検査を通った Project では起きない）。
    pub fn open(
        project: &ProjectDefinitionV1,
        page: Option<&str>,
    ) -> Result<Self, ProjectPatchError> {
        let first = project
            .pages
            .first()
            .ok_or_else(|| ProjectPatchError::Invalid(vec!["No pages".into()]))?;
        let current = page
            .filter(|id| project.page(id).is_some())
            .unwrap_or(&first.id);
        Ok(PageRouter {
            current: current.to_owned(),
            history: Vec::new(),
        })
    }

    /// 今いる Page の id。
    pub fn current(&self) -> &str {
        &self.current
    }

    /// 戻りの履歴（古い順）。試験と、戻るボタンを押せるかの判定に使う。
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// `to` へ移る。今の Page を履歴に積む。
    ///
    /// 使い方: `match r.navigate(&p, &id) { Ok(Some(sw)) => …, Ok(None) => {/* 同じ Page */}, Err(e) => line = e.to_string() }`
    /// 保証する: `to` が無ければ `PageNotFound` で、居場所も履歴も変えない。保証しない: 移った先の画面の状態（呼ぶ側の `activate`）。
    pub fn navigate(
        &mut self,
        project: &ProjectDefinitionV1,
        to: &str,
    ) -> Result<Option<PageSwitch>, ProjectPatchError> {
        if project.page(to).is_none() {
            return Err(ProjectPatchError::PageNotFound(to.into()));
        }
        if to == self.current {
            return Ok(None);
        }
        let from = std::mem::replace(&mut self.current, to.to_owned());
        self.history.push(from.clone());
        Ok(Some(PageSwitch {
            from,
            to: to.to_owned(),
        }))
    }

    /// 1 つ前の Page へ戻る。履歴が空なら None（誤りではない: 戻るボタンを押せなくするだけ）。
    ///
    /// 保証する: 消えた Page へは戻らない（`reconcile` 済みの履歴だけを辿る）。
    pub fn back(&mut self, project: &ProjectDefinitionV1) -> Option<PageSwitch> {
        while let Some(to) = self.history.pop() {
            if to != self.current && project.page(&to).is_some() {
                let from = std::mem::replace(&mut self.current, to.clone());
                return Some(PageSwitch { from, to });
            }
        }
        None
    }

    /// `page_id` を消した時に移る先（消す前の Project で呼ぶ）。次の Page、無ければ前の Page。
    /// 消す op 自体は返さない: `ProjectPatchOp::RemovePage` を `apply_project` で当て、最後の 1 枚は apply が `LastPage` で拒否する。
    ///
    /// 使い方: `let land = r.landing_after_delete(&p, &id)?; g.apply_project(app, &hash, &[RemovePage{..}], DirectEdit)?; r.reconcile(&next);`
    /// 保証する: 最後の 1 枚なら `LastPage`（理由の 1 行は `Display`）・無い id なら `PageNotFound`。返す id は `page_id` と違う。
    pub fn landing_after_delete(
        &self,
        project: &ProjectDefinitionV1,
        page_id: &str,
    ) -> Result<String, ProjectPatchError> {
        let i = project
            .pages
            .iter()
            .position(|p| p.id == page_id)
            .ok_or_else(|| ProjectPatchError::PageNotFound(page_id.into()))?;
        if project.pages.len() == 1 {
            return Err(ProjectPatchError::LastPage(page_id.into()));
        }
        let land = project
            .pages
            .get(i + 1)
            .unwrap_or_else(|| &project.pages[i - 1]);
        Ok(land.id.clone())
    }

    /// Project が変わった後（apply・開き直し）に居場所と履歴を合わせる。消えた id を履歴から抜き、続けて並んだ同じ id を 1 つにする。
    /// 今の Page が消えていたら `landing`（`landing_after_delete` の結果）へ、それも無ければ先頭へ移る。
    ///
    /// 保証する: 呼んだ後 `current` と `history` の全 id が `project.pages` に在る。移った時は PageSwitch を返す（呼ぶ側が `activate`）。
    pub fn reconcile(
        &mut self,
        project: &ProjectDefinitionV1,
        landing: Option<&str>,
    ) -> Option<PageSwitch> {
        let first = project.pages.first()?;
        let mut moved = None;
        if project.page(&self.current).is_none() {
            let to = landing
                .filter(|id| project.page(id).is_some())
                .unwrap_or(&first.id)
                .to_owned();
            let from = std::mem::replace(&mut self.current, to.clone());
            moved = Some(PageSwitch { from, to });
        }
        self.history.retain(|id| project.page(id).is_some());
        self.history.dedup();
        while self.history.last() == Some(&self.current) {
            self.history.pop();
        }
        moved
    }
}

/// 「＋Page」の op を作る（純関数・当てない）。route は title から作り、既存の route と重ならないよう末尾に番号を足す。
/// `page_id` は呼ぶ側が渡す（`format!("page.{}", storage::new_id())`。ここでは乱数を持たない）。root は空の `column`（`stack` は部品の表に無い）。
/// app_ref は Project の先頭の app（入力途中の状態の保存先 `main.{app}` と、後で置く部品の action / binding の行き先）。
///
/// 使い方: `let op = new_page(&p, format!("page.{}", new_id()), "Tests")?; g.apply_project(app, &hash, &[op], RevisionOrigin::DirectEdit)?`
/// 保証する: 返す op は `AddPage{index: None}`（末尾）で、id が既に在れば `DuplicatePage`。保証しない: title の重なり（同じ title は許す）。
pub fn new_page(
    project: &ProjectDefinitionV1,
    page_id: String,
    title: &str,
) -> Result<ProjectPatchOp, ProjectPatchError> {
    if project.page(&page_id).is_some() {
        return Err(ProjectPatchError::DuplicatePage(page_id));
    }
    let title = non_blank(title)?;
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    let base = format!("/{}", if slug.is_empty() { "page" } else { slug });
    let taken = |r: &str| project.pages.iter().any(|p| p.route == r);
    let route = (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|r| !taken(r))
        .expect("route は有限個しか埋まらない");
    Ok(ProjectPatchOp::AddPage {
        page: PageDefinition {
            root: UiNode {
                key: format!("{page_id}-root"),
                component: "column".into(),
                ..UiNode::default()
            },
            id: page_id,
            route,
            title: title.to_owned(),
            app_ref: project.app_refs.first().cloned(),
            revision: 1,
        },
        index: None,
    })
}

/// 名前替えの op を作る（純関数・当てない）。route と id は変えない（deep link を壊さない）。
///
/// 保証する: 無い id は `PageNotFound`、空白だけの title は `Invalid` で返す。
/// 契約（FX3 から・署名は変えない）: 同じ Project の別の Page が同じ名前（trim・大文字小文字を見ない）を持てば `DuplicateTitle(打った名前の trim)`。
/// その Page 自身の今の名前と同じ（大文字小文字だけの違いは名前替え = op を返す）なら op は返すが、呼ぶ側（`Generated::page_intent` の `Rename` の腕）が
/// 当てる前に止めて何も書かない（`Move` の同じ位置と同じ規則）。作る所は帯の欄の Enter（`PageIntent::Rename`）だけ。自然言語の編集の `rename Docs to Tests` は
/// `composer::project` が先に `EditError::AlreadyApplied("a page named “Tests”")` で拒む（同じ文 `There's already a page named “Tests”.`・op を作らない）。
/// 例: `rename_page(&p, &b, "Tests")` → B 以外に `Tests` が在れば `Err(DuplicateTitle("Tests"))`。
/// Wave: FX3
pub fn rename_page(
    project: &ProjectDefinitionV1,
    page_id: &str,
    title: &str,
) -> Result<ProjectPatchOp, ProjectPatchError> {
    if project.page(page_id).is_none() {
        return Err(ProjectPatchError::PageNotFound(page_id.into()));
    }
    let title = non_blank(title)?;
    let same = |other: &str| other.trim().to_lowercase() == title.to_lowercase();
    if project.pages.iter().any(|pg| pg.id != page_id && same(&pg.title)) {
        return Err(ProjectPatchError::DuplicateTitle(title.to_owned()));
    }
    Ok(ProjectPatchOp::RenamePage {
        page_id: page_id.into(),
        title: title.to_owned(),
    })
}

fn non_blank(title: &str) -> Result<&str, ProjectPatchError> {
    match title.trim() {
        "" => Err(ProjectPatchError::Invalid(vec!["The page name is empty".into()])),
        t => Ok(t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::project::apply;

    fn project(ids: &[&str]) -> ProjectDefinitionV1 {
        let mut p = ProjectDefinitionV1 {
            id: "project.t".into(),
            name: "t".into(),
            goal: "t".into(),
            app_refs: Vec::new(),
            pages: Vec::new(),
            resources: Vec::new(),
            terminal_specs: Vec::new(),
            surface_specs: Vec::new(),
            revision: 1,
        };
        for id in ids {
            let ProjectPatchOp::AddPage { page, .. } = new_page(&p, (*id).into(), id).unwrap()
            else {
                unreachable!()
            };
            p.pages.push(page);
        }
        p
    }

    fn assert_sound(r: &PageRouter, p: &ProjectDefinitionV1, step: usize) {
        assert!(
            p.page(r.current()).is_some(),
            "{step}: current {} が無い",
            r.current()
        );
        for id in r.history() {
            assert!(p.page(id).is_some(), "{step}: 履歴に消えた {id}");
        }
        assert!(
            r.history().windows(2).all(|w| w[0] != w[1]),
            "{step}: 同じ id が続く {:?}",
            r.history()
        );
        assert_ne!(
            r.history().last().map(String::as_str),
            Some(r.current()),
            "{step}: 履歴が current で終わる"
        );
    }

    /// Done 1: 3 Page を 50 回ランダムに移り・消し（足し直し）・戻しても、id と履歴が壊れない。Page の id は作った時のまま。
    #[test]
    fn fifty_random_moves_keep_ids_and_history() {
        let mut p = project(&["page.a", "page.b", "page.c"]);
        let mut r = PageRouter::open(&p, None).unwrap();
        assert_eq!(r.current(), "page.a");
        let mut seed: u64 = 0x5eed;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n
        };
        let mut made = 3;
        let (mut navs, mut backs, mut deletes) = (0, 0, 0);
        for step in 0..50 {
            match next(4) {
                0 | 1 => {
                    let to = p.pages[next(p.pages.len())].id.clone();
                    let before = r.current().to_owned();
                    match r.navigate(&p, &to).unwrap() {
                        Some(sw) => assert_eq!(
                            (sw.from.as_str(), sw.to.as_str()),
                            (before.as_str(), to.as_str())
                        ),
                        None => assert_eq!(before, to),
                    }
                    navs += 1;
                }
                2 => {
                    let before = r.current().to_owned();
                    let expect = r.history().last().cloned();
                    match r.back(&p) {
                        Some(sw) => assert_eq!((sw.from, Some(sw.to)), (before, expect)),
                        None => assert!(expect.is_none()),
                    }
                    backs += 1;
                }
                _ => {
                    let gone = p.pages[next(p.pages.len())].id.clone();
                    let land = r.landing_after_delete(&p, &gone).unwrap();
                    assert_ne!(land, gone);
                    let ids_before: Vec<String> = p.pages.iter().map(|pg| pg.id.clone()).collect();
                    p = apply(
                        &p,
                        &[ProjectPatchOp::RemovePage {
                            page_id: gone.clone(),
                        }],
                        &[],
                    )
                    .unwrap();
                    let was_current = r.current() == gone;
                    let sw = r.reconcile(&p, Some(&land));
                    if was_current {
                        assert_eq!(
                            sw.map(|s| s.to),
                            Some(land.clone()),
                            "{step}: 消した Page から次の Page へ"
                        );
                    } else {
                        assert!(sw.is_none(), "{step}: 今の Page が残れば動かない");
                    }
                    made += 1;
                    let op = new_page(&p, format!("page.n{made}"), "Page").unwrap();
                    p = apply(&p, &[op], &[]).unwrap();
                    let kept: Vec<String> =
                        ids_before.into_iter().filter(|id| *id != gone).collect();
                    assert_eq!(
                        p.pages[..2]
                            .iter()
                            .map(|pg| pg.id.clone())
                            .collect::<Vec<_>>(),
                        kept,
                        "{step}: 残った Page の id と順は変わらない"
                    );
                    deletes += 1;
                }
            }
            assert_eq!(p.pages.len(), 3);
            assert_sound(&r, &p, step);
        }
        assert!(
            navs > 0 && backs > 0 && deletes > 0,
            "3 種とも回った: {navs}/{backs}/{deletes}"
        );
    }

    #[test]
    fn unknown_page_changes_nothing_and_same_page_is_none() {
        let p = project(&["page.a", "page.b"]);
        let mut r = PageRouter::open(&p, Some("page.gone")).unwrap();
        assert_eq!(r.current(), "page.a", "古い deep link は先頭へ");
        assert_eq!(
            r.navigate(&p, "page.x"),
            Err(ProjectPatchError::PageNotFound("page.x".into()))
        );
        assert_eq!((r.current(), r.history().len()), ("page.a", 0));
        assert_eq!(r.navigate(&p, "page.a"), Ok(None));
        assert_eq!(r.back(&p), None);
    }

    #[test]
    fn last_page_cannot_be_deleted_and_says_why() {
        let p = project(&["page.a", "page.b"]);
        let r = PageRouter::open(&p, None).unwrap();
        assert_eq!(
            r.landing_after_delete(&p, "page.b").unwrap(),
            "page.a",
            "次が無ければ前"
        );
        assert_eq!(
            r.landing_after_delete(&p, "page.a").unwrap(),
            "page.b",
            "次へ"
        );
        let one = project(&["page.a"]);
        let e = r.landing_after_delete(&one, "page.a").unwrap_err();
        assert_eq!(e, ProjectPatchError::LastPage("page.a".into()));
        assert!(e.to_string().contains("消せません"), "{e}");
        assert_eq!(
            apply(
                &one,
                &[ProjectPatchOp::RemovePage {
                    page_id: "page.a".into()
                }],
                &[]
            ),
            Err(e)
        );
    }

    #[test]
    fn new_page_routes_are_unique_and_rename_keeps_id_and_route() {
        let mut p = project(&["page.a"]);
        for n in 0..3 {
            let op = new_page(&p, format!("page.t{n}"), "Tests").unwrap();
            p = apply(&p, &[op], &[]).unwrap();
        }
        let routes: Vec<_> = p.pages.iter().map(|pg| pg.route.as_str()).collect();
        assert_eq!(routes, ["/page-a", "/tests", "/tests-2", "/tests-3"]);
        assert_eq!(
            new_page(&p, "page.a".into(), "x"),
            Err(ProjectPatchError::DuplicatePage("page.a".into()))
        );
        assert!(matches!(
            new_page(&p, "page.z".into(), "  "),
            Err(ProjectPatchError::Invalid(_))
        ));
        let op = rename_page(&p, "page.t1", " 結果 ").unwrap();
        let q = apply(&p, &[op], &[]).unwrap();
        let (a, b) = (p.page("page.t1").unwrap(), q.page("page.t1").unwrap());
        assert_eq!(
            (b.title.as_str(), &b.route, &b.id),
            ("結果", &a.route, &a.id)
        );
        assert_eq!(
            rename_page(&p, "page.x", "y"),
            Err(ProjectPatchError::PageNotFound("page.x".into()))
        );
    }
}
