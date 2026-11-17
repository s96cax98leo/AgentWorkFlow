//! SDLC 的段：有哪幾段、順序、每段對應的 workflow 範本，以及各處依段判斷的性質。
//!
//! 這是唯一的來源。原本分散在 recipe、save_run、pack、program、hooks、init、script、schema、state
//! 與 xtask 的段名單，都改成從這張表算出來（常數用 `const fn` 在編譯期篩出，型別和原本一樣是 `[&str; N]`）。
//!
//! 每個布林欄位對應一份「搬過來之前就存在」的清單，欄位說明寫了原本的位置；
//! 各份清單本來就不一樣（有的只有其中幾段），照原樣搬，不統一。
//!
//! JS 端 `plugin/workflow-templates/snippets/recipe.js` 的 `recipeSdlc`／`recipeOrder` 是另一份，
//! 由 `cargo xtask check-recipes` 和 Rust 對跑；methodology.schema.json 裡段名的 enum 也還是寫死的。

/// 一段的定義。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    /// 段名（run.json 的 `stages` 鍵、run 目錄底下的子目錄名）。
    pub id: &'static str,
    /// 這一段的標準 workflow 範本名（`plugin/workflow-templates/<template>.js`；原本是 `state::stage_workflow` 的 `sdlc-{stage}`）。
    pub template: &'static str,
    /// `awf next --workflow` 另外准用的範本（原本是 `state::machine` 寫死的 design → `sdlc-design-revise`）。
    pub alt_templates: &'static [&'static str],
    /// 給人看的段名（原本是 `deliverables::flow` 核准紀錄表的 match）。
    pub label: &'static str,
    /// 有人工閘門（原本 `save_run::GATED`）。
    pub gated: bool,
    /// profile 的 `gates.extra` 可以另外加閘門（原本 `save_run::policy::EXTRA_GATE_STAGES`）。
    pub extra_gate: bool,
    /// 閘門一律由人核准、不接受政策核准（原本 `save_run::HUMAN_ONLY`）。
    pub human_only: bool,
    /// 派工走執行核心，trace 每筆要有 model 與 tier（原本 `save_run::KERNEL_STAGES`）。
    pub kernel: bool,
    /// 可以用自訂腳本（`awf next --script`；原本 `script::STAGES`）。
    pub custom_script: bool,
    /// 存檔時算連結缺口（原本 `save_run::links::GAP_STAGES`）。
    pub link_gaps: bool,
    /// 存檔與政策核准時做規格檢查（原本 `save_run::speccheck::spec_issues` 的 match）。
    pub spec_check: bool,
    /// 核准時記規格基準（原本 `save_run::cli` 核准流程的 match）。
    pub spec_baseline: bool,
    /// 存檔與政策核准時做拆工檢查（原本 `save_run::checks::plan_check_for_with` 的 match）。
    pub plan_check: bool,
    /// 限制文件可以套用（原本 `init::profile::CONSTRAINT_STAGES` 與 `schema::CONSTRAINT_SDLC_STAGES`，兩份內容相同）。
    pub constraint: bool,
    /// 限制文件的簡寫沒寫段時的預設（原本 `init::profile::CONSTRAINT_DEFAULT_STAGES`）。
    pub constraint_default: bool,
    /// sync 走過中轉分支後，這一段改帶中轉分支名（原本 `state::args::stage_args` 的清單）。
    pub transit_branch: bool,
    /// `cargo xtask check-budget` 依 recipe 量派工數的段（原本 `xtask::check_budget::STAGES`）。
    pub budget: bool,
}

