//! 旧 MVP の canvas（CompatCanvas・`model.rs` の Session / Panel）の 1 部品: 一言から Session を作る（この file の上）・panel の配置（`layout`）・
//! panel の UI tree（`ui_tree`）。使うのは app.rs の旧 canvas だけ（`main.rs` が `crate::layout` / `crate::ui_tree` の path を残す）。
use crate::model::*;

fn hit(text:&str, terms:&[&str])->bool{terms.iter().any(|word|text.contains(word))}

// Offline deterministic fallback: chooses known, bounded UI/action templates only.
// No arbitrary code is synthesized or executed from prompts.
pub fn generate(prompt:&str,repo:&str)->Session {
    let lower=prompt.to_lowercase();
    let github=hit(&lower,&["github","git hub","リポジトリ","issue","pull request","pr","コード","repo"]);
    let desktop=hit(&lower,&["desktop","ide","vscode","vs code","ブラウザ","画面","アプリ","mac","adobe"]);
    let web=hit(&lower,&["web","api","サービス","json"]);
    let feature=hit(&lower,&["feature","機能","ライブラリ","取り込","他人の","公開repo","subdir"]);
    let mut s=Session::starter();
    s.id=new_id();s.name=prompt.chars().take(23).collect();s.prompt=prompt.to_owned();s.repo=repo.to_owned();s.panels.clear();
    fn add_panel(s:&mut Session,kind:PanelKind,title:&str){let index=s.panels.len();s.panels.push(Panel::new(kind,title,index%2,index/2));}
    if github {add_panel(&mut s,PanelKind::GitHubIssues,"GitHub Issues");add_panel(&mut s,PanelKind::GitHubPulls,"GitHub Pull Requests");}
    if web {add_panel(&mut s,PanelKind::WebService,"Web Service");}
    if desktop {add_panel(&mut s,PanelKind::Desktop,"Desktop Actions / 画面の操作");}
    if feature {add_panel(&mut s,PanelKind::Feature,"Repo Feature / 部分導入");}
    if s.panels.is_empty() {add_panel(&mut s,PanelKind::Notes,"この仕事のメモ");}
    add_panel(&mut s,PanelKind::Workflow,"Workflow / 実行");
    let mut steps=vec![];
    if github {
        let issues=Action::new("Issueを取得",ActionKind::GitHubIssues{repo:repo.into()});steps.push(issues.id);s.actions.push(issues);
        let prs=Action::new("PRを取得",ActionKind::GitHubPulls{repo:repo.into()});steps.push(prs.id);s.actions.push(prs);
    }
    if web {let a=Action::new("Web JSONを読む",ActionKind::WebRead);steps.push(a.id);s.actions.push(a);}
    s.workflows.push(Workflow{id:new_id(),name:"情報を集める".into(),steps});
    if desktop {s.workflows.push(Workflow{id:new_id(),name:DESKTOP_WORKFLOW.into(),steps:vec![]});}
    s.history.push("統合UIとWorkflow草案を生成。外部への書き込みは未実行。".into());
    s
}

pub fn valid_workflow(session:&Session,workflow:&Workflow)->Result<Vec<Action>,String>{
    if workflow.steps.is_empty(){return Err("Workflowに実行ステップがありません".into())}
    if workflow.steps.len()>20 {return Err("Workflowは最大20ステップです".into())}
    workflow.steps.iter().map(|id|session.find_action(*id).cloned().ok_or_else(||"Workflowが参照する操作がありません".into())).collect()
}

pub fn disjoint(a:&[Action],b:&[Action])->bool{
    !a.iter().filter_map(|s|s.kind.exclusive_resource())
        .any(|key|b.iter().filter_map(|s|s.kind.exclusive_resource()).any(|other|other==key))
}

// ---- layout（旧 src/layout.rs）----
pub mod layout {
    //! Canvas geometry; logical coordinates are relative to the canvas, not the OS window.
    use crate::model::Panel;

    pub const GRID: f32 = 8.0;
    pub const MIN_W: f32 = 260.0;
    pub const MIN_H: f32 = 185.0;
    pub const HEADER_H: f32 = 42.0;

    pub fn snap(n: f32) -> f32 {
        if n.is_finite() { (n / GRID).round() * GRID } else { 0.0 }
    }

