use crate::answer::NoulAnswer;
use crate::error::{Error, PolicyError};
use crate::ids::{ActionId, BatteryId, QuestionId};
use crate::verdict::{Decision, UnsureReason, Verdict, hint};
use indexmap::IndexMap;
use serde::Deserialize;

fn blank_action_id() -> ActionId {
    ActionId::new("")
}

fn default_review_below() -> f64 {
    1.0
}

fn default_signal() -> Signal {
    Signal::Confidence
}

fn default_yes_auto() -> f64 {
    0.90
}

fn default_no_auto() -> f64 {
    0.10
}

fn default_cascade_min() -> f64 {
    0.80
}

fn default_battery() -> BatteryId {
    BatteryId::new("tool-gate")
}

/// Thresholds for `gate` and `ask`.
///
/// `shipped`, `from_toml_str`, and `load` are the constructors. They run
/// `finish`, which is the only place that sets `sealed`. A struct literal
/// outside this module cannot name that field.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct Policy {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub fail: Fail,
    #[serde(default)]
    pub shadow: bool,
    #[serde(default = "default_cascade_min")]
    pub cascade_min: f64,
    pub choice: ChoiceGates,
    #[serde(default)]
    pub noul: NoulPolicy,
    #[serde(default)]
    pub default_action: Option<ActionPolicy>,
    #[serde(default)]
    pub actions: IndexMap<ActionId, ActionPolicy>,
    #[serde(default = "default_battery")]
    pub battery: BatteryId,
    /// Set only by [`Policy::shipped`]. A path load leaves this empty.
    #[serde(skip)]
    pub shipped_id: Option<String>,
    /// Set by [`Policy::load`] when the spec is a `.toml` path.
    #[serde(skip)]
    pub source_path: Option<String>,
    /// Set only by `finish` after the invariant checks.
    #[serde(skip)]
    sealed: bool,
}

fn default_schema() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, Deserialize)]
pub struct ChoiceGates {
    pub escalate_below: f64,
    #[serde(default = "default_review_below")]
    pub review_below: f64,
    #[serde(default = "default_signal")]
    pub signal: Signal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Confidence,
    TopProb,
    Margin,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, Deserialize)]
pub struct NoulPolicy {
    #[serde(default = "default_yes_auto")]
    pub yes_auto: f64,
    #[serde(default = "default_no_auto")]
    pub no_auto: f64,
}

impl Default for NoulPolicy {
    fn default() -> Self {
        Self {
            yes_auto: default_yes_auto(),
            no_auto: default_no_auto(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct ActionPolicy {
    #[serde(default = "blank_action_id", skip_deserializing)]
    pub action_id: ActionId,
    #[serde(default)]
    pub auto: Option<f64>,
    pub review: f64,
    pub when_unsure: UnsureVerdict,
    pub class: HarmClass,
    #[serde(default)]
    pub block_on: Vec<Block>,
    /// Host tool name, such as `Bash`. Unset matches only the action id.
    #[serde(default)]
    pub tool: Option<String>,
    /// Command prefixes for `tool`. The first matching prefix row wins for
    /// one simple command. A stricter row later in the same string replaces it.
    #[serde(default)]
    pub prefixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct Block {
    pub id: QuestionId,
    #[serde(default)]
    pub when: BlockWhen,
    #[serde(default)]
    pub yes_auto: Option<f64>,
    #[serde(default)]
    pub no_auto: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockWhen {
    #[default]
    Yes,
    No,
    Unsure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarmClass {
    None,
    Read,
    Write,
    Exec,
    Network,
    Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fail {
    #[default]
    Closed,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsureVerdict {
    ReviewGuess,
    Escalate,
}

pub struct EffectiveGates {
    pub escalate_below: f64,
    pub auto: Option<f64>,
    pub review: f64,
    pub when_unsure: UnsureVerdict,
    pub block_on: Vec<Block>,
    pub harm_bumped: Option<(HarmClass, HarmClass)>,
}

#[derive(Debug, Clone, Copy)]
pub struct NoulObs<'a> {
    pub id: &'a str,
    pub p: f64,
}

impl Policy {
    pub fn shipped(name: &str) -> Result<Self, PolicyError> {
        let raw = match name {
            "tool-gate" => include_str!("../policies/tool-gate.toml"),
            "triage" => include_str!("../policies/triage.toml"),
            "review" => include_str!("../policies/review.toml"),
            "screen" => include_str!("../policies/screen.toml"),
            other => {
                return Err(PolicyError::Config(format!("unknown policy {other}")));
            }
        };
        let mut policy = Self::from_toml_str(raw)?;
        policy.shipped_id = Some(name.to_string());
        Ok(policy)
    }

    /// Bump the arm when that pack's questions or thresholds change.
    pub fn shipped_pack_version(name: &str) -> u32 {
        match name {
            "tool-gate" | "triage" | "review" | "screen" => 1,
            _ => 0,
        }
    }

    /// Shipped id, or a path whose extension is `toml` in any ASCII case.
    pub fn load(spec: &str) -> Result<Self, Error> {
        if Self::toml_extension(spec) {
            let text = std::fs::read_to_string(spec).map_err(|err| {
                Error::Io(std::io::Error::new(err.kind(), format!("{spec}: {err}")))
            })?;
            let mut policy = Self::from_toml_str(&text)?;
            policy.source_path = Some(spec.to_string());
            Ok(policy)
        } else {
            Ok(Self::shipped(spec)?)
        }
    }

    pub fn from_toml_str(raw: &str) -> Result<Self, PolicyError> {
        let policy: Self =
            toml::from_str(raw).map_err(|err| PolicyError::Config(err.to_string()))?;
        policy.finish()
    }

    fn toml_extension(spec: &str) -> bool {
        std::path::Path::new(spec)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
    }

    fn finish(mut self) -> Result<Self, PolicyError> {
        self.pre_invariant_checks()?;
        let Some(mut default_action) = self.default_action.take() else {
            return Err(PolicyError::Invariant("default_action".to_string()));
        };
        default_action.action_id = ActionId::new("default");
        self.default_action = Some(default_action);
        for (key, row) in &mut self.actions {
            row.action_id = key.clone();
        }
        self.check_invariants()?;
        self.sealed = true;
        Ok(self)
    }

    pub(crate) fn ensure_checked(&self) -> Result<(), PolicyError> {
        if !self.sealed {
            return Err(PolicyError::Invariant("unchecked policy".to_string()));
        }
        self.pre_invariant_checks()?;
        self.check_invariants()
    }

    fn pre_invariant_checks(&self) -> Result<(), PolicyError> {
        if self.schema_version != 1 {
            return Err(PolicyError::Schema(self.schema_version));
        }
        if self.choice.escalate_below == 0.0 {
            return Err(PolicyError::MissingUnsure);
        }
        Ok(())
    }

    fn check_invariants(&self) -> Result<(), PolicyError> {
        let e = self.choice.escalate_below;
        in_unit("escalate_below", e)?;
        in_unit("review_below", self.choice.review_below)?;
        if self.choice.review_below < e {
            return Err(PolicyError::Invariant(
                "review_below >= escalate_below".to_string(),
            ));
        }
        in_unit("yes_auto", self.noul.yes_auto)?;
        in_unit("no_auto", self.noul.no_auto)?;
        if self.noul.yes_auto < self.noul.no_auto {
            return Err(PolicyError::Invariant("yes_auto >= no_auto".to_string()));
        }
        in_unit("cascade_min", self.cascade_min)?;
        let cascade_floor = e
            .max(2.0 * self.noul.yes_auto - 1.0)
            .max(1.0 - 2.0 * self.noul.no_auto);
        if self.cascade_min + 1e-12 < cascade_floor {
            return Err(PolicyError::Invariant(
                "cascade_min is below its floor".to_string(),
            ));
        }
        let Some(default_action) = self.default_action.as_ref() else {
            return Err(PolicyError::Invariant("default_action".to_string()));
        };
        check_action(e, self.choice.review_below, default_action)?;
        for action in self.actions.values() {
            check_action(e, self.choice.review_below, action)?;
        }
        Ok(())
    }
}

fn in_unit(name: &str, value: f64) -> Result<(), PolicyError> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(PolicyError::Invariant(format!("{name} must be in 0..=1")))
    }
}

fn check_action(
    escalate_below: f64,
    review_below: f64,
    action: &ActionPolicy,
) -> Result<(), PolicyError> {
    in_unit("review", action.review)?;
    if action.review < escalate_below {
        return Err(PolicyError::Invariant(
            "review >= escalate_below".to_string(),
        ));
    }
    if let Some(auto) = action.auto {
        in_unit("auto", auto)?;
        let mut auto_eff = auto.max(escalate_below);
        if action.class >= HarmClass::Write {
            auto_eff = auto_eff.max(review_below);
        }
        if auto_eff + 1e-12 < action.review {
            return Err(PolicyError::Invariant("auto_eff >= review".to_string()));
        }
    }
    for block in &action.block_on {
        if let Some(yes) = block.yes_auto {
            in_unit("block.yes_auto", yes)?;
        }
        if let Some(no) = block.no_auto {
            in_unit("block.no_auto", no)?;
        }
        if let (Some(yes), Some(no)) = (block.yes_auto, block.no_auto)
            && yes < no
        {
            return Err(PolicyError::Invariant(
                "block yes_auto >= no_auto".to_string(),
            ));
        }
    }
    Ok(())
}

/// Exact action id wins. Otherwise the first simple command whose `tool`
/// matches `tool_name` and whose non-empty `prefixes` match. An empty
/// prefix list does not match, so that row cannot replace `default_action`.
///
/// `rm` matches `rm`, `/bin/rm`, `RM`, `sudo rm`, `sudo -nu root rm`,
/// `FOO=1 rm`, `FOO+=1 rm`, `env rm`, `cd x && rm`, `bash -c 'rm ...'`, and
/// `env -S 'rm ...'`. A null byte is removed first, because bash
/// removes it: `rm` followed by a null and `-rf` matches, and `rm`
/// followed by a null and `dir` stays `rmdir`. `$'rm'` and `$"rm"`
/// match too, because bash runs those words as `rm`. An ANSI-C null
/// ends that word (`$'rm\x00dir'` is `rm`). `echo $'rm'` does not match.
/// `exec rm` and `eval 'rm ...'` match. `exec -a rm echo` and
/// `eval echo` do not. `if`, `then`, `else`, `elif`, `do`, `while`,
/// `until`, `!`, `{`, and `(` are skipped, so `if true; then rm`,
/// `{ rm; }`, and `(rm)` match. `for`
/// is not skipped. `$(rm)`, `echo "$(rm)"`, and a backtick `rm` match.
/// Single quotes do not run `$(rm)`. The stricter row wins across the
/// whole string, so `git push $(rm)` and `git push && rm` match `bash.rm`.
/// `git push $(echo rm)` stays `git.push`.
/// `bash -O extglob -c` and `bash +O extglob -c` match.
/// `find -exec rm`, `find -execdir rm`, `find -ok rm`, and `find -delete`
/// match. `find -name rm`, `find -name -delete`, and `find -exec echo rm`
/// do not. `ssh host rm` and `flock file rm` match. `ssh host` and
/// `ssh -p 22 rm` do not: the word after the options is the destination,
/// not the command. `timeout 1 rm` and `xargs rm` match.
/// `timeout -- rm` keeps `rm` as the duration. `timeout 1 echo rm`
/// and `xargs echo rm` do not match. Homebrew names the same binaries
/// `gtimeout`, `gxargs`, `genv`, `gnice`, `gnohup`, `gstdbuf`, and `grm`.
/// `git push` matches `git push`, `git\tpush`, `/usr/bin/git push`, and
/// `git -C repo push`. `git.exe push` and `rm.exe` match those rows too.
/// One trailing `.exe`, `.cmd`, `.bat`, or `.com` is ignored, in any
/// ASCII case, including on `timeout.exe` and `busybox.exe`. Dots after
/// that suffix are ignored (`git.exe.`), and a quoted trailing space is
/// ignored (`"rm.exe "`).
/// A shell that reads its script from stdin runs a heredoc or a
/// here-string, so `bash <<EOF` and `bash <<< 'rm'` match. `bash
/// /dev/stdin` and `bash -s` do too. `source /dev/stdin` runs that
/// body in the same shell. `cat` and `bash -c` do not. An unquoted
/// `#` comment does not run `$(rm)`. `<(rm)` and `>(rm)` do.
/// A redirect is not a word. `rm>/dev/null`, `>/dev/null rm`, and
/// `2>/dev/null rm` match. `echo rm>/tmp/x` does not, because `rm` is
/// only an argument. `2&>file rm` runs `2`, not `rm`.
/// `f() { rm; }; f`
/// matches. The same definition with no call does not. `cmd /c rm`,
/// `cmd -c rm`, and `powershell -Command rm` match. `powershell rm`
/// matches, because Windows PowerShell defaults a bare word to
/// `-Command`. `pwsh rm` does not: that host defaults the word to
/// `-File`. `pwsh -i -c rm` matches, because `-i` takes no value.
/// `powershell -i xml -c rm` matches, and `xml` is the input format.
/// `pwsh -cwa` joins the command the same way as `-Command`. `-c`
/// stays `-Command`. Words after `-File` are script arguments, so
/// `powershell -File script.ps1 -Command rm` does not match, and the
/// file is not read. `powershell -EncodedCommand`, `-E`,
/// and `-EC` match once that value is decoded from UTF-16LE base64.
/// `-com` is that command. `-co` is not. On `powershell`, bare words
/// after that switch are still the command. A colon glues the value
/// (`-ec:<base64>`, `-com:rm`). A switch that takes one value is
/// skipped, including `-w hidden`, `-ep bypass`, and `-wd`. A value
/// that is not base64, or not UTF-16LE, is not the script.
/// `rmdir`, `git push-all`, `echo rm`,
/// `command -v rm`, `sudo -l`, an empty word, `rmdir.exe`, `rm.exe.bak`,
/// `rm.`, and `git.exe.exe` do not match.
pub fn matched_action<'a>(
    policy: &'a Policy,
    tool_name: &str,
    command: Option<&str>,
) -> Option<&'a ActionId> {
    if let Some(id) = policy
        .actions
        .keys()
        .find(|id| id.0.eq_ignore_ascii_case(tool_name))
    {
        return Some(id);
    }
    let command = command.unwrap_or("").trim_start();
    let mut functions = Functions::new();
    matched_in(policy, tool_name, command, 0, &mut functions)
}

const MAX_SHELL_DEPTH: u32 = 8;

type Functions = std::collections::HashMap<String, String>;

fn matched_in<'a>(
    policy: &'a Policy,
    tool_name: &str,
    command: &str,
    depth: u32,
    functions: &mut Functions,
) -> Option<&'a ActionId> {
    if depth > MAX_SHELL_DEPTH {
        return None;
    }
    let mut best = None;
    for part in tokenize_parts(command) {
        match part {
            ShellPart::Function { name, body } => {
                functions.insert(name, body);
            }
            ShellPart::Segment(segment) => {
                consider_segment(policy, tool_name, &segment, depth, functions, &mut best);
            }
        }
    }
    best
}

fn consider_segment<'a>(
    policy: &'a Policy,
    tool_name: &str,
    segment: &ShellSegment,
    depth: u32,
    functions: &mut Functions,
    best: &mut Option<&'a ActionId>,
) {
    let Some((argv, split_script)) = executed_argv(&segment.tokens) else {
        scan_subshells(
            policy,
            tool_name,
            &segment.substitutions,
            depth,
            functions,
            best,
        );
        return;
    };
    if let Some(script) = split_script {
        let mut child = Functions::new();
        prefer(
            policy,
            best,
            matched_in(policy, tool_name, &script, depth + 1, &mut child),
        );
    }
    let child_script = shell_script(&argv)
        .or_else(|| cmd_script(&argv))
        .or_else(|| powershell_script(&argv))
        .or_else(|| su_script(&argv));
    if let Some(script) = child_script {
        // `pwsh -Command -` reads the heredoc. The dash itself is not a command.
        // `bash -c -` is the script "-", not stdin.
        if is_stdin_script(&script) && is_powershell_base(argv.first().map(String::as_str)) {
            scan_isolated(policy, tool_name, &segment.heredocs, depth, best);
        } else {
            let mut child = Functions::new();
            prefer(
                policy,
                best,
                matched_in(policy, tool_name, &script, depth + 1, &mut child),
            );
        }
    } else if let Some(script) = eval_script(&argv) {
        prefer(
            policy,
            best,
            matched_in(policy, tool_name, &script, depth + 1, functions),
        );
    } else if let Some(body) = argv.first().and_then(|name| functions.get(name).cloned()) {
        prefer(
            policy,
            best,
            matched_in(policy, tool_name, &body, depth + 1, functions),
        );
    } else {
        if let Some(id) = prefix_hit(policy, tool_name, &argv) {
            prefer(policy, best, Some(id));
        }
        if find_deletes(&argv) {
            prefer(policy, best, destructive_rm_action(policy, tool_name));
        }
        let scripts = find_exec_commands(&argv);
        scan_isolated(policy, tool_name, &scripts, depth, best);
        let remotes = remote_scripts(&argv);
        scan_isolated(policy, tool_name, &remotes, depth, best);
        let arms = case_arm_scripts(&argv);
        scan_isolated(policy, tool_name, &arms, depth, best);
        let aliases = git_alias_scripts(&argv);
        scan_isolated(policy, tool_name, &aliases, depth, best);
        if shell_reads_stdin(&argv) {
            scan_isolated(policy, tool_name, &segment.heredocs, depth, best);
        }
    }
    if sourced_stdin(&argv) {
        for body in &segment.heredocs {
            prefer(
                policy,
                best,
                matched_in(policy, tool_name, body, depth + 1, functions),
            );
        }
    }
    scan_subshells(
        policy,
        tool_name,
        &segment.substitutions,
        depth,
        functions,
        best,
    );
}

/// A new process. It does not see shell functions from this string.
fn scan_isolated<'a>(
    policy: &'a Policy,
    tool_name: &str,
    scripts: &[String],
    depth: u32,
    best: &mut Option<&'a ActionId>,
) {
    for script in scripts {
        let mut child = Functions::new();
        prefer(
            policy,
            best,
            matched_in(policy, tool_name, script, depth + 1, &mut child),
        );
    }
}