/// 全部的段，依 SDLC 順序。
pub const TABLE: [Stage; 7] = [
    Stage {
        id: "plan",
        template: "sdlc-plan",
        alt_templates: &[],
        label: "規劃",
        gated: true,
        extra_gate: false,
        human_only: true,
        kernel: true,
        custom_script: false,
        link_gaps: false,
        spec_check: false,
        spec_baseline: true,
        plan_check: true,
        constraint: true,
        constraint_default: false,
        transit_branch: false,
        budget: true,
    },
    Stage {
        id: "design",
        template: "sdlc-design",
        alt_templates: &["sdlc-design-revise"],
        label: "設計",
        gated: true,
        extra_gate: false,
        human_only: false,
        kernel: true,
        custom_script: true,
        link_gaps: true,
        spec_check: true,
        spec_baseline: true,
        plan_check: true,
        constraint: true,
        constraint_default: true,
        transit_branch: false,
        budget: true,
    },
    Stage {
        id: "build",
        template: "sdlc-build",
        alt_templates: &[],
        label: "實作",
        gated: false,
        extra_gate: true,
        human_only: false,
        kernel: true,
        custom_script: true,
        link_gaps: false,
        spec_check: false,
        spec_baseline: false,
        plan_check: false,
        constraint: true,
        constraint_default: false,
        transit_branch: true,
        budget: true,
    },
    Stage {
        id: "sync",
        template: "sdlc-sync",
        alt_templates: &[],
        label: "同步",
        gated: false,
        extra_gate: true,
        human_only: false,
        kernel: true,
        custom_script: false,
        link_gaps: false,
        spec_check: false,
        spec_baseline: false,
        plan_check: false,
        constraint: true,
        constraint_default: false,
        transit_branch: true,
        budget: false,
    },
    Stage {
        id: "test",
        template: "sdlc-test",
        alt_templates: &[],
        label: "驗證",
        gated: true,
        extra_gate: false,
        human_only: false,
        kernel: true,
        custom_script: false,
        link_gaps: true,
        spec_check: true,
        spec_baseline: false,
        plan_check: false,
        constraint: true,
        constraint_default: false,
        transit_branch: true,
        budget: true,
    },
    Stage {
        id: "deliver",
        template: "sdlc-deliver",
        alt_templates: &[],
        label: "交付",
        gated: true,
        extra_gate: false,
        human_only: false,
        kernel: true,
        custom_script: true,
        link_gaps: true,
        spec_check: true,
        spec_baseline: false,
        plan_check: false,
        constraint: true,
        constraint_default: true,
        transit_branch: true,
        budget: true,
    },
    Stage {
        id: "merge",
        template: "sdlc-merge",
        alt_templates: &[],
        label: "合併",
        gated: false,
        extra_gate: false,
        human_only: false,
        kernel: true,
        custom_script: false,
        link_gaps: false,
        spec_check: false,
        spec_baseline: false,
        plan_check: false,
        constraint: true,
        constraint_default: false,
        transit_branch: false,
        budget: false,
    },
];

/// 篩段用的性質（[`Stage`] 的布林欄位；`All` 是全部）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    All,
    Gated,
    ExtraGate,
    HumanOnly,
    Kernel,
    CustomScript,
    LinkGaps,
    SpecCheck,
    SpecBaseline,
    PlanCheck,
    Constraint,
    ConstraintDefault,
    TransitBranch,
    Budget,
}

impl Flag {
    /// 這一段有沒有這個性質。
    pub const fn of(self, s: &Stage) -> bool {
        match self {
            Flag::All => true,
            Flag::Gated => s.gated,
            Flag::ExtraGate => s.extra_gate,
            Flag::HumanOnly => s.human_only,
            Flag::Kernel => s.kernel,
            Flag::CustomScript => s.custom_script,
            Flag::LinkGaps => s.link_gaps,
            Flag::SpecCheck => s.spec_check,
            Flag::SpecBaseline => s.spec_baseline,
            Flag::PlanCheck => s.plan_check,
            Flag::Constraint => s.constraint,
            Flag::ConstraintDefault => s.constraint_default,
            Flag::TransitBranch => s.transit_branch,
            Flag::Budget => s.budget,
        }
    }
}

/// 有這個性質的段數（編譯期用）。
pub const fn count(f: Flag) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < TABLE.len() {
        if f.of(&TABLE[i]) {
            n += 1;
        }
        i += 1;
    }
    n
}

/// 有這個性質的段名，依 SDLC 順序（編譯期用；`N` 要等於 [`count`]）。
pub const fn pick<const N: usize>(f: Flag) -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    let mut j = 0;
    while i < TABLE.len() {
        if f.of(&TABLE[i]) {
            out[j] = TABLE[i].id;
            j += 1;
        }
        i += 1;
    }
    assert!(j == N, "pick 的長度和 count 不一致");
    out
}

/// 全部段名，依 SDLC 順序。
pub const IDS: [&str; count(Flag::All)] = pick(Flag::All);
pub const GATED: [&str; count(Flag::Gated)] = pick(Flag::Gated);
pub const EXTRA_GATE: [&str; count(Flag::ExtraGate)] = pick(Flag::ExtraGate);
pub const HUMAN_ONLY: [&str; count(Flag::HumanOnly)] = pick(Flag::HumanOnly);
pub const KERNEL: [&str; count(Flag::Kernel)] = pick(Flag::Kernel);
pub const CUSTOM_SCRIPT: [&str; count(Flag::CustomScript)] = pick(Flag::CustomScript);
pub const LINK_GAPS: [&str; count(Flag::LinkGaps)] = pick(Flag::LinkGaps);
pub const CONSTRAINT: [&str; count(Flag::Constraint)] = pick(Flag::Constraint);
pub const CONSTRAINT_DEFAULT: [&str; count(Flag::ConstraintDefault)] = pick(Flag::ConstraintDefault);
pub const BUDGET: [&str; count(Flag::Budget)] = pick(Flag::Budget);

