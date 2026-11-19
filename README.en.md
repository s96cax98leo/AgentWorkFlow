# AgentWorkFlow

[繁體中文](README.md) | **English**

> The detailed docs under `docs/` are currently written in Traditional Chinese.

AgentWorkFlow is a workflow template for Claude Code agent teams that can be applied to any project. It only defines the **workflow**: how to split work, how to parallelize it, where to stop for human approval, what format to hand work off in, and who may merge a PR. Development methodology (TDD, DDD, spec-driven, and so on) and conventions (code style, commit format, branch naming, PR size, document format) are defined by the user.

This README covers the **approach** first (why it is designed this way, how one requirement goes from start to finish, who guards what), then installation and usage.

- **Development requirements** go through an AI-native SDLC: plan → design → build → sync → test → deliver → merge, with human approval between stages.
- **Other events** (briefs, upstream changes, meetings, PR reviews, requirement changes, decision follow-through, pre-demo checks, post-merge monitoring) are each handled in parallel and produce documents.
- **Website analysis**: analyzes a site in parallel from five angles (technical architecture, infrastructure, business model, performance, competitors), then synthesizes one report.

All documents, comments, and agent prompts are in Traditional Chinese. The agents' output language is configurable (`{{OUTPUT_LANGUAGE}}`, default Traditional Chinese). The tool is a single executable, `awf` (Rust, shipped with the plugin), which supports macOS, Linux, and Windows (PowerShell) and does not need Python (the bundled executable is currently macOS arm64 only; see Requirements). The current version is **1.2.2**; see [`CHANGELOG.md`](CHANGELOG.md) for changes.

## Approach

### 1. Design principles

| Principle | Meaning | How it is enforced |
|---|---|---|
| Define the workflow, not the methodology | The template does not decide TDD, commit format, or branch naming for you | These are written in the project's development methodology profile (`methodology.json`); agents and workflows only read from the profile |
| Humans decide at gates; agents work between gates | An agent never decides by itself that it is "OK to move to the next stage" | At the end of plan, design, test, and deliver, the flow stops and lists the highlights in the conversation; approval is recorded with `awf save-run approve`; plan is always approved by a human |
| The author does not verify; the verifier does not modify | The same agent cannot both implement and declare a pass | Implementation, verification, review, rebuttal, and merge are separate roles; read-only roles have no Edit/Write tools |
| Do not trust what an agent says | An agent reporting "passed" does not count | Gates are recomputed by a deterministic tool (`awf`) from actual data: envelope contents, head SHA, CI results, diff shape, whether run record files exist |
| A missing handoff is not "no findings" | An agent that fails, times out, or does not hand off cannot be treated as "no problems" | The workflow lists missing handoffs in `missing`; a missing review, rebuttal, or verification handoff always stops and goes to a human |
| Gates only get stricter | When data is missing, unreadable, or inconsistent, the tool always holds | A flow can be tightened with `awf save-run escalate`; the tool never downgrades automatically |
| Auxiliary features do not block the flow | If the knowledge base, context packs, or cost statistics break, they only warn | These tools take no part in gate decisions; if a hook cannot run, it lets the action through and does not lock the user out |
| Do not break existing settings | Applying the template to an existing project only adds files | `CLAUDE.md` only gets one appended reference line, settings are deep-merged; role rules apply only to the template's own agents |

### 2. How one requirement goes from start to finish

```
You: /sdlc <requirement>
 │
 ├─ Main session lists the plan (which workflow, how many agents, which irreversible actions) ──► starts only after you approve
 │
 ├─ plan     triage decides requirement type → locate line ranges → analyze → write intent → skeptic challenges ──► Gate: human approves intent and recipe
 ├─ design   outline → (split into sub-plans planned in parallel if too large) → spec, plan, subtasks and file ownership ──► Gate: approve spec + plan
 ├─ build    subtasks layered by dependency; each in a layer implemented in parallel in its own worktree; integrator merges each layer
 ├─ sync     a deterministic tool decides how to catch up with main (rebase / merge / transit branch)
 ├─ test     verifier checks each criterion at the current HEAD + anti-cheating checks; design and security review as needed ──► Gate: confirm verification results
 ├─ deliver  review the whole branch along the review axes, skeptic rebuts each finding, write the PR description ──► Gate: you decide whether to push and open the PR
 └─ merge    (off by default) steward checks requirement against evidence; merges only if awf merge-gate passes ──► monitor main after merge
```

- Each stage is one workflow script (`sdlc-*.js`), run by the main session with the Workflow tool. A script cannot read the time and cannot wait for human input midway, so wherever a human must decide, the workflow is split into two scripts.
- The return value of each stage is saved into the run directory and validated by `awf save-run stage`; before the next stage starts, the whole gate chain must pass.
- **Not every requirement goes through all seven stages**: triage suggests a recipe (new feature, bug, CR, hotfix, refactor, dependency upgrade, docs, test-only, performance, spike, revert), and you confirm and lock it at the plan gate. See "Routing by requirement type" below for details.
- Whatever the flow, **the intent, rules.md, human approval of plan, and every agent's run record** cannot be skipped.

### 3. Who does the work, who guards it

| Work | Role | Must not |
|---|---|---|
| Write intent, spec, split tasks | `dev-spec-author` | Implement |
| Write acceptance tests | `dev-acceptance-tester` | Write product code |
| Implement one subtask | `dev-implementer` | Edit files owned by others, edit acceptance tests |
| Merge subtask branches, catch up with main | `dev-integrator` | Slip in extra changes |
| Verify, review | `dev-verifier`, `dev-design-reviewer`, `dev-security-reviewer`, `tpm-reviewer` | Change code (read-only) |
| Rebut others' conclusions | `tpm-skeptic` | Let things pass when unsure (when unsure, the rebuttal stands) |
| Merge the PR | `dev-merge-steward` | Approve the PR, edit any file |
| Research, write documents | `tpm-researcher`, `tpm-writer` | Write to upstream, push |

- **Every finding goes through rebuttal**: for each finding a reviewer raises, the skeptic tries to overturn it, and only findings that survive are adopted.
- **No agent can approve a PR**, and neither can the main session.
- These limits do not rely on prompts: a role's tool list lacks the corresponding tool, and hooks additionally block writes to certain paths and certain commands by role.

### 4. Deterministic tools: gates decide from data

Agents hand off to each other in JSON envelopes (status, summary, artifacts, evidence, confidence, open_questions, derived_work, payload); the format is defined by a schema, and a hook validates it at handoff. All later decisions are left to deterministic tools that contain no LLM (subcommands of `awf`):

| Tool | What it decides |
|---|---|
| `awf save-run` | Saves each stage's return value, validates envelopes, records approvals. For policy approval it reads the actual content: verification items that did not pass, anti-cheating findings, blocking issues, missing agent handoffs, and missing record files all prevent automatic approval |
| `awf recipe` | Which stages this requirement should go through; later it cross-checks against the actual diff (claims to change docs but touches code, skipped design but touches a migration) |
| `awf sync-branch` | How to catch up with main, and checks that the transit branch has no leftover conflict markers or smuggled-in changes |
| `awf publish` | Pushes only feature branches of the fork; before pushing, scans for secrets and checks that the whole gate chain is approved |
| `awf merge-gate` | Requirement mapping table, the head SHA at review time, required checks, test count, risk level, daily limit, allowlist, freeze flag; everything is written to the audit log |