/// `$(...)` inherits functions and does not publish definitions back.
fn scan_subshells<'a>(
    policy: &'a Policy,
    tool_name: &str,
    scripts: &[String],
    depth: u32,
    functions: &Functions,
    best: &mut Option<&'a ActionId>,
) {
    for script in scripts {
        let mut child = functions.clone();
        prefer(
            policy,
            best,
            matched_in(policy, tool_name, script, depth + 1, &mut child),
        );
    }
}

fn prefer<'a>(policy: &Policy, best: &mut Option<&'a ActionId>, candidate: Option<&'a ActionId>) {
    let Some(candidate) = candidate else {
        return;
    };
    let replace = match *best {
        None => true,
        Some(previous) => action_rank(policy, candidate) > action_rank(policy, previous),
    };
    if replace {
        *best = Some(candidate);
    }
}

/// A destructive block outranks `when_unsure = escalate`, which outranks
/// a higher auto threshold. Equal rank keeps the earlier match.
fn action_rank(policy: &Policy, id: &ActionId) -> (u8, u8, i32) {
    let Some(row) = policy.actions.get(id) else {
        return (0, 0, 0);
    };
    let destructive = u8::from(row.block_on.iter().any(|block| block.id.0 == "destructive"));
    let escalate = u8::from(matches!(row.when_unsure, UnsureVerdict::Escalate));
    let auto = row.auto.unwrap_or_else(|| {
        policy
            .default_action
            .as_ref()
            .and_then(|action| action.auto)
            .unwrap_or(0.8)
    });
    let auto_millis = (auto * 1000.0).round() as i32;
    (destructive, escalate, auto_millis)
}

fn prefix_hit<'a>(policy: &'a Policy, tool_name: &str, argv: &[String]) -> Option<&'a ActionId> {
    for (id, row) in &policy.actions {
        let Some(tool) = row.tool.as_deref() else {
            continue;
        };
        if !tool.eq_ignore_ascii_case(tool_name) {
            continue;
        }
        let hit = row.prefixes.iter().any(|prefix| {
            let words: Vec<String> = prefix.split_whitespace().map(str::to_string).collect();
            !words.is_empty() && argv_matches(argv, &words)
        });
        if hit {
            return Some(id);
        }
    }
    None
}

fn destructive_rm_action<'a>(policy: &'a Policy, tool_name: &str) -> Option<&'a ActionId> {
    policy
        .actions
        .iter()
        .find(|(id, row)| {
            id.0 == "bash.rm"
                && row
                    .tool
                    .as_deref()
                    .is_some_and(|tool| tool.eq_ignore_ascii_case(tool_name))
        })
        .map(|(id, _)| id)
}

fn remote_scripts(argv: &[String]) -> Vec<String> {
    let mut scripts = Vec::new();
    if let Some(script) = ssh_remote_command(argv) {
        scripts.push(script);
    }
    scripts.extend(ssh_proxy_commands(argv));
    scripts.extend(rsync_shells(argv));
    scripts.extend(ncat_commands(argv));
    for script in [
        flock_command(argv),
        docker_command(argv),
        parallel_command(argv),
        kubectl_exec_command(argv),
        chroot_command(argv),
        nsenter_command(argv),
        systemd_run_command(argv),
    ]
    .into_iter()
    .flatten()
    {
        scripts.push(script);
    }
    scripts
}

fn find_exec_commands(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    let base = command_basename(first);
    if !base.eq_ignore_ascii_case("find") && !base.eq_ignore_ascii_case("gfind") {
        return Vec::new();
    }
    let mut commands = Vec::new();
    let mut index = 1;
    while index < argv.len() {
        let flag = argv[index].as_str();
        if matches!(flag, "-exec" | "-execdir" | "-ok" | "-okdir") {
            index += 1;
            let start = index;
            while index < argv.len() && !is_find_terminator(&argv[index]) {
                index += 1;
            }
            if start < index {
                commands.push(argv[start..index].join(" "));
            }
        }
        index += 1;
    }
    commands
}

fn is_find_terminator(token: &str) -> bool {
    matches!(token, ";" | "+" | "\\;")
}

fn find_deletes(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let base = command_basename(first);
    if !base.eq_ignore_ascii_case("find") && !base.eq_ignore_ascii_case("gfind") {
        return false;
    }
    let mut index = 1;
    while index < argv.len() {
        let flag = argv[index].as_str();
        if flag == "-delete" {
            return true;
        }
        if matches!(flag, "-exec" | "-execdir" | "-ok" | "-okdir") {
            index += 1;
            while index < argv.len() && !is_find_terminator(&argv[index]) {
                index += 1;
            }
            index += 1;
            continue;
        }
        let skip = find_predicate_arguments(flag);
        if skip > 0 {
            index += 1 + skip;
            continue;
        }
        index += 1;
    }
    false
}

fn find_predicate_arguments(flag: &str) -> usize {
    if flag == "-fprintf" {
        return 2;
    }
    usize::from(matches!(
        flag,
        "-name"
            | "-iname"
            | "-path"
            | "-ipath"
            | "-regex"
            | "-iregex"
            | "-wholename"
            | "-iwholename"
            | "-lname"
            | "-ilname"
            | "-samefile"
            | "-fstype"
            | "-user"
            | "-group"
            | "-uid"
            | "-gid"
            | "-perm"
            | "-size"
            | "-type"
            | "-xtype"
            | "-context"
            | "-printf"
            | "-fprint"
            | "-newer"
            | "-anewer"
            | "-cnewer"
            | "-mmin"
            | "-amin"
            | "-cmin"
            | "-mtime"
            | "-atime"
            | "-ctime"
            | "-used"
            | "-links"
            | "-inum"
            | "-maxdepth"
            | "-mindepth"
            | "-regextype"
            | "-D"
    ))
}

fn ssh_remote_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("ssh") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            index += 1;
            break;
        }
        if token.starts_with('-') {
            let takes_next =
                ssh_opt_takes_value(token) && !token.contains('=') && !ssh_value_is_glued(token);
            index += 1;
            if takes_next {
                index += 1;
            }
            continue;
        }
        break;
    }
    if index >= argv.len() {
        return None;
    }
    index += 1;
    if index >= argv.len() {
        return None;
    }
    Some(argv[index..].join(" "))
}

fn is_openssh_client(name: &str) -> bool {
    let base = command_basename(name);
    base.eq_ignore_ascii_case("ssh")
        || base.eq_ignore_ascii_case("scp")
        || base.eq_ignore_ascii_case("sftp")
}

fn ssh_proxy_commands(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !is_openssh_client(first) {
        return Vec::new();
    }
    let mut commands = Vec::new();
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" || !token.starts_with('-') {
            break;
        }
        let spec = if token == "-o" {
            index += 1;
            argv.get(index).map(String::as_str)
        } else {
            token.strip_prefix("-o")
        };
        if let Some(command) = spec.and_then(|spec| spec.strip_prefix("ProxyCommand="))
            && !command.is_empty()
        {
            commands.push(command.to_string());
        }
        index += 1;
    }
    commands
}

fn rsync_shells(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("rsync") {
        return Vec::new();
    }
    let mut commands = Vec::new();
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if let Some(command) = token.strip_prefix("--rsh=") {
            if !command.is_empty() {
                commands.push(command.to_string());
            }
        } else if token == "-e" || token == "--rsh" {
            index += 1;
            if let Some(command) = argv.get(index)
                && !command.is_empty()
            {
                commands.push(command.clone());
            }
        }
        index += 1;
    }
    commands
}

fn ncat_commands(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("ncat") {
        return Vec::new();
    }
    let mut commands = Vec::new();
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if let Some(command) = token
            .strip_prefix("--exec=")
            .or_else(|| token.strip_prefix("--sh-exec="))
            && !command.is_empty()
        {
            commands.push(command.to_string());
        } else if matches!(token, "-e" | "--exec" | "-c" | "--sh-exec") {
            index += 1;
            if let Some(command) = argv.get(index)
                && !command.is_empty()
            {
                commands.push(command.clone());
            }
        }
        index += 1;
    }
    commands
}

fn ssh_value_is_glued(token: &str) -> bool {
    let flags = token.strip_prefix('-').unwrap_or(token);
    flags.len() > 1 && !flags.starts_with('-')
}

fn ssh_opt_takes_value(token: &str) -> bool {
    let name = token.strip_prefix("--").unwrap_or(token);
    matches!(
        name,
        "-b" | "-c"
            | "-D"
            | "-E"
            | "-e"
            | "-F"
            | "-i"
            | "-J"
            | "-L"
            | "-l"
            | "-m"
            | "-O"
            | "-o"
            | "-p"
            | "-Q"
            | "-R"
            | "-S"
            | "-W"
            | "-w"
            | "bind_address"
            | "cipher"
            | "dynamic"
            | "logfile"
            | "escape"
            | "config"
            | "identity"
            | "jump"
            | "local"
            | "login"
            | "mac"
            | "option"
            | "port"
            | "query"
            | "remote"
            | "session"
            | "stdio"
            | "tunnel"
    )
}