    pub fn move_panel(panel: &mut Panel, dx: f32, dy: f32, canvas_w: f32, canvas_h: f32) -> bool {
        if panel.locked || !dx.is_finite() || !dy.is_finite() { return false }
        let before = (panel.x, panel.y);
        panel.x = snap((panel.x + dx).max(0.0).min((canvas_w - panel.width).max(0.0)));
        panel.y = snap((panel.y + dy).max(0.0).min((canvas_h - HEADER_H).max(0.0)));
        before != (panel.x, panel.y)
    }

    pub fn resize_panel(panel: &mut Panel, dx: f32, dy: f32, canvas_w: f32, canvas_h: f32) -> bool {
        if panel.locked || !dx.is_finite() || !dy.is_finite() { return false }
        let before = (panel.width, panel.height);
        let max_w = (canvas_w - panel.x).max(MIN_W);
        let max_h = (canvas_h - panel.y).max(MIN_H);
        panel.width = snap((panel.width + dx).clamp(MIN_W, max_w.max(MIN_W)));
        panel.height = snap((panel.height + dy).clamp(MIN_H, max_h.max(MIN_H)));
        before != (panel.width, panel.height)
    }

    pub fn fit_panel(panel: &mut Panel, canvas_w: f32, canvas_h: f32) {
        if !panel.x.is_finite() { panel.x = 0.0; }
        if !panel.y.is_finite() { panel.y = 0.0; }
        if !panel.width.is_finite() { panel.width = MIN_W; }
        if !panel.height.is_finite() { panel.height = MIN_H; }
        panel.width = panel.width.max(MIN_W).min(canvas_w.max(MIN_W));
        panel.height = panel.height.max(MIN_H).min(canvas_h.max(MIN_H));
        panel.x = panel.x.max(0.0).min((canvas_w - panel.width).max(0.0));
        panel.y = panel.y.max(0.0).min((canvas_h - HEADER_H).max(0.0));
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::model::PanelKind;
        #[test] fn snap_uses_eight_pixel_grid() { assert_eq!(snap(18.0),16.0); assert_eq!(snap(21.0),24.0); }
        #[test] fn locked_panels_do_not_move() {
            let mut p=Panel::new(PanelKind::Notes,"test",0,0);
            p.locked=true; let x=p.x;
            assert!(!move_panel(&mut p,100.0,100.0,900.0,900.0)); assert_eq!(p.x,x);
        }
        #[test] fn layout_fits_small_viewports() {
            let mut p=Panel::new(PanelKind::Notes,"test",3,5);
            fit_panel(&mut p,370.0,300.0);
            assert!(p.x+p.width<=370.01); assert!(p.y+HEADER_H<=300.01);
        }
    }
} // mod layout

