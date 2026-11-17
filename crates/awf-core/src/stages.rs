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

// ------------------------------------------------------------------ 自訂段（profile 的 `flow`）
//
// 上面的表是編譯期的、只有內建七段。使用者在 profile 的 `flow.stages` 加的段在執行時才知道，由 [`Resolved`] 解析：
// 內建表＋自訂段的順序、每個自訂段的腳本與核准者、給人看的段名。需要認得自訂段的呼叫端用它；只處理內建段性質的維持用常數。
//
// 這裡的讀法是寬鬆的（讀不懂的項目當成沒宣告）：recipe 點名了沒宣告的段會在 recipe 解析時報錯，所以寬鬆只會讓流程停下、不會放行。
// 完整的規則（腳本位置、gate、撞名、循環）在 `schema::flow_errors`，閘門讀 profile 時會跑。
// 順序的算法要和 JS 端 `snippets/recipe.js` 的 `recipeFlowOrder` 一致（`cargo xtask check-recipes` 對跑）。

use serde_json::Value;

/// 自訂段最多幾個（只看 `flow.stages` 的前幾筆）。
pub const FLOW_MAX: usize = 8;
/// 自訂段的腳本要放在專案的這個目錄底下（team agent 寫不到）。
pub const FLOW_DIR: &str = ".claude/flows/";
/// 這一版 `run` 只收這個值。
pub const FLOW_RUN: &str = "script";
/// 這一版 `gate` 只收這個值：自訂段做完一律停下等人核准。
pub const FLOW_GATE: &str = "human";
pub const FLOW_APPROVER_MAX: usize = 40;
pub const FLOW_LABEL_MAX: usize = 20;
pub const FLOW_ARTIFACTS_MAX: usize = 8;
/// 自訂段不能接在它後面的內建段（merge 是信任邊界，自訂段一定在它之前）。
pub const FLOW_NO_ANCHOR: &str = "merge";

/// 自訂段的 id：小寫英數與連字號、英文字母開頭、32 字以內。
pub fn flow_id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty() && b.len() <= 32 && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// 自訂段的腳本路徑：專案內的相對路徑、在 [`FLOW_DIR`] 底下、副檔名 `.js`、不含 `..`、不含反斜線與控制字元。
pub fn flow_script_ok(p: &str) -> bool {
    let Some(rest) = p.strip_prefix(FLOW_DIR) else { return false };
    p.len() <= 200
        && p.ends_with(".js")
        && !p.contains('\\')
        && !p.chars().any(char::is_control)
        && rest.split('/').all(|seg| !seg.is_empty() && !seg.starts_with('.'))
}

/// 產出檔名（放在 `<run>/<段>/` 底下）：單一檔名，不含路徑分隔、不是 `.` 開頭、80 字以內、沒有控制字元。
pub fn flow_artifact_ok(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= 80 && !name.starts_with('.') && !name.contains(['/', '\\']) && !name.chars().any(char::is_control)
}

/// 一行、沒有控制字元、去頭尾空白後不是空的、`max` 字以內。
pub fn flow_text_ok(t: &str, max: usize) -> bool {
    !t.trim().is_empty() && t.chars().count() <= max && !t.chars().any(char::is_control)
}

/// 一個自訂段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Custom {
    pub id: String,
    /// 接在哪一段後面（內建段，或另一個自訂段）。
    pub after: String,
    /// 腳本（專案內的相對路徑；不合 [`flow_script_ok`] 時是空字串，用到的地方會停下）。
    pub script: String,
    /// 誰核准（只記錄與顯示；沒寫是空字串）。
    pub approver: String,
    /// 做完要有的產出檔名（`<run>/<段>/<檔名>`）。
    pub artifacts: Vec<String>,
}

/// 內建表加上 profile 的自訂段。
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    order: Vec<String>,
    customs: Vec<Custom>,
    labels: Vec<(String, String)>,
}

fn flow_put(id: &str, listed: &[Custom], order: &mut Vec<String>) {
    order.push(id.to_string());
    for c in listed.iter().filter(|c| c.after == id) {
        flow_put(&c.id, listed, order);
    }
}