fn flock_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("flock") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            index += 1;
            break;
        }
        if token == "-c" || token == "--command" {
            return argv.get(index + 1).cloned();
        }
        if token.starts_with('-') {
            let takes_next = flock_opt_takes_value(token) && !token.contains('=');
            index += 1;
            if takes_next {
                index += 1;
            }
            continue;
        }
        break;
    }
    if index >= argv.len() {
        return None;
    }
    index += 1;
    if index >= argv.len() {
        return None;
    }
    if argv[index] == "-c" || argv[index] == "--command" {
        return argv.get(index + 1).cloned();
    }
    Some(argv[index..].join(" "))
}

fn flock_opt_takes_value(token: &str) -> bool {
    let name = token.strip_prefix("--").unwrap_or(token);
    matches!(
        name,
        "-w" | "-E" | "wait" | "conflict-exit-code" | "timeout"
    )
}

fn docker_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("docker") && !base.eq_ignore_ascii_case("podman") {
        return None;
    }
    let sub = argv.get(1)?;
    if sub != "exec" && sub != "run" {
        return None;
    }
    let mut index = 2;
    let mut entrypoint = None;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            index += 1;
            break;
        }
        if let Some(value) = token.strip_prefix("--entrypoint=") {
            entrypoint = Some(value.to_string());
            index += 1;
            continue;
        }
        if token == "--entrypoint" {
            index += 1;
            if let Some(value) = argv.get(index) {
                entrypoint = Some(value.clone());
                index += 1;
            }
            continue;
        }
        if token.starts_with('-') {
            let takes_next = docker_opt_takes_value(token) && !token.contains('=');
            index += 1;
            if takes_next {
                index += 1;
            }
            continue;
        }
        break;
    }
    if index >= argv.len() {
        return None;
    }
    index += 1;
    let tail = if index < argv.len() {
        argv[index..].join(" ")
    } else {
        String::new()
    };
    if let Some(bin) = entrypoint {
        if tail.is_empty() {
            return Some(bin);
        }
        return Some(format!("{bin} {tail}"));
    }
    if tail.is_empty() {
        return None;
    }
    Some(tail)
}

fn docker_opt_takes_value(token: &str) -> bool {
    let name = token.strip_prefix("--").unwrap_or(token);
    matches!(
        name,
        "-u" | "-w"
            | "-e"
            | "-n"
            | "--user"
            | "user"
            | "workdir"
            | "env"
            | "name"
            | "entrypoint"
            | "hostname"
            | "network"
            | "volume"
            | "mount"
    ) || name == "-v"
}

fn parallel_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("parallel") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() {
        let token = &argv[index];
        if token == ":::" || token == "::::" {
            break;
        }
        if token.starts_with('-') {
            let takes_next = parallel_opt_takes_value(token)
                && !token.contains('=')
                && (token.starts_with("--") || token.len() == 2);
            index += 1;
            if takes_next {
                index += 1;
            }
            continue;
        }
        break;
    }
    let start = index;
    while index < argv.len() && argv[index] != ":::" && argv[index] != "::::" {
        index += 1;
    }
    if start == index {
        return None;
    }
    Some(argv[start..index].join(" "))
}

fn kubectl_exec_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("kubectl") || argv.get(1).map(String::as_str) != Some("exec") {
        return None;
    }
    let mut index = 2;
    while index < argv.len() && argv[index] != "--" {
        let token = &argv[index];
        if token.starts_with('-') && token != "--" {
            let takes_next = matches!(token.as_str(), "-c" | "-n" | "--container" | "--namespace")
                && !token.contains('=');
            index += 1;
            if takes_next {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    if index < argv.len() && argv[index] == "--" {
        index += 1;
    }
    if index >= argv.len() {
        return None;
    }
    Some(argv[index..].join(" "))
}

fn chroot_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("chroot") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() && argv[index].starts_with('-') {
        let takes_next =
            matches!(argv[index].as_str(), "-u" | "--userspec") && !argv[index].contains('=');
        index += 1;
        if takes_next {
            index += 1;
        }
    }
    if index >= argv.len() {
        return None;
    }
    index += 1;
    if index >= argv.len() {
        return None;
    }
    Some(argv[index..].join(" "))
}

fn nsenter_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("nsenter") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() && argv[index].starts_with('-') {
        let takes_next = matches!(
            argv[index].as_str(),
            "-t" | "-S" | "-G" | "-r" | "--target" | "--setuid" | "--setgid" | "--root"
        ) && !argv[index].contains('=');
        index += 1;
        if takes_next {
            index += 1;
        }
    }
    if index >= argv.len() {
        return None;
    }
    Some(argv[index..].join(" "))
}

fn systemd_run_command(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("systemd-run") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() && (argv[index].starts_with('-') || argv[index] == "--") {
        if argv[index] == "--" {
            index += 1;
            break;
        }
        let takes_next = matches!(
            argv[index].as_str(),
            "-p" | "-u" | "--property" | "--unit" | "--uid" | "--gid" | "--slice" | "--description"
        ) && !argv[index].contains('=');
        index += 1;
        if takes_next {
            index += 1;
        }
    }
    if index >= argv.len() {
        return None;
    }
    Some(argv[index..].join(" "))
}

fn parallel_opt_takes_value(token: &str) -> bool {
    let name = token.strip_prefix("--").unwrap_or(token);
    matches!(
        name,
        "-j" | "-S" | "-a" | "-C" | "jobs" | "sshlogin" | "arg-file" | "colsep" | "timeout"
    )
}

fn argv_matches(argv: &[String], prefix: &[String]) -> bool {
    if argv.is_empty() || prefix.is_empty() {
        return false;
    }
    if !same_tool(command_basename(&argv[0]), command_basename(&prefix[0])) {
        return false;
    }
    if prefix.len() == 1 {
        return true;
    }
    let mut index = 1;
    for word in &prefix[1..] {
        index = skip_options(argv, index);
        if index >= argv.len() || !same_word(&argv[index], word) {
            return false;
        }
        index += 1;
    }
    true
}

fn executed_argv(tokens: &[String]) -> Option<(Vec<String>, Option<String>)> {
    let mut index = 0;
    let mut split_script = None;
    while index < tokens.len() {
        if is_assignment(&tokens[index]) {
            index += 1;
            continue;
        }
        // `coproc rm` and `coproc name { rm; }` both run rm.
        if command_basename(&tokens[index]).eq_ignore_ascii_case("coproc") {
            index += 1;
            if index + 1 < tokens.len() && tokens[index] != "{" && tokens[index + 1] == "{" {
                index += 1;
            }
            continue;
        }
        // `for rm in a` stays `for`. These words are not the command.
        if leading_shell_keyword(&tokens[index]) {
            index += 1;
            continue;
        }
        let lowered = command_basename(&tokens[index]).to_ascii_lowercase();
        let base = gnu_alias(&lowered);
        if !is_wrapper(base) {
            break;
        }
        index += 1;
        let Some((next, script)) = skip_wrapper_flags(base, tokens, index) else {
            return split_script.map(|script| (Vec::new(), Some(script)));
        };
        if split_script.is_none() {
            split_script = script;
        }
        index = next;
    }
    Some((tokens[index..].to_vec(), split_script))
}

fn leading_shell_keyword(token: &str) -> bool {
    matches!(
        token,
        "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!" | "{" | "("
    )
}

fn skip_wrapper_flags(
    wrapper: &str,
    tokens: &[String],
    mut index: usize,
) -> Option<(usize, Option<String>)> {
    let mut split_script = None;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if is_assignment(token) {
            index += 1;
            continue;
        }
        if token == "--" {
            // `timeout -- 1 rm` still has a duration after the option end.
            if wrapper == "timeout" {
                return Some(((index + 2).min(tokens.len()), split_script));
            }
            return Some((index + 1, split_script));
        }
        if !token.starts_with('-') {
            break;
        }
        if wrapper_does_not_execute(wrapper, token) {
            return split_script.map(|script| (tokens.len(), Some(script)));
        }
        if let Some((name, value)) = token.split_once('=') {
            if split_script.is_none() && env_split_flag(wrapper, name) {
                split_script = Some(value.to_string());
            }
            index += 1;
            continue;
        }
        if let Some(letters) = short_cluster(token) {
            let next = tokens.get(index + 1).map(String::as_str);
            let (step, script) = consume_short_cluster(wrapper, letters, next);
            if split_script.is_none() {
                split_script = script;
            }
            index += step;
            continue;
        }
        let takes_value = flag_takes_value(wrapper, token);
        let record_split = env_split_flag(wrapper, token);
        index += 1;
        if takes_value && index < tokens.len() {
            if record_split && split_script.is_none() {
                split_script = Some(tokens[index].clone());
            }
            index += 1;
        }
    }
    // The first word after timeout's options is the duration, not the command.
    if wrapper == "timeout" && index < tokens.len() {
        index += 1;
    }
    Some((index, split_script))
}

fn short_cluster(token: &str) -> Option<&str> {
    let letters = token.strip_prefix('-')?;
    if letters.is_empty() || letters.starts_with('-') {
        None
    } else {
        Some(letters)
    }
}

fn env_split_flag(wrapper: &str, flag: &str) -> bool {
    if wrapper != "env" {
        return false;
    }
    let name = flag.strip_prefix("--").unwrap_or(flag);
    name == "-S" || name == "split-string"
}

fn consume_short_cluster(
    wrapper: &str,
    letters: &str,
    next: Option<&str>,
) -> (usize, Option<String>) {
    for (pos, letter) in letters.char_indices() {
        if !flag_takes_value(wrapper, &format!("-{letter}")) {
            continue;
        }
        let rest = &letters[pos + letter.len_utf8()..];
        let (step, value) = if rest.is_empty() {
            match next {
                Some(word) => (2, Some(word)),
                None => (1, None),
            }
        } else {
            (1, Some(rest))
        };
        let script = if wrapper == "env" && letter == 'S' {
            value.map(str::to_string)
        } else {
            None
        };
        return (step, script);
    }
    (1, None)
}

fn wrapper_does_not_execute(wrapper: &str, flag: &str) -> bool {
    if let Some(name) = flag.strip_prefix("--") {
        return match wrapper {
            "sudo" | "doas" => matches!(name, "list" | "validate"),
            "command" => name == "help",
            _ => false,
        };
    }
    let Some(letters) = flag.strip_prefix('-') else {
        return false;
    };
    if letters.is_empty() || letters.starts_with('-') {
        return false;
    }
    match wrapper {
        "sudo" | "doas" => letters.contains('l') || letters.contains('v'),
        "command" => letters.contains('v') || letters.contains('V'),
        _ => false,
    }
}

fn skip_options(argv: &[String], mut index: usize) -> usize {
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            return index + 1;
        }
        if !token.starts_with('-') {
            return index;
        }
        let takes_value = !token.contains('=') && option_takes_value(token);
        index += 1;
        if takes_value && index < argv.len() {
            index += 1;
        }
    }
    index
}

fn same_tool(token: &str, prefix: &str) -> bool {
    same_word(token, prefix) || (same_word(token, "grm") && same_word(prefix, "rm"))
}

/// Homebrew installs the same coreutils and findutils binaries under a
/// `g` name (`gtimeout`, `grm`). The shell runs that name as the unprefixed tool.
fn gnu_alias(name: &str) -> &str {
    match name {
        "genv" => "env",
        "gnice" => "nice",
        "gnohup" => "nohup",
        "gstdbuf" => "stdbuf",
        "gtimeout" => "timeout",
        "gxargs" => "xargs",
        other => other,
    }
}

fn is_wrapper(name: &str) -> bool {
    const WRAPPERS: &[&str] = &[
        "sudo", "doas", "pkexec", "env", "command", "nice", "nohup", "time", "busybox", "ionice",
        "stdbuf", "setsid", "exec", "timeout", "xargs",
    ];
    WRAPPERS
        .iter()
        .any(|wrapper| name.eq_ignore_ascii_case(wrapper))
}

fn flag_takes_value(wrapper: &str, flag: &str) -> bool {
    let name = flag.strip_prefix("--").unwrap_or(flag);
    match wrapper {
        "sudo" | "doas" => matches!(
            name,
            "-u" | "-g"
                | "-h"
                | "-p"
                | "-C"
                | "-T"
                | "-R"
                | "-D"
                | "-U"
                | "-a"
                | "user"
                | "group"
                | "host"
                | "prompt"
                | "chdir"
                | "role"
                | "type"
                | "command-timeout"
                | "close-from"
        ),
        "env" => matches!(
            name,
            "-u" | "-S" | "-C" | "unset" | "chdir" | "split-string" | "argv0"
        ),
        "nice" | "ionice" => matches!(name, "-n" | "-c" | "-p" | "adjustment"),
        "time" => matches!(name, "-f" | "-o" | "format" | "output"),
        "stdbuf" => matches!(name, "-i" | "-o" | "-e" | "input" | "output" | "error"),
        "pkexec" => name == "user",
        "exec" => name == "-a",
        "timeout" => matches!(name, "-k" | "-s" | "kill-after" | "signal"),
        "xargs" => matches!(
            name,
            "-a" | "-I"
                | "-L"
                | "-n"
                | "-P"
                | "-s"
                | "-d"
                | "-E"
                | "arg-file"
                | "replace"
                | "max-lines"
                | "max-args"
                | "max-procs"
                | "max-chars"
                | "delimiter"
                | "process-slot-var"
                | "eof"
        ),
        _ => false,
    }
}

fn option_takes_value(flag: &str) -> bool {
    let name = flag.strip_prefix("--").unwrap_or(flag);
    matches!(
        name,
        "-C" | "-c"
            | "git-dir"
            | "work-tree"
            | "namespace"
            | "super-prefix"
            | "config-env"
            | "exec-path"
    )
}