Workflows also do deterministic checks of their own: sub-plan scopes must not overlap, the consolidated result must not silently drop subtasks, consolidation must not change verification results, and all shards must verify the same HEAD. If anything does not match, the flow stops and goes to a human.

### 5. Every agent leaves a record

- Each task brief designates one record file, `<run directory>/<stage>/<label>.md`: what was done, which commands were run and their results, the conclusion, and what is uncertain.
- `awf save-run stage` produces `執行紀錄索引.md` (the run record index) from the trace the workflow returns; a missing record file counts as a missing handoff and blocks policy approval.
- The run directory keeps the whole process: the requirement, recipe, each stage's return value, approval records (who, when), each agent's record, and the merge audit. Later, "who made this decision, on what basis" can always be looked up.

### 6. Saving tokens: limit what is read in, not what is written out

Measured before the redesign, the plan stage of one requirement took 21 to 29M tokens, of which **95% was context re-reading** (an agent reads a large file whole, then re-reads it on every later turn), and output was only a few percent. So the way to save tokens is to make agents **read less, read precisely, and use the right model**, not to tell them to write less.

**Not limited**: the agents' output tokens (no output cap is set, and reasoning effort is not lowered); the deliverable files such as intent, spec, research conclusions, review reports, and run records have no length limit.

**Can be limited (all adjusted in the profile)**. The read budget is **off by default**: whether to turn it on and what thresholds to use is up to you; init asks, and you can edit `methodology.json` directly later.

| Mechanism | What it does | Setting |
|---|---|---|
| Model tiering | triage suggests which tier (`light` / `standard` / `strong`) each role should use, and the tool enforces a floor: merge, security review, and the plan stage's spec-author always use the strongest model, and the AI cannot go lower. If a handoff is missing, it is re-dispatched once with a tier higher | `models` |
| Read budget (off by default) | When on, a hook blocks template roles from reading large files whole (suggested threshold: files over 400 lines must be read in segments) and gives each agent a read quota; when it runs out, the agent is told to wrap up and hand off | `limits.mode`: unset or `off` = disabled, `warn` = remind only, `enforce` = block; thresholds are `whole_file_lines` and `read_budget` |
| Knowledge layer | `awf kb` indexes documents as "which lines of which file"; agents query first and read only those lines; the output can be opened in Obsidian | `knowledge` |
| Context pack | Before each stage starts, a tool (not an agent) prepares the requirement, the line ranges hit in the knowledge base, and the records of earlier stages | `awf pack` |
| Two-step research | First "locate" line ranges, then analysis agents read only those ranges. Locating is done by a tool first (costs no tokens), and a cheap model is dispatched only if that is not enough | Built into `sdlc-plan`; `awf locate` |
| Subtask context pack | Before build, prepares for each subtask the line ranges of the matching spec sections, the constraint documents that apply, and the interfaces of dependent subtasks; the implementing agent does not need to read the whole spec | `awf pack build`, the subtask's `spec_refs` |
| Decision memory | Questions a human has answered at a gate are recorded, and later context packs include the relevant ones, so agents do not ask again | `awf save-run approve --answer`, `decide` |
| Research reuse | When plan is re-run, research angles whose source files have not changed are reused directly; the gate re-checks the sources' hashes | `awf pack --reuse-from`, `--keep-research` |
| Dispatch cap | The maximum number of agents one workflow may dispatch; at the cap it stops and goes to a human, with no silent truncation | `limits.max_agents` |
| Turn budget | When an agent's tool calls reach the threshold (default 50), it is reminded to write down its findings and hand in; by default it only reminds, and only `enforce` blocks | `limits.turns` |
| Section-level context packs | The test stage, the deliver stage, and the design revision round also receive only the relevant sections and line ranges, instead of reading the whole spec and design | `awf pack` (automatic); `--no-slices` turns it off |
| Knowledge graph | When a graph has been built, the context pack includes the files, symbols, and impact range for the requirement; without a graph the flow is unchanged | `awf kg build`; `knowledge` |

The only thing that touches output is the **handoff envelope**: summary is at most 400 characters and evidence at most 12 items. It governs only the JSON an agent returns to the workflow; the details are still written in full in files, and the envelope carries only paths.

To turn on the read budget: choose `warn` or `enforce` in the init Q&A, or write `"read_limits": {"mode": "enforce", "whole_file_lines": 400, "read_budget": 800000}` in the answers file, or add `mode` to `limits` in `methodology.json` directly. When the quota runs out the agent wraps up early, which on large projects may reduce the completeness of research, so it is best to observe with `warn` first and then decide on thresholds.

Measured on the fictional project in `tests/bench/` (67 files, about 6,100 lines), the 0.4.0 plan stage (read budget set to `enforce`): the first run took 2.69M tokens and 6.9 minutes; re-running the same run took 0.76M (28% of the first run). The pre-redesign figures were measured on a much larger real project; the two are different scenarios and cannot be divided directly. See [`CHANGELOG.md`](CHANGELOG.md) for the full numbers and limitations. To see what your own run cost:

```bash
awf cost --project . --run-dir <run 目錄>
```

### 7. The trust boundary is on the server side

- **Hooks are a local speed bump**: they block pushing, writing to upstream, unauthorized writes, reading secret files, and every merge and approval path. They can be bypassed, so they are not the trust boundary.
- **The real trust boundary is on GitHub**: rulesets, the gate App's required check, and CODEOWNERS. Auto-merge is off by default; before enabling it, set things up following the checklist in [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md).
- **Push to the fork only**: upstream and forbidden repos are always blocked for everyone, including the main session.

### 8. Two parts, two distribution channels

| Part | Contents | How it gets into a project | How to update |
|---|---|---|---|
| `plugin/` | agents, skills, slash commands, hooks, workflow templates, the `awf` executable (`bin/`) | Install the Claude Code plugin once per machine, shared by all projects | `claude plugin update agent-workflow@agentworkflow`, then `/reload-plugins` |
| `plugin/scaffold/` | Hard rules, guard settings, schemas, development methodology profiles, golden-set, settings, `CLAUDE.md` template | `awf init` copies it into the target project and replaces `{{placeholders}}` | Run `awf init --upgrade` in the project |

Since 0.5 the workflow templates are also in the plugin (`plugin/workflow-templates/`) and are not copied into the project: the state machine `awf next` generates a script for each stage from the template plus the project's settings, so the flow updates along with the plugin. Since 1.0 the tools are not copied into the project either: `awf` is provided by the plugin's `bin/`, and the project has no copy of the tool. The settings to be copied into the project (`scaffold/`) also live in the plugin directory, so once the plugin is installed you can run `awf init` directly without cloning this repo. A project still has to run init once, because the rules, guard settings, and development methodology profile are specific to each project.

You can add your own agents to any of the plan, design, test, and deliver stages (`/agent-workflow:add-agent`), and you can replace the whole development methodology profile; see [`docs/自訂與擴充.md`](docs/自訂與擴充.md).