/// 全部的段，依 SDLC 順序。
pub fn all() -> &'static [Stage] {
    &TABLE
}

/// 全部段名，依 SDLC 順序。
pub fn ids() -> &'static [&'static str] {
    &IDS
}

/// 段名對應的定義；不是 SDLC 的段回 None。
pub fn get(id: &str) -> Option<&'static Stage> {
    TABLE.iter().find(|s| s.id == id)
}

/// 是不是 SDLC 的段。
pub fn is_sdlc(id: &str) -> bool {
    get(id).is_some()
}

/// 在 SDLC 裡的位置（從 0 起）。
pub fn order_of(id: &str) -> Option<usize> {
    TABLE.iter().position(|s| s.id == id)
}

/// 這一段的標準範本名。
pub fn template_of(id: &str) -> Option<&'static str> {
    get(id).map(|s| s.template)
}

/// 這一段准用的其他範本。
pub fn alt_templates_of(id: &str) -> &'static [&'static str] {
    get(id).map_or(&[], |s| s.alt_templates)
}

/// 給人看的段名。
pub fn label_of(id: &str) -> Option<&'static str> {
    get(id).map(|s| s.label)
}

/// 這一段有沒有這個性質（不是 SDLC 的段一律 false）。
pub fn has(id: &str, f: Flag) -> bool {
    get(id).is_some_and(|s| f.of(s))
}

/// 有這個性質的段名，依 SDLC 順序。
pub fn with(f: Flag) -> Vec<&'static str> {
    TABLE.iter().filter(|s| f.of(s)).map(|s| s.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 搬過來之前各處的字面值：表改了要逐項確認，不要直接改這裡讓測試過。
    const OLD_SDLC: [&str; 7] = ["plan", "design", "build", "sync", "test", "deliver", "merge"];

    #[test]
    fn order_matches_recipe() {
        assert_eq!(IDS, OLD_SDLC);
        assert_eq!(ids(), &OLD_SDLC);
        for (i, s) in OLD_SDLC.iter().enumerate() {
            assert_eq!(order_of(s), Some(i));
        }
        assert_eq!(order_of("design-revise"), None);
    }

    #[test]
    fn templates_match_old_mapping() {
        for s in OLD_SDLC {
            assert_eq!(template_of(s), Some(format!("sdlc-{s}").as_str()));
        }
        assert_eq!(alt_templates_of("design"), &["sdlc-design-revise"]);
        for s in OLD_SDLC.iter().filter(|s| **s != "design") {
            assert!(alt_templates_of(s).is_empty());
        }
        assert_eq!(template_of("brief"), None);
    }

    #[test]
    fn flags_match_old_lists() {
        assert_eq!(GATED, ["plan", "design", "test", "deliver"]);
        assert_eq!(EXTRA_GATE, ["build", "sync"]);
        assert_eq!(HUMAN_ONLY, ["plan"]);
        assert_eq!(KERNEL, OLD_SDLC);
        assert_eq!(CUSTOM_SCRIPT, ["design", "build", "deliver"]);
        assert_eq!(LINK_GAPS, ["design", "test", "deliver"]);
        assert_eq!(with(Flag::SpecCheck), ["design", "test", "deliver"]);
        assert_eq!(with(Flag::SpecBaseline), ["plan", "design"]);
        assert_eq!(with(Flag::PlanCheck), ["plan", "design"]);
        assert_eq!(CONSTRAINT, OLD_SDLC);
        assert_eq!(CONSTRAINT_DEFAULT, ["design", "deliver"]);
        assert_eq!(with(Flag::TransitBranch), ["build", "sync", "test", "deliver"]);
        assert_eq!(BUDGET, ["plan", "design", "build", "test", "deliver"]);
        assert_eq!(with(Flag::All), OLD_SDLC);
        assert!(!has("brief", Flag::All));
    }

    #[test]
    fn labels_match_old_match() {
        let old = [("plan", "規劃"), ("design", "設計"), ("build", "實作"), ("sync", "同步"), ("test", "驗證"), ("deliver", "交付"), ("merge", "合併")];
        for (s, l) in old {
            assert_eq!(label_of(s), Some(l));
        }
        assert_eq!(label_of("brief"), None);
    }
}