fn eval_script(tokens: &[String]) -> Option<String> {
    let base = command_basename(tokens.first()?);
    if !base.eq_ignore_ascii_case("eval") {
        return None;
    }
    let mut rest = &tokens[1..];
    if rest.first().is_some_and(|token| token == "--") {
        rest = &rest[1..];
    }
    if rest.is_empty() {
        return None;
    }
    Some(rest.join(" "))
}

fn is_shell_name(name: &str) -> bool {
    const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "ksh", "ash", "fish"];
    SHELLS.iter().any(|shell| name.eq_ignore_ascii_case(shell))
}

fn is_stdin_script(token: &str) -> bool {
    matches!(token, "/dev/stdin" | "/dev/fd/0" | "-")
}

fn shell_reads_stdin(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let base = command_basename(first);
    if !is_shell_name(base) {
        return false;
    }
    let mut index = 1;
    let mut saw_s = false;
    let mut after_dash = false;
    while index < argv.len() {
        let token = &argv[index];
        if !after_dash && token == "--" {
            after_dash = true;
            index += 1;
            continue;
        }
        if !after_dash && shell_opt_takes_value(token) {
            index += 1;
            if index < argv.len() {
                index += 1;
            }
            continue;
        }
        if !after_dash && let Some(flags) = token.strip_prefix('-') {
            if flags.contains('c') {
                return false;
            }
            if flags.contains('s') {
                saw_s = true;
            }
            index += 1;
            continue;
        }
        if !after_dash && token.starts_with('+') {
            index += 1;
            continue;
        }
        if saw_s || is_stdin_script(token) {
            index += 1;
            continue;
        }
        return false;
    }
    true
}

fn sourced_stdin(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let base = command_basename(first);
    if base != "source" && base != "." {
        return false;
    }
    let mut index = 1;
    if argv.get(index).is_some_and(|token| token == "--") {
        index += 1;
    }
    argv.get(index).is_some_and(|token| is_stdin_script(token))
}

fn cmd_script(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("cmd") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() {
        if is_cmd_run_switch(&argv[index]) {
            let script = argv[index + 1..].join(" ");
            return if script.is_empty() {
                None
            } else {
                Some(script)
            };
        }
        index += 1;
    }
    None
}

// `/c` and `/k` run the rest of the line. `cmd` accepts one leading
// `-` for those two letters too. `--c` and `-command` do not.
fn is_cmd_run_switch(token: &str) -> bool {
    let Some(name) = token.strip_prefix('-').or_else(|| token.strip_prefix('/')) else {
        return false;
    };
    if name.starts_with('-') || name.starts_with('/') {
        return false;
    }
    name.eq_ignore_ascii_case("c") || name.eq_ignore_ascii_case("k")
}

fn is_powershell_base(name: Option<&str>) -> bool {
    let Some(name) = name else {
        return false;
    };
    let base = command_basename(name);
    base.eq_ignore_ascii_case("powershell") || base.eq_ignore_ascii_case("pwsh")
}

fn powershell_script(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("powershell") && !base.eq_ignore_ascii_case("pwsh") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        // A bare word is not a switch. Windows PowerShell defaults it
        // to -Command. pwsh defaults it to -File, which is not read.
        // `hidden` after `-w` is the switch value, so it is not seen here.
        let Some((name, inline)) = ps_switch_parts(token) else {
            if base.eq_ignore_ascii_case("powershell") {
                return ps_command_script(None, &argv[index..]);
            }
            return None;
        };
        // Command before encoded: `-c` is `-Command`, and it is also a
        // prefix of `-EncodedCommand`. `-cwa` is `-CommandWithArgs`.
        if is_ps_command_name(name) || is_ps_command_with_args_name(name) {
            return ps_command_script(inline, &argv[index + 1..]);
        }
        // One value. Later words are not part of this script, unlike `-Command`.
        if is_ps_encoded_command_name(name) {
            let encoded = match inline {
                Some(value) => value,
                None => argv.get(index + 1)?.as_str(),
            };
            return decode_ps_encoded_script(encoded);
        }
        // Words after -File belong to the script. Do not read the file,
        // and do not treat a later -Command as the engine command.
        if is_ps_file_name(name) {
            return None;
        }
        if ps_name_takes_value(name, base) {
            index += 1;
            // `-w:hidden` already holds the value. Do not also skip `-enc`.
            if inline.is_none() && index < argv.len() {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    None
}

// The first `:` glues the value (`-ec:<base64>`, `-com:rm`). `--` and
// `/` are the same prefix as `-`.
fn ps_switch_parts(token: &str) -> Option<(&str, Option<&str>)> {
    if !token.starts_with('-') && !token.starts_with('/') {
        return None;
    }
    let (head, inline) = match token.split_once(':') {
        Some((head, value)) => (head, Some(value)),
        None => (token, None),
    };
    Some((ps_switch_name(head)?, inline))
}

fn ps_switch_name(token: &str) -> Option<&str> {
    token
        .strip_prefix("--")
        .or_else(|| token.strip_prefix('-'))
        .or_else(|| token.strip_prefix('/'))
}

// `-c` is the alias. `configurationname` starts with `con`, so `-com`
// is only `-Command`. `-co` matches both and is not the command.
fn is_ps_command_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name == "c" {
        return true;
    }
    name.len() >= 3 && "command".starts_with(&name)
}

// `-cwa` is the alias. A prefix of `commandwithargs` of at least three
// letters is the same switch. `-c` and `-com` stay `-Command`.
fn is_ps_command_with_args_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name == "cwa" {
        return true;
    }
    if is_ps_command_name(&name) {
        return false;
    }
    name.len() >= 3 && "commandwithargs".starts_with(&name)
}

fn ps_command_script(inline: Option<&str>, rest: &[String]) -> Option<String> {
    let inline = inline.filter(|text| !text.is_empty());
    if inline.is_none() && rest.is_empty() {
        return None;
    }
    let mut script = String::new();
    if let Some(inline) = inline {
        script.push_str(inline);
    }
    for word in rest {
        if !script.is_empty() {
            script.push(' ');
        }
        script.push_str(word);
    }
    Some(script)
}

// pwsh accepts `-e` and `-enc` as prefixes of `-EncodedCommand`, plus the
// alias `-ec`. `-ec` is not a prefix: `encodedcommand` starts with `en`.
fn is_ps_encoded_command_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.is_empty() && ("encodedcommand".starts_with(&name) || name == "ec")
}

// UTF-16LE base64, same as pwsh `-EncodedCommand`. Space, tab, CR, and LF
// are ignored. A value that is not base64, or not UTF-16LE, is not a script.
fn decode_ps_encoded_script(value: &str) -> Option<String> {
    let bytes = decode_standard_base64(value)?;
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let script = String::from_utf16(&units).ok()?;
    if script.is_empty() {
        None
    } else {
        Some(script)
    }
}

fn decode_standard_base64(value: &str) -> Option<Vec<u8>> {
    let filtered: Vec<u8> = value
        .bytes()
        .filter(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        .collect();
    if filtered.is_empty() || !filtered.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(filtered.len() / 4 * 3);
    let quanta = filtered.len() / 4;
    for (index, chunk) in filtered.chunks_exact(4).enumerate() {
        let Ok(chunk) = <[u8; 4]>::try_from(chunk) else {
            return None;
        };
        push_base64_quantum(chunk, index + 1 == quanta, &mut out)?;
    }
    Some(out)
}

fn push_base64_quantum(chunk: [u8; 4], allow_pad: bool, out: &mut Vec<u8>) -> Option<()> {
    let pad = if chunk[2] == b'=' {
        if chunk[3] != b'=' {
            return None;
        }
        2
    } else if chunk[3] == b'=' {
        1
    } else {
        0
    };
    if pad > 0 && !allow_pad {
        return None;
    }
    if chunk[0] == b'=' || chunk[1] == b'=' {
        return None;
    }
    let first = b64_sextet(chunk[0])?;
    let second = b64_sextet(chunk[1])?;
    out.push((first << 2) | (second >> 4));
    if pad == 2 {
        return Some(());
    }
    let third = b64_sextet(chunk[2])?;
    out.push(((second & 0x0f) << 4) | (third >> 2));
    if pad == 1 {
        return Some(());
    }
    let fourth = b64_sextet(chunk[3])?;
    out.push(((third & 0x03) << 6) | fourth);
    Some(())
}

fn b64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

// `-f` through `-file`. Later tokens are script arguments, so the scan
// stops. The file is not read.
fn is_ps_file_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.is_empty() && "file".starts_with(&name)
}

// One argument, then the scan continues. A non-empty prefix of any name
// below counts. `-w` prefixes both `windowstyle` and `workingdirectory`;
// both take a value, so one word is skipped. `-ep` and `-wd` are not
// leading prefixes (`executionpolicy` starts with `ex`, `workingdirectory`
// with `wo`). Called after the command, encoded, and file checks.
// `-c` and `-e` also prefix `configurationname` and `executionpolicy`,
// and `-co` prefixes both `command` and `configurationname`, so those
// are not this kind of switch. pwsh `-i` is `-Interactive` and takes
// no value. Windows PowerShell `-i` is still `-InputFormat`.
fn ps_name_takes_value(name: &str, base: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }
    if name == "i" && base.eq_ignore_ascii_case("pwsh") {
        return false;
    }
    if matches!(name.as_str(), "ep" | "wd") {
        return true;
    }
    // pwsh -SettingsFile takes a path. `-set` is long enough not to be `-s`.
    if name.len() >= 3 && "settingsfile".starts_with(&name) {
        return true;
    }
    const NAMES: &[&str] = &[
        "executionpolicy",
        "inputformat",
        "outputformat",
        "windowstyle",
        "version",
        "workingdirectory",
        "psconsolefile",
        "configurationname",
    ];
    if !NAMES.iter().any(|candidate| candidate.starts_with(&name)) {
        return false;
    }
    !("command".starts_with(&name) || "encodedcommand".starts_with(&name))
}

fn case_arm_scripts(tokens: &[String]) -> Vec<String> {
    let Some(first) = tokens.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("case") {
        return Vec::new();
    }
    let mut scripts = Vec::new();
    let mut index = 1;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        let starts_arm = token.ends_with(')');
        if !starts_arm {
            index += 1;
            continue;
        }
        index += 1;
        let start = index;
        while index < tokens.len() {
            let word = tokens[index].as_str();
            if matches!(word, ";;" | ";&" | ";;&" | "esac") {
                break;
            }
            index += 1;
        }
        if start < index {
            scripts.push(tokens[start..index].join(" "));
        }
    }
    scripts
}

fn git_alias_scripts(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("git") {
        return Vec::new();
    }
    let mut aliases = Vec::new();
    let mut helpers = Vec::new();
    let mut external_diffs = Vec::new();
    let mut ssh_commands = Vec::new();
    let mut difftool_cmds = Vec::new();
    let mut diff_tools = Vec::new();
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" || !token.starts_with('-') {
            break;
        }
        if token == "-c" || token == "--config" {
            index += 1;
            if let Some((key, value)) = argv.get(index).and_then(|spec| spec.split_once('=')) {
                if key == "credential.helper" {
                    if let Some(shell) = value.strip_prefix('!') {
                        helpers.push(shell.to_string());
                    }
                } else if key == "core.sshCommand" {
                    let script = value.strip_prefix('!').unwrap_or(value);
                    if !script.is_empty() {
                        ssh_commands.push(script.to_string());
                    }
                } else if key == "diff.external" {
                    let script = value.strip_prefix('!').unwrap_or(value);
                    if !script.is_empty() {
                        external_diffs.push(script.to_string());
                    }
                } else if key == "diff.tool" && !value.is_empty() {
                    diff_tools.push(value.to_string());
                } else if let Some(rest) = key.strip_prefix("difftool.") {
                    if let Some(tool) = rest.strip_suffix(".cmd") {
                        let script = value.strip_prefix('!').unwrap_or(value);
                        if !tool.is_empty() && !script.is_empty() {
                            difftool_cmds.push((tool.to_string(), script.to_string()));
                        }
                    }
                } else if let Some(name) = key.strip_prefix("alias.") {
                    aliases.push((name.to_string(), value.to_string()));
                }
            }
        }
        index += 1;
    }
    let mut scripts = Vec::new();
    if argv.iter().any(|word| word == "diff") {
        scripts.extend(external_diffs);
    }
    if argv.iter().any(|word| word == "difftool")
        && let Some(tool) = selected_difftool(argv, &diff_tools)
    {
        for (name, command) in difftool_cmds {
            if name == tool {
                scripts.push(command);
            }
        }
    }
    if git_uses_ssh(argv) {
        scripts.extend(ssh_commands);
    }
    if argv.iter().any(|word| word == "credential") {
        for shell in helpers {
            if !shell.is_empty() {
                scripts.push(shell);
            }
        }
    }
    for (name, value) in aliases {
        let invoked = argv.iter().any(|word| word == &name);
        if !invoked {
            continue;
        }
        if let Some(shell) = value.strip_prefix('!') {
            scripts.push(shell.to_string());
        } else if !value.is_empty() {
            scripts.push(format!("git {value}"));
        }
    }
    scripts
}

fn selected_difftool(argv: &[String], configured: &[String]) -> Option<String> {
    let mut index = 0;
    let mut from_cli = None;
    while index < argv.len() {
        let token = argv[index].as_str();
        if let Some(name) = token.strip_prefix("--tool=") {
            if !name.is_empty() {
                from_cli = Some(name.to_string());
            }
        } else if token == "--tool"
            && let Some(name) = argv.get(index + 1)
            && !name.is_empty()
            && !name.starts_with('-')
        {
            from_cli = Some(name.clone());
        }
        index += 1;
    }
    from_cli.or_else(|| configured.last().cloned())
}