**Adding your own constraints**: register the documents you want agents to follow (architecture `ARCHITECTURE.md`, design `DESIGN.md`, frontend and backend standards, review `REVIEW.md`, and so on) in the profile's `constraints`, and specify which stages and paths they apply to. The workflow gives them only to the agents they match (a frontend standard is not sent to a subtask that only changes the backend), and a gate can require a point-by-point cross-check or run a check command you specify.

## Requirements

- Claude Code
- The tool is the `awf` executable bundled in the plugin; no interpreter or package needs to be installed separately. **Right now the plugin bundles only the macOS arm64 (Apple Silicon) executable.** The code supports x64 and arm64 on macOS, Linux, and Windows, but the executables for the other platforms have not been published yet, so you have to build it yourself first (this needs a Rust toolchain); see "Building it yourself" below and [`docs/建置與發佈.md`](docs/建置與發佈.md).
- git; if you want to open PRs or merge, you also need `gh`.
- Node.js 18 or later, needed only for dry-run validation of workflows.

## Installation

### 1. Install the plugin (once per machine)

Open Claude Code in any directory and run:

```
/plugin marketplace add https://github.com/s96cax98leo/AgentWorkFlow
/plugin install agent-workflow@agentworkflow
```

You can also run these in a terminal with the same effect:

```bash
claude plugin marketplace add https://github.com/s96cax98leo/AgentWorkFlow
claude plugin install agent-workflow@agentworkflow
```

- The repo is public; no GitHub login is needed. The URL can also be shortened to `s96cax98leo/AgentWorkFlow`.
- The marketplace name is `agentworkflow` and the plugin name is `agent-workflow`.
- By default it installs at the user level (scope: user), so it is available when you open Claude Code in any directory. After installing, use `/plugin` to confirm that the agents, skills, and commands are loaded.

**When installation fails**

| Symptom | Cause | What to do |
|---|---|---|
| `Cannot add marketplace "agentworkflow": its source doesn't match its extraKnownMarketplaces entry…` | This machine previously added a marketplace of the same name, `agentworkflow`, from a local folder (for example a cloned path), and the source recorded in the user settings differs from the GitHub URL this time | First run `claude plugin marketplace remove agentworkflow`, then add and install again as above. Remove also removes the plugins installed from it, so install must be redone |
| `/plugin marketplace list` shows the Source of `agentworkflow` as `Folder (…)` | Same as above: the local-folder version is in use and will not follow GitHub updates | To switch to the GitHub version, handle it as in the previous row; to keep the local version, do nothing |
| Installed, but the agents or commands do not appear | Not reloaded yet | Run `/reload-plugins`, or restart Claude Code |
| Not visible in a particular project | It was installed at project level (`--scope project` or `local`) | Install at user level with `claude plugin install agent-workflow@agentworkflow --scope user` |

### 2. Apply the template to the target project (once per project)

After the plugin is installed, run this in Claude Code (the plugin's `bin/` is already on Claude Code's PATH):

```bash
awf init --target ~/work/my-project
```

You do not need to clone this repo: the settings that `awf init` copies are in the plugin directory, and it finds them by itself. You can also run `/agent-workflow:init-workflow` to have it ask the questions in the conversation and then run init for you.

When running in your own terminal (not inside Claude Code), `awf` is not on the PATH, so write the full path to the plugin's `bin/awf` (under `~/.claude/plugins/`, in this plugin's directory), or add that `bin/` to your PATH. The commands below are all written as `awf …`.

init first asks for the value of each placeholder, then confirms your development habits: the development methodology profile, whether tests come first, the code style tool or command, the commit format, the branch prefix, the PR size limit, and the document format. Every question has a default; just press Enter to accept it. You can also run `/agent-workflow:init-workflow` in Claude Code to have it ask in the conversation and then run init for you.

init writes `.claude/teams/VERSION` (the template version applied) and `.claude/teams/workflow.config` (the placeholder values for this run) into the project; later, `--upgrade` and `--force` reuse them when `--config` is not given.

To validate the output, run this in the target project root (the "next steps" printed at the end of `awf init` give the same commands with the actual paths):

```bash
awf doctor
awf render --all --out _tpm/wf-check
awf check-agents --plugin-dir <plugin 目錄>
node <plugin 目錄>/workflow-templates/check_workflows.mjs --project . --wf-dir _tpm/wf-check   # dry run, requires Node.js
```

### Building it yourself (optional)

If the bundled executables do not include your platform, or you want to use a development version, clone this repo and build it yourself (you need the Rust toolchain and nothing else):

```bash
git clone https://github.com/s96cax98leo/AgentWorkFlow.git ~/tools/AgentWorkFlow
cd ~/tools/AgentWorkFlow
cargo build --release -p awf
target/release/awf init --target ~/work/my-project
```

The built executable looks upward from its own location to find this repo's `plugin/` as the template. To make the plugin's launcher (`plugin/bin/awf`) use it, point the environment variable `AWF_BIN` at `target/release/awf`. How to build and verify the executables for the five release platforms is in [`docs/建置與發佈.md`](docs/建置與發佈.md).

## Quick start

```
/sdlc Add an "Export CSV" button to the order list that exports the currently filtered results
```

The main session first lists the plan: the event type, workflow, parameters, how many agents it expects to dispatch, and which actions are irreversible. **Nothing starts until you approve.**

The first step of `sdlc-plan` is triage: it decides which kind of requirement this is (with evidence) and suggests a recipe. You confirm the intent and recipe at the plan gate, and afterward each stage follows the recipe. Taking `feature` as an example:

| Stage | workflow | What it does | At the end |
|---|---|---|---|
| 1 plan | `sdlc-plan` | triage classifies; each research angle first locates line ranges, then analyzes (angles whose sources have not changed reuse the last conclusion); spec-author writes `intent.md`; skeptic challenges it | Gate: approve intent and recipe |
| 2 design | `sdlc-design` | First an outline to judge the size; if too large, split into sub-plans planned in parallel and then consolidated; produces spec, plan, subtasks, and file ownership, and skeptics check consistency in parallel | Gate: approve spec + plan (to request changes, use `sdlc-design-revise`) |
| 3 build | `sdlc-build` | Writes tests first or after, per the profile; subtasks are layered by dependency, each subtask in a layer is implemented in parallel in its own worktree, and the integrator merges each layer | On interruption it goes to a human; re-run with the remaining subtasks |
| 3.5 sync | `sdlc-sync` | `awf sync-branch` decides how to catch up with main: already caught up means no change; never pushed and no conflicts means rebase; already pushed and no conflicts means merge main; with conflicts it goes through a transit branch and opens a PR to replace the original | If head changed, test re-verifies |
| 4 test | `sdlc-test` | verifier checks each criterion and does anti-cheating checks, adds design and security review per the recipe; on failure it fixes, at most 2 rounds | Gate: confirm the verification results |
| 5 deliver | `sdlc-deliver` | Reviews the PR along the recipe's review axes, skeptic rebuts each finding, writes a draft PR description | Gate: you decide at that moment whether to push and open the PR |
| 6 merge | `sdlc-merge` | When auto_merge is enabled, the steward checks the requirement against the evidence (bound to the head at review time), and `awf merge-gate` decides whether it can be merged | After merge, `event-post-merge` monitors |

For a complete walkthrough example, see [`docs/套用範例.md`](docs/套用範例.md).