// ---- ui_tree（旧 src/ui_tree.rs）----
pub mod ui_tree {
    //! A semantic, testable UI tree for the native egui renderer.
    //! This is NOT a browser DOM. Browser DOM/AX is observed by the driver,
    //! while these nodes describe Kangae's generated controls and their bindings.
    use crate::model::{ActionKind, Panel, PanelKind, Session};
    use std::collections::HashSet;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Role {
        // Wave 2 の汎用 renderer が使う（UI_UX_DOM_SPEC §4）
        #[allow(dead_code)]
        Workspace,
        Region, Heading, Text, DataView, Input, Button, Workflow, Status }
    impl Role {
        pub fn label(self) -> &'static str {
            match self { Self::Workspace => "workspace", Self::Region => "region",
                Self::Heading => "heading", Self::Text => "text", Self::DataView => "data-view",
                Self::Input => "input", Self::Button => "button", Self::Workflow => "workflow",
                Self::Status => "status" }
        }
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Availability { Ready, NeedsConnection, NeedsReview, Informational }
    impl Availability {
        pub fn label(self) -> &'static str {
            match self { Self::Ready => "利用可能", Self::NeedsConnection => "未接続",
                Self::NeedsReview => "要確認", Self::Informational => "情報表示" }
        }
    }
    #[derive(Debug, Clone)]
    pub struct UiNode {
        // Wave 2 の汎用 renderer が使う（UI_UX_DOM_SPEC §4）
        #[allow(dead_code)]
        pub key: String,
        pub role: Role,
        pub name: String,
        pub source: String,
        pub availability: Availability,
        pub action_id: Option<u64>,
        pub children: Vec<UiNode>,
    }
    impl UiNode {
        fn leaf(key: String, role: Role, name: impl Into<String>, source: impl Into<String>,
                availability: Availability, action_id: Option<u64>) -> Self {
            Self { key, role, name: name.into(), source: source.into(), availability,
                action_id, children: vec![] }
        }
    }

    pub fn panel_tree(session: &Session, panel: &Panel) -> UiNode {
        let key = format!("session/{}/panel/{}", session.id, panel.id);
        let mut node = UiNode::leaf(key.clone(), Role::Region, panel.title.clone(),
            source_label(&panel.kind), Availability::Informational, None);
        node.children.push(UiNode::leaf(format!("{key}/title"), Role::Heading,
            panel.title.clone(), source_label(&panel.kind), Availability::Informational, None));
        match panel.kind {
            PanelKind::GitHubIssues | PanelKind::GitHubPulls => {
                let issues = panel.kind == PanelKind::GitHubIssues;
                let target = session.actions.iter().find(|a| if issues {
                    matches!(&a.kind, ActionKind::GitHubIssues { .. })
                } else { matches!(&a.kind, ActionKind::GitHubPulls { .. }) });
                let available = !session.repo.trim().is_empty() && target.is_some();
                node.children.push(UiNode::leaf(format!("{key}/list"), Role::DataView,
                    if issues { "Issue一覧" } else { "Pull Request一覧" }, "GitHub REST",
                    if available { Availability::Ready } else { Availability::NeedsConnection },
                    target.map(|a| a.id)));
                if issues {
                    node.children.push(UiNode::leaf(format!("{key}/issue-title"), Role::Input,
                        "Issueタイトル", "GitHub REST", Availability::NeedsReview, None));
                    node.children.push(UiNode::leaf(format!("{key}/create"), Role::Button,
                        "Issue作成", "GitHub REST", Availability::NeedsReview, None));
                }
            }
            PanelKind::WebService => {
                let action = session.actions.iter().find(|a| matches!(&a.kind, ActionKind::WebRead));
                node.children.push(UiNode::leaf(format!("{key}/read"), Role::DataView,
                    "Web JSON", "Configured HTTPS endpoint",
                    if action.is_some() { Availability::Ready } else { Availability::NeedsConnection },
                    action.map(|a| a.id)));
            }
            PanelKind::Desktop => {
                let actions = session.actions.iter().filter(|a| matches!(&a.kind, ActionKind::Desktop { .. }));
                for a in actions {
                    node.children.push(UiNode::leaf(format!("{key}/action/{}", a.id), Role::Button,
                        a.name.clone(), "画面の操作 / AX", Availability::NeedsReview, Some(a.id)));
                }
                if node.children.len() == 1 {
                    node.children.push(UiNode::leaf(format!("{key}/binding"), Role::Status,
                        "画面の操作を登録してください", "画面の操作", Availability::NeedsConnection, None));
                }
            }
            PanelKind::Workflow => {
                for workflow in &session.workflows {
                    let valid = !workflow.steps.is_empty() && workflow.steps.iter().all(|id| session.find_action(*id).is_some());
                    node.children.push(UiNode::leaf(format!("{key}/workflow/{}", workflow.id), Role::Workflow,
                        workflow.name.clone(), "Shibubu Runtime",
                        if valid { Availability::NeedsReview } else { Availability::NeedsConnection }, None));
                }
            }
            PanelKind::Feature => {
                node.children.push(UiNode::leaf(format!("{key}/import"), Role::Button,
                    "公開Repoの部分導入", "GitHub sparse-checkout", Availability::NeedsReview, None));
            }
            PanelKind::Notes => {
                node.children.push(UiNode::leaf(format!("{key}/notes"), Role::Text,
                    "作業メモ", "Local", Availability::Informational, None));
            }
        }
        node
    }

    // Wave 2 の汎用 renderer が使う（UI_UX_DOM_SPEC §4）
    #[allow(dead_code)]
    pub fn workspace_tree(session: &Session) -> UiNode {
        let mut root = UiNode::leaf(format!("session/{}", session.id), Role::Workspace,
            session.name.clone(), "Shibubu", Availability::Informational, None);
        root.children = session.panels.iter().filter(|p| !p.hidden)
            .map(|p| panel_tree(session, p)).collect();
        root
    }

    // Wave 2 の汎用 renderer が使う（UI_UX_DOM_SPEC §4）
    #[allow(dead_code)]
    pub fn validate(session: &Session, tree: &UiNode) -> Result<(), String> {
        fn visit(session: &Session, node: &UiNode, keys: &mut HashSet<String>) -> Result<(), String> {
            if !keys.insert(node.key.clone()) { return Err(format!("UI key duplicated: {}", node.key)); }
            if let Some(id) = node.action_id {
                if session.find_action(id).is_none() { return Err(format!("UI refers to missing action: {id}")); }
            }
            for child in &node.children { visit(session, child, keys)?; }
            Ok(())
        }
        visit(session, tree, &mut HashSet::new())
    }

    pub fn flatten(node: &UiNode, out: &mut Vec<UiNode>) {
        out.push(node.clone());
        for child in &node.children { flatten(child, out); }
    }

    pub fn source_label(kind: &PanelKind) -> &'static str {
        match kind { PanelKind::GitHubIssues | PanelKind::GitHubPulls => "GitHub",
            PanelKind::WebService => "Web service", PanelKind::Desktop => "画面の操作",
            PanelKind::Feature => "GitHub feature", PanelKind::Workflow => "Workflow",
            PanelKind::Notes => "Local" }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::engine;
        #[test]
        fn generated_structure_has_stable_keys_and_real_bindings() {
            let s = engine::generate("GitHubとIDEを統合", "rust-lang/rust");
            let tree = workspace_tree(&s);
            validate(&s, &tree).unwrap();
            let mut nodes=Vec::new(); flatten(&tree, &mut nodes);
            assert!(nodes.iter().any(|n| n.role==Role::DataView && n.action_id.is_some()));
            assert!(nodes.iter().any(|n| n.name.contains("画面の操作を登録") && n.availability==Availability::NeedsConnection));
        }
        #[test]
        fn unconnected_desktop_never_exposes_fake_action() {
            let s = engine::generate("IDE", "");
            let tree = workspace_tree(&s);
            let mut nodes=Vec::new(); flatten(&tree,&mut nodes);
            assert!(!nodes.iter().any(|n|n.source=="画面の操作 / AX" && n.availability==Availability::Ready));
        }
        #[test]
        fn no_duplicates_across_panels() {
            let s=engine::generate("GitHub Web IDE", "rust-lang/rust");
            validate(&s,&workspace_tree(&s)).unwrap();
        }
    }
} // mod ui_tree