fn git_uses_ssh(argv: &[String]) -> bool {
    argv.iter().any(|word| {
        matches!(
            word.as_str(),
            "ls-remote" | "fetch" | "pull" | "push" | "clone"
        ) || word.contains("ssh://")
            || word.starts_with("git@")
    })
}

fn fish_inline_script(token: &str) -> Option<String> {
    token
        .strip_prefix("--command=")
        .or_else(|| token.strip_prefix("--init-command="))
        .map(str::to_string)
}

fn su_script(tokens: &[String]) -> Option<String> {
    let base = command_basename(tokens.first()?);
    if !base.eq_ignore_ascii_case("su") {
        return None;
    }
    let mut index = 1;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if let Some(script) = token.strip_prefix("--command=") {
            return Some(script.to_string());
        }
        if token == "-c" || token == "--command" {
            return tokens.get(index + 1).cloned();
        }
        index += 1;
    }
    None
}

fn shell_script(tokens: &[String]) -> Option<String> {
    let base = command_basename(tokens.first()?);
    if !is_shell_name(base) {
        return None;
    }
    let mut index = 1;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "--" {
            return None;
        }
        if shell_opt_takes_value(token) {
            index += 1;
            if index < tokens.len() {
                index += 1;
            }
            continue;
        }
        if base.eq_ignore_ascii_case("fish") {
            if let Some(script) = fish_inline_script(token) {
                return Some(script);
            }
            if token == "-C" || token == "--init-command" {
                return tokens.get(index + 1).cloned();
            }
        }
        if let Some(script) = c_argument(token, tokens.get(index + 1)) {
            return Some(script);
        }
        // `+e` and `+o errexit` still run `-c`. A plus flag is not a path.
        if token.starts_with('-') || token.starts_with('+') {
            index += 1;
            continue;
        }
        return None;
    }
    None
}

fn shell_opt_takes_value(token: &str) -> bool {
    let name = token.strip_prefix("--").unwrap_or(token);
    // Separate `-o`, `+o`, `-O`, and `+O` take the next word.
    // Glued `-Oextglob` does not.
    matches!(name, "-o" | "+o" | "-O" | "+O" | "rcfile" | "init-file")
}

fn c_argument(token: &str, next: Option<&String>) -> Option<String> {
    if token == "-c" || token == "--command" {
        return next.cloned();
    }
    let flags = token.strip_prefix('-')?;
    if flags.is_empty() || flags.starts_with('-') || !flags.contains('c') {
        return None;
    }
    next.cloned()
}

fn is_assignment(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    let mut plus = false;
    for ch in chars {
        if ch == '=' {
            return true;
        }
        if ch == '+' && !plus {
            plus = true;
            continue;
        }
        if plus || !(ch.is_ascii_alphanumeric() || ch == '_') {
            return false;
        }
    }
    false
}

fn command_basename(token: &str) -> &str {
    let base = token
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(token);
    // A quoted image can keep a trailing space (`"rm.exe "`). Windows
    // ignores that space. The suffix check has to see `.exe`.
    strip_image_suffix(base.trim_end_matches([' ', '\t']))
}

/// One Windows image suffix. The extension has to be the whole tail, so
/// `rm.exe.bak` stays `rm.exe.bak` and `git.exe.exe` stays `git.exe`.
/// Trailing dots are removed only when that suffix is under them
/// (`git.exe.` is `git`, `rm.` stays `rm.`).
fn strip_image_suffix(name: &str) -> &str {
    const SUFFIXES: &[&str] = &[".exe", ".cmd", ".bat", ".com"];
    let candidate = name.trim_end_matches('.');
    if candidate.is_empty() {
        return name;
    }
    for suffix in SUFFIXES {
        if candidate.len() <= suffix.len() {
            continue;
        }
        let end = candidate.len() - suffix.len();
        // `你好` is longer than `.exe`, but byte 2 is inside a character.
        // `split_at` would panic and hide a later `rm`.
        let (Some(stem), Some(tail)) = (candidate.get(..end), candidate.get(end..)) else {
            continue;
        };
        if tail.eq_ignore_ascii_case(suffix) {
            return stem;
        }
    }
    name
}

fn same_word(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Bash `$'...'` word. A decoded null ends the word. A raw null in the
/// source is skipped, because the bash reader deletes it before parsing.
fn read_ansi_c<I>(chars: &mut std::iter::Peekable<I>, token: &mut String)
where
    I: Iterator<Item = char>,
{
    let mut truncated = false;
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            continue;
        }
        if truncated {
            if ch == '\'' {
                return;
            }
            continue;
        }
        if ch == '\'' {
            return;
        }
        if ch != '\\' {
            token.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            token.push('\\');
            return;
        };
        if next == '\0' {
            token.push('\\');
            continue;
        }
        if push_ansi_escape(chars, token, next) {
            truncated = true;
        }
    }
}

fn push_ansi_escape<I>(chars: &mut std::iter::Peekable<I>, token: &mut String, next: char) -> bool
where
    I: Iterator<Item = char>,
{
    match next {
        '\\' | '\'' | '"' | '?' => token.push(next),
        'a' => token.push('\u{7}'),
        'b' => token.push('\u{8}'),
        'e' | 'E' => token.push('\u{1b}'),
        'f' => token.push('\u{c}'),
        'n' => token.push('\n'),
        'r' => token.push('\r'),
        't' => token.push('\t'),
        'v' => token.push('\u{b}'),
        'c' => {
            let Some(ctrl) = chars.next() else {
                token.push('\\');
                token.push('c');
                return false;
            };
            if ctrl == '\0' {
                token.push('\\');
                token.push('c');
                return false;
            }
            let value = (ctrl as u32) & 0x1f;
            if value == 0 {
                return true;
            }
            token.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
        }
        'x' | 'X' => return push_ansi_number(chars, token, 16, 2, next),
        'u' => return push_ansi_number(chars, token, 16, 4, next),
        'U' => return push_ansi_number(chars, token, 16, 8, next),
        '0'..='7' => {
            let mut value = next.to_digit(8).unwrap_or(0);
            for _ in 1..3 {
                let Some(digit) = chars.peek().copied() else {
                    break;
                };
                let Some(part) = digit.to_digit(8) else {
                    break;
                };
                chars.next();
                value = value * 8 + part;
            }
            return push_ansi_value(token, u64::from(value));
        }
        _ => {
            token.push('\\');
            token.push(next);
        }
    }
    false
}

fn push_ansi_number<I>(
    chars: &mut std::iter::Peekable<I>,
    token: &mut String,
    radix: u32,
    max: usize,
    introducer: char,
) -> bool
where
    I: Iterator<Item = char>,
{
    let mut value = 0u64;
    let mut count = 0;
    while count < max {
        let Some(digit) = chars.peek().copied() else {
            break;
        };
        let Some(part) = digit.to_digit(radix) else {
            break;
        };
        chars.next();
        value = value * u64::from(radix) + u64::from(part);
        count += 1;
    }
    if count == 0 {
        token.push('\\');
        token.push(introducer);
        return false;
    }
    push_ansi_value(token, value)
}

fn push_ansi_value(token: &mut String, value: u64) -> bool {
    if value == 0 {
        return true;
    }
    match u32::try_from(value).ok().and_then(char::from_u32) {
        Some(ch) => token.push(ch),
        None => token.push('\u{FFFD}'),
    }
    false
}

/// Bash `$"..."` word. A backslash escapes `$`, backtick, `"`, `\`, and a newline.
fn read_dollar_double<I>(chars: &mut std::iter::Peekable<I>, token: &mut String)
where
    I: Iterator<Item = char>,
{
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            continue;
        }
        if ch == '"' {
            return;
        }
        if ch != '\\' {
            token.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            token.push('\\');
            return;
        };
        if next == '\0' {
            token.push('\\');
            continue;
        }
        match next {
            '\\' | '$' | '`' | '"' => token.push(next),
            '\n' => {}
            _ => {
                token.push('\\');
                token.push(next);
            }
        }
    }
}

struct ShellSegment {
    tokens: Vec<String>,
    heredocs: Vec<String>,
    substitutions: Vec<String>,
}

enum ShellPart {
    Function { name: String, body: String },
    Segment(ShellSegment),
}

struct PendingHeredoc {
    owner: usize,
    delim: String,
    dash: bool,
}

fn tokenize_parts(command: &str) -> Vec<ShellPart> {
    let mut parts = Vec::new();
    let mut current = Vec::new();
    let mut token = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    let mut quoted = false;
    let mut heredocs: Vec<PendingHeredoc> = Vec::new();
    let mut substitutions = Vec::new();
    let mut stdin_bodies = Vec::new();
    while let Some(ch) = chars.next() {
        // Bash deletes NUL, including inside quotes (`echo 'a\0b'` prints ab).
        if ch == '\0' {
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
                continue;
            }
            if open == '"' && ch == '\\' {
                if let Some(next) = chars.next()
                    && next != '\0'
                {
                    token.push(next);
                }
                continue;
            }
            // `echo "$(rm)"` runs rm. Single quotes never reach this branch.
            if open == '"' && ch == '$' && chars.peek() == Some(&'(') {
                chars.next();
                substitutions.push(read_balanced_parens(&mut chars));
                continue;
            }
            token.push(ch);
            continue;
        }
        if ch == '$'
            && let Some(&next) = chars.peek()
        {
            if next == '\'' || next == '"' {
                chars.next();
                quoted = true;
                if next == '\'' {
                    read_ansi_c(&mut chars, &mut token);
                } else {
                    read_dollar_double(&mut chars, &mut token);
                }
                continue;
            }
            if next == '(' {
                chars.next();
                substitutions.push(read_balanced_parens(&mut chars));
                continue;
            }
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                quoted = true;
            }
            '\\' => {
                if let Some(next) = chars.next()
                    && next != '\n'
                    && next != '\0'
                {
                    token.push(next);
                }
            }
            '`' => substitutions.push(read_backtick(&mut chars)),
            '(' | ')' => {
                push_token(&mut token, &mut current, &mut quoted);
                current.push(ch.to_string());
            }
            '{' if token.is_empty() && brace_is_its_own_word(chars.peek().copied()) => {
                if let Some(name) = function_header(&current) {
                    if !substitutions.is_empty() {
                        parts.push(ShellPart::Segment(ShellSegment {
                            tokens: Vec::new(),
                            heredocs: Vec::new(),
                            substitutions: std::mem::take(&mut substitutions),
                        }));
                    }
                    current.clear();
                    let body = read_brace_body(&mut chars);
                    parts.push(ShellPart::Function { name, body });
                } else {
                    current.push("{".to_string());
                }
            }
            '<' | '>' if chars.peek() == Some(&'(') => {
                chars.next();
                substitutions.push(read_balanced_parens(&mut chars));
            }
            '<' if chars.peek() == Some(&'<') => {
                chars.next();
                push_token(&mut token, &mut current, &mut quoted);
                if chars.peek() == Some(&'<') {
                    chars.next();
                    if let Some(body) = read_here_string(&mut chars) {
                        stdin_bodies.push(body);
                    }
                } else if let Some((delim, dash)) = read_heredoc_delim(&mut chars) {
                    heredocs.push(PendingHeredoc {
                        owner: parts.len(),
                        delim,
                        dash,
                    });
                }
            }
            '<' | '>' => {
                consume_redirect_tail(&mut chars, ch);
                if !quoted && is_fd_word(&token) {
                    token.clear();
                } else {
                    push_token(&mut token, &mut current, &mut quoted);
                }
                skip_redirect_word(&mut chars, &mut substitutions);
            }
            '&' if chars.peek() == Some(&'>') => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
                push_token(&mut token, &mut current, &mut quoted);
                skip_redirect_word(&mut chars, &mut substitutions);
            }
            ' ' | '\t' | '\r' => push_token(&mut token, &mut current, &mut quoted),
            '\n' => {
                push_token(&mut token, &mut current, &mut quoted);
                push_shell_segment(
                    &mut current,
                    &mut substitutions,
                    &mut stdin_bodies,
                    &mut parts,
                );
                for item in std::mem::take(&mut heredocs) {
                    let body = read_heredoc_body(&mut chars, &item.delim, item.dash);
                    if let Some(ShellPart::Segment(segment)) = parts.get_mut(item.owner) {
                        segment.heredocs.push(body);
                    }
                }
            }
            '&' | '|' | ';' => {
                if matches!(ch, '&' | '|') && chars.peek() == Some(&ch) {
                    chars.next();
                }
                push_token(&mut token, &mut current, &mut quoted);
                push_shell_segment(
                    &mut current,
                    &mut substitutions,
                    &mut stdin_bodies,
                    &mut parts,
                );
            }
            '#' if token.is_empty() => {
                while let Some(&next) = chars.peek() {
                    if next == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            _ => token.push(ch),
        }
    }
    push_token(&mut token, &mut current, &mut quoted);
    push_shell_segment(
        &mut current,
        &mut substitutions,
        &mut stdin_bodies,
        &mut parts,
    );
    parts
}

fn brace_is_its_own_word(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(ch) => {
            ch.is_whitespace()
                || matches!(
                    ch,
                    ';' | '&' | '|' | '<' | '>' | '(' | ')' | '{' | '}' | '\'' | '"' | '\\'
                )
        }
    }
}

fn function_header(tokens: &[String]) -> Option<String> {
    if tokens.len() == 3 && tokens[1] == "(" && tokens[2] == ")" && is_function_name(&tokens[0]) {
        return Some(tokens[0].clone());
    }
    if tokens.len() == 2 && tokens[0] == "function" && is_function_name(&tokens[1]) {
        return Some(tokens[1].clone());
    }
    if tokens.len() == 4
        && tokens[0] == "function"
        && tokens[2] == "("
        && tokens[3] == ")"
        && is_function_name(&tokens[1])
    {
        return Some(tokens[1].clone());
    }
    None
}

