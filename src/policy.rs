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
    /// Command prefixes for `tool`. The first matching action wins.
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
/// Single quotes do not run `$(rm)`. A command substitution is checked
/// after the simple commands, so `git push $(rm)` stays `git.push`.
/// `bash -O extglob -c` and `bash +O extglob -c` match.
/// `find -exec rm`, `find -execdir rm`, and `find -ok rm` match.
/// `find -delete`, `find -name rm`, and `find -exec echo rm` do not.
/// `ssh host rm` and `flock file rm` match. `ssh host` and `ssh -p 22 rm`
/// do not: the word after the options is the destination, not the command.
/// `timeout 1 rm` and `xargs rm` match.
/// `timeout -- rm` keeps `rm` as the duration. `timeout 1 echo rm`
/// and `xargs echo rm` do not match. Homebrew names the same binaries
/// `gtimeout`, `gxargs`, `genv`, `gnice`, `gnohup`, `gstdbuf`, and `grm`.
/// `git push` matches `git push`, `git\tpush`, `/usr/bin/git push`, and
/// `git -C repo push`. `git.exe push` and `rm.exe` match those rows too.
/// One trailing `.exe`, `.cmd`, `.bat`, or `.com` is ignored, in any
/// ASCII case, including on `timeout.exe` and `busybox.exe`. Dots after
/// that suffix are ignored (`git.exe.`), and a quoted trailing space is
/// ignored (`"rm.exe "`).
/// A later command does not replace an earlier hit: `git push && rm` stays
/// `git.push`. `rmdir`, `git push-all`, `echo rm`, `find -delete`,
/// `command -v rm`, `sudo -l`, an empty word, a heredoc body, `rmdir.exe`,
/// `rm.exe.bak`, `rm.`, and `git.exe.exe` do not match. `cmd /c` is not
/// unwrapped.
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
    matched_in(policy, tool_name, command, 0)
}

const MAX_SHELL_DEPTH: u32 = 8;

fn matched_in<'a>(
    policy: &'a Policy,
    tool_name: &str,
    command: &str,
    depth: u32,
) -> Option<&'a ActionId> {
    if depth > MAX_SHELL_DEPTH {
        return None;
    }
    let (segments, substitutions) = tokenize_segments(command);
    for segment in segments {
        let Some((argv, split_script)) = executed_argv(&segment) else {
            continue;
        };
        if let Some(script) = split_script
            && let Some(id) = matched_in(policy, tool_name, &script, depth + 1)
        {
            return Some(id);
        }
        if let Some(script) = shell_script(&argv) {
            if let Some(id) = matched_in(policy, tool_name, &script, depth + 1) {
                return Some(id);
            }
            continue;
        }
        if let Some(script) = eval_script(&argv) {
            if let Some(id) = matched_in(policy, tool_name, &script, depth + 1) {
                return Some(id);
            }
            continue;
        }
        for (id, row) in &policy.actions {
            let Some(tool) = row.tool.as_deref() else {
                continue;
            };
            if !tool.eq_ignore_ascii_case(tool_name) {
                continue;
            }
            let hit = row.prefixes.iter().any(|prefix| {
                let words: Vec<String> = prefix.split_whitespace().map(str::to_string).collect();
                !words.is_empty() && argv_matches(&argv, &words)
            });
            if hit {
                return Some(id);
            }
        }
        for script in find_exec_commands(&argv) {
            if let Some(id) = matched_in(policy, tool_name, &script, depth + 1) {
                return Some(id);
            }
        }
        for script in [
            ssh_remote_command(&argv),
            flock_command(&argv),
            docker_command(&argv),
            parallel_command(&argv),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(id) = matched_in(policy, tool_name, &script, depth + 1) {
                return Some(id);
            }
        }
    }
    // A simple command wins over a substitution later in the same text.
    for script in substitutions {
        if let Some(id) = matched_in(policy, tool_name, &script, depth + 1) {
            return Some(id);
        }
    }
    None
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
    if !base.eq_ignore_ascii_case("docker") {
        return None;
    }
    let sub = argv.get(1)?;
    if sub != "exec" && sub != "run" {
        return None;
    }
    let mut index = 2;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            index += 1;
            break;
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
    if index >= argv.len() {
        return None;
    }
    Some(argv[index..].join(" "))
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

fn shell_script(tokens: &[String]) -> Option<String> {
    let base = command_basename(tokens.first()?);
    const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "ksh", "ash", "fish"];
    if !SHELLS.iter().any(|shell| base.eq_ignore_ascii_case(shell)) {
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
        if let Some(script) = c_argument(token, tokens.get(index + 1)) {
            return Some(script);
        }
        if token.starts_with('-') {
            index += 1;
            continue;
        }
        return None;
    }
    None
}

fn shell_opt_takes_value(token: &str) -> bool {
    let name = token.strip_prefix("--").unwrap_or(token);
    // Separate `-O` and `+O` take the next word. Glued `-Oextglob` does not.
    matches!(name, "-o" | "-O" | "+O" | "rcfile" | "init-file")
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

fn tokenize_segments(command: &str) -> (Vec<Vec<String>>, Vec<String>) {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    let mut token = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    let mut quoted = false;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut substitutions = Vec::new();
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
            '<' if chars.peek() == Some(&'<') => {
                chars.next();
                push_token(&mut token, &mut current, &mut quoted);
                if chars.peek() == Some(&'<') {
                    chars.next();
                    skip_here_word(&mut chars);
                } else if let Some(spec) = read_heredoc_delim(&mut chars) {
                    heredocs.push(spec);
                }
            }
            ' ' | '\t' | '\r' => push_token(&mut token, &mut current, &mut quoted),
            '\n' => {
                push_token(&mut token, &mut current, &mut quoted);
                push_segment(&mut current, &mut segments);
                for (delim, dash) in heredocs.drain(..) {
                    skip_until_delim(&mut chars, &delim, dash);
                }
            }
            '&' | '|' | ';' => {
                if matches!(ch, '&' | '|') && chars.peek() == Some(&ch) {
                    chars.next();
                }
                push_token(&mut token, &mut current, &mut quoted);
                push_segment(&mut current, &mut segments);
            }
            _ => token.push(ch),
        }
    }
    push_token(&mut token, &mut current, &mut quoted);
    push_segment(&mut current, &mut segments);
    (segments, substitutions)
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

fn skip_here_word<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) {
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
    }
    if matches!(chars.peek(), Some('\'' | '"')) {
        let quote = chars.next().unwrap_or('"');
        for ch in chars.by_ref() {
            if ch == quote {
                break;
            }
        }
        return;
    }
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            break;
        }
        chars.next();
    }
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

fn skip_until_delim<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
    delim: &str,
    dash: bool,
) {
    let mut line = String::new();
    loop {
        match chars.next() {
            None => break,
            Some('\n') => {
                let text = if dash {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if text == delim {
                    break;
                }
                line.clear();
            }
            Some(ch) => line.push(ch),
        }
    }
}

fn push_segment(current: &mut Vec<String>, segments: &mut Vec<Vec<String>>) {
    if !current.is_empty() {
        segments.push(std::mem::take(current));
    }
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
            "parallel rm ::: /tmp/x",
            "parallel -j 4 rm -rf /tmp ::: x",
            "parallel --jobs=4 rm ::: x",
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
            "git push && rm -rf /",
            "git push $(rm -rf /)",
            "find . -exec git push \\;",
            "env -S 'git push'",
            "cd x && git push && rm -rf /",
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
            "find . -delete",
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
            "git.exe push && rm.exe -rf /",
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
}