#[cfg(test)]
mod tests {
 use super::*;
 #[test] fn prompts_generate_real_action_templates(){let s=generate("GitHubとIDEとWeb APIを一つに", "owner/repo");assert!(s.panels.len()>=5);assert!(s.actions.iter().any(|a|matches!(&a.kind,ActionKind::GitHubIssues{..})));}
 #[test] fn unsupported_prompt_cannot_become_command(){let s=generate("rm -rf /", "");assert!(s.actions.is_empty());}
 /// FX4（FX-017）: 旧 canvas の語を英語の事実の語に。Desktop の Workflow は `DESKTOP_WORKFLOW`・最初の Notes の部品は `Notes`。
 /// 保存済みの旧 session の前の名前 `Desktopで処理する` も同じ Workflow として探せる（`Workflow::is_desktop`・data は書き換えない）。
 #[test] fn fx4_old_canvas_words_are_english_and_old_sessions_still_match(){
     let s=generate("IDE","");
     let wf=s.workflows.iter().find(|w|w.is_desktop()).expect("Desktop の Workflow が無い");
     assert_eq!(wf.name,DESKTOP_WORKFLOW);
     let old=Workflow{id:1,name:"Desktopで処理する".into(),steps:vec![]};
     assert!(old.is_desktop(),"保存済みの旧 session の Workflow を探せない");
     assert!(!Workflow{id:2,name:"情報を集める".into(),steps:vec![]}.is_desktop());
     assert_eq!(Session::starter().panels[0].title,"Notes");
 }
 #[test] fn empty_workflow_rejected(){let s=generate("メモ", "");assert!(valid_workflow(&s,&s.workflows[0]).is_err());}
 #[test] fn same_github_write_parallel_refused(){
     let a=Action::new("write",ActionKind::GitHubCreateIssue{repo:"sample/repo".into(),title:"t".into(),body:"b".into()});
     assert!(!disjoint(std::slice::from_ref(&a),std::slice::from_ref(&a)));
 }
 #[test] fn same_window_parallel_refused(){let a=Action::new("test",ActionKind::Desktop{pid:12,window_id:13,label:"Save".into(),role:"button".into(),tool:DesktopTool::Click,text:String::new(),app_name:String::new()});assert!(!disjoint(std::slice::from_ref(&a),std::slice::from_ref(&a)));}
}