fn is_function_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn read_brace_body<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut body = String::new();
    let mut depth = 1usize;
    let mut quote = None;
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            continue;
        }
        if let Some(open) = quote {
            body.push(ch);
            if open == '"' && ch == '\\' {
                if let Some(next) = chars.next()
                    && next != '\0'
                {
                    body.push(next);
                }
                continue;
            }
            if ch == open {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                body.push(ch);
            }
            '{' => {
                depth += 1;
                body.push(ch);
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                body.push(ch);
            }
            _ => body.push(ch),
        }
    }
    body
}

/// Body of `$(...)`. The opening `(` is already consumed. Quotes hide a `)`.
fn read_balanced_parens<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut inner = String::new();
    let mut depth = 1usize;
    let mut quote = None;
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            continue;
        }
        if let Some(open) = quote {
            if open == '"' && ch == '\\' {
                inner.push(ch);
                if let Some(next) = chars.next()
                    && next != '\0'
                {
                    inner.push(next);
                }
                continue;
            }
            inner.push(ch);
            if ch == open {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                inner.push(ch);
            }
            '\\' => {
                inner.push(ch);
                if let Some(next) = chars.next()
                    && next != '\0'
                {
                    inner.push(next);
                }
            }
            '(' => {
                depth += 1;
                inner.push(ch);
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                inner.push(ch);
            }
            _ => inner.push(ch),
        }
    }
    inner
}

/// Body of an unquoted backtick command. The opening backtick is consumed.
fn read_backtick<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut inner = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            continue;
        }
        if ch == '`' {
            break;
        }
        if ch == '\\' {
            let Some(next) = chars.next() else {
                inner.push('\\');
                break;
            };
            if next == '\0' {
                inner.push('\\');
                continue;
            }
            match next {
                '$' | '`' | '\\' => inner.push(next),
                '\n' => {}
                other => {
                    inner.push('\\');
                    inner.push(other);
                }
            }
            continue;
        }
        inner.push(ch);
    }
    inner
}

fn push_token(token: &mut String, current: &mut Vec<String>, quoted: &mut bool) {
    if *quoted || !token.is_empty() {
        current.push(std::mem::take(token));
    }
    *quoted = false;
}

fn is_fd_word(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_digit())
}

/// `>>`, `>&`, `>|`, `<>`, and `<&`. The first character is already consumed.
fn consume_redirect_tail<I>(chars: &mut std::iter::Peekable<I>, first: char)
where
    I: Iterator<Item = char>,
{
    let second = chars.peek().copied();
    let take = matches!(
        (first, second),
        ('>', Some('>' | '&' | '|')) | ('<', Some('>' | '&'))
    );
    if take {
        chars.next();
    }
}

/// The filename after a redirect. `$(...)` and backticks in that word still run.
fn skip_redirect_word<I>(chars: &mut std::iter::Peekable<I>, substitutions: &mut Vec<String>)
where
    I: Iterator<Item = char>,
{
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
    }
    match chars.peek().copied() {
        Some('\'') => {
            chars.next();
            for ch in chars.by_ref() {
                if ch == '\'' {
                    break;
                }
            }
        }
        Some('"') => {
            chars.next();
            skip_double_quoted_target(chars, substitutions);
        }
        Some(_) => skip_unquoted_target(chars, substitutions),
        None => {}
    }
}

fn skip_unquoted_target<I>(chars: &mut std::iter::Peekable<I>, substitutions: &mut Vec<String>)
where
    I: Iterator<Item = char>,
{
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '<' | '>') {
            break;
        }
        chars.next();
        if ch == '\0' {
            continue;
        }
        if ch == '\\' {
            chars.next();
            continue;
        }
        if ch == '`' {
            substitutions.push(read_backtick(chars));
            continue;
        }
        if ch == '$' && chars.peek() == Some(&'(') {
            chars.next();
            substitutions.push(read_balanced_parens(chars));
        }
    }
}

fn skip_double_quoted_target<I>(chars: &mut std::iter::Peekable<I>, substitutions: &mut Vec<String>)
where
    I: Iterator<Item = char>,
{
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            continue;
        }
        if ch == '"' {
            return;
        }
        if ch == '\\' {
            chars.next();
            continue;
        }
        if ch == '`' {
            substitutions.push(read_backtick(chars));
            continue;
        }
        if ch == '$' && chars.peek() == Some(&'(') {
            chars.next();
            substitutions.push(read_balanced_parens(chars));
        }
    }
}

fn read_here_string<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
) -> Option<String> {
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
    }
    if chars.peek() == Some(&'$') {
        chars.next();
        match chars.peek().copied() {
            Some('\'') => {
                chars.next();
                let mut body = String::new();
                read_ansi_c(chars, &mut body);
                return Some(body);
            }
            Some('"') => {
                chars.next();
                let mut body = String::new();
                read_dollar_double(chars, &mut body);
                return Some(body);
            }
            _ => {
                let mut body = String::from("$");
                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '<' | '>') {
                        break;
                    }
                    chars.next();
                    if ch != '\0' {
                        body.push(ch);
                    }
                }
                return Some(body);
            }
        }
    }
    let quote = chars.peek().copied();
    if matches!(quote, Some('\'' | '"')) {
        let quote = chars.next().unwrap_or('"');
        let mut body = String::new();
        for ch in chars.by_ref() {
            if ch == quote {
                break;
            }
            if ch != '\0' {
                body.push(ch);
            }
        }
        return Some(body);
    }
    let mut body = String::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '<' | '>') {
            break;
        }
        chars.next();
        if ch != '\0' {
            body.push(ch);
        }
    }
    if body.is_empty() { None } else { Some(body) }
}

fn read_heredoc_delim<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
) -> Option<(String, bool)> {
    let dash = chars.peek() == Some(&'-');
    if dash {
        chars.next();
    }
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
    }
    let mut delim = String::new();
    if matches!(chars.peek(), Some('\'' | '"')) {
        let quote = chars.next().unwrap_or('"');
        for ch in chars.by_ref() {
            if ch == quote {
                break;
            }
            delim.push(ch);
        }
    } else {
        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() {
                break;
            }
            delim.push(ch);
            chars.next();
        }
    }
    if delim.is_empty() {
        None
    } else {
        Some((delim, dash))
    }
}

fn read_heredoc_body<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
    delim: &str,
    dash: bool,
) -> String {
    let mut body = String::new();
    let mut line = String::new();
    loop {
        let end = match chars.next() {
            None => true,
            Some('\n') => false,
            Some(ch) => {
                line.push(ch);
                continue;
            }
        };
        let text = if dash {
            line.trim_start_matches('\t')
        } else {
            line.as_str()
        };
        if text == delim {
            break;
        }
        if end {
            if !line.is_empty() {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&line);
            }
            break;
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&line);
        line.clear();
    }
    body
}

fn push_shell_segment(
    current: &mut Vec<String>,
    substitutions: &mut Vec<String>,
    stdin_bodies: &mut Vec<String>,
    parts: &mut Vec<ShellPart>,
) {
    if current.is_empty() && substitutions.is_empty() && stdin_bodies.is_empty() {
        return;
    }
    parts.push(ShellPart::Segment(ShellSegment {
        tokens: std::mem::take(current),
        heredocs: std::mem::take(stdin_bodies),
        substitutions: std::mem::take(substitutions),
    }));
}

pub fn effective_gates(
    policy: &Policy,
    action_id: &ActionId,
    harm_class: Option<HarmClass>,
) -> Result<EffectiveGates, PolicyError> {
    policy.ensure_checked()?;
    let Some(default_action) = policy.default_action.as_ref() else {
        return Err(PolicyError::UnknownAction(action_id.clone()));
    };
    let action = policy.actions.get(action_id).unwrap_or(default_action);
    let escalate_below = policy.choice.escalate_below;
    let mut auto = action.auto.map(|value| value.max(escalate_below));
    if action.class >= HarmClass::Write {
        auto = auto.map(|value| value.max(policy.choice.review_below));
    }
    let mut harm_bumped = None;
    if let Some(decoded) = harm_class {
        if decoded > action.class {
            auto = None;
            harm_bumped = Some((action.class, decoded));
        } else if decoded < action.class && action.class >= HarmClass::Write {
            // The confidence is in the lower label. It must not auto this row.
            auto = None;
        }
    }
    Ok(EffectiveGates {
        escalate_below,
        auto,
        review: action.review.max(escalate_below),
        when_unsure: action.when_unsure,
        block_on: action.block_on.clone(),
        harm_bumped,
    })
}

pub fn verdict_from_signal(s: f64, gates: &EffectiveGates, action_id: ActionId) -> Verdict {
    if s < gates.escalate_below {
        let verdict = match gates.when_unsure {
            UnsureVerdict::ReviewGuess => Verdict::Review,
            UnsureVerdict::Escalate => Verdict::Escalate,
        };
        return verdict(hint(
            action_id,
            vec![UnsureReason::BelowFloor {
                confidence: s,
                floor: gates.escalate_below,
            }],
        ));
    }
    if let Some(auto) = gates.auto
        && s >= auto
    {
        return Verdict::Auto(hint(action_id, Vec::new()));
    }
    if s >= gates.review {
        Verdict::Review(hint(
            action_id,
            vec![UnsureReason::ReviewFloor {
                confidence: s,
                floor: gates.review,
                auto: gates.auto,
            }],
        ))
    } else {
        Verdict::Escalate(hint(
            action_id,
            vec![UnsureReason::BelowAuto {
                confidence: s,
                auto: gates.auto.unwrap_or(gates.review),
            }],
        ))
    }
}