## Routing by requirement type (recipe)

There are 11 built-in recipes, defined in the profile's `recipes`, which can be overridden:

| recipe | Used for | Stages run | Focus of plan | Measured dispatch count (plan→deliver) |
|---|---|---|---|---|
| `feature` | New feature | All 7 stages | Ordinary intent | 31 |
| `bugfix` | Wrong behavior | No design | Symptom, reproduction, root cause | 17 |
| `change` | CR, changing an existing feature | All 7 stages | Which existing ACs to change, keep, or delete | 30 |
| `hotfix` | Production incident | No design, no sync | Same as bugfix; a follow-up run must always be opened | 13 |
| `refactor` | Refactoring without changing behavior | No design | Behavior that must not change | 22 |
| `deps` | Dependency upgrade | No design, security review forced | Upstream changes and breaking changes | 21 |
| `docs` | Docs, settings, copy only | plan, build, deliver, merge | Research | 12 |
| `test-only` | Adding tests, fixing flaky tests | No design, no sync | Research | 16 |
| `perf` | Performance | All 7 stages | Measure a baseline | 28 |
| `spike` | Research, feasibility | plan only, cannot push | Research question and criteria | 8 |
| `revert` | Rolling back after merge | No design, no sync | Reason for rollback, scope, conditions for restoring | 16 |

- **Can only be tightened**: triage only suggests; after a human approves at the plan gate it is locked in run.json; afterward it can only be tightened with `awf save-run escalate`, never downgraded. Modifiers (`ui`, `security`, `migration`, `public_api`, `large`) layered on top also only tighten; for example, `migration` forces a design stage and a rollback check.
- **Cannot be skipped**: the intent (four required sections plus the type-specific sections), human approval of plan, rules.md, every agent's run record, merge_gate, and push to the fork only.
- **Cross-check against the diff**: `awf publish` and `awf merge-gate --recipe` judge from the files actually changed whether the recipe is enough. For example, docs but code was changed, no design but a migration was touched, or `.claude/` or `.github/` was changed without a security review: all are blocked, with a hint to use escalate. The CI template has no run.json, so this check takes effect only in local publish and the steward's gate.
- **Run record**: each agent writes its record at `<run_dir>/<stage>/<label>.md`, and `awf save-run stage` produces `執行紀錄索引.md`. For long output such as tests, use `awf run-brief` to condense it before reading, and keep the full log separately.
- **How it was measured and its limits**: the dispatch counts come from the dry run of `cargo xtask check-budget` (fake return values; bugfix and hotfix use 2 subtasks, docs uses 1, the others use 4), and actual numbers vary with the number of subtasks and review findings. The dry run measures "how many agents are dispatched" and "how many characters the task briefs have", and cannot measure the tokens agents actually spend reading files and writing output.