impl Resolved {
    /// 從 profile（methodology.json 的內容；null 或沒有 `flow` 就只有內建段）解析。
    pub fn of(profile: &Value) -> Resolved {
        let mut listed: Vec<Custom> = Vec::new();
        let raw = profile.get("flow").and_then(|f| f.get("stages")).and_then(Value::as_array);
        for x in raw.into_iter().flatten().take(FLOW_MAX) {
            let (Some(id), Some(after)) = (x.get("id").and_then(Value::as_str), x.get("after").and_then(Value::as_str)) else { continue };
            if !flow_id_ok(id) || is_sdlc(id) || listed.iter().any(|c| c.id == id) {
                continue;
            }
            let text = |k: &str| x.get(k).and_then(Value::as_str).unwrap_or_default();
            listed.push(Custom {
                id: id.to_string(),
                after: after.to_string(),
                script: Some(text("script")).filter(|p| flow_script_ok(p)).unwrap_or_default().to_string(),
                approver: Some(text("approver")).filter(|a| flow_text_ok(a, FLOW_APPROVER_MAX)).map(str::trim).unwrap_or_default().to_string(),
                artifacts: x.get("artifacts").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|a| flow_artifact_ok(a)).take(FLOW_ARTIFACTS_MAX).map(str::to_string).collect(),
            });
        }
        // 順序：每個內建段後面接「after 是它」的自訂段（依宣告順序），每個自訂段後面再接「after 是它」的；
        // 接不到內建段的（after 指向不存在的段、merge，或彼此循環）不算宣告過
        let mut order: Vec<String> = Vec::new();
        for b in IDS {
            if b == FLOW_NO_ANCHOR {
                order.push(b.to_string());
            } else {
                flow_put(b, &listed, &mut order);
            }
        }
        let customs: Vec<Custom> = listed.into_iter().filter(|c| order.contains(&c.id)).collect();
        let mut labels = Vec::new();
        if let Some(Value::Object(l)) = profile.get("flow").and_then(|f| f.get("labels")) {
            for (k, v) in l {
                let known = is_sdlc(k) || customs.iter().any(|c| &c.id == k);
                if let Some(t) = v.as_str().filter(|t| known && flow_text_ok(t, FLOW_LABEL_MAX)) {
                    labels.push((k.clone(), t.trim().to_string()));
                }
            }
        }
        Resolved { order, customs, labels }
    }

    /// 全部的段（內建＋自訂），依流程順序。
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// 有宣告（而且接得上）的自訂段，依宣告順序。
    pub fn customs(&self) -> &[Custom] {
        &self.customs
    }

    pub fn custom(&self, id: &str) -> Option<&Custom> {
        self.customs.iter().find(|c| c.id == id)
    }

    pub fn is_custom(&self, id: &str) -> bool {
        self.custom(id).is_some()
    }

    /// 給人看的段名：profile 的 `flow.labels` → 內建段的 label → None。只影響顯示。
    pub fn label(&self, id: &str) -> Option<String> {
        self.own_label(id).map(str::to_string).or_else(|| label_of(id).map(str::to_string))
    }

    /// profile 自己給的段名（沒給回 None；和 [`Resolved::label`] 不同，不退回內建的）。
    pub fn own_label(&self, id: &str) -> Option<&str> {
        self.labels.iter().find(|(k, _)| k == id).map(|(_, v)| v.as_str())
    }
}

/// profile 的全部段（內建＋自訂），依流程順序。沒有 `flow` 時就是 [`IDS`]。
pub fn flow_order(profile: &Value) -> Vec<String> {
    Resolved::of(profile).order
}

/// run.json 的 `stages` 裡要看的段，依流程順序：沒有自訂段時就是 [`IDS`]（和原本逐字相同）；
/// 有自訂段（存檔時標了 `flow`）時照 run.json 的順序（存檔時已經照流程排好），只留內建段與自訂段。給「這個 run 現在停在哪」這類摘要用。
pub fn names_in_run(stages: &serde_json::Map<String, Value>) -> Vec<String> {
    let custom = |k: &str| stages.get(k).and_then(|s| s.get("flow")) == Some(&Value::Bool(true));
    if !stages.keys().any(|k| !is_sdlc(k) && custom(k)) {
        return IDS.iter().map(|s| s.to_string()).collect();
    }
    let mut out: Vec<String> = stages.keys().filter(|k| is_sdlc(k) || custom(k)).cloned().collect();
    out.extend(IDS.iter().filter(|s| !stages.contains_key(**s)).map(|s| s.to_string()));
    out
}

/// 一份段清單裡不是內建段的名字（run.json 的 `recipe.resolved.stages` 裡的自訂段），照原順序。
pub fn customs_in(stages: &Value) -> Vec<String> {
    stages.as_array().into_iter().flatten().filter_map(Value::as_str).filter(|s| !is_sdlc(s)).map(str::to_string).collect()
}

/// 這個 run 鎖定的 recipe 點名的自訂段（run.json 沒有 recipe、或沒有點名時是空的）。
pub fn run_customs(run: &Value) -> Vec<String> {
    run.get("recipe").and_then(|r| r.get("resolved")).and_then(|r| r.get("stages")).map(customs_in).unwrap_or_default()
}