/// Synthetic signal helper. A block id with no row in `nouls` stays inert.
/// `gate` does not use this path: a missing block answer there is a decode failure.
pub fn verdict_with_blocks(
    policy: &Policy,
    action_id: &ActionId,
    s: f64,
    harm_class: Option<HarmClass>,
    nouls: &[NoulObs<'_>],
) -> Result<Verdict, PolicyError> {
    let gates = effective_gates(policy, action_id, harm_class)?;
    for block in &gates.block_on {
        let Some(obs) = nouls.iter().find(|row| row.id == block.id.0) else {
            continue;
        };
        let decision = NoulAnswer { p: obs.p }.decide(&policy.noul, Some(block));
        let fires = matches!(
            (block.when, &decision),
            (BlockWhen::Yes, Decision::Known(true))
                | (BlockWhen::No, Decision::Known(false))
                | (BlockWhen::Unsure, Decision::Unsure { .. })
        );
        if fires {
            return Ok(Verdict::Escalate(hint(
                action_id.clone(),
                vec![UnsureReason::Battery {
                    id: block.id.clone(),
                    when: block.when,
                    excerpt: String::new(),
                }],
            )));
        }
    }
    Ok(verdict_from_signal(s, &gates, action_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::{Policy, effective_gates, matched_action};
    use crate::ids::ActionId;

    #[test]
    fn raw_toml_is_unchecked_until_finish() {
        let raw = include_str!("../policies/tool-gate.toml");
        let policy: Policy = toml::from_str(raw).expect("toml");
        assert!(!policy.sealed);
        match effective_gates(&policy, &ActionId::new("tag"), None) {
            Err(err) => assert!(err.to_string().contains("unchecked"), "{err}"),
            Ok(_) => panic!("raw toml must not pass effective_gates"),
        }

        let checked = Policy::from_toml_str(raw).expect("finish");
        assert!(checked.sealed);
        effective_gates(&checked, &ActionId::new("tag"), None).expect("sealed policy");
    }

    #[test]
    fn bash_prefixes_do_not_steal_a_longer_token() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = matched_action(&policy, "Bash", Some("rm -rf /tmp"))
            .expect("rm")
            .0
            .as_str();
        assert_eq!(rm, "bash.rm");
        assert_eq!(
            matched_action(&policy, "Bash", Some("git push origin"))
                .expect("push")
                .0
                .as_str(),
            "git.push"
        );
        assert!(matched_action(&policy, "Bash", Some("git push-all")).is_none());
        assert!(matched_action(&policy, "Bash", Some("ls")).is_none());
        assert!(matched_action(&policy, "Bash", Some("rmdir /tmp")).is_none());
        assert_eq!(
            matched_action(&policy, "Bash", Some("/bin/rm -rf /tmp"))
                .expect("path rm")
                .0
                .as_str(),
            "bash.rm"
        );
        assert_eq!(
            matched_action(&policy, "bash", Some("RM -rf /tmp"))
                .expect("case rm")
                .0
                .as_str(),
            "bash.rm"
        );
        assert_eq!(
            matched_action(&policy, "Bash", Some("git\tpush origin"))
                .expect("tab push")
                .0
                .as_str(),
            "git.push"
        );
        assert_eq!(
            matched_action(&policy, "Bash", Some("/usr/bin/git push origin"))
                .expect("path git")
                .0
                .as_str(),
            "git.push"
        );
    }

    #[test]
    fn bash_prefixes_match_wrappers_separators_and_git_options() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "sudo rm -rf /",
            "sudo -n rm -rf /",
            "sudo -u root rm -rf /",
            "sudo -nu root rm -rf /",
            "sudo -- rm -rf /",
            "cd x && rm -rf ~",
            "FOO=1 rm -rf /",
            "FOO+=1 rm -rf /",
            "A+=1 B+=2 rm -rf /",
            "env rm -rf /",
            "env FOO=1 rm -rf /",
            "env -iu PATH rm -rf /",
            "env -S 'rm -rf /'",
            "bash -c 'rm -rf /'",
            "bash -lc \"rm -rf /tmp\"",
            "sudo bash -c 'rm -rf /'",
            "rm -rf / && git push",
            "bash -cx 'rm -rf /'",
            "bash -o pipefail -c 'rm -rf /'",
            "command -p rm -rf /",
            "cat <<'EOF'\ntext\nEOF\nrm -rf /tmp",
            "cat <<EOF && rm -rf /\nbody\nEOF",
            "if true; then rm -rf /tmp/x; fi",
            "if rm -rf /tmp/x; then true; fi",
            "! rm -rf /tmp/x",
            "echo $(rm -rf /tmp/x)",
            "echo \"$(rm -rf /tmp/x)\"",
            "echo `rm -rf /tmp/x`",
            "bash -O extglob -c 'rm -rf /tmp/x'",
            "bash +O extglob -c 'rm -rf /tmp/x'",
            "bash -Oextglob -c 'rm -rf /tmp/x'",
            "{ rm --version; }",
            "( rm --version )",
            "(rm --version)",
            "true && { rm --version; }",
            "find . -exec rm -rf / {}",
            "find . -exec rm {} \\;",
            "find . -execdir rm {} +",
            "find . -ok rm {} \\;",
            "find . -exec echo {} \\; -exec rm {} \\;",
            "gfind . -exec rm {} +",
            "ssh host rm -rf /tmp",
            "ssh -n host rm -rf /tmp",
            "ssh -p 22 host rm -rf /tmp",
            "ssh -o StrictHostKeyChecking=no host rm -rf /tmp",
            "flock /tmp/lock rm -rf /tmp",
            "flock -n /tmp/lock rm -rf /tmp",
            "flock -w 2 /tmp/lock rm -rf /tmp",
            "flock /tmp/lock -c 'rm -rf /tmp'",
            "flock -c 'rm -rf /tmp' /tmp/lock",
            "docker exec box rm -rf /tmp",
            "docker exec -it box rm -rf /tmp",
            "docker exec -u root box rm -rf /tmp",
            "docker run --rm ubuntu rm -rf /tmp",
            "docker run --entrypoint=/bin/rm alpine -rf /tmp",
            "docker run --entrypoint /bin/rm alpine -rf /tmp",
            "su -c 'rm -rf /tmp'",
            "su --command='rm -rf /tmp'",
            "ssh -o ProxyCommand='rm -rf /tmp/x' example.com",
            "scp -o ProxyCommand='rm -rf /tmp/x' host:/tmp /tmp",
            "sftp -o ProxyCommand='rm -rf /tmp/x' host",
            "ssh -oProxyCommand='rm -rf /tmp/x' example.com",
            "rsync -e 'rm -rf /tmp/x' host:/tmp /tmp",
            "rsync --rsh='rm -rf /tmp/x' host:/tmp /tmp",
            "ncat --exec 'rm -rf /tmp/x' host 80",
            "ncat -c 'rm -rf /tmp/x' host 80",
            "ncat --sh-exec='rm -rf /tmp/x' host 80",
            "parallel rm ::: /tmp/x",
            "parallel -j 4 rm -rf /tmp ::: x",
            "parallel --jobs=4 rm ::: x",
            "podman exec box rm -rf /tmp",
            "podman run --rm ubuntu rm -rf /tmp",
            "kubectl exec pod -- rm -rf /tmp",
            "kubectl exec -it pod -- rm -rf /tmp",
            "chroot /mnt rm -rf /tmp",
            "nsenter -t 1 -m rm -rf /tmp",
            "systemd-run rm -rf /tmp",
            "systemd-run --uid 0 rm -rf /tmp",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command}"
            );
        }
        let push = [
            "git -C repo push",
            "git -C repo push origin",
            "sudo git push",
            "sudo -nu root git push",
            "cd repo && git push",
            "git\t-C\trepo\tpush",
            "/usr/bin/git --no-pager push",
            "git --git-dir=/repo push",
            "find . -exec git push \\;",
            "env -S 'git push'",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command}"
            );
        }
        let neither = [
            "rmdir /tmp",
            "git push-all",
            "echo rm -rf /",
            "FOO+1 rm -rf /",
            "echo 'rm -rf /'",
            "echo '$(rm -rf /tmp)'",
            "for rm in a",
            "{rm --version;}",
            "f() { rm --version; }",
            "env -S 'echo rm'",
            "sudo -nu root rmdir /tmp",
            "bash -c 'echo rm'",
            "find . -name rm",
            "find . -exec echo rm \\;",
            "ssh host",
            "ssh -p 22 rm",
            "ssh host ls",
            "flock /tmp/lock",
            "flock -w 2 /tmp/lock",
            "docker exec box",
            "docker run ubuntu",
            "docker ps",
            "parallel echo rm ::: a",
            "podman exec box",
            "kubectl exec pod",
            "kubectl get pods",
            "chroot /mnt",
            "nsenter -t 1 -m",
            "systemd-run --unit job",
            "git status",
            "git -C repo status",
            "git -C push status",
            "ls",
            "bash -crm 'echo ok'",
            "bash --rcfile -c 'rm -rf /'",
            "command -v rm",
            "command -V rm",
            "sudo -l rm",
            "sudo -v rm",
            "git '' push",
            "bash -c '' rm",
            "cat <<'EOF'\nrm -rf /\nEOF",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command}"
            );
        }
        // Bash deletes a null byte. `rm` + NUL + ` -rf` is rm. `rm` + NUL + `dir` is rmdir.
        let dropped = "rm\u{0} -rf /tmp";
        assert_eq!(
            matched_action(&policy, "Bash", Some(dropped)).map(|id| id.0.as_str()),
            Some("bash.rm"),
            "{dropped:?}"
        );
        let later_line = "echo ok\nrm\u{0} -rf /";
        assert_eq!(
            matched_action(&policy, "Bash", Some(later_line)).map(|id| id.0.as_str()),
            Some("bash.rm"),
            "{later_line:?}"
        );
        let glued = "rm\u{0}dir /tmp";
        assert!(
            matched_action(&policy, "Bash", Some(glued)).is_none(),
            "{glued:?}"
        );
    }

    #[test]
    fn ansi_c_and_locale_quotes_match_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "$'rm' --version",
            "$\"rm\" --version",
            "$'r\\x6d' --version",
            "$'r\\155' --version",
            "$'r\\u006d' --version",
            "$'r\\u6d' --version",
            "bash -c $'rm --version'",
            "$'rm\\x00dir' --version",
            "$'rm\\000dir' --version",
            "$'rm\\0dir' --version",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let push = [
            "$'git' push",
            "$'git' $'push'",
            "git $'push'",
            "bash -c $'git push'",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        let neither = [
            "echo $'rm'",
            "echo $\"rm\"",
            "$'rmdir' /tmp",
            "$\"rmdir\" /tmp",
            "$'r\\m' --version",
            "$\"r\\m\" --version",
            "$\"rm --version\"",
            "$'rm\\n--version'",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    #[test]
    fn redirection_does_not_hide_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "rm>/dev/null",
            "rm>/dev/null --version",
            ">/dev/null rm -rf /tmp/x",
            "2>/dev/null rm -rf /tmp/x",
            "&>/dev/null rm -rf /tmp/x",
            "rm -rf /tmp/x>/dev/null",
            "rm -rf /tmp/x 2>&1",
            "rm>/tmp/$(rm -rf /tmp/y)",
            "rm>\"$(rm -rf /tmp/y)\"",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let push = [
            "git push origin main 2>&1",
            "git push origin main>/tmp/log",
            "git push origin main 2>&1 | tail -5",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        let neither = [
            "echo rm>/tmp/x",
            "echo >/tmp/x",
            "2&>/dev/null rm -rf /tmp/x",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    #[test]
    fn exec_and_eval_match_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "exec rm --version",
            "exec -a name rm --version",
            "exec -- rm --version",
            "exec -cl rm --version",
            "command exec rm --version",
            "sudo exec rm --version",
            "eval rm --version",
            "eval 'rm --version'",
            "eval $\"rm --version\"",
            "eval -- rm --version",
            "eval 'echo ok; rm --version'",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let push = ["exec git push", "eval 'git push'"];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        let neither = [
            "exec -a rm echo hello",
            "exec rmdir /tmp",
            "echo exec rm",
            "eval echo ok",
            "eval 'echo rm'",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    #[test]
    fn timeout_and_xargs_match_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "timeout 1 rm --version",
            "timeout --foreground 1 rm --version",
            "timeout -k 1 2 rm --version",
            "timeout -k1 2 rm --version",
            "timeout -s TERM 1 rm --version",
            "timeout --signal=TERM 1 rm --version",
            "timeout 1s rm --version",
            "timeout -- 1 rm --version",
            "timeout 1 command rm --version",
            "xargs rm --version",
            "xargs -n 1 rm --version",
            "xargs -n1 rm --version",
            "xargs -- rm --version",
            "xargs -E _ rm --version",
            "xargs -0 rm --version",
            "xargs -e rm --version",
            "xargs -a /dev/null rm --version",
            "command xargs rm --version",
            "timeout 1 bash -c 'rm --version'",
            "xargs sh -c 'rm --version'",
            "timeout 1 sudo rm --version",
            "gtimeout 1 rm --version",
            "gxargs rm --version",
            "gstdbuf -oL rm --version",
            "gnice rm --version",
            "genv rm --version",
            "genv -S 'rm --version'",
            "gnohup rm --version",
            "grm --version",
            "gtimeout 1 grm --version",
            "/opt/homebrew/bin/grm --version",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let push = ["timeout 1 git push", "xargs git push"];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        let neither = [
            "timeout 1 rmdir --version",
            "timeout 1 echo rm",
            "timeout -- rm --version",
            "timeout 1 -- rm --version",
            "xargs echo rm",
            "echo timeout 1 rm",
            "echo xargs rm",
            "echo grm",
            "gfalse rm --version",
            "grmdir /tmp",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    #[test]
    fn windows_image_suffix_uses_the_same_row() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "rm.exe -rf /tmp/x",
            "RM.EXE -rf /tmp/x",
            "rm.cmd -rf /tmp/x",
            "rm.bat -rf /tmp/x",
            "rm.com -rf /tmp/x",
            "rm.ExE -rf /tmp/x",
            "./rm.exe -rf /tmp/x",
            "C:/Windows/rm.exe -rf C:/temp",
            "'C:\\Windows\\rm.exe' -rf /tmp/x",
            "\"C:/Program Files/rm.exe\" -rf /tmp/x",
            "\"rm.exe \" -rf /tmp/x",
            "rm.exe. -rf /tmp/x",
            "rm.exe... -rf /tmp/x",
            "\"rm.exe. \" -rf /tmp/x",
            "FOO=1 rm.exe -rf /tmp/x",
            "FOO+=1 rm.exe -rf /tmp/x",
            "sudo.exe rm.exe -rf /tmp/x",
            "sudo.exe -nu root rm.cmd -rf /tmp/x",
            "timeout.exe 1 rm -rf /tmp/x",
            "timeout.EXE 1 rm.exe -rf /tmp/x",
            "timeout.exe --foreground 1 rm.CMD -rf /tmp/x",
            "timeout.exe -- 1 rm.exe --version",
            "busybox.exe rm -rf /tmp/x",
            "busybox.cmd rm.exe -rf /tmp/x",
            "xargs.exe rm.exe --version",
            "gtimeout.exe 1 grm.exe --version",
            "GRM.EXE --version",
            "bash.exe -c 'rm.exe -rf /'",
            "bash.CMD -c 'rm.bat -rf /'",
            "eval.exe 'rm.exe --version'",
            "exec.exe rm.exe --version",
            "env.exe -S 'rm.exe -rf /'",
            "你好 && rm -rf /tmp/x",
            "héllo && rm -rf /tmp/x",
            "git.exe push && rm.exe -rf /",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let push = [
            "git.exe push",
            "GIT.EXE push origin",
            "git.cmd push",
            "git.bat push",
            "git.com push",
            "git.Exe push origin",
            "\"C:/Program Files/Git/cmd/git.exe\" push",
            "'C:\\Program Files\\Git\\cmd\\git.exe' push",
            "git.exe\tpush",
            "git.exe. push",
            "git.EXE... push origin",
            "timeout.exe 1 git.exe push",
            "sudo.exe git.cmd push",
            "bash.exe -c 'git.exe push'",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        let neither = [
            "rmdir.exe /tmp",
            "git.exe push-all",
            "rm.exe.bak -rf /tmp",
            "rm.executable -rf /tmp",
            "rm. -rf /tmp",
            "rm.exe.bak. -rf /tmp",
            "git.exe.bak push",
            "gitexe push",
            "git.exe.exe push",
            "echo.exe rm -rf /",
            "echo rm.exe",
            "grmdir.exe /tmp",
            "timeout.exe 1 rmdir.exe /tmp",
            "timeout.exe -- rm.exe --version",
            "C:/Program Files/Git/cmd/git.exe push",
            "你好",
            "héllo",
            "\u{1F600}x",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    /// The shell runs `rm` in each of these strings. The old table locked
    /// the miss (`git.push`, or no action) because the test copied the
    /// matcher instead of the execution.
    #[test]
    fn shell_runs_of_rm_match_bash_rm_not_the_earlier_row() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "bash <<'EOF'\nrm -rf /\nEOF",
            "bash <<EOF\nrm -rf /\nEOF",
            "sh <<'EOF'\nrm -rf /tmp/x\nEOF",
            "sudo bash <<'EOF'\nrm -rf /tmp/x\nEOF",
            "f() { rm -rf /tmp/x; }; f",
            "f() { rm -rf /tmp/x; }; f extra",
            "f() { git push && rm -rf /tmp/x; }; f",
            "f() { rm -rf /tmp/x; }; echo $(f)",
            "f() { rm -rf /tmp/x; }; eval f",
            "cmd /c rm -rf /tmp/x",
            "cmd.exe /c rm -rf /tmp/x",
            "cmd /c \"rm -rf /tmp/x\"",
            "cmd /c \"git push && rm -rf /tmp/x\"",
            "powershell -Command rm -rf /tmp/x",
            "powershell.exe -Command rm -rf /tmp/x",
            "pwsh -Command rm -rf /tmp/x",
            "find . -delete",
            "find . -name foo -delete",
            "gfind . -delete",
            "git push $(rm -rf /)",
            "git push origin main && rm -rf /tmp/x",
            "git push origin main; rm -rf /tmp/x",
            "git push origin main | rm -rf /tmp/x",
            "git push origin main & rm -rf /tmp/x",
            "cd x && git push && rm -rf /",
            "git.exe push && rm.exe -rf /",
            "bash <<< 'rm -rf /tmp/x'",
            "bash <<< $'rm -rf /tmp/x'",
            "bash <<< \"$(rm -rf /tmp/x)\"",
            "cat <(rm -rf /tmp/x)",
            "cat >(rm -rf /tmp/x)",
            "zsh <<< 'rm -rf /tmp/x'",
            "bash -s <<< 'rm -rf /tmp/x'",
            "bash /dev/stdin <<'EOF'\nrm -rf /tmp/x\nEOF",
            "bash -s -- extra <<'EOF'\nrm -rf /tmp/x\nEOF",
            ". /dev/stdin <<'EOF'\nrm -rf /tmp/x\nEOF",
            "echo ok# $(rm -rf /tmp/x)",
            "echo \"# $(rm -rf /tmp/x)\"",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let push = [
            "git push origin main",
            "git push $(echo rm)",
            "bash <<< 'git push origin'",
            "source /dev/stdin <<'EOF'\ngit push origin\nEOF",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        let neither = [
            "cat <<'EOF'\nrm -rf /\nEOF",
            "cat <<EOF\nrm -rf /\nEOF",
            "f() { rm --version; }",
            "f() { echo rm; }; f",
            "find . -name rm",
            "find . -name -delete",
            "find . -exec echo rm \\;",
            "cmd /c echo rm",
            "powershell -Command echo rm",
            "pwsh -Command echo rm",
            "echo cmd /c rm -rf /",
            "bash -c 'echo ok' <<'EOF'\nrm -rf /\nEOF",
            "bash <<'EOF'\necho ok\nEOF",
            "f() { rm -rf /tmp/x; }; bash -c f",
            "$(f() { rm -rf /tmp/x; }); f",
            "bash <<'EOF'\nf() { rm -rf /tmp/x; }\nEOF\nf",
            "find . -exec echo -delete \\;",
            "f() { rm -rf /tmp/x; }; find . -exec f \\;",
            "cat <<< 'rm -rf /tmp/x'",
            "bash -c 'echo ok' <<< 'rm -rf /tmp/x'",
            "bash script.sh <<'EOF'\nrm -rf /tmp/x\nEOF",
            "source file.sh <<'EOF'\nrm -rf /tmp/x\nEOF",
            "echo ok # $(rm -rf /tmp/x)",
            "echo ok # `rm -rf /tmp/x`",
            "echo '<(rm -rf /tmp/x)'",
            "echo <(echo rm)",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    /// `-EncodedCommand` is UTF-16LE base64. These match `bash.rm` only
    /// after that decode. Raw `rm` words, UTF-8 base64 of that text, and
    /// any value that is not UTF-16LE `rm` stay unmatched.
    #[test]
    fn powershell_encoded_command_is_the_script() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let payload = "cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=";
        assert_eq!(
            super::decode_ps_encoded_script(payload).as_deref(),
            Some("rm -rf /tmp/x")
        );
        // Same bytes with a space, a non-zero pad bit, and a `+` sextet.
        assert_eq!(
            super::decode_ps_encoded_script("cgBt ACAALQByAGYAIAAvAHQAbQBwAC8AeAA=").as_deref(),
            Some("rm -rf /tmp/x")
        );
        assert_eq!(
            super::decode_ps_encoded_script("cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAB=").as_deref(),
            Some("rm -rf /tmp/x")
        );
        assert_eq!(
            super::decode_ps_encoded_script("cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA+AA==").as_deref(),
            Some("rm -rf /tmp/x>")
        );
        let rm = [
            "powershell -EncodedCommand cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "powershell.exe -EncodedCommand cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh -ec cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "powershell -E cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh -Enc cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh -encoded cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh --ec cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh /ec cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh -NoProfile -ExecutionPolicy Bypass -ec cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh -ec 'cgBt ACAALQByAGYAIAAvAHQAbQBwAC8AeAA='",
            "pwsh -ec cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAB=",
            "pwsh -ec cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA+AA==",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        // UTF-8 base64 of `rm -rf /tmp/x`, not UTF-16LE. A byte-wise decode
        // would match `bash.rm` by accident.
        let utf8_rm = "cm0gLXJmIC90bXAveA==";
        let echo_rm = "ZQBjAGgAbwAgAHIAbQA=";
        assert!(super::decode_ps_encoded_script(utf8_rm).is_none());
        assert!(super::decode_ps_encoded_script("QQ==").is_none());
        assert!(super::decode_ps_encoded_script("!!!").is_none());
        assert!(super::decode_ps_encoded_script("ANg=").is_none());
        assert_eq!(
            super::decode_ps_encoded_script(echo_rm).as_deref(),
            Some("echo rm")
        );
        let neither = [
            "powershell -EncodedCommand rm -rf /tmp/x",
            "powershell -EncodedCommand 'rm -rf /tmp/x'",
            "pwsh -ec cm0gLXJmIC90bXAveA==",
            "pwsh -E QQ==",
            "pwsh -ec ANg=",
            "powershell -EncodedCommand !!!",
            "powershell -EncodedCommand",
            "pwsh -ec ZQBjAGgAbwAgAHIAbQA=",
            "pwsh -encodedarguments cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
            "pwsh -ea cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    /// A bare word used to end the scan, so `-w hidden` and `-ep bypass`
    /// hid `-enc`. A colon-glued value is the switch value. `-com` is
    /// `-Command`. `-co` is not. On `pwsh` the words after `-co` are
    /// not a command script. UTF-8 base64 and `-File` still do not
    /// match `bash.rm`.
    #[test]
    fn powershell_short_switches_still_reach_the_script() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let payload = "cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=";
        let utf8_rm = "cm0gLXJmIC90bXAveA==";
        assert_eq!(
            super::powershell_script(&[
                "powershell".to_string(),
                "-com:rm".to_string(),
                "-rf".to_string(),
                "/tmp/x".to_string(),
            ])
            .as_deref(),
            Some("rm -rf /tmp/x")
        );
        let rm = [
            format!("powershell -w hidden -enc {payload}"),
            format!("powershell -nop -ep bypass -enc {payload}"),
            format!("powershell -ex bypass -enc {payload}"),
            "powershell -com rm -rf /tmp/x".to_string(),
            format!("powershell -ec:{payload}"),
            format!("powershell -E:{payload}"),
            format!("powershell -EncodedCommand:{payload}"),
            "powershell -com:rm -rf /tmp/x".to_string(),
            "powershell -com:FOO=1 rm -rf /tmp/x".to_string(),
            format!("powershell -wd /tmp -enc {payload}"),
            "pwsh -o Text -c rm -rf /tmp/x".to_string(),
            "pwsh -WorkingDirectory /tmp -c rm -rf /tmp/x".to_string(),
            format!("powershell -w:hidden -enc {payload}"),
            format!("pwsh --ec:{payload}"),
            format!("pwsh /ec:{payload}"),
            format!("powershell -con name -enc {payload}"),
        ];
        for command in &rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let neither = [
            format!("powershell -enc {utf8_rm}"),
            format!("powershell -w hidden -enc {utf8_rm}"),
            format!("powershell -ec:{utf8_rm}"),
            "powershell -ep bypass -enc !!!".to_string(),
            "powershell -w hidden -enc QQ==".to_string(),
            "pwsh -co rm -rf /tmp/x".to_string(),
            "powershell -File rm -rf /tmp/x".to_string(),
            "powershell -com:FOO=1".to_string(),
        ];
        for command in &neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    /// `pwsh -i` takes no value, so `-c` still runs. Windows PowerShell
    /// `-i` is `-InputFormat`. A bare word on `powershell` is the
    /// command. `pwsh` leaves that word unmatched. `cmd -c` is `/c`.
    /// `-cwa` joins like `-Command`, and `-c` stays `-Command`. Words
    /// after `-File` are not the engine command.
    #[test]
    fn pwsh_i_bare_command_and_cmd_dash_c_match() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "pwsh -i -c rm -rf /tmp/x",
            "powershell -i xml -c rm -rf /tmp/x",
            "powershell rm -rf /tmp/x",
            "powershell -co rm -rf /tmp/x",
            "pwsh -cwa rm -rf /tmp/x",
            "pwsh -cwa 'rm -rf /tmp/x'",
            "pwsh -commandw rm -rf /tmp/x",
            "pwsh -c rm -rf /tmp/x",
            "pwsh -Command rm -rf /tmp/x",
            "cmd -c rm -rf /tmp/x",
            "cmd -k rm -rf /tmp/x",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let neither = [
            "pwsh rm -rf /tmp/x",
            "powershell -File script.ps1 -Command rm -rf /tmp/x",
            "pwsh -f script.ps1 -c rm -rf /tmp/x",
            "pwsh -cw rm -rf /tmp/x",
            "cmd -command rm -rf /tmp/x",
            "cmd --c rm -rf /tmp/x",
            "cmd -x rm -rf /tmp/x",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    /// pwsh -SettingsFile takes a path, then still runs -Command.
    /// `pwsh -Command -` runs the heredoc, not the dash.
    #[test]
    fn powershell_settings_file_and_stdin_heredoc_match() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "pwsh -SettingsFile C:\\x.json -c rm -rf /tmp/x",
            "pwsh -set C:\\x.json -Command rm -rf /tmp/x",
            "pwsh -NoProfile -Command - <<'EOF'\nrm -rf /tmp/x\nEOF",
            "powershell -c - <<'EOF'\nrm -rf /tmp/x\nEOF",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        assert!(
            matched_action(
                &policy,
                "Bash",
                Some("bash -c - <<'EOF'\nrm -rf /tmp/x\nEOF")
            )
            .is_none(),
            "bash -c does not read the heredoc"
        );
    }

    /// `bash +e -c` and `bash +o errexit -c` still run the script.
    /// A plus flag used to end the scan, so `rm` missed `bash.rm`.
    #[test]
    fn shell_plus_options_still_run_the_script() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let commands = [
            ("bash +e -c 'rm -rf /tmp/x'", "bash.rm"),
            ("bash +ex -c 'rm -rf /tmp/x'", "bash.rm"),
            ("sh +e -c 'rm -rf /tmp/x'", "bash.rm"),
            ("zsh +e -c 'rm -rf /tmp/x'", "bash.rm"),
            ("bash +o errexit -c 'rm -rf /tmp/x'", "bash.rm"),
            ("zsh +o errexit -c 'git push'", "git.push"),
            ("bash +o errexit <<'EOF'\nrm -rf /tmp/x\nEOF", "bash.rm"),
            ("bash --init-file /dev/null -c 'rm -rf /tmp/x'", "bash.rm"),
            ("bash --rcfile /dev/null -c 'rm -rf /tmp/x'", "bash.rm"),
            ("case x in x) rm -rf /tmp/x ;; esac", "bash.rm"),
            ("coproc { rm -rf /tmp/x; }", "bash.rm"),
            ("coproc rm -rf /tmp/x", "bash.rm"),
            ("fish --command='rm -rf /tmp/x'", "bash.rm"),
            ("fish -C 'rm -rf /tmp/x'", "bash.rm"),
            ("fish --init-command='rm -rf /tmp/x'", "bash.rm"),
            ("git -c 'alias.p=!rm -rf /tmp/x' p", "bash.rm"),
            ("git -c alias.p=push p", "git.push"),
            (
                "git -c credential.helper='!rm -rf /tmp/x' credential fill",
                "bash.rm",
            ),
            (
                "git -c diff.external='rm -rf /tmp/x' diff --no-index a b",
                "bash.rm",
            ),
            (
                "git -c difftool.x.cmd='rm -rf /tmp/x' difftool --tool=x a b",
                "bash.rm",
            ),
            (
                "git -c difftool.x.cmd='rm -rf /tmp/x' -c diff.tool=x difftool a b",
                "bash.rm",
            ),
            (
                "git -c core.sshCommand='rm -rf /tmp/x' ls-remote ssh://example/no.git",
                "bash.rm",
            ),
        ];
        for (command, expected) in commands {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|action| action.0.as_str()),
                Some(expected),
                "{command:?}"
            );
        }
        assert!(
            matched_action(&policy, "Bash", Some("git -c alias.p=push status")).is_none(),
            "an alias that is not invoked does not match"
        );
        assert!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c credential.helper='!rm -rf /tmp/x' status")
            )
            .is_none(),
            "a credential helper is not used by status"
        );
        assert!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c diff.external='rm -rf /tmp/x' status")
            )
            .is_none(),
            "an external diff is not used by status"
        );
        assert!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c difftool.x.cmd='rm -rf /tmp/x' diff --no-index a b")
            )
            .is_none(),
            "difftool.cmd is not used by diff"
        );
        assert!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c core.sshCommand='rm -rf /tmp/x' status")
            )
            .is_none(),
            "sshCommand is not used by status"
        );
    }
}