For details see [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on routing by requirement type (依需求類型分流).

## Core concepts

**Gate**: at the end of plan, design, test, and deliver, the main session lists the highlights in the conversation and waits for the decision maker to approve; approval is recorded with `awf save-run approve`. plan is always approved by a human; other stages can use policy approval (`--by policy`) instead, in which case the tool reads the stage's actual result: verification items that did not pass, anti-cheating findings, blocking open questions, missing agent handoffs, missing run records, or an interrupted workflow all prevent automatic approval. When a requirement is cancelled, record it with `awf save-run abandon`; after that no stage can be approved and nothing can be pushed.

**Handoff envelope**: every `dev-*` / `tpm-*` agent hands off in a JSON envelope with the fields status, summary, artifacts, evidence, confidence, open_questions, derived_work, payload. Results are written as files and the envelope carries only paths; summary is at most 400 characters and evidence at most 12 items. A hook validates the envelope, and a non-conforming one cannot be handed off.

**Development methodology profile**: each stage's practice, the conventions, and the recipes are written in the target project's `.claude/teams/methodology.json`. Two are built in: `default` (spec-driven, tests first, Conventional Commits) and `lightweight` (requirements come from issues, tests added after implementation, free format). Changes to this file take effect when the next stage starts.

**Parallel processing**: the trigger conditions are in the profile's `parallelism`: `split_threshold` (how many estimated subtasks trigger splitting into sub-plans, default 8), `max_sub_plans` (the sub-plan limit, default 4), and `shard_verification` (whether verification is sharded in parallel when there are sub-plans, default on). If sub-plan scopes overlap, subtasks are silently dropped in consolidation, or verification results are changed in consolidation, the workflow stops and goes to a human.

**Trusted auto-merge** (off by default):
- `dev-merge-steward` checks the requirement against the evidence item by item, with the evidence bound to the head SHA; when they do not match, it produces a follow-up new requirement and sends the PR back.
- `awf merge-gate` re-checks the mapping table, required checks, test count, risk level, daily limit, allowlist, and freeze flag with deterministic rules, and writes the result to the audit log.
- No agent can approve a PR.
- The real trust boundary is on the server side: the GitHub ruleset, the gate App's required check, and CODEOWNERS. Before enabling it, complete the setup following the checklist in [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md).

**Adding your own agents**: run `/agent-workflow:add-agent` in Claude Code. It asks which stage to add it to, which template role's guard rules to apply, and what the dispatched work is, and creates the agent definition file for you if needed. The settings are written in `extensions` in `methodology.json` and `extension_agents` in `guard.config.json`. An extension agent runs in parallel with the stage's built-in agents, and its blocker / important findings block policy approval. Only read-only roles can be applied; the build and merge stages are not open to extension.

**Adding your own stages**: declare a custom stage in the profile's `flow` (for example customer acceptance after test); it runs only when a recipe names it, and it joins the whole gate chain. The content can be a script you write yourself (`run: "script"`), or a sequence of steps declared as data (`run: "steps"`: fanout, challenge, write, loop, check, executed by the plugin's `flow-steps` template; the `check` and loop exit commands are run by `awf` itself at save time). A stage declared with steps can be set to policy approval; all others always wait for a human. Event types can also use the same steps, or use `extends` to change only part of a built-in type. See [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on custom stages (自訂段), and [`docs/事件處理.md`](docs/事件處理.md).

**How human decisions feed back into the flow** (1.2):
- When a stage only needs a human to answer questions, there is no need to rerun it: `awf approve --run <run> <stage> --by <person> --answer "#N=<answer>"`. Every blocking question must be answered; a stage passed this way can no longer be approved by policy.
- When the answers change the intent, run the plan revision round with `awf next --revise`: only the intent author and the skeptic are dispatched again, and the research is not redone.
- Hand-edited generated scripts or result files, manually supplied results (`awf done --output`), and stages run with a custom script are all marked as non-standard results and can only be approved by a human.
- **Proceed on the recommendation** (off by default, `decisions.auto_accept`): low-risk open questions go ahead on the agent's recommendation and are recorded as unconfirmed; until a human confirms them, policy approval, publishing, and merging are all refused.
- **Local merge** (`conventions.merge.mode: local`): for projects that do not use PRs, a human merges and then runs `awf next --confirm-merge --by <person>`, and the tool checks that the merge commit equals the reviewed content before closing the run. Agent merges still go only through PRs and the merge gate.

**Other adjustable items** (1.2, all set in the profile):
- **Profile inheritance**: `extends` points to a built-in profile or another one under `.claude/teams/`, so you only write the parts you change.
- **Task brief language**: `prompt_lang` (`zh-TW` or `en`) switches the language of the common prefix, and `prompt_overrides` can rewrite individual sentences. The body of the built-in templates' task briefs and the fixed headings that the gates check are still in Chinese for now.
- **Turn and read budgets**: `limits.turns` reminds an agent to hand in its work when it has run too many turns (by default it only reminds and does not block); `limits.mode` turns on the read budget (off by default).
- **Deliverable templates**: deliverables can use your own Markdown template, and the sections are generated from it; see [`docs/交付文件.md`](docs/交付文件.md), the section on using your own template (用你自己的範本).

**Guard hooks**: there is a single entry point, `awf hook`.
- `guard-shell` covers both the Bash and PowerShell tools and blocks pushing, writing to upstream, pushing protected branches, and every merge and approval path: `gh pr merge`, `--approve`, `gh api`, GraphQL, curl, Invoke-RestMethod.
- `guard-paths` blocks unauthorized writes, and reading secret files (`.env`, keys, credentials) with Read and Grep. Roles that may only write records may write only `.md` files in runs_dir; no template agent can modify `.claude/teams/`, run.json, or the run record index.
- `envelope` validates handoff envelopes.
- Role rules apply only to the template's own agents; your existing agents are unaffected.
- Trade-offs on failure: when a hook cannot run (executable not found, timeout), it lets the action through so the tool is not locked up; when it runs but the input is broken or a command exceeds 128KB, it blocks; when loading the guard settings fails, it falls back to the built-in minimal rules. To block even when it cannot run, use `awf init --hooks project`.
- Hooks are only a local speed bump and can be bypassed; the real trust boundary is on the server side, see [`SECURITY.md`](SECURITY.md).

## Commands and events

| What you say | workflow or skill | Required parameters |
|---|---|---|
| `/sdlc <requirement>`, develop, fix a bug, change a requirement | `sdlc-plan` → `sdlc-design` → `sdlc-build` → `sdlc-sync` → `sdlc-test` → `sdlc-deliver` → `sdlc-merge` (which stages actually run depends on the recipe) | repo, epic, requirement, base, run_dir |
| `/brief <brief>`, work from a task sheet | `event-brief` (code subtasks are routed into the SDLC) | brief, run_dir |
| New upstream PR, main moved forward, CI is red | `event` (type `upstream`) | what, repo, run_dir |
| Organize a meeting, transcript | `event` (type `meeting`) | source, run_dir |
| Review a PR, review a branch | `event-pr-review` | target, repo, run_dir |
| Requirement updated | `event` (type `reqs-change`) | change, run_dir |
| Someone decided, finalized | `event` (type `decision`) | decision, run_dir |
| `/demo-check`, before a demo, rehearsal | `event` (type `demo-check`) | target, run_dir |
| Branch is behind main, PR has conflicts | `sdlc-sync` | repo, epic, run_dir |
| Merge this PR | `sdlc-merge` | repo, pr, run_dir |
| An event you defined yourself (for example an incident review) | `event` (type written in `events` in `methodology.json`) | That type's `args`, run_dir |
| main is red after a merge | `event-post-merge` | repo, run_dir |
| `/agent-workflow:add-agent`, add my own agent | `add-agent` skill | none |
| `/agent-workflow:correct`, turn this mistake into something that cannot be done | `correct` skill | Description or source of the mistake |
| `/agent-workflow:status`, where is it now | `status` skill | Run directory (optional) |
| `/agent-workflow:cost`, how many tokens were spent | `cost` skill | Run directory (optional) |
| `/agent-workflow:kb`, where is this written in the docs | `kb` skill | The question to look up |
| `/agent-workflow:doctor`, check the installation | `doctor` skill | none |
| `/agent-workflow:awf`, which `awf` command do I need | `awf` skill (a table of every subcommand: whether it writes, who decides) | what you want to do, or a subcommand name |
| `/agent-workflow:init-workflow`, set up this project | `init-workflow` skill | none |
| `/agent-workflow:settings`, `/agent-workflow:settings-missing`, change settings, fill in what is not configured | `settings`, `settings-missing` skills | none |
| where is this run stuck, summarize the key points before I approve, where did the cost go | lookup agent `awf-lookup` (read-only agent, dispatched directly by the main conversation) | none |
| rebuild the index, update the knowledge graph | refresh agent `awf-refresh` (can run only a fixed set of refresh commands) | none |
| `/agent-workflow:deliverables`, which documents to deliver, inventory before closing | `deliverables` skill | configure, status, check, or what to do |
| Compare options, A or B | `team-debate` skill | none |
| Analyze a website | `site-report` skill | URL |

`/event <description>` determines the event type automatically. The full rules for event classification and gates are written in the `tpm-team` skill.

The plugin provides 4 commands: `/sdlc`, `/event`, `/brief`, `/demo-check`. Everything else is a skill, invoked as `/agent-workflow:<skill name>` (for example `/agent-workflow:add-agent`); per the [Claude Code docs](https://code.claude.com/docs/en/skills), the `agent-workflow:` prefix can also be omitted when no other command has the same name. `status`, `cost`, and `doctor` share names with Claude Code built-in commands, so use the full name.

### Subcommands added in 1.0

The flow calls some of them automatically (for example the context pack queries the knowledge graph, and save-run cross-checks the spec); when you want to run them yourself, this is the entry point and description. Every subcommand has `--help`.

| Subcommand | What it does | Docs |
|---|---|---|
| `awf kg` | Knowledge graph: connects specs, documents, the link ledger, and the code index into one graph (`build`, `locate`, `trace`, `impact`, `check`); context packs and task briefs carry locations. An auxiliary feature that you may skip building | [`docs/知識圖.md`](docs/知識圖.md) |
| `awf spec` | Spec layer: reads the spec directory (current specs and changes), `status`, `validate`, `show`, `apply`; when the requirement source is set to `openspec`, gates use it for cross-checking | [`docs/規格層.md`](docs/規格層.md) |
| `awf program` | Multi-requirement scheduling: reads `<runs_dir>/program.json` and lists each requirement's status, what blocks it, and which ones can start now (`status`, `next`; read-only) | [`docs/監控與整合.md`](docs/監控與整合.md), the section on multi-requirement scheduling (多需求排程) |
| `awf scrub` | De-identification scan: scans file contents and path names with rules the project defines itself. Off by default | [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on de-identification scan (去客戶化掃描) |
| `awf ci` | The project's test / lint CI: generates GitHub Actions files from the profile's `ci` (`render` only prints, `--write` writes; `check`). Off by default | [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on the project's CI (專案的 CI) |
| `awf golden` | Runs the project's golden-set: positive and negative examples of the handoff envelope, the various payloads, and the profile. Use it after changing the schema or the profile format | [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on adding a payload (新增 payload) |
| `awf tokens` | Design-system token checks: scans for hard-coded color codes (`check`) and cross-checks `tokens.css` against the source of truth (`verify`) | [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on the design-system skill (設計系統 skill), and the `design-system-template` skill |

## Roles

| Group | Role | Responsibility |
|---|---|---|
| SDLC | `dev-spec-author` | Writes intent; produces the outline, plans or consolidates sub-plans; writes spec and plan, splits subtasks and file ownership |
| | `dev-acceptance-tester` | Writes tests from the acceptance criteria (tests first or after is decided by the profile), writes tests only |
| | `dev-implementer` | Implements one subtask in its own worktree, changing only the files it owns |
| | `dev-integrator` | Merges subtask branches into the integration branch in dependency order; conflicts are handled and classified through a transit branch |
| | `dev-verifier` | Verifies each criterion at the current HEAD and does anti-cheating checks, reports only |
| | `dev-design-reviewer` | When there are UI changes, tests design and accessibility in a browser, reviews only and does not change |
| | `dev-security-reviewer` | When there is a security aspect, does threat model, permissions, secrets, and supply chain review, reviews only and does not change |
| | `dev-merge-steward` | The trusted merge agent: checks the requirement against the evidence, merges only when the gate passes, sends the PR back on mismatch; never approves |
| | `dev-release-watcher` | Post-merge monitoring: attributes failures, proposes a revert and a freeze recommendation, writes only its own run record |
| Events | `tpm-planner` | Splits an event into subtasks and a dependency graph; in sdlc-plan it also does triage to decide the requirement type |
| | `tpm-researcher` | Researches one specific question, with conclusions citing `path:line` or a command; in upstream mode it analyzes upstream read-only: predicts conflicts, impact scope, and the overlap with our branch |
| | `tpm-reviewer` | Reviews a PR or branch along a specified aspect and leaves suggestions only; deliver mode reviews the whole integration branch along the recipe's review axes and reports only findings with confidence ≥ 0.8 |
| | `tpm-skeptic` | Dedicated to rebutting others' findings; when unsure, the rebuttal stands |
| | `tpm-writer` | Writes finalized material into documents, then runs `awf check-rules` |
| Website analysis | `tech-architect`, `infra-analyst`, `biz-analyst`, `perf-auditor`, `competitor-scout`, `lead-synthesizer` | Responsible respectively for technical architecture, infrastructure, business model, performance, and competitors; finally lead-synthesizer cross-validates and synthesizes the overall report |
| Data helpers | `awf-lookup`, `awf-refresh` | Lookup agent: finds data using read-only `awf` subcommands and file reads, organizes it, and reports (modes: diagnosis, gate summary, cost, deliverables inventory, settings health check). Refresh agent: rebuilds the knowledge base index, updates the knowledge graph, and rewrites the deliverables master index. Both only read the project's files, do not hand off envelopes, and are dispatched directly by the main conversation |

The above are 14 core roles (9 dev, 5 tpm), 6 website analysis agents, and 2 data helpers. There are also 2 legacy-name shims, kept for compatibility for one version and removed in the next: the legacy name `dev-pr-reviewer` has been merged into `tpm-reviewer`, and the legacy name `tpm-upstream-analyst` has been merged into `tpm-researcher`. Projects still using the legacy names should run `awf init --upgrade`.

## Updating and pinning versions

**Update the plugin**:

```bash
claude plugin update agent-workflow@agentworkflow   # or update from the /plugin UI in Claude Code
```

After updating, run `/reload-plugins` in Claude Code to load the new version. Running only `/plugin marketplace update agentworkflow` updates only the marketplace's catalog list and does **not** update the installed plugin.

**Update an already-init project**: a plugin update does not touch the scaffold files in the project, so each project must run this separately:

```bash
# First update the plugin as above (scaffold lives in the plugin, so the new template is only available after the plugin is updated)
awf init --target . --upgrade [--adopt-new-fields]
```

Upgrading does not wipe your settings; at the end it lists the topics that are not yet configured, which you can fill in with `/agent-workflow:settings-missing`. Run `awf doctor` once after upgrading.

For the complete steps for upgrading from 0.x to 1.0, see [`docs/升級到1.0.md`](docs/升級到1.0.md).

`--upgrade` replaces the template files, keeps your settings, and lists the new fields missing from the profile; `--adopt-new-fields` adds those fields to your `methodology.json`. If you are upgrading from 0.2.x and want recipe routing, or from 0.3.x and want model tiering, the read budget, and the knowledge layer, add `--adopt-new-fields`. For details see [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on upgrading an already-init project (升級已 init 的專案).

**Pinning versions and installing safely**:

1. Before updating, read [`CHANGELOG.md`](CHANGELOG.md), especially the Breaking changes (破壞性變更) and Upgrade steps (升級步驟) sections, and the changes under `plugin/hooks/`.
2. To control when updates happen, do not add the marketplace from GitHub; instead clone, check out a version tag, and add it from the local path:

   ```bash
   git clone https://github.com/s96cax98leo/AgentWorkFlow.git ~/tools/AgentWorkFlow
   git -C ~/tools/AgentWorkFlow checkout v0.3.0   # replace with the tag you want; `git tag` lists released versions
   claude plugin marketplace add ~/tools/AgentWorkFlow
   claude plugin install agent-workflow@agentworkflow
   ```

   To upgrade, check out the new tag, then run `claude plugin marketplace update agentworkflow` and `claude plugin update agent-workflow@agentworkflow`. A local path and the GitHub URL are mutually exclusive; to change the source, first run `claude plugin marketplace remove agentworkflow` (see "When installation fails" under "Installation").
3. Keep auto-update turned off for third-party marketplaces.
4. For the security policy and how to report vulnerabilities, see [`SECURITY.md`](SECURITY.md).

## init options

```
awf init --target <專案> [--config workflow.config] [--answers <答案.json>] [--defaults]
                [--hooks plugin|project] [--with-mcp] [--with-merge-gate]
                [--upgrade] [--adopt-new-fields] [--force] [--overwrite-user-files]
awf init --detect --target <專案> [--json] [--unset]              # detect and print only
awf init --target <專案> --reconfigure [<主題>] [--unset] [--answers <檔>]   # change the settings of a project that is already set up; --unset handles only topics not yet configured
```

To change the settings of a project that is already set up, the easiest way is `/agent-workflow:settings` (asks everything again) or `/agent-workflow:settings-missing` (asks only what is not yet configured) in Claude Code.

| Option | Description |
|---|---|
| `--config` | Reads placeholders from a config file (copy `workflow.config.example` from the plugin directory to `workflow.config` and edit it); if not given, it asks interactively |
| `--answers` | An answers file (JSON) for the development methodology and conventions, no interaction, suited to CI; see [`docs/自訂與擴充.md`](docs/自訂與擴充.md) for the format |
| `--defaults` | Use all default values, with the default profile |
| `--detect [--json]` | Only detect the project's current state and list suggestions by topic, writing no files |
| `--reconfigure` | Change settings of an already-applied project: uses the last answers (`.claude/teams/init-answers.json`) as defaults and changes only the settings that differ |
| `--reconfigure <topic>` | Re-ask only one topic and neither ask nor touch the others (`--reconfigure=<topic>` also works; with `--answers`, the answers file may contain only that topic's keys). Topics that can be asked on their own: `requirements`, `models`, `scrub`, `links`, `ci`, `build_loop`, `auto_merge`, `hooks`, `knowledge`, `deliverables`, `parallelism`, `guard`, `main_session`; the questions on development methodology, conventions, constraint documents, and the read budget are interrelated, so ask a whole round without a topic. For descriptions of the topics see `awf init --detect` |
| `--hooks plugin` | Default. Hooks are registered by the plugin |
| `--hooks project` | Hooks are instead registered in the project's `settings.json`, with the executable's absolute path written in. **Recommended on Windows.** Mutually exclusive with plugin registration |
| `--with-mcp` | Generates a real `.mcp.json`; without it, only `.mcp.json.example` is generated |
| `--with-merge-gate` | Puts in `.github/workflows/agent-merge-gate.yml`. Not included by default, because it makes every PR fail before the gate App is set up |
| `--upgrade` | Upgrades an already-init project (the recommended way to update): template files are replaced with this version, the user's settings are kept, and new fields missing from the profile are listed. When `--config` is not given, it reuses `.claude/teams/workflow.config` (`--force` does the same) |
| `--adopt-new-fields` | Used with `--upgrade`: adds the missing profile fields to `methodology.json` without changing existing values |
| `--force` | Overwrites existing template files; the rules for keeping user settings are the same as `--upgrade` |
| `--overwrite-user-files` | Also overwrites the user's own files (`CLAUDE.md`, `settings.json`, `methodology.json`, `guard.config.json`, the project rules section) as a whole; use only when you want to reset |

init generates: `.claude/teams/` (including `guard.config.json`, `methodology.json`, `VERSION`, `workflow.config`), `.claude/settings.json`, `CLAUDE.md`, `.mcp.json.example`, and the run directory.

### Placeholders

| Placeholder | Meaning | Default |
|---|---|---|
| `{{PROJECT_NAME}}`, `{{CLIENT_NAME}}` | Project name, generic customer name | `my-project`, `示範客戶` (demo customer) |
| `{{REPOS_DIR}}` | The directory where our clones live (relative to `--target`) | `repos` |
| `{{UPSTREAM_REPO}}`, `{{FORK_REPO}}` | Upstream (not writable), our fork (the push target) | none |
| `{{FORBIDDEN_REPOS}}` | Directories or repos that must never be touched | none |
| `{{OUTPUT_LANGUAGE}}` | The agents' output language | Traditional Chinese |
| `{{TASK_ID_PREFIX}}`, `{{TASK_AUTHORITY}}` | Task ID prefix, the upstream system responsible for issuing IDs | `TASK-`, task management system |
| `{{DECISION_OWNER}}`, `{{STAKEHOLDER}}` | The person who makes the final call, the requester | the user, the requester |
| `{{NOTIFY_HOW}}` | How to notify a person when a decision is needed (a sentence written into rule 11 of rules.md; empty means asking in the conversation is enough). Sending progress events to an external command is a separate matter, set with the profile's `progress.notify` | Push to phone (PushNotification) |
| `{{BRANCH_PREFIX}}`, `{{ALLOWED_BRANCH_PREFIXES}}` | Development branch prefix, allowed prefixes for new branches | `feature/`; `feature/,bugfix/,hotfix/,release/,merge/` |
| `{{PROTECTED_BRANCHES}}` | Protected branches (`protected_branches` in `guard.config.json`; no direct push, merges go through a PR), comma-separated; empty means `main,master` | `main,master` |
| `{{BASE_BRANCH}}`, `{{BASE_REMOTE}}` | The mainline branch and the remote it lives on (`base_branch`, `base_remote` in `guard.config.json`): when no `base` is given, each stage's comparison baseline is `<remote>/<branch>` and the PR's target branch is the mainline branch. This is a separate matter from protected branches | `main`, `origin` |
| `{{SPECS_DIR}}`, `{{WORKTREES_DIR}}`, `{{SPEC_TEMPLATES_DIR}}` | In the target repo, the spec directory (`<SPECS_DIR>/<epic>/` holds intent, spec, plan, and the verification report), the worktree directory (a single directory name), and the document template directory, written to `specs_dir`, `worktrees_dir`, `spec_templates_dir` in `guard.config.json`; templates, tools, and hooks all read them. They cannot be absolute paths or contain `..`, and cannot be under `.claude`, `.github`, or `.git`; while the template directory is still at its default, it follows the spec directory | `specs`, `.worktrees`, `specs/_templates` |
| `{{RUNS_DIR}}`, `{{BRIEFS_DIR}}` | Run directory, brief directory | `_tpm/runs`, `_tpm/briefs` |
| `{{DOCS_DIRS}}` | Document directories tpm-writer may write to (written together with the run directory and brief directory as `writer_dirs` in `guard.config.json`), comma-separated; only relative paths inside the workspace are accepted | `docs` |
| `{{ATTRIBUTION_LINE}}` | The attribution line at the end of a commit | `Assisted-by: Claude` |
| `{{DESIGN_SYSTEM_NAME}}` | Design system name | `my-design-system` |
| `{{AGENT_NS}}` | The namespace prefix used when calling agents | `agent-workflow:` |
| `{{METHODOLOGY}}` | Development methodology profile: a built-in name or the path to your own JSON | `default` |

In interactive mode, `PROTECTED_BRANCHES`, `DOCS_DIRS`, and `BASE_BRANCH` are asked together in the interview's guard topic (not repeated in the placeholder round; `BASE_REMOTE` is not asked, so to change it write it in `workflow.config` or in `guard.base_remote` of the answers file); to change them later use `awf init --reconfigure guard`. `awf init --detect` looks at the default branch of the remote `origin` to find the mainline branch, and suggests `guard.base_branch` if it is not `main`. `SPECS_DIR` is also asked in the guard topic (when the project has no `specs/` but has a directory like `docs/specs/`, `--detect` suggests `guard.specs_dir`); `WORKTREES_DIR` and `SPEC_TEMPLATES_DIR` are not asked, so to change them write them in `workflow.config` or in `guard.worktrees_dir` and `guard.spec_templates_dir` of the answers file.

There are also two more placeholders, `AWF_HOOK_CMD` and `AWF_HOOK_SHELL`, which are the launch command and shell for `--hooks project`; init computes them automatically and you do not fill them in.

### Applying to an existing project

init only adds files to an existing project, and your existing agents, skills, commands, and settings keep working as before:

- It does not touch `.claude/agents`, `skills`, or `commands`.
- An existing `CLAUDE.md` is not overwritten; only one line, `@.claude/teams/agent-workflow.CLAUDE.md`, is appended at the end.
- `settings.json` and `.mcp.json` are deep-merged, and your values take precedence.
- Role rules apply only to agents carrying the `agent-workflow:` namespace and to agents whose names exactly match template roles.

The main session's restrictions are controlled by two switches in `guard.config.json`: `main_session_git_rules` and `main_session_github_rules`. Whatever the switches are set to, upstream and forbidden repos are always blocked; when `auto_merge` is enabled, the main session also can never merge or approve. For details see [`docs/自訂與擴充.md`](docs/自訂與擴充.md), the section on applying to an existing project (套用到既有專案).

## Repo structure

```
AgentWorkFlow/
├── .claude-plugin/marketplace.json   # marketplace (name agentworkflow)
├── .github/                          # this repo's CI (ci.yml) and Dependabot settings
├── plugin/                           # the plugin itself (name agent-workflow); only this directory is present when installed from the marketplace, and it is also the awf init template
│   ├── agents/                       # 24 agents in total: 14 core roles, 2 legacy-name shims, 6 website analysis agents, 2 data helpers
│   ├── skills/                       # tpm-team, awf, init-workflow, settings, settings-missing, add-agent, correct, deliverables, status, cost, kb, doctor, team-debate, the site-report series, design-system-template
│   ├── commands/                     # 4 commands: /sdlc, /event, /brief, /demo-check
│   ├── workflow-templates/           # workflow templates (8 sdlc, 4 event: the shared event plus event-brief, event-pr-review, event-post-merge, and flow-steps, which runs when a custom stage is declared with steps) and legacy.json (fingerprints of legacy copies); awf next generates scripts stage by stage
│   │   ├── snippets/                 # fragments shared by several workflows (awf build embeds them into the templates)
│   │   ├── scenarios/                # dry-run scenario files (one per workflow)
│   │   └── check_workflows.mjs       # Node script that dry-runs all workflows
│   ├── scaffold/                     # what awf init copies into the target project (settings, schemas, profiles, rules, golden-set; no tools)
│   │   ├── .claude/teams/            # rules.md, schemas/, methodologies/, deliverables/, golden-set/, guard.config.json.tmpl
│   │   ├── .github/workflows/        # agent-merge-gate.yml.tmpl (optional)
│   │   ├── CLAUDE.md.tmpl、.mcp.json.tmpl
│   │   └── _runs/                    # run directory notes
│   ├── workflow.config.example       # placeholder values
│   ├── legacy-tools.json             # fingerprints of the tool files 0.x copied into projects (awf init --upgrade uses them to remove unmodified legacy copies)
│   ├── bin/                          # the awf executable (launcher plus per-platform executables)
│   └── hooks/                        # hooks.json: registers awf hook (guard-shell, guard-paths, envelope, session-start, mark-dirty, prompt-context)
├── crates/                           # awf's source (Rust): awf-core (all logic), awf (executable entry point), xtask (build and checks)
├── CHANGELOG.md、SECURITY.md、LICENSE
├── docs/                             # documentation
└── tests/                            # test data: regression baselines (parity/), hook cases (hooks/), de-identification list (scrub_terms.txt), requirement for the benchmark run (bench/)
```

## Documentation

| Document | Contents |
|---|---|
| [`docs/開發模式總覽.md`](docs/開發模式總覽.md) | Overview of the whole way of working: roles, gates, dispatch, envelopes, phone-based decisions |
| [`docs/ai-native-sdlc.md`](docs/ai-native-sdlc.md) | Each SDLC stage's agents, inputs and outputs, gates, worktrees, and the integration branch |
| [`docs/系統設計方法.md`](docs/系統設計方法.md) | The SA / SD method: the four steps from the System Design Primer, feature level and system level, topics for trade-off checks |
| [`docs/事件處理.md`](docs/事件處理.md) | event-* workflows, option debate, website analysis |
| [`docs/agent-協作規則.md`](docs/agent-協作規則.md) | Task brief template, envelope format, background agents, acceptance evidence |
| [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md) | Layers of defense for auto-merge, pre-enable checklist, revert drill |
| [`docs/adr-agent-auto-merge.md`](docs/adr-agent-auto-merge.md) | Architecture decision record for auto-merge |
| [`docs/自訂與擴充.md`](docs/自訂與擴充.md) | Development methodology profile, recipes, parallelism parameters, adding your own agents to a workflow, adding agents / skills / hooks / workflows, applying to an existing project, upgrading |
| [`docs/監控與整合.md`](docs/監控與整合.md) | Viewing progress and cost from outside: `awf watch`, the event stream, notifications, the status line, OTel, multi-requirement scheduling (`awf program`) |
| [`docs/知識圖.md`](docs/知識圖.md) | The knowledge graph (`awf kg`): building the graph, querying, how the flow uses it, automatic hooks |
| [`docs/規格層.md`](docs/規格層.md) | The spec layer (`awf spec`): spec directory format, subcommands, hooking into save-run and gates |
| [`docs/踩坑手冊.md`](docs/踩坑手冊.md) | Lessons learned in practice: symptom, cause, what to do |
| [`docs/參考-pstack.md`](docs/參考-pstack.md) | An external plugin that was consulted (pstack): differences, what was adopted and what was not |
| [`docs/套用範例.md`](docs/套用範例.md) | Going from init to deliver with the fictional "demo customer ACME" |
| [`CHANGELOG.md`](CHANGELOG.md) | Changes and upgrade steps for each version |
| [`SECURITY.md`](SECURITY.md) | Vulnerability reporting, threat model summary, installation advice |

## Developing this template

The tools and tests are all Rust (`cargo`); only a few workflow validation steps need Node.js. The full set of checks is the two commands below, and both must pass:

```bash
cargo test --workspace                # unit tests, regression baselines (tests/parity/), hook cases (tests/hooks/), init smoke test
cargo xtask check-all                 # de-identification scan, golden, doc cross-check, recipe / execution kernel / pattern / dispatch budget, supply chain, launchers, snippet drift, each subcommand's selftest, workflow dry run
```

To run just one of them:

```bash
cargo xtask scrub [路徑…]             # run only the de-identification scan
cargo xtask check-docs                # doc consistency (counts and lists of roles, workflows, commands)
cargo xtask check-recipes             # recipe resolution is identical between the tool side and the JS snippet
cargo xtask check-budget              # dispatch count and task brief length stay within 10% of the baseline
cargo xtask check-supply              # supply chain: actions pinned to SHA, dependency sources and allowlist, Node scripts use only built-in modules
cargo test -p awf --test smoke_init   # run only the init smoke test
cargo test -p awf --test hooks_cases  # run only the hook cases (Bash and PowerShell)
cargo run -q -p awf -- <子指令> --selftest   # selftest of a single subcommand, e.g. state, merge-gate, save-run
```

For the other `cargo xtask` subcommands (golden, check-kernel, check-patterns, check-launchers, gen-legacy, bench, dist, checksums) see `cargo xtask --help`. How to update the regression baselines is in [`tests/parity/README.md`](tests/parity/README.md).

- **CI** (`.github/workflows/ci.yml`): every push and PR runs `cargo build`, `cargo test --workspace`, and `cargo xtask check-all` on macos-latest (this is currently the only platform verified in CI; the executables for the five platforms are built by a separate, manually triggered `dist` job). All actions used by CI are pinned to commit SHAs.
- **Dependabot** (`.github/dependabot.yml`): weekly, it checks whether the GitHub Actions used by CI have new versions and, if so, opens a PR automatically that updates both the SHA and the version comment. These PRs are opened by GitHub's `dependabot[bot]` and are this repo's own configuration, not submitted by outsiders; decide whether to merge only after CI is green and you have read the upstream release notes.
- The **de-identification scan** reads two lists: `tests/scrub_terms.txt` (public generic rules: local paths, credential formats, personal emails) and `tests/scrub_terms.local.txt` (real names to block, not under version control). Examples always use the fictional "demo customer ACME".

Development rules for Claude Code are written in [`CLAUDE.md`](CLAUDE.md).

## License

MIT, see [`LICENSE`](LICENSE).