/// 不靠 profile、只從一份已經排好的段清單（run.json 鎖定的 `recipe.resolved.stages`）推出這個 run 的完整順序：
/// 內建段照 SDLC 順序（清單沒有的也在），自訂段接在清單裡它前面最近的那個內建段後面（清單裡的先後不變）。
/// 沒有自訂段時就是 [`IDS`]。清單是不是照 profile 的順序，由 [`flow_tamper`] 另外核對。
pub fn run_order(want: &Value) -> Vec<String> {
    let mut anchored: Vec<(String, String)> = Vec::new();
    let mut last = IDS[0].to_string();
    for s in want.as_array().into_iter().flatten().filter_map(Value::as_str) {
        if is_sdlc(s) {
            last = s.to_string();
        } else if !anchored.iter().any(|(c, _)| c == s) {
            anchored.push((s.to_string(), last.clone()));
        }
    }
    let mut out = Vec::new();
    for b in IDS {
        out.push(b.to_string());
        out.extend(anchored.iter().filter(|(_, a)| a == b).map(|(c, _)| c.clone()));
    }
    out
}

/// run.json 鎖定的段清單（`recipe.resolved.stages`）和現在的 profile 對不上的地方（空清單＝沒問題；清單裡沒有自訂段時一律空）：
/// - 點名的自訂段在 profile 的 `flow` 裡沒有（被拿掉、改名，或宣告不合規則）。
/// - 段的先後和 profile 算出來的順序不同（手改過，或 `after` 在 plan 之後被改了）。
///
/// 閘門（核准、publish、merge-gate）與 `awf next` 共用：對不上一律停下交人，不默默略過。
pub fn flow_tamper(profile: &Value, resolved_stages: &Value) -> Vec<String> {
    let named = customs_in(resolved_stages);
    if named.is_empty() {
        return Vec::new();
    }
    let flow = Resolved::of(profile);
    let gone: Vec<&str> = named.iter().map(String::as_str).filter(|c| !flow.is_custom(c)).collect();
    if !gone.is_empty() {
        return vec![format!(
            "run.json 的 recipe 點名了自訂段 {}，但現在的 profile（flow.stages）沒有宣告：不採信、不略過（把宣告加回去，或 abandon 後重開 run）",
            gone.join("、")
        )];
    }
    let have: Vec<&str> = resolved_stages.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let want: Vec<&str> = flow.order().iter().map(String::as_str).filter(|s| have.contains(s)).collect();
    if have != want {
        return vec![format!("run.json 的 recipe 段的順序是 {}，照現在的 profile（flow）應該是 {}（手改過，或 plan 之後改了 flow）：不採信", have.join(" → "), want.join(" → "))];
    }
    Vec::new()
}

/// 這個 run 鎖定的 recipe 點名的自訂段裡，還沒「做完而且由人核准」的（run.json 的 `stages`；不靠 profile）。
/// `before`：只看排在這一段之前的（None＝全部）。publish 與 merge-gate 用：自訂段沒過就不能推、不能合。
pub fn flow_unapproved(run: &Value, before: Option<&str>) -> Vec<String> {
    let want = run.get("recipe").and_then(|r| r.get("resolved")).and_then(|r| r.get("stages")).cloned().unwrap_or(Value::Null);
    let named = customs_in(&want);
    if named.is_empty() {
        return Vec::new();
    }
    let order = run_order(&want);
    let limit = before.and_then(|b| order.iter().position(|s| s == b)).unwrap_or(order.len());
    let mut out = Vec::new();
    for c in named {
        if order.iter().position(|s| *s == c).is_none_or(|p| p >= limit) {
            continue;
        }
        let st = run.get("stages").and_then(|s| s.get(&c));
        let status = st.and_then(|s| s.get("status")).and_then(Value::as_str).unwrap_or("沒有這一段");
        let ap = st.and_then(|s| s.get("approved"));
        let human = ap.is_some_and(|a| a.get("by").and_then(Value::as_str).is_some_and(|b| !b.is_empty() && b != "policy") && a.get("kind").and_then(Value::as_str) != Some("policy"));
        if status != "done" {
            out.push(format!("自訂段 {c} 狀態是 {status}，還沒 done"));
        } else if !human {
            out.push(format!("自訂段 {c} 還沒有人核准（自訂段一律由人核准）"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flow(stages: Value) -> Value {
        json!({"flow": {"stages": stages, "labels": {"design": "系統設計", "uat": "客戶驗收", "nope": "x"}}})
    }

    #[test]
    fn no_flow_is_builtin_order() {
        assert_eq!(flow_order(&Value::Null), IDS);
        assert_eq!(flow_order(&json!({"flow": {"stages": []}})), IDS);
        assert_eq!(run_order(&json!(["plan", "build", "merge"])), IDS);
        assert!(flow_tamper(&Value::Null, &json!(OLD_SDLC)).is_empty());
    }

    #[test]
    fn customs_follow_their_anchor_in_declaration_order() {
        let p = flow(json!([
            {"id": "uat", "after": "test", "script": ".claude/flows/uat.js"},
            {"id": "perf-check", "after": "test", "script": ".claude/flows/perf.js"},
            {"id": "signoff", "after": "uat", "script": ".claude/flows/s.js", "approver": " 示範客戶 ACME 的 PM ", "artifacts": ["驗收報告.md", "../x", ".hidden"]},
        ]));
        let r = Resolved::of(&p);
        assert_eq!(r.order(), ["plan", "design", "build", "sync", "test", "uat", "signoff", "perf-check", "deliver", "merge"]);
        assert_eq!(r.custom("signoff").map(|c| (c.approver.as_str(), c.artifacts.clone())), Some(("示範客戶 ACME 的 PM", vec!["驗收報告.md".to_string()])));
        assert_eq!(r.label("design").as_deref(), Some("系統設計"));
        assert_eq!(r.label("uat").as_deref(), Some("客戶驗收"));
        assert_eq!(r.label("build").as_deref(), Some("實作"));
        assert_eq!(r.label("nope"), None);
    }

    #[test]
    fn bad_entries_are_not_declared() {
        let p = flow(json!([
            {"id": "a", "after": "merge", "script": ".claude/flows/a.js"},
            {"id": "plan", "after": "plan", "script": ".claude/flows/a.js"},
            {"id": "b", "after": "c", "script": ".claude/flows/a.js"},
            {"id": "c", "after": "b", "script": ".claude/flows/a.js"},
            {"id": "Bad", "after": "test", "script": ".claude/flows/a.js"},
            {"id": "d", "after": "nowhere", "script": ".claude/flows/a.js"},
            {"id": "ok", "after": "deliver", "script": "scripts/ok.js"},
            {"id": "ok", "after": "plan", "script": ".claude/flows/ok.js"},
        ]));
        let r = Resolved::of(&p);
        assert_eq!(r.order(), ["plan", "design", "build", "sync", "test", "deliver", "ok", "merge"]);
        assert_eq!(r.custom("ok").map(|c| c.script.as_str()), Some(""), "腳本不在 .claude/flows/ 底下：留空，用到時停下");
        assert_eq!(r.customs().len(), 1);
        // 只看前 FLOW_MAX 筆
        let many: Vec<Value> = (0..10).map(|i| json!({"id": format!("s{i}"), "after": "test", "script": ".claude/flows/s.js"})).collect();
        assert_eq!(Resolved::of(&flow(json!(many))).customs().len(), FLOW_MAX);
    }

    #[test]
    fn script_paths() {
        for ok in [".claude/flows/uat.js", ".claude/flows/sub/uat-2.js"] {
            assert!(flow_script_ok(ok), "{ok}");
        }
        for bad in ["uat.js", ".claude/flows/", ".claude/flows/.js", ".claude/flows/../teams/x.js", ".claude/flows/a.mjs", "/abs/.claude/flows/a.js", ".claude/flows//a.js", ".claude\\flows\\a.js", ".claude/flows/.hid/a.js", ".claude/workflows/a.js"] {
            assert!(!flow_script_ok(bad), "{bad}");
        }
    }

    #[test]
    fn run_order_and_tamper() {
        let p = flow(json!([{"id": "uat", "after": "test", "script": ".claude/flows/uat.js"}]));
        let locked = json!(["plan", "build", "test", "uat", "deliver", "merge"]);
        assert_eq!(run_order(&locked), ["plan", "design", "build", "sync", "test", "uat", "deliver", "merge"]);
        assert!(flow_tamper(&p, &locked).is_empty());
        // 被從 profile 拿掉
        assert!(flow_tamper(&Value::Null, &locked)[0].contains("沒有宣告"));
        // 順序被手改（挪到 deliver 後面）
        assert!(flow_tamper(&p, &json!(["plan", "build", "test", "deliver", "uat", "merge"]))[0].contains("順序"));
    }

    #[test]
    fn unapproved_customs() {
        let run = |uat: Value| json!({"recipe": {"resolved": {"stages": ["plan", "build", "test", "uat", "deliver", "merge"]}}, "stages": {"uat": uat}});
        assert!(flow_unapproved(&json!({"stages": {}}), None).is_empty());
        assert_eq!(flow_unapproved(&run(json!({"status": "pending"})), None).len(), 1);
        assert_eq!(flow_unapproved(&run(json!({"status": "done"})), None).len(), 1);
        assert_eq!(flow_unapproved(&run(json!({"status": "done", "approved": {"by": "policy", "kind": "policy"}})), None).len(), 1);
        assert!(flow_unapproved(&run(json!({"status": "done", "approved": {"by": "pm", "kind": "human"}})), None).is_empty());
        assert!(flow_unapproved(&run(json!({"status": "pending"})), Some("test")).is_empty());
        assert_eq!(flow_unapproved(&run(json!({"status": "pending"})), Some("merge")).len(), 1);
    }

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
