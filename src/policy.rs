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
/// Past eight nested shells the remainder is not read. A non-blank remainder
/// matches `shell.deep` when that row exists, so it does not use `default_action`.
///
/// `rm` matches `rm`, `/bin/rm`, `RM`, `sudo rm`, `sudo -nu root rm`,
/// `FOO=1 rm`, `FOO+=1 rm`, `env rm`, `cd x && rm`, `bash -c 'rm ...'`, and
/// `env -S 'rm ...'`. A null byte is removed first, because bash
/// removes it: `rm` followed by a null and `-rf` matches, and `rm`
/// followed by a null and `dir` stays `rmdir`. `$'rm'` and `$"rm"`
/// match too, because bash runs those words as `rm`. An ANSI-C null
/// ends that word (`$'rm\x00dir'` is `rm`). `echo $'rm'` does not match.
/// `exec rm` and `eval 'rm ...'` match. `trap 'rm ...' EXIT` and
/// `trap rm EXIT` match too, because bash runs that action when the
/// signal arrives. The builtin is only the word `trap`. `TRAP`,
/// `/bin/trap`, and `sudo trap` do not run it. Keyword `time trap`
/// does. `command time trap` does not, because `command` runs the
/// `time` program. A signal bash rejects (`SIGEXIT`, `64`) does not
/// match unless another word is a real signal. `KILL`, `STOP`, and
/// `9` do not match: bash stores the handler and the kernel does not
/// deliver those signals. `trap - EXIT`,
/// `trap '' EXIT`, `trap EXIT`, and `trap -p` do not. `exec -a rm echo` and
/// `eval echo` do not. `if`, `then`, `else`, `elif`, `do`, `while`,
/// `until`, `!`, `{`, and `(` are skipped, so `if true; then rm`,
/// `{ rm; }`, and `(rm)` match. `for`
/// is not skipped. `$(rm)`, `echo "$(rm)"`, and a backtick `rm` match,
/// including inside double quotes and `$"..."`. Single quotes do not
/// run `$(rm)`. The stricter row wins across the
/// whole string, so `git push $(rm)` and `git push && rm` match `bash.rm`.
/// `git push $(echo rm)` stays `git.push`.
/// `bash -O extglob -c` and `bash +O extglob -c` match.
/// `find -exec rm`, `find -execdir rm`, `find -ok rm`, and `find -delete`
/// match. `find -name rm`, `find -name -delete`, and `find -exec echo rm`
/// do not. `ssh host rm` and `flock file rm` match. `ssh host` and
/// `ssh -p 22 rm` do not: the word after the options is the destination,
/// not the command. `ssh -o ProxyCommand=rm`, and the same `-o` on
/// `scp` and `sftp`, match that command. A value after `-p` or `-i`
/// does not hide a later `ProxyCommand`. The option name ignores ASCII
/// case. A space works the same as `=`, and whitespace around `=` is
/// ignored. An empty command and `none` do not match.
/// `git -c credential.helper='!rm' credential` matches, including
/// another ASCII case, `credential.https://example.com.helper`, and
/// `credential..helper`. An empty helper clears the ones already seen.
/// A helper with no `!` is a program name, not a shell. Section and
/// variable names ignore ASCII case. The subsection in `difftool.x.cmd`
/// stays case-sensitive. An alias name does not: `alias.P` runs for
/// `p` and `P`. `core.sshCommand` matches an ssh remote and a bare
/// `push`, and it does not match an `https://` URL or a local path.
/// `GIT_SSH_COMMAND`
/// replaces that config when the assignment is present, including when
/// the value is empty. `GIT_SSH` is one program path, used only when
/// neither command is set. `export NAME=value` lasts for a later command
/// in the same shell, including `builtin export`. A function named
/// `export` runs instead of that builtin. `export -n` drops the name.
/// A pipeline does not publish it. A bare assignment does not
/// reach a later `git` either. It does reach `eval` in this shell, so
/// `x='git push'; eval "$x"` matches `git.push`. `eval '$x'`,
/// `eval ${x}`, `eval "$x extra"`, `x+=`, `declare`, and `typeset` do
/// too. So do `{ x=...; }`, `then`, `!`, keyword `time`, and `do`.
/// `local` and `declare` with no value hide the global name inside the
/// function. `local` reaches `eval` in that function only.
/// `declare -x NAME` exports the current value, including inside a
/// function until it returns. A prefix assignment does not persist.
/// `x=... eval '$x'` still runs, because that `$x` expands inside eval.
/// `x=... eval "$x"` does not. A pipeline, `(x=...)`, and a background
/// command do not reach a later eval.
/// `readonly` sets that name for the whole shell, including inside a
/// function. `readonly -x` sets nothing. A command substitution that
/// only prints, such as `$(echo 'git push')` or `$(printf '%s' 'git push')`,
/// is the text bash would print. `eval` of that text and a bare
/// substitution match. `echo` of the same text does not.
/// A printer mixed with another command still inserts that text, and
/// the other command is scored. `echo ... | cat` prints the echo. The
/// last stage is the stdout, so `echo ... | true` prints nothing.
/// `echo ... && true` still prints. `false && echo` prints nothing.
/// `echo ... || true` prints the echo and does not run the right side.
/// `cat path` and `$(<path)` are a regular file, up to one megabyte.
/// `cat` of several paths keeps each file that exists. `cat < path`,
/// `grep . path`, and `head -n 1 path` are that text. `grep -h`,
/// `grep -i`, `grep -w`, and `grep -o '.*'` still print the matching
/// line. `grep -E` and `egrep` use that same match. `fgrep` is a fixed
/// string. `grep -G` is the basic match. `grep -P` and `ggrep` use
/// that same match. `gfgrep` is `fgrep`. `gegrep` is `egrep`. `grep -a` and `grep -I` still print a text line.
/// `grep -e git -e zzzz` prints a line that matches either pattern.
/// `grep -n`, `grep -H`,
/// and `grep -o .` do not. A missing path
/// is not those bytes. A larger file matches `shell.deep`.
/// `tail`, `sed -n p`, `awk '{print}'`, and `cut -c 1-` print that text
/// too. `sed -n 3p` and `awk 'NR==3'` print that line. `awk 'NR==1 {print}'`
/// and `awk '{ print }'` print it too. `awk '/git/'` prints a line
/// that contains `git`. `gawk` is that same `awk`.
/// `awk 'gsub(/git/,"git")'` prints that line too. `awk '{printf "%s\n", $0}'`
/// prints each line. `awk 'END {print}'` prints the last line. `head -c`
/// of a short file prints that file. `tail -c +1` prints it too.
/// `cat -u` still prints the file. `stdbuf -o0 cat` prints it too.
/// `gcat`, `ghead -c`, `gtail -c +1`, `gfold`, `gfmt`, `gpaste`, and `gsort`
/// print that file. `gcut -c 1-`, `gdd if=path`, `gtac`, `guniq`,
/// `gexpand`, `gunexpand`, `gpr -t`, `gtr`, `gtee`, and `gbase64 -d`
/// print it too. `timeout 1 cat`
/// and `gxargs cat` print it too. `env cat`, `nice cat`, and `nohup cat`
/// print the file. `genv`, `gnice`, and `gnohup` do too. `time cat`,
/// `exec cat`, and `sudo cat` print the file. `gsplit --filter=cat`
/// and `split --filter=cat` print the file. `gsplit -n 1/1` prints it
/// too. `gsplit --filter` runs that command. `while read -r`
/// then `printf` or `echo` of that name prints the file. `&&` between
/// the `read` and that print does too. `|| true`, `&& false`, and `&& :`
/// after that print still leave the line. `"${line}"` and `"${line[0]}"`
/// are that same name. `IFS=`, `builtin`, and `command` before `read`
/// or `mapfile` still print that line. `mapfile -t` then `echo` of `$x` prints the first
/// line, and `"${x[@]}"` prints every line. `mapfile -t` then
/// `printf` or `echo` of the first element prints that line. `read -r line < path`
/// then `printf` or `echo` of that name prints that line. Without `-n`,
/// `sed 3p` still prints every line. `sort`, `uniq`, and `tac` print
/// every line. `sort -s` and `sort -z` still print every line. `sort -c`
/// and `sort -o` do not. `uniq -d` prints a repeated line. `tac -s`
/// prints the same bytes. `sort --compress-program` runs that program.
/// `base64 -d` prints the decoded text. `-w0`, `--ignore-garbage`, and
/// `-di` do too. `openssl base64 -d -in` and
/// `openssl enc -a -d -in` print that text too. A redirect into
/// `openssl base64 -d` does too. `python3 -c` that writes
/// `base64.b64decode` of that file prints the decoded text. `ruby -e`
/// `unpack1("m")` of `File.binread` prints it too. `php -r` that
/// echoes `base64_decode` of `file_get_contents` prints it. Node
/// `Buffer.from` of that file with `"base64"` prints it. `perl
/// -MMIME::Base64` that prints `decode_base64` prints it too.
/// `basenc --base64 -d` prints that text. `basenc --base16 -d` prints
/// the decoded hex. `basenc --base32 -d` prints the decoded text.
/// `basenc --base64url`, `--base32hex`, `--base2msbf`, `--base2lsbf`,
/// and `--z85` do too. `--base58` does too. `-i` and `-w0` do not
/// change that decode. A separate `--wrap 0` does too.
/// `xxd -r -p` prints the
/// decoded text. `uudecode -p` prints the
/// decoded text. `gbasenc` is that same
/// `basenc`. `python3 -m base64 -d` and `-u` print it too.
/// `gzip -dc`, `gunzip -c`, and `gzcat` print the decompressed text.
/// `python3` `gzip.open`, `bz2.open`, and `lzma.open` print that text.
/// `gzip.decompress`, `bz2.decompress`, and `lzma.decompress` do too.
/// Either quote around `rb` is that read. `ruby -rzlib` `GzipReader`
/// and `Zlib.gunzip` print it. Node `zlib.gunzipSync` and `unzipSync`
/// print it. `perl -MIO::Uncompress::Gunzip` and `php` `gzdecode` print
/// it too. `zlib.decompress` and Node `inflateSync` print a zlib
/// payload. `unzipSync` prints gzip or zlib. `perl` `Bunzip2` and
/// `php` `bzdecompress` print a bzip2 payload. Node
/// `brotliDecompressSync` prints a brotli payload.
/// Those language readers do not read a compress payload.
/// `gzip -dc` and `uncompress -c` still do.
/// `gzip.GzipFile` prints a gzip payload. `Zlib::Inflate` prints a
/// zlib payload. Node `inflateRawSync` and `php` `gzinflate` print a
/// raw deflate payload. `bz2.BZ2File` and `lzma.LZMAFile` print
/// those payloads. `Zlib.inflate` and `php` `gzuncompress` print a
/// zlib payload. `perl` `RawInflate` prints a raw deflate payload.
/// `php` `bzread` of `bzopen` prints a bzip2 payload.
/// `BZ2Decompressor` and `LZMADecompressor` print those payloads.
/// `stream_get_contents` of `bzopen` prints a bzip2 payload.
/// `bzread` of `bzopen` with length 4096 prints it.
/// `compress.bzip2://` prints it. `zip://` of a named member prints
/// that member. `zipfile.Path` `read_bytes` prints it. `Archive::Zip`
/// `contents` prints it. `GzipReader.new` prints a gzip payload.
/// `GzipReader.open` with `&:read` prints it. `ZipFile.open` and
/// `namelist()[0]` print a zip member.
/// `gzip.open` in text mode, `gzfile`, and `compress.zlib://` print
/// a gzip payload. `GzipReader` `readlines` prints it too.
/// `bz2.open` and `lzma.open` in text mode print those payloads.
/// `gzread` of `gzopen` and `stream_get_contents` of `gzopen` print
/// a gzip payload. `gzgets`, `gzpassthru`, and `fread` of `gzopen`
/// print it too. `readgzfile` prints it. `GzipReader.wrap` prints it.
/// `gzip.open`
/// with `encoding` prints it. A `GzipReader` block prints it.
/// `perl` `AnyInflate` prints gzip, zlib, or raw deflate.
/// `bzip2 -dc` and `bzcat` do too. `xz -dc` and `xzcat` do too.
/// `unlzma -c` prints an lzma payload.
/// `zstd -dc` does too. `lz4 -dc` and `lz4cat` print an lz4 frame.
/// `brotli -dc` prints a brotli stream. `pzstd -dc` prints a zstd
/// frame. `tar -xOf` prints the named member. `unzip -p` prints that
/// member. `tar -xzOf` prints a member from a gzip archive. `-xJOf`
/// and `-xjOf` do too. `tar --zstd -xOf` prints a zstd member.
/// `uncompress -c` prints a compress payload. `tar -xOf` of a `.Z`
/// archive prints the member. `bsdtar -xOf` prints it too.
/// `gtar -I lz4` and `gtar -I brotli` print that member.
/// `gtar -xOf` of an lzma archive prints it too.
/// `python3` tarfile and zipfile reads print the member.
/// `perl -MArchive::Tar` prints that member.
/// `7z e -so` prints the archive.
/// `ruby -rbase64`
/// that prints `Base64.decode64` of `File.binread` prints it.
/// `binascii.a2b_base64` of that file prints it too.
/// `paste`, `expand`, `unexpand`, `column`, `fold`, `fmt`, `pr`, and
/// `strings` print that line too. `cat -v`, `cat -e`, `cat -t`, and
/// `cat -s` still print it. `tee` copies its input. `tr` and
/// `iconv -f utf-8 -t utf-8` print that text. `join` of a file with
/// itself prints the line. `gjoin` is that same `join`. `comm -12` and
/// `gcomm -12` print a line that is in both files. `gbase32 -d` prints
/// the decoded text. `perl -pe ''` and `ruby -pe ''` print each
/// line. `perl -ne 'print'` and `ruby -ne 'print'` print the current line.
/// `perl -pe 'print'` prints it too, unless the script clears `$_`.
/// `perl -0777 -pe ''` still prints the file. `perl -lpe ''` prints it too.
/// `perl -w -pe ''` prints it too. `perl -X -pe ''` prints it too.
/// `jq -Rr .` prints each raw line.
/// `base32 -d` prints the decoded text. `-i` does too. `dd if=path` copies that
/// file. `cut -d ' ' -f 1-` keeps the line. `sed -n l` prints the line.
/// `sed -n '/git/p'` prints a line that contains `git`.
/// `sed -n 's/git/git/p'` prints that line too.
/// `sed -n 1l` and `sed -n '$l'` print that line too. `gsed` is that
/// same `sed`. A python one-liner that writes `open(sys.argv[1]).read()`,
/// or prints that read with `end=""`,
/// a node one-liner that writes `readFileSync`, `ruby -e` that prints
/// `File.read`, `IO.read`, or `ARGF.read`, `ruby -e` that `puts` that read,
/// and `php -r` that echoes
/// or prints `file_get_contents` print the file. A node one-liner that
/// `console.log`s `readFileSync` prints it too.
/// `os.system(open(sys.argv[1]).read())`, node `execSync` of
/// `readFileSync`, `File.binread`, `IO.binread`, `Path.read_bytes`,
/// `open(..., "rb")`, `open(...).read().decode()`, `File.open(...).read`,
/// and `perl -e 'print <>'`, `File.readlines(...).join`,
/// `Path.read_bytes` after `from pathlib import Path`, and
/// `readFileSync` with `'utf8'`,
/// `perl -e 'print while <>'`, `while (<>) { print }`, and
/// `print $_ while <>` print the file. Python `print` of `open`
/// with `encoding='utf-8'`, `sys.stdout.write` of `Path.read_text()`,
/// and that read with `encoding='utf-8'` print it. `join("")`,
/// `File.foreach`, `IO.foreach`, and `puts File.readlines` print it.
/// `readFileSync` with `'utf-8'` or `"utf-8"`, and `require('fs')`
/// `readFileSync` with or without `'utf8'`, print it. PHP `implode`
/// of `file`, `stream_get_contents` of `fopen`, and `fread` of
/// `fopen` with length 4096 print it. `fgets` of `fopen` prints the
/// first line.
/// `print while <>;` prints the file. `from pathlib import Path`
/// before `import sys`, `sys.stdout.write` of `open` with
/// `encoding='utf-8'`, and `''.join` of `readlines` print it.
/// `File.read(...).each_line` and `File.read` with `encoding: "UTF-8"`
/// print it. Node `{readFileSync}` from `require('fs')` prints it.
/// PHP `foreach` of `file` and `file_get_contents` with `false` print it.
/// `while (<>) { print; }`, `readlines.join("\n")`, `read_text` with
/// `encoding='utf-8'`, a bare `fs=require('fs')` read, `{encoding:'utf8'}`,
/// and `print implode` of `file` print it too.
/// `console.log` of `readFileSync` with `'utf8'`, `print $_ while <>;`,
/// `File.read` with `mode: "r"`, `file_get_contents` with `true`,
/// `writelines` of an encoded `open`, and `puts` of `readlines.join`
/// print it.
/// `ruby -e 'system(File.read(ARGV[0]))'`, and `php -r`
/// `system(file_get_contents($argv[1]));` run that file.
/// `source <(echo 'git push')` and `bash <(echo 'git push')` run
/// the printed text. `mapfile` and `read < path` set a value for a later
/// `eval`. `printf -v name` stores a value for a later `eval`. Inside a
/// function that name is global unless it is already local. `read` and
/// `read -r` do that from one here-string, heredoc, or `< path`. No
/// input does not.
/// `status` does not run these,
/// and a later argument named `credential` is not that subcommand.
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
/// A shell that reads its script from stdin runs a heredoc or a
/// here-string, so `bash <<EOF` and `bash <<< 'rm'` match. `bash
/// /dev/stdin` and `bash -s` do too. `bash < path` and `bash -s < path`
/// score that file. `bash -c 'source /dev/stdin' < path` and
/// `bash -c 'bash -s' < path` do too. `bash -c 'echo hi' < path`
/// does not.
/// `source < path` does not run the file. `source /dev/stdin` runs that
/// body in the same shell. `source path` and `bash path` run a regular
/// file at that path. `cat path`, `head path`, and `tail path` score
/// the text those commands print, including `gcat`, `ghead`, and
/// `gtail`. An unquoted `C:\dir\file` keeps the backslashes, so
/// `bash`, `source`, and `cat` read that file. The quoted path and
/// `C:/dir/file` already did. A missing path stays unmatched.
/// `cat < path`, `head < path`, and `tail < path` score
/// that same text, including a single-quoted path and a double-quoted
/// path with no expansion. `head - < path`
/// and `tail - < path` do too.
/// A missing path does not match. `bash -c` and
/// `bash -s` do not read a later path. `cat` and `bash -c` do not run
/// a heredoc body.
/// An unquoted heredoc still expands `$(...)` and backticks first, so
/// `cat <<EOF` with `$(rm)` matches. A quoted delimiter does not.
/// Any quote in that word quotes it, so `cat <<E'OF'` and `cat <<$'EOF'`
/// do not expand, and the next command is still scored.
/// The here-string word is expanded first, so `cat <<< $(rm)` matches.
/// Inside double quotes, a backslash immediately before a newline is
/// removed, so a wrapped `bash -c "git push"` matches `git.push`.
/// A backslash before any other ordinary character stays, so
/// `"C:\dir\file"` keeps the separators. `$`, a backtick, `"`, and `\`
/// still escape. A backtick inside those quotes runs too. An unquoted
/// `#` comment does not run `$(rm)`. `<(rm)` and `>(rm)` do.
/// A redirect is not a word. `rm>/dev/null`, `>/dev/null rm`, and
/// `2>/dev/null rm` match. `echo rm>/tmp/x` does not, because `rm` is
/// only an argument. `2&>file rm` runs `2`, not `rm`.
/// `f() { rm; }; f`
/// matches. The same definition with no call does not. `cmd /c rm`,
/// `cmd -c rm`, and `powershell -Command rm` match. `powershell rm`
/// matches, because Windows PowerShell defaults a bare word to
/// `-Command`. `pwsh rm` does not match when that word is not a file:
/// `pwsh` defaults it to `-File`. A real file at that path is read.
/// `pwsh -i -c rm` matches, because `-i` takes no value.
/// `powershell -i xml -c rm` matches, and `xml` is the input format.
/// `pwsh -cwa` joins the command the same way as `-Command`. `-c`
/// stays `-Command`. Words after `-File` are script arguments, so
/// `powershell -File script.ps1 -Command rm` does not match when
/// `script.ps1` is missing. When the file exists, its body is scored
/// and the later `-Command` is not the engine command. `-File` and
/// `-f` read the first operand. `-File -` reads the heredoc or the
/// `< path` redirect, not a file named `-`. `powershell -Command` of
/// a `.ps1` path reads that file. Arguments after the path stay
/// arguments. `& path` reads that file. `Invoke-Expression` of
/// `Get-Content -Raw` reads that file when `-Raw` is before or after
/// the path, and when the same `Get-Content` is piped into
/// `Invoke-Expression`. `Get-Content` without `-Raw`, and `iex`, do
/// not. `-Command -` reads the heredoc and a `< path` redirect.
/// `cmd /c` of a `.bat` or `.cmd` path reads the file, and `call`
/// of that path does too.
/// `cmd /c rm` stays the command string. A missing file stays
/// unmatched. `powershell -EncodedCommand`, `-E`,
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
    let mut env = Vec::new();
    let mut scope = ShellScope::default();
    matched_in(
        policy,
        tool_name,
        command,
        0,
        &mut functions,
        &mut env,
        &mut scope,
        &GitBindings::default(),
    )
}

const MAX_SHELL_DEPTH: u32 = 8;

type Functions = std::collections::HashMap<String, String>;

#[allow(clippy::too_many_arguments)]
fn matched_in<'a>(
    policy: &'a Policy,
    tool_name: &str,
    command: &str,
    depth: u32,
    functions: &mut Functions,
    env: &mut Vec<(String, String)>,
    scope: &mut ShellScope,
    git: &GitBindings,
) -> Option<&'a ActionId> {
    if depth > MAX_SHELL_DEPTH {
        // The rest is not scanned. Missing it would score default_action, which can auto.
        if command.trim().is_empty() {
            return None;
        }
        return policy.actions.keys().find(|id| id.0 == "shell.deep");
    }
    let mut best = None;
    for part in tokenize_parts(command) {
        match part {
            ShellPart::Function { name, body } => {
                functions.insert(name, body);
            }
            ShellPart::Segment(segment) => {
                let subshell = segment.tokens.first().is_some_and(|token| token == "(");
                if segment.isolated || subshell {
                    let mut copy = env.clone();
                    let mut scoped = scope.clone();
                    consider_segment(
                        policy,
                        tool_name,
                        &segment,
                        depth,
                        functions,
                        &mut copy,
                        &mut scoped,
                        git,
                        &mut best,
                    );
                } else {
                    consider_segment(
                        policy, tool_name, &segment, depth, functions, env, scope, git, &mut best,
                    );
                }
            }
        }
    }
    best
}

// A child needs the exported environment and the git bindings.
// A prefix assignment is not published back.
#[allow(clippy::too_many_arguments)]
fn consider_segment<'a>(
    policy: &'a Policy,
    tool_name: &str,
    segment: &ShellSegment,
    depth: u32,
    functions: &mut Functions,
    exported: &mut Vec<(String, String)>,
    scope: &mut ShellScope,
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    // Expansions in the value run before the name is set.
    // `{`, `!`, and keyword `time` always run. `then` and `do` run only
    // on some paths.
    if let Some((assigned, conditional)) = assignment_suffix(&segment.tokens) {
        scan_subshells(
            policy,
            tool_name,
            &segment.substitutions,
            depth,
            functions,
            scope,
            exported,
            git,
            best,
        );
        for token in assigned {
            scope.assign_token(token, conditional);
        }
        return;
    }
    let Some(executed) = executed_argv(&segment.tokens, &segment.in_eval, exported) else {
        scan_subshells(
            policy,
            tool_name,
            &segment.substitutions,
            depth,
            functions,
            scope,
            exported,
            git,
            best,
        );
        return;
    };
    // `$(...)` does not see a prefix assignment. A child process does.
    let local = &executed.env;
    if let Some(path) = cd_target(&executed.argv) {
        scope.dir = Some(path);
    }
    // Non-interactive bash runs `BASH_ENV`. An interactive bash runs
    // `--rcfile` or `--init-file`. The other combination does not.
    if let Some(path) = bash_startup_file(&executed.argv, &executed.env) {
        score_script_path(policy, tool_name, path, depth, local, git, best);
    }
    if let Some(path) = zsh_startup_file(&executed.argv, &executed.env) {
        score_script_path(policy, tool_name, &path, depth, local, git, best);
    }
    // A function named `export` runs instead of the builtin.
    // `command export` and `builtin export` keep the builtin.
    let export_is_function = is_export(&executed.argv)
        && !executed.exec_wrapper
        && !executed.bypass_function
        && executed
            .argv
            .first()
            .is_some_and(|word| functions.contains_key(word));
    if is_export(&executed.argv) && !executed.exec_wrapper && !export_is_function {
        scan_subshells(
            policy,
            tool_name,
            &segment.substitutions,
            depth,
            functions,
            scope,
            exported,
            git,
            best,
        );
        apply_exports(exported, scope, &executed.argv, local);
        return;
    }
    let run_detached =
        |script: &str, functions: &mut Functions, best: &mut Option<&'a ActionId>| {
            let mut child_env = local.clone();
            let mut child_scope = ShellScope::default();
            prefer(
                policy,
                best,
                matched_in(
                    policy,
                    tool_name,
                    script,
                    depth + 1,
                    functions,
                    &mut child_env,
                    &mut child_scope,
                    git,
                ),
            );
        };
    if let Some(script) = &executed.split_script {
        let mut child = Functions::new();
        run_detached(script, &mut child, best);
    }
    // A batch path is a file, not the command string `cmd /c` would scan.
    if let Some(path) = cmd_batch_operand(&executed.argv) {
        score_script_path(policy, tool_name, path, depth, local, git, best);
    } else {
        let child_script = shell_script(&executed.argv)
            .or_else(|| cmd_script(&executed.argv))
            .or_else(|| powershell_script(&executed.argv))
            .or_else(|| su_script(&executed.argv));
        if let Some(script) = child_script {
            // `pwsh -Command -` reads the heredoc. The dash itself is not a command.
            // `bash -c -` is the script "-", not stdin.
            if let Some(path) = powershell_launched_file(&executed.argv, &script) {
                score_script_path(policy, tool_name, &path, depth, local, git, best);
            } else if is_stdin_script(&script)
                && is_powershell_base(executed.argv.first().map(String::as_str))
            {
                score_ps_input(
                    policy,
                    tool_name,
                    PsInput {
                        heredocs: &segment.heredocs,
                        stdin_file: segment.stdin_file.as_deref(),
                    },
                    depth,
                    local,
                    git,
                    best,
                );
            } else {
                let mut child = Functions::new();
                run_detached(&script, &mut child, best);
                if script_reads_stdin(&script)
                    && let Some(path) = segment.stdin_file.as_deref()
                {
                    score_script_path(policy, tool_name, path, depth, local, git, best);
                }
            }
        } else if let Some(path) = powershell_file_operand(&executed.argv) {
            if path == "-" {
                score_ps_input(
                    policy,
                    tool_name,
                    PsInput {
                        heredocs: &segment.heredocs,
                        stdin_file: segment.stdin_file.as_deref(),
                    },
                    depth,
                    local,
                    git,
                    best,
                );
            } else {
                score_script_path(policy, tool_name, path, depth, local, git, best);
            }
        } else if let Some(scripts) =
            eval_script(&executed.argv, &executed.in_eval, scope, exported, local)
        {
            for script in &scripts {
                score_here(
                    policy, tool_name, script, depth, functions, local, exported, scope, git, best,
                );
            }
        } else if !executed.exec_wrapper
            && !executed.bypass_function
            && let Some(body) = executed
                .argv
                .first()
                .and_then(|name| functions.get(name).cloned())
        {
            // `local` and `declare` inside the function end when it returns.
            // A bare assignment still changes this shell.
            scope.locals.push(Vec::new());
            score_here(
                policy, tool_name, &body, depth, functions, local, exported, scope, git, best,
            );
            scope.pop_frame(exported);
        } else {
            // `bash script` starts a new process. `source script` is handled
            // below, in this shell. `cat`, `head`, and `tail` score the text
            // they would print.
            score_shell_file(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_makefile(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_ninja(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_vim(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_package_scripts(policy, tool_name, &executed.argv, depth, local, git, best);
            score_package_exec(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_just_recipe(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_uv_run(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_swift_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_awk_program(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_osascript(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_bun_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_expect_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_tcl_script(policy, tool_name, &executed.argv, depth, local, git, best);
            score_sqlite_shell(policy, tool_name, &executed.argv, depth, local, git, best);
            score_octave_script(policy, tool_name, &executed.argv, depth, local, git, best);
            score_julia_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_elixir_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_erlang_script(policy, tool_name, &executed.argv, depth, local, git, best);
            score_go_run(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_cargo_run(policy, tool_name, &executed.argv, depth, local, git, best);
            score_coffee_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_haskell_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_mix_run(policy, tool_name, &executed.argv, depth, local, git, best);
            score_clojure_script(policy, tool_name, &executed.argv, depth, local, git, best);
            let host_dir = scope.dir.as_deref();
            let node_run = node_run_bodies(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &node_run, depth, local, git, best);
            let bun_tests = bun_test_sources(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &bun_tests, depth, local, git, best);
            let java_main = java_commands(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &java_main, depth, local, git, best);
            let kotlin = kotlin_commands(&executed.argv, scope.dir.as_deref());
            scan_isolated(policy, tool_name, &kotlin, depth, local, git, best);
            let lua_modules = lua_module_commands(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &lua_modules, depth, local, git, best);
            let loaded = inline_loaded_commands(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &loaded, depth, local, git, best);
            let julia_includes = julia_include_commands(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &julia_includes, depth, local, git, best);
            let octave_sources = octave_source_commands(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &octave_sources, depth, local, git, best);
            let generated = go_generate_commands(&executed.argv, host_dir);
            scan_isolated(policy, tool_name, &generated, depth, local, git, best);
            let preloaded = env_preload_commands(&executed.argv, local, host_dir);
            scan_isolated(policy, tool_name, &preloaded, depth, local, git, best);
            let awk_path = awk_path_commands(&executed.argv, local, host_dir);
            scan_isolated(policy, tool_name, &awk_path, depth, local, git, best);
            score_cmake_script(
                policy,
                tool_name,
                &executed.argv,
                depth,
                local,
                git,
                scope.dir.as_deref(),
                best,
            );
            score_sbcl_script(policy, tool_name, &executed.argv, depth, local, git, best);
            score_interpreter(
                policy,
                tool_name,
                InterpSource {
                    argv: &executed.argv,
                    stdin_file: segment.stdin_file.as_deref(),
                },
                depth,
                local,
                git,
                host_dir,
                best,
            );
            score_printed_file(
                policy,
                tool_name,
                &executed.argv,
                segment.stdin_file.as_deref(),
                depth,
                local,
                git,
                best,
            );
            if let Some(id) = prefix_hit(policy, tool_name, &executed.argv) {
                prefer(policy, best, Some(id));
            }
            if find_deletes(&executed.argv) {
                prefer(policy, best, destructive_rm_action(policy, tool_name));
            }
            let scripts = find_exec_commands(&executed.argv);
            scan_isolated(policy, tool_name, &scripts, depth, local, git, best);
            let compress = sort_compress_commands(&executed.argv);
            scan_isolated(policy, tool_name, &compress, depth, local, git, best);
            let filters = split_filter_commands(&executed.argv);
            scan_isolated(policy, tool_name, &filters, depth, local, git, best);
            let remotes = remote_scripts(&executed.argv);
            scan_isolated(policy, tool_name, &remotes, depth, local, git, best);
            let arms = case_arm_scripts(&executed.argv);
            scan_isolated(policy, tool_name, &arms, depth, local, git, best);
            let traps = if executed.blocks_shell_builtin {
                Vec::new()
            } else {
                trap_scripts(&executed.argv)
            };
            scan_isolated(policy, tool_name, &traps, depth, local, git, best);
            let found = git_alias_scripts(&executed.argv, local, git, scope.dir.as_deref());
            scan_isolated(policy, tool_name, &found.commands, depth, local, git, best);
            for body in &found.shells {
                let mut child = Functions::new();
                let mut child_env = local.clone();
                let mut child_scope = ShellScope::default();
                prefer(
                    policy,
                    best,
                    matched_in(
                        policy,
                        tool_name,
                        body,
                        depth + 1,
                        &mut child,
                        &mut child_env,
                        &mut child_scope,
                        &found.bindings,
                    ),
                );
            }
            if shell_reads_stdin(&executed.argv) {
                score_ps_input(
                    policy,
                    tool_name,
                    PsInput {
                        heredocs: &segment.heredocs,
                        stdin_file: segment.stdin_file.as_deref(),
                    },
                    depth,
                    local,
                    git,
                    best,
                );
            }
        }
    }
    // A function named `source` runs instead of the builtin.
    // `command source` still reads the file.
    if !call_runs_function(&executed, functions) {
        if sourced_stdin(&executed.argv) {
            for body in &segment.heredocs {
                score_here(
                    policy, tool_name, body, depth, functions, local, exported, scope, git, best,
                );
            }
        } else {
            score_sourced_file(
                policy,
                tool_name,
                &executed.argv,
                depth,
                functions,
                local,
                exported,
                scope,
                git,
                best,
            );
        }
    }
    scan_subshells(
        policy,
        tool_name,
        &segment.substitutions,
        depth,
        functions,
        scope,
        exported,
        git,
        best,
    );
    // The words are expanded before `declare` stores them.
    scope.bind_declaration(&executed.argv, exported);
    // A function named `printf`, `read`, or `mapfile` runs instead.
    if !call_runs_function(&executed, functions) {
        scope.bind_printf_v(&executed.argv);
        let input = SegmentInput {
            heredocs: &segment.heredocs,
            file: segment.stdin_file.as_deref(),
            proc_body: segment.stdin_proc.as_deref(),
            heredoc: segment.read_heredoc,
        };
        if scope.bind_read(&executed.argv, &input, depth) {
            prefer(policy, best, unscanned_action(policy));
        }
        let bound = scope.bind_mapfile(&executed.argv, &input, depth);
        if bound.unscanned {
            prefer(policy, best, unscanned_action(policy));
        }
        if let Some(callback) = bound.callback {
            score_here(
                policy, tool_name, &callback, depth, functions, local, exported, scope, git, best,
            );
        }
        score_proc_input(
            policy,
            tool_name,
            &executed.argv,
            &segment.proc_inputs,
            depth,
            functions,
            local,
            exported,
            scope,
            git,
            best,
        );
    }
}

fn is_export(argv: &[String]) -> bool {
    argv.first()
        .is_some_and(|word| command_basename(word) == "export")
}

/// The first word names a function, and `command` or `builtin` did not
/// peel it. That function runs instead of a builtin with the same name.
fn call_runs_function(executed: &ExecutedArgv, functions: &Functions) -> bool {
    !executed.exec_wrapper
        && !executed.bypass_function
        && executed
            .argv
            .first()
            .is_some_and(|word| functions.contains_key(word))
}

/// `export NAME=value` lasts for later commands in this shell.
/// `export NAME` copies the prefix value. `export -n` drops the name.
/// A prefix assignment does not.
fn apply_exports(
    exported: &mut Vec<(String, String)>,
    scope: &mut ShellScope,
    argv: &[String],
    local: &[(String, String)],
) {
    let mut unexport = false;
    let mut options = true;
    for token in argv.iter().skip(1) {
        if options {
            if token == "--" {
                options = false;
                continue;
            }
            if let Some(drop_name) = export_drop_flag(token) {
                unexport |= drop_name;
                continue;
            }
        }
        if unexport {
            if let Some((name, _)) = plain_assignment(token) {
                unset_env(exported, name);
            } else if env_name(token) {
                unset_env(exported, token);
            }
            continue;
        }
        if apply_assignment(exported, token) {
            if let Some(name) = assignment_name(token)
                && let Some(value) = last_assignment(exported, name)
            {
                scope.set_visible(name, value);
            }
            continue;
        }
        let copied = if env_name(token) {
            last_assignment(local, token)
                .map(str::to_string)
                .or_else(|| scope.lookup(token).map(str::to_string))
        } else {
            None
        };
        if let Some(value) = copied {
            set_env(exported, token, &value);
            scope.set_visible(token, &value);
        }
    }
}

/// `Some(true)` is `-n`: a later command must not see that name.
/// `Some(false)` is `-f` or `-p`, which is not an assignment.
fn export_drop_flag(token: &str) -> Option<bool> {
    let letters = token.strip_prefix('-')?;
    if letters.is_empty() || !letters.chars().all(|ch| matches!(ch, 'n' | 'f' | 'p')) {
        return None;
    }
    Some(letters.contains('n'))
}

/// Names assigned in this shell. A child process does not see `global`
/// unless `export` copied the name. `locals` is one frame per function
/// call. `eval` reads the frames and then `global`. `alts` are the other
/// values a `then` or `do` assignment might still hold. `saved_exports`
/// puts back a function-local `declare -x` when the function returns.
/// `arrays` are `mapfile` and `readarray`. A scalar assignment drops the
/// array at that depth.
#[derive(Clone, Default)]
struct ShellScope {
    /// Directory set by `cd` for a later command in this shell.
    dir: Option<String>,
    global: Vec<(String, String)>,
    locals: Vec<Vec<(String, String)>>,
    alts: Vec<ShellAlt>,
    saved_exports: Vec<SavedExport>,
    arrays: Vec<ShellArray>,
}

#[derive(Clone)]
struct ShellArray {
    /// `0` is global. A function frame is `index + 1`.
    depth: usize,
    name: String,
    values: Vec<String>,
}

#[derive(Clone)]
struct ShellAlt {
    /// `0` is global. A function frame is `index + 1`.
    depth: usize,
    name: String,
    value: String,
}

#[derive(Clone)]
struct SavedExport {
    depth: usize,
    name: String,
    previous: Option<String>,
}

impl ShellScope {
    fn lookup(&self, name: &str) -> Option<&str> {
        for frame in self.locals.iter().rev() {
            if let Some(value) = last_assignment(frame, name) {
                return Some(value);
            }
        }
        last_assignment(&self.global, name)
    }

    fn value<'a>(&'a self, name: &str, exported: &'a [(String, String)]) -> Option<&'a str> {
        self.lookup(name)
            .or_else(|| last_assignment(exported, name))
    }

    fn local_frame(&self, name: &str) -> Option<usize> {
        self.locals
            .iter()
            .rposition(|frame| frame.iter().any(|(key, _)| key == name))
    }

    fn assign_token(&mut self, token: &str, conditional: bool) -> bool {
        let Some(name) = assignment_name(token) else {
            return false;
        };
        let index = self.local_frame(name);
        let applied = if let Some(index) = index {
            apply_assignment(&mut self.locals[index], token)
        } else {
            apply_assignment(&mut self.global, token)
        };
        if !applied {
            return false;
        }
        let depth = self.local_frame(name).map(|index| index + 1).unwrap_or(0);
        self.drop_array(name, depth);
        if conditional {
            if let Some(value) = self.lookup(name) {
                self.alts.push(ShellAlt {
                    depth,
                    name: name.to_string(),
                    value: value.to_string(),
                });
            }
        } else {
            self.alts
                .retain(|alt| alt.name != name || alt.depth != depth);
        }
        true
    }

    /// Other values of `name` that `eval` should score. A local frame
    /// hides a global alternative. An inner function hides an outer one.
    fn alt_values(&self, name: &str) -> Vec<String> {
        self.alts
            .iter()
            .filter(|alt| alt.name == name && self.alt_visible(alt))
            .map(|alt| alt.value.clone())
            .collect()
    }

    fn alt_visible(&self, alt: &ShellAlt) -> bool {
        if alt.depth == 0 {
            return self.local_frame(&alt.name).is_none();
        }
        if alt.depth > self.locals.len() {
            return false;
        }
        !self.locals[alt.depth..]
            .iter()
            .any(|frame| frame.iter().any(|(key, _)| key == &alt.name))
    }

    fn pop_frame(&mut self, exported: &mut Vec<(String, String)>) {
        let dropped = self.locals.len();
        self.locals.pop();
        let restores: Vec<(String, Option<String>)> = self
            .saved_exports
            .iter()
            .rev()
            .filter(|save| save.depth == dropped)
            .map(|save| (save.name.clone(), save.previous.clone()))
            .collect();
        for (name, previous) in restores {
            match previous {
                Some(value) => set_env(exported, &name, &value),
                None => unset_env(exported, &name),
            }
        }
        self.saved_exports.retain(|save| save.depth != dropped);
        let depth_now = self.locals.len();
        self.alts.retain(|alt| alt.depth <= depth_now);
        self.arrays.retain(|array| array.depth <= depth_now);
    }

    fn set_visible(&mut self, name: &str, value: &str) {
        let index = self.local_frame(name);
        let depth = index.map(|slot| slot + 1).unwrap_or(0);
        self.drop_array(name, depth);
        if let Some(index) = index {
            set_env(&mut self.locals[index], name, value);
        } else {
            set_env(&mut self.global, name, value);
        }
    }

    fn drop_array(&mut self, name: &str, depth: usize) {
        self.arrays
            .retain(|array| array.name != name || array.depth != depth);
    }

    fn drop_scalar(&mut self, name: &str, depth: usize) {
        if depth == 0 {
            self.global.retain(|(key, _)| key != name);
            return;
        }
        if let Some(frame) = self.locals.get_mut(depth - 1) {
            frame.retain(|(key, _)| key != name);
        }
    }

    /// `mapfile` with no `local` is global, including inside a function.
    /// A name that is already local stays in that frame.
    fn set_array(&mut self, name: &str, values: Vec<String>) {
        let depth = self.local_frame(name).map(|index| index + 1).unwrap_or(0);
        self.drop_array(name, depth);
        self.drop_scalar(name, depth);
        self.arrays.push(ShellArray {
            depth,
            name: name.to_string(),
            values,
        });
    }

    /// The array at the depth that hides outer frames. A scalar at that
    /// depth hides an outer array, so this returns nothing.
    fn visible_array(&self, name: &str) -> Option<&[String]> {
        for (index, frame) in self.locals.iter().enumerate().rev() {
            let depth = index + 1;
            let array = self
                .arrays
                .iter()
                .find(|item| item.depth == depth && item.name == name);
            if array.is_some() || last_assignment(frame, name).is_some() {
                return array.map(|item| item.values.as_slice());
            }
        }
        let array = self
            .arrays
            .iter()
            .find(|item| item.depth == 0 && item.name == name);
        if array.is_some() || last_assignment(&self.global, name).is_some() {
            return array.map(|item| item.values.as_slice());
        }
        None
    }

    /// `declare` and `typeset` set a shell variable. Inside a function
    /// they stay in that function unless `-g` is set. `local` does
    /// nothing at the top level. `readonly` sets a global name, including
    /// from inside a function. `-x` on `declare` exports the current
    /// value until a function returns. `readonly -x` is not an option,
    /// so that command sets nothing.
    fn bind_declaration(&mut self, argv: &[String], exported: &mut Vec<(String, String)>) {
        let Some(first) = argv.first() else {
            return;
        };
        let base = command_basename(first);
        let declare_like =
            base.eq_ignore_ascii_case("declare") || base.eq_ignore_ascii_case("typeset");
        let local = base.eq_ignore_ascii_case("local");
        let readonly = base.eq_ignore_ascii_case("readonly");
        if !declare_like && !local && !readonly {
            return;
        }
        if local && self.locals.is_empty() {
            return;
        }
        let mut to_global = readonly || (declare_like && self.locals.is_empty());
        let mut also_export = false;
        let mut options = true;
        for token in argv.iter().skip(1) {
            if options {
                if token == "--" {
                    options = false;
                    continue;
                }
                if let Some(flags) = declaration_flags(token) {
                    if readonly {
                        // `-a`, `-A`, and `-p` still accept `name=value`.
                        // `-f` is a function. Any other letter is an error
                        // and bash stores nothing (`readonly -x`).
                        let plain = flags.chars().all(|ch| matches!(ch, 'a' | 'A' | 'p'));
                        if !plain {
                            return;
                        }
                        continue;
                    }
                    if flags.contains('g') {
                        to_global = true;
                    }
                    if flags.contains('x') {
                        also_export = true;
                    }
                    continue;
                }
            }
            if let Some(name) = assignment_name(token) {
                if self.assign_decl(token, to_global) && also_export {
                    self.export_here(name, to_global, exported);
                }
                continue;
            }
            if env_name(token) {
                self.remember_name(token, to_global);
                if also_export {
                    self.export_here(token, to_global, exported);
                }
            }
        }
    }

    /// `printf -v name format args` stores that text in the shell.
    /// It is not an export. Inside a function the name is global unless
    /// it is already local. `--` after the name ends the flags. `--`
    /// before `-v`, or any other dash flag, is left alone. Trailing
    /// newlines stay in the value.
    fn bind_printf_v(&mut self, argv: &[String]) {
        let Some(first) = argv.first() else {
            return;
        };
        if command_basename(first) != "printf" {
            return;
        }
        if argv.get(1).is_none_or(|token| token != "-v") {
            return;
        }
        let Some(name) = argv.get(2) else {
            return;
        };
        if !env_name(name) {
            return;
        }
        let mut index = 3usize;
        if argv.get(index).is_some_and(|token| token == "--") {
            index += 1;
        }
        let Some(text) = printf_literal(&argv[index..]) else {
            return;
        };
        self.set_visible(name, &text);
    }

    /// `read` and `read -r` assign from one here-string, heredoc, or
    /// `< path`. The first line only. Other flags do not assign. No
    /// input does not invent stdin. No name stores `REPLY`. One name
    /// drops leading and trailing space, tab, and newline. Later names
    /// split on those characters. Internal spaces stay. Returns whether
    /// the file is past one megabyte.
    fn bind_read(&mut self, argv: &[String], input: &SegmentInput<'_>, depth: u32) -> bool {
        let Some(first) = argv.first() else {
            return false;
        };
        if command_basename(first) != "read" {
            return false;
        }
        let mut index = 1usize;
        let mut options = true;
        let mut names = Vec::new();
        while index < argv.len() {
            let token = &argv[index];
            if options {
                if token == "--" {
                    options = false;
                    index += 1;
                    continue;
                }
                if let Some(flags) = token.strip_prefix('-') {
                    if flags.is_empty() || !flags.chars().all(|ch| ch == 'r') {
                        return false;
                    }
                    index += 1;
                    continue;
                }
            }
            if !env_name(token) {
                return false;
            }
            names.push(token.clone());
            index += 1;
        }
        if names.is_empty() {
            names.push("REPLY".to_string());
        }
        let body = match command_text(input, depth) {
            CmdText::Text(body) => body,
            CmdText::Unscanned => return true,
            CmdText::Absent => return false,
        };
        let line = body.split('\n').next().unwrap_or("");
        let fields = read_ifs_fields(line, names.len());
        for (name, value) in names.iter().zip(fields.iter()) {
            self.set_visible(name, value);
        }
        false
    }

    /// `mapfile` and `readarray` fill an array from the same input as
    /// `read`. No name stores `MAPFILE`. `-n 0` copies every record.
    /// The callback runs when enough records were read.
    fn bind_mapfile(
        &mut self,
        argv: &[String],
        input: &SegmentInput<'_>,
        depth: u32,
    ) -> MapfileBound {
        let Some(opts) = mapfile_opts(argv) else {
            return MapfileBound::default();
        };
        let text = match command_text(input, depth) {
            CmdText::Absent => return MapfileBound::default(),
            CmdText::Unscanned => {
                return MapfileBound {
                    unscanned: true,
                    callback: opts.callback,
                };
            }
            CmdText::Text(text) => text,
        };
        let raw = mapfile_records(&text, opts.delim, opts.strip);
        let lines_read = match opts.limit {
            Some(limit) => raw.len().min(opts.skip.saturating_add(limit)),
            None => raw.len(),
        };
        let callback = opts.callback.filter(|_| lines_read >= opts.quantum);
        if opts.origin > MAPFILE_ORIGIN_CAP {
            return MapfileBound {
                unscanned: true,
                callback,
            };
        }
        let kept = raw.into_iter().skip(opts.skip);
        let kept: Vec<String> = match opts.limit {
            Some(limit) => kept.take(limit).collect(),
            None => kept.collect(),
        };
        let mut values = Vec::with_capacity(opts.origin.saturating_add(kept.len()));
        values.resize(opts.origin, String::new());
        values.extend(kept);
        self.set_array(&opts.name, values);
        MapfileBound {
            unscanned: false,
            callback,
        }
    }

    /// `local x` with no value hides the outer name. It does not clear a
    /// value this frame already has.
    fn remember_name(&mut self, name: &str, to_global: bool) {
        if to_global || self.locals.is_empty() {
            if self.local_frame(name).is_none() && last_assignment(&self.global, name).is_none() {
                self.global.push((name.to_string(), String::new()));
            }
            return;
        }
        if let Some(frame) = self.locals.last_mut()
            && !frame.iter().any(|(key, _)| key == name)
        {
            frame.push((name.to_string(), String::new()));
        }
    }

    /// `declare -x NAME` exports the current shell value. Inside a
    /// function that export ends when the function returns.
    fn export_here(&mut self, name: &str, to_global: bool, exported: &mut Vec<(String, String)>) {
        let Some(value) = self.lookup(name).map(str::to_string) else {
            return;
        };
        if !to_global && !self.locals.is_empty() {
            let depth = self.locals.len();
            if !self
                .saved_exports
                .iter()
                .any(|item| item.depth == depth && item.name == name)
            {
                let previous = last_assignment(exported, name).map(str::to_string);
                self.saved_exports.push(SavedExport {
                    depth,
                    name: name.to_string(),
                    previous,
                });
            }
        }
        set_env(exported, name, &value);
    }

    fn assign_decl(&mut self, token: &str, to_global: bool) -> bool {
        let Some(name) = assignment_name(token) else {
            return false;
        };
        let depth = if to_global || self.locals.is_empty() {
            0
        } else {
            self.locals.len()
        };
        let applied = if depth == 0 {
            apply_assignment(&mut self.global, token)
        } else if let Some(frame) = self.locals.last_mut() {
            apply_assignment(frame, token)
        } else {
            false
        };
        if applied {
            self.drop_array(name, depth);
        }
        applied
    }
}

/// `read` / `mapfile` input that won on this command. A later redirect
/// replaces an earlier one. `heredoc` is set only when a here-string or
/// heredoc is the one `read` uses.
struct SegmentInput<'a> {
    heredocs: &'a [String],
    file: Option<&'a str>,
    proc_body: Option<&'a str>,
    heredoc: bool,
}

enum CmdText {
    Text(String),
    Unscanned,
    Absent,
}

fn command_text(input: &SegmentInput<'_>, depth: u32) -> CmdText {
    if let Some(body) = input.proc_body {
        return match fold_print(body, depth + 1, &mut Vec::new()) {
            Printed::Text(text) => CmdText::Text(text),
            Printed::Unscanned => CmdText::Unscanned,
            Printed::Unknown => CmdText::Absent,
        };
    }
    if let Some(path) = input.file {
        return match load_script_file(path) {
            Some(ScriptFile::Text(text)) => CmdText::Text(text),
            Some(ScriptFile::Unscanned) => CmdText::Unscanned,
            None => CmdText::Absent,
        };
    }
    if input.heredoc
        && let Some(body) = input.heredocs.last()
    {
        return CmdText::Text(body.clone());
    }
    CmdText::Absent
}

/// Past this many empty slots, `mapfile -O` matches `shell.deep`
/// instead of allocating the index.
const MAPFILE_ORIGIN_CAP: usize = 100_000;

#[derive(Default)]
struct MapfileBound {
    unscanned: bool,
    callback: Option<String>,
}

struct MapfileOpts {
    name: String,
    strip: bool,
    /// `None` copies every record. `-n 0` is unlimited.
    limit: Option<usize>,
    skip: usize,
    origin: usize,
    delim: char,
    callback: Option<String>,
    /// Bash runs the callback after this many records. The default is
    /// 5000. Zero is rejected by bash.
    quantum: usize,
}

fn mapfile_opts(argv: &[String]) -> Option<MapfileOpts> {
    let base = command_basename(argv.first()?);
    if base != "mapfile" && base != "readarray" {
        return None;
    }
    let mut opts = MapfileOpts {
        name: String::new(),
        strip: false,
        limit: None,
        skip: 0,
        origin: 0,
        delim: '\n',
        callback: None,
        quantum: 5000,
    };
    let mut options = true;
    let mut saw_name = false;
    let mut index = 1usize;
    while index < argv.len() {
        let token = &argv[index];
        if options && token == "--" {
            options = false;
            index += 1;
            continue;
        }
        if options && token.starts_with('-') {
            if !take_mapfile_flag(argv, &mut index, &mut opts) {
                return None;
            }
            continue;
        }
        if saw_name || !env_name(token) {
            return None;
        }
        opts.name = token.clone();
        saw_name = true;
        index += 1;
    }
    if opts.name.is_empty() {
        opts.name = "MAPFILE".to_string();
    }
    Some(opts)
}

fn take_mapfile_flag(argv: &[String], index: &mut usize, opts: &mut MapfileOpts) -> bool {
    let arg = &argv[*index];
    if arg == "-t" {
        opts.strip = true;
        *index += 1;
        return true;
    }
    if let Some(count) = attached_count(argv, index, "-n") {
        opts.limit = (count > 0).then_some(count);
        return true;
    }
    if let Some(count) = attached_count(argv, index, "-s") {
        opts.skip = count;
        return true;
    }
    if let Some(count) = attached_count(argv, index, "-O") {
        opts.origin = count;
        return true;
    }
    if let Some(count) = attached_count(argv, index, "-c") {
        if count == 0 {
            return false;
        }
        opts.quantum = count;
        return true;
    }
    if arg == "-d" {
        let Some(raw) = argv.get(*index + 1) else {
            return false;
        };
        opts.delim = raw.chars().next().unwrap_or('\0');
        *index += 2;
        return true;
    }
    if let Some(rest) = arg.strip_prefix("-d")
        && !rest.is_empty()
    {
        opts.delim = rest.chars().next().unwrap_or('\0');
        *index += 1;
        return true;
    }
    if arg == "-u" {
        if argv.get(*index + 1).is_some_and(|text| text == "0") {
            *index += 2;
            return true;
        }
        return false;
    }
    if arg == "-u0" {
        *index += 1;
        return true;
    }
    if arg == "-C" {
        let Some(callback) = argv.get(*index + 1) else {
            return false;
        };
        opts.callback = Some(callback.clone());
        *index += 2;
        return true;
    }
    if let Some(rest) = arg.strip_prefix("-C")
        && !rest.is_empty()
    {
        opts.callback = Some(rest.to_string());
        *index += 1;
        return true;
    }
    false
}

/// `-n 4` or `-n4`. `None` when `flag` is not this word.
fn attached_count(argv: &[String], index: &mut usize, flag: &str) -> Option<usize> {
    let arg = argv.get(*index)?;
    if arg == flag {
        let count = line_count(argv.get(*index + 1)?)?;
        *index += 2;
        return Some(count);
    }
    let rest = arg.strip_prefix(flag)?;
    if rest.is_empty() {
        return None;
    }
    let count = line_count(rest)?;
    *index += 1;
    Some(count)
}

fn mapfile_records(text: &str, delim: char, strip: bool) -> Vec<String> {
    let mut records = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch == delim {
            if !strip {
                current.push(ch);
            }
            records.push(std::mem::take(&mut current));
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        records.push(current);
    }
    records
}

fn declaration_flags(token: &str) -> Option<&str> {
    let flags = token.strip_prefix('-')?;
    if flags.is_empty() || !flags.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }
    Some(flags)
}

fn publish_env_changes(
    parent: &mut Vec<(String, String)>,
    before: &[(String, String)],
    after: &[(String, String)],
) {
    for (key, value) in after {
        if last_assignment(before, key) != Some(value.as_str()) {
            set_env(parent, key, value);
        }
    }
}

/// Score `script` in this shell. Assignments it makes stay here.
/// `exported` only gains names the script exports.
#[allow(clippy::too_many_arguments)]
fn score_here<'a>(
    policy: &'a Policy,
    tool_name: &str,
    script: &str,
    depth: u32,
    functions: &mut Functions,
    local: &[(String, String)],
    exported: &mut Vec<(String, String)>,
    scope: &mut ShellScope,
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let before = local.to_vec();
    let mut child_env = local.to_vec();
    prefer(
        policy,
        best,
        matched_in(
            policy,
            tool_name,
            script,
            depth + 1,
            functions,
            &mut child_env,
            scope,
            git,
        ),
    );
    publish_env_changes(exported, &before, &child_env);
}

/// A new process. It does not see shell functions from this string.
fn scan_isolated<'a>(
    policy: &'a Policy,
    tool_name: &str,
    scripts: &[String],
    depth: u32,
    env: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    for script in scripts {
        let mut child = Functions::new();
        let mut child_env = env.to_vec();
        let mut child_scope = ShellScope::default();
        prefer(
            policy,
            best,
            matched_in(
                policy,
                tool_name,
                script,
                depth + 1,
                &mut child,
                &mut child_env,
                &mut child_scope,
                git,
            ),
        );
    }
}

fn unscanned_action(policy: &Policy) -> Option<&ActionId> {
    policy.actions.keys().find(|id| id.0 == "shell.deep")
}

/// `cat`, `head`, `tail`, `fold`, `fmt`, `paste`, `expand`,
/// `dd if=path`, `iconv` of one charset, `perl -pe`, `tee`, `join` of a
/// file with itself, `comm -12`, `jq -Rr .`, `python3 -c` that reads
/// `sys.argv[1]`, `node -e` that reads `process.argv[1]`, `php -r`
/// that reads `$argv[1]`, `split --filter=cat`, `tr` of a redirect, and
/// `base64 -d`, `basenc --base64 -d`, `gzip -dc`, `bzip2 -dc`,
/// `xz -dc`, `zstd -dc`, `lz4 -dc`, `brotli -dc`, `tar -xOf`,
/// `unzip -p`, `7z e -so`, `base32 -d`, `openssl base64 -d`,
/// `xxd -r -p`, `uudecode -p`, `uncompress -c`, and `unlzma -c` print
/// a regular file.
/// That text is scored the same way as `bash path`. A missing path, a
/// directory, and an empty print stay unmatched. A file past one
/// megabyte is `shell.deep`. `cat < path` and `fold < path` read that
/// redirect. A file operand still wins, so `cat other < path` scores
/// `other`.
#[allow(clippy::too_many_arguments)]
fn score_printed_file<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    stdin: Option<&str>,
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let Some(first) = argv.first() else {
        return;
    };
    let name = stage_command_name(first);
    let args = &argv[1..];
    if !matches!(
        name.as_str(),
        "cat"
            | "head"
            | "tail"
            | "grep"
            | "egrep"
            | "fgrep"
            | "ggrep"
            | "sed"
            | "gsed"
            | "awk"
            | "gawk"
            | "cut"
            | "sort"
            | "uniq"
            | "tac"
            | "paste"
            | "expand"
            | "unexpand"
            | "column"
            | "fold"
            | "fmt"
            | "pr"
            | "strings"
            | "dd"
            | "iconv"
            | "perl"
            | "ruby"
            | "tee"
            | "join"
            | "comm"
            | "jq"
            | "python"
            | "python3"
            | "node"
            | "php"
            | "split"
            | "tr"
            | "base64"
            | "base32"
            | "basenc"
            | "openssl"
            | "xxd"
            | "uudecode"
            | "uncompress"
            | "gzip"
            | "gunzip"
            | "gzcat"
            | "zcat"
            | "bzip2"
            | "bunzip2"
            | "bzcat"
            | "xz"
            | "unxz"
            | "xzcat"
            | "lzma"
            | "unlzma"
            | "lzcat"
            | "zstd"
            | "unzstd"
            | "zstdcat"
            | "pzstd"
            | "lz4"
            | "unlz4"
            | "lz4cat"
            | "brotli"
            | "tar"
            | "unzip"
            | "7z"
            | "7za"
            | "7zz"
    ) {
        return;
    }
    let stdin_body = stdin.map(path_body);
    if matches!(stdin_body, Some(PathBody::Unscanned)) && printer_reads_stdin(&name, args) {
        prefer(policy, best, unscanned_action(policy));
        return;
    }
    let input = match &stdin_body {
        Some(PathBody::Text(text)) => Some(text.as_str()),
        _ => None,
    };
    let mut substitutions = Vec::new();
    let run = match name.as_str() {
        "cat" => cat_run(args, input, depth, &mut substitutions),
        "head" => head_run(args, input, depth, &mut substitutions),
        "tail" => tail_run(args, input, depth, &mut substitutions),
        "grep" | "egrep" | "fgrep" | "ggrep" => {
            grep_run(&name, args, input, depth, &mut substitutions)
        }
        "sed" | "gsed" => sed_run(args, input, depth, &mut substitutions),
        "awk" | "gawk" => awk_run(args, input, depth, &mut substitutions),
        "cut" => cut_run(args, input, depth, &mut substitutions),
        "sort" => sort_run(args, input, depth, &mut substitutions),
        "uniq" => uniq_run(args, input, depth, &mut substitutions),
        "tac" => tac_run(args, input, depth, &mut substitutions),
        "paste" | "expand" | "unexpand" | "column" | "fold" | "fmt" | "pr" | "strings" => {
            identity_run(&name, args, input, depth, &mut substitutions)
        }
        "dd" => dd_run(args, input, depth, &mut substitutions),
        "iconv" => iconv_run(args, input, depth, &mut substitutions),
        "perl" | "ruby" => line_print_run(&name, args, input, depth, &mut substitutions),
        "tee" => tee_run(args, input),
        "join" => join_run(args, input, depth, &mut substitutions),
        "comm" => comm_run(args, input, depth, &mut substitutions),
        "jq" => jq_run(args, input, depth, &mut substitutions),
        "python" | "python3" => python_cat_run(args, input, depth, &mut substitutions),
        "node" => node_cat_run(args, input, depth, &mut substitutions),
        "php" => php_cat_run(args, input, depth, &mut substitutions),
        "split" => split_run(args, input, depth, &mut substitutions),
        "tr" => tr_run(args, input),
        "base64" => base64_run(args, input, depth, &mut substitutions),
        "base32" => base32_run(args, input, depth, &mut substitutions),
        "basenc" => basenc_run(args, input, depth, &mut substitutions),
        "openssl" => openssl_b64_run(args, input, depth, &mut substitutions),
        "xxd" => xxd_run(args, input, depth, &mut substitutions),
        "uudecode" => uudecode_run(args, input, depth, &mut substitutions),
        "uncompress" => decoded_bytes_run(
            uncompress_query(args),
            input,
            depth,
            &mut substitutions,
            gzip_decode,
        ),
        "gzip" | "gunzip" | "gzcat" | "zcat" => {
            gzip_run(&name, args, input, depth, &mut substitutions)
        }
        "bzip2" | "bunzip2" | "bzcat" => bzip2_run(&name, args, input, depth, &mut substitutions),
        "xz" | "unxz" | "xzcat" | "lzma" | "unlzma" | "lzcat" => {
            xz_run(&name, args, input, depth, &mut substitutions)
        }
        "zstd" | "unzstd" | "zstdcat" | "pzstd" => {
            zstd_run(&name, args, input, depth, &mut substitutions)
        }
        "lz4" | "unlz4" | "lz4cat" => lz4_run(&name, args, input, depth, &mut substitutions),
        "brotli" => brotli_run(&name, args, input, depth, &mut substitutions),
        "tar" => tar_run(args, input, depth, &mut substitutions),
        "unzip" => unzip_run(args, input, depth, &mut substitutions),
        "7z" | "7za" | "7zz" => sevenz_run(args, input, depth, &mut substitutions),
        _ => return,
    };
    match run {
        Run::Produced { text, .. } if text.chars().any(|ch| !ch.is_whitespace()) => {
            scan_isolated(
                policy,
                tool_name,
                std::slice::from_ref(&text),
                depth,
                local,
                git,
                best,
            );
        }
        Run::Unscanned => prefer(policy, best, unscanned_action(policy)),
        _ => {}
    }
}

/// `cat`, `head`, and `tail` read a `< path` redirect only when they
/// have no file operand. `cat other < path` reads `other`.
fn printer_reads_stdin(name: &str, args: &[String]) -> bool {
    match name {
        "cat" => matches!(
            cat_operands(args),
            Some(ops) if ops.is_empty() || ops.iter().any(|op| op == "-")
        ),
        "head" => head_query(args)
            .is_some_and(|query| query.file.as_deref().is_none_or(|file| file == "-")),
        "tail" => tail_query(args)
            .is_some_and(|query| query.file.as_deref().is_none_or(|file| file == "-")),
        "grep" | "egrep" | "fgrep" | "ggrep" => {
            grep_query(args).is_some_and(|query| query.file.is_none())
        }
        "sed" | "gsed" => sed_query(args).is_some_and(|query| query.file.is_none()),
        "awk" | "gawk" => awk_query(args).is_some_and(|query| query.file.is_none()),
        "dd" => dd_query(args).is_some_and(|query| query.is_none()),
        "iconv" => iconv_query(args).is_some_and(|query| query.is_none()),
        "perl" | "ruby" => line_print_query(name, args).is_some_and(|query| query.is_none()),
        "tee" => tee_query(args),
        "tr" => !args.is_empty(),
        "base64" => base64_query(args).is_some_and(|query| query.file.is_none()),
        "base32" => base32_query(args).is_some_and(|query| query.file.is_none()),
        "openssl" => openssl_b64_query(args).is_some_and(|query| query.file.is_none()),
        "xxd" => xxd_query(args).is_some_and(|file| file.is_none()),
        "uudecode" => uudecode_query(args).is_some_and(|file| file.is_none()),
        "uncompress" => uncompress_query(args).is_some_and(|query| query.file.is_none()),
        "basenc" => basenc_query(args).is_some_and(|query| query.file.is_none()),
        "gzip" | "gunzip" | "gzcat" | "zcat" => {
            gzip_query(name, args).is_some_and(|query| query.file.is_none())
        }
        "bzip2" | "bunzip2" | "bzcat" => {
            bzip2_query(name, args).is_some_and(|query| query.file.is_none())
        }
        "xz" | "unxz" | "xzcat" | "lzma" | "unlzma" | "lzcat" => {
            xz_query(name, args).is_some_and(|query| query.file.is_none())
        }
        "zstd" | "unzstd" | "zstdcat" | "pzstd" => {
            zstd_query(name, args).is_some_and(|query| query.file.is_none())
        }
        "lz4" | "unlz4" | "lz4cat" => {
            lz4_query(name, args).is_some_and(|query| query.file.is_none())
        }
        "brotli" => brotli_query(name, args).is_some_and(|query| query.file.is_none()),
        "tar" => tar_query(args).is_some_and(|query| query.archive == "-"),
        "unzip" => unzip_query(args).is_some_and(|query| query.archive == "-"),
        "7z" | "7za" | "7zz" => sevenz_query(args).is_some_and(|query| query.archive == "-"),
        "join" => join_query(args).is_some_and(|files| files.iter().any(|file| file == "-")),
        "comm" => {
            comm_query(args).is_some_and(|(_, _, _, left, right)| left == "-" || right == "-")
        }
        "jq" => jq_query(args).is_some_and(|file| file.as_deref() == Some("-")),
        "python" | "python3" => {
            python_cat_query(args).is_some_and(|file| file.as_deref() == Some("-"))
        }
        "node" => node_cat_query(args).is_some_and(|file| file.as_deref() == Some("-")),
        "php" => php_cat_query(args).is_some_and(|file| file.as_deref() == Some("-")),
        "paste" | "expand" | "unexpand" | "column" | "fold" | "fmt" | "pr" | "strings" => {
            identity_query(name, args).is_some_and(|query| query.is_none())
        }
        _ => false,
    }
}

/// Body of a script file. A missing path stays unmatched. Past one
/// megabyte the body is `shell.deep`.
fn score_script_path<'a>(
    policy: &'a Policy,
    tool_name: &str,
    path: &str,
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    match load_script_file(path) {
        Some(ScriptFile::Text(text)) => {
            scan_isolated(
                policy,
                tool_name,
                std::slice::from_ref(&text),
                depth,
                local,
                git,
                best,
            );
        }
        Some(ScriptFile::Unscanned) => prefer(policy, best, unscanned_action(policy)),
        None => {}
    }
}

struct PsInput<'a> {
    heredocs: &'a [String],
    stdin_file: Option<&'a str>,
}

/// PowerShell `-Command -` and `-File -` read the heredoc and a
/// `< path` redirect. The dash is not a file name.
fn score_ps_input<'a>(
    policy: &'a Policy,
    tool_name: &str,
    input: PsInput<'_>,
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    scan_isolated(policy, tool_name, input.heredocs, depth, local, git, best);
    if let Some(path) = input.stdin_file {
        score_script_path(policy, tool_name, path, depth, local, git, best);
    }
}

/// `bash path` and `sh path` run that file in a new process.
#[allow(clippy::too_many_arguments)]
fn score_shell_file<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(path) = shell_file_operand(argv) else {
        return;
    };
    let Some(loaded) = load_script_file(&shell_path(path, dir)) else {
        return;
    };
    match loaded {
        ScriptFile::Text(text) => {
            scan_isolated(
                policy,
                tool_name,
                std::slice::from_ref(&text),
                depth,
                local,
                git,
                best,
            );
        }
        ScriptFile::Unscanned => prefer(policy, best, unscanned_action(policy)),
    }
}

/// `source path` and `. path` run that file in this shell.
/// Assignments it makes stay visible to a later `eval`.
#[allow(clippy::too_many_arguments)]
fn score_sourced_file<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    functions: &mut Functions,
    local: &[(String, String)],
    exported: &mut Vec<(String, String)>,
    scope: &mut ShellScope,
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let Some(path) = source_operand(argv) else {
        return;
    };
    let Some(loaded) = load_script_file(path) else {
        return;
    };
    match loaded {
        ScriptFile::Text(text) => {
            score_here(
                policy, tool_name, &text, depth, functions, local, exported, scope, git, best,
            );
        }
        ScriptFile::Unscanned => prefer(policy, best, unscanned_action(policy)),
    }
}

/// `source <(script)` runs in this shell. `bash <(script)` runs in a new
/// process. The first process substitution is the script. `bash -c`,
/// `bash -s`, and a real path do not read it.
#[allow(clippy::too_many_arguments)]
fn score_proc_input<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    proc_inputs: &[String],
    depth: u32,
    functions: &mut Functions,
    local: &[(String, String)],
    exported: &mut Vec<(String, String)>,
    scope: &mut ShellScope,
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let Some(body) = proc_inputs.first() else {
        return;
    };
    let Some(first) = argv.first() else {
        return;
    };
    let base = command_basename(first);
    let source = base == "source" || base == ".";
    if source {
        if source_operand(argv).is_some() || sourced_stdin(argv) {
            return;
        }
    } else if !is_shell_name(base)
        || shell_script(argv).is_some()
        || shell_file_operand(argv).is_some()
        || shell_flag_has(argv, 'c')
        || shell_flag_has(argv, 's')
    {
        return;
    }
    match printed_output(body, depth + 1, &mut Vec::new()) {
        Printed::Text(text) => {
            if source {
                score_here(
                    policy, tool_name, &text, depth, functions, local, exported, scope, git, best,
                );
            } else {
                scan_isolated(
                    policy,
                    tool_name,
                    std::slice::from_ref(&text),
                    depth,
                    local,
                    git,
                    best,
                );
            }
        }
        Printed::Unscanned => prefer(policy, best, unscanned_action(policy)),
        Printed::Unknown => {}
    }
}

/// `$(...)` inherits functions and does not publish definitions back.
/// It also does not see a prefix assignment on the command being expanded.
#[allow(clippy::too_many_arguments)]
fn scan_subshells<'a>(
    policy: &'a Policy,
    tool_name: &str,
    scripts: &[String],
    depth: u32,
    functions: &Functions,
    scope: &ShellScope,
    env: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    for script in scripts {
        if script == UNSCANNED_SUBST {
            prefer(policy, best, unscanned_action(policy));
            continue;
        }
        let mut child = functions.clone();
        let mut child_scope = scope.clone();
        let mut child_env = env.to_vec();
        prefer(
            policy,
            best,
            matched_in(
                policy,
                tool_name,
                script,
                depth + 1,
                &mut child,
                &mut child_env,
                &mut child_scope,
                git,
            ),
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
    // `scp -P` and `sftp -P` take a port. `ssh` uses `-p`.
    let port_is_uppercase = command_basename(first).eq_ignore_ascii_case("scp")
        || command_basename(first).eq_ignore_ascii_case("sftp");
    let mut commands = Vec::new();
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" || !token.starts_with('-') {
            break;
        }
        if token == "-o" {
            index += 1;
            if let Some(command) = argv.get(index).and_then(|spec| proxy_command_script(spec)) {
                commands.push(command.to_string());
            }
            index += 1;
            continue;
        }
        if let Some(spec) = token.strip_prefix("-o")
            && !spec.is_empty()
        {
            if let Some(command) = proxy_command_script(spec) {
                commands.push(command.to_string());
            }
            index += 1;
            continue;
        }
        // `-p 22` is a port, not the end of the options. The next
        // `-o ProxyCommand` still runs.
        let takes_next = (ssh_opt_takes_value(token) || (port_is_uppercase && token == "-P"))
            && !token.contains('=')
            && !ssh_value_is_glued(token);
        index += 1;
        if takes_next && index < argv.len() {
            index += 1;
        }
    }
    commands
}

/// OpenSSH option line. The keyword ignores ASCII case. Whitespace or
/// `=` separates the command, and whitespace around `=` is ignored.
/// `none` clears the command. A longer keyword is a different option.
fn proxy_command_script(spec: &str) -> Option<&str> {
    let spec = trim_ssh_start(spec);
    let keyword = "proxycommand";
    // `get` refuses a cut inside a character. `split_at` would panic.
    let head = spec.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &spec[keyword.len()..];
    let rest = if rest.starts_with(is_ssh_space) {
        let rest = trim_ssh_start(rest);
        if let Some(after) = rest.strip_prefix('=') {
            trim_ssh_start(after)
        } else {
            rest
        }
    } else if let Some(after) = rest.strip_prefix('=') {
        trim_ssh_start(after)
    } else {
        return None;
    };
    if rest.is_empty() || rest.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(rest)
    }
}

fn is_ssh_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000b}' | '\u{000c}')
}

fn trim_ssh_start(text: &str) -> &str {
    text.trim_start_matches(is_ssh_space)
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

struct ExecutedArgv {
    argv: Vec<String>,
    /// Parallel to `argv`. True when that word was entirely single quotes
    /// or `$'...'`, so `eval` expands it after a prefix assignment.
    in_eval: Vec<bool>,
    split_script: Option<String>,
    env: Vec<(String, String)>,
    exec_wrapper: bool,
    /// `command` or `builtin` was peeled, so a shell function of the
    /// same name does not run.
    bypass_function: bool,
    /// A peeled wrapper execs a file, so `trap` is not the builtin.
    /// Keyword `time` stays false. `command time` becomes true.
    blocks_shell_builtin: bool,
}

fn executed_argv(
    tokens: &[String],
    flags: &[bool],
    inherited: &[(String, String)],
) -> Option<ExecutedArgv> {
    let mut index = 0;
    let mut split_script = None;
    let mut env = inherited.to_vec();
    let mut exec_wrapper = false;
    let mut bypass_function = false;
    let mut blocks_shell_builtin = false;
    let mut saw_command_or_builtin = false;
    let mut allow_assign = true;
    while index < tokens.len() {
        if allow_assign
            && (apply_assignment(&mut env, &tokens[index]) || is_assignment(&tokens[index]))
        {
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
        let raw = tokens[index].as_str();
        let lowered = command_basename(raw).to_ascii_lowercase();
        let base = gnu_alias(&lowered);
        if !is_wrapper(base) {
            break;
        }
        if !shell_builtin_wrapper(raw, saw_command_or_builtin) {
            blocks_shell_builtin = true;
        }
        if matches!(raw, "command" | "builtin") {
            saw_command_or_builtin = true;
        }
        index += 1;
        if !runs_in_parent(base) {
            exec_wrapper = true;
        } else {
            bypass_function = true;
        }
        let start = index;
        let Some((next, script)) = skip_wrapper_flags(base, tokens, index, &mut env) else {
            return split_script.map(|script| ExecutedArgv {
                argv: Vec::new(),
                in_eval: Vec::new(),
                split_script: Some(script),
                env,
                exec_wrapper,
                bypass_function,
                blocks_shell_builtin,
            });
        };
        if next == start {
            // `nice FOO=bar` does not assign FOO. `command exec` still
            // has to see `exec` as the next program.
            if start < tokens.len() && apply_assignment_token(&tokens[start]) {
                break;
            }
            continue;
        }
        if tokens
            .get(start..next)
            .is_some_and(|range| range.iter().any(|token| token == "--"))
        {
            allow_assign = false;
        }
        if split_script.is_none() {
            split_script = script;
        }
        index = next;
    }
    let argv = tokens[index..].to_vec();
    let in_eval = (0..argv.len())
        .map(|offset| flags.get(index + offset).copied().unwrap_or(false))
        .collect();
    Some(ExecutedArgv {
        argv,
        in_eval,
        split_script,
        env,
        exec_wrapper,
        bypass_function,
        blocks_shell_builtin,
    })
}

/// `command` and `builtin` run the next builtin in this shell. Keyword
/// `time` does too, until one of those has forced the next word to be a
/// program. `command time` is `/usr/bin/time`, which cannot run `trap`.
fn shell_builtin_wrapper(token: &str, command_or_builtin_seen: bool) -> bool {
    matches!(token, "command" | "builtin") || (token == "time" && !command_or_builtin_seen)
}

fn runs_in_parent(wrapper: &str) -> bool {
    matches!(wrapper, "command" | "builtin")
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
    env: &mut Vec<(String, String)>,
) -> Option<(usize, Option<String>)> {
    let mut split_script = None;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if apply_assignment_token(token) {
            if wrapper_sets_env(wrapper) {
                apply_assignment(env, token);
            } else {
                // `nice FOO=bar` runs a program named `FOO=bar`. It is not an assignment.
                break;
            }
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

/// Command substitution sees the same Homebrew names. `Cat` stays `Cat`
/// because bash does not run that spelling as `cat`.
fn stage_command_name(token: &str) -> String {
    let base = command_basename(token);
    let lowered = base.to_ascii_lowercase();
    let aliased = gnu_alias(&lowered);
    match aliased {
        "gcat" => "cat".to_string(),
        "ghead" => "head".to_string(),
        "gtail" => "tail".to_string(),
        "gfold" => "fold".to_string(),
        "gfmt" => "fmt".to_string(),
        "gpaste" => "paste".to_string(),
        "gsort" => "sort".to_string(),
        "gcut" => "cut".to_string(),
        "gdd" => "dd".to_string(),
        "gtac" => "tac".to_string(),
        "guniq" => "uniq".to_string(),
        "gexpand" => "expand".to_string(),
        "gunexpand" => "unexpand".to_string(),
        "gpr" => "pr".to_string(),
        "gtr" => "tr".to_string(),
        "gtee" => "tee".to_string(),
        "gtar" | "bsdtar" => "tar".to_string(),
        "gbase64" => "base64".to_string(),
        "gbasenc" => "basenc".to_string(),
        "gbase32" => "base32".to_string(),
        "gjoin" => "join".to_string(),
        "gcomm" => "comm".to_string(),
        "gfgrep" => "fgrep".to_string(),
        "gegrep" => "egrep".to_string(),
        "gsplit" => "split".to_string(),
        _ => {
            if aliased == lowered && base != lowered {
                base.to_string()
            } else {
                aliased.to_string()
            }
        }
    }
}

fn is_wrapper(name: &str) -> bool {
    const WRAPPERS: &[&str] = &[
        "sudo", "doas", "pkexec", "env", "command", "builtin", "nice", "nohup", "time", "busybox",
        "ionice", "stdbuf", "setsid", "exec", "timeout", "xargs",
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

const EVAL_VARIANT_CAP: usize = 8;

fn eval_script(
    tokens: &[String],
    in_eval: &[bool],
    scope: &ShellScope,
    exported: &[(String, String)],
    command_env: &[(String, String)],
) -> Option<Vec<String>> {
    let base = command_basename(tokens.first()?);
    if !base.eq_ignore_ascii_case("eval") {
        return None;
    }
    let mut start = 1;
    if tokens.get(start).is_some_and(|token| token == "--") {
        start += 1;
    }
    if start >= tokens.len() {
        return None;
    }
    let mut variants = vec![String::new()];
    for (offset, token) in tokens[start..].iter().enumerate() {
        let single = in_eval.get(start + offset).copied().unwrap_or(false);
        let pieces = expand_eval_word(token, single, scope, exported, command_env);
        variants = join_eval_words(&variants, &pieces);
    }
    Some(variants)
}

/// `$name` and `${name}` in one `eval` word. One pass: the value is not
/// scanned for another `$name`. `$(...)` stays a command substitution.
/// A single-quoted word sees a prefix assignment. A double-quoted word
/// was expanded before that prefix, so the prefix is ignored.
fn expand_eval_word(
    token: &str,
    in_eval: bool,
    scope: &ShellScope,
    exported: &[(String, String)],
    command_env: &[(String, String)],
) -> Vec<String> {
    let mut pieces: Vec<Vec<String>> = Vec::new();
    let mut literal = String::new();
    let mut rest = token;
    while let Some(dollar) = rest.find('$') {
        literal.push_str(&rest[..dollar]);
        rest = &rest[dollar + 1..];
        if rest.starts_with('(') {
            literal.push('$');
            continue;
        }
        if let Some((spec, span)) = braced_parameter(rest) {
            push_eval_literal(&mut literal, &mut pieces);
            let original = format!("${{{}}}", &rest[1..span - 1]);
            pieces.push(parameter_text(
                spec,
                &original,
                in_eval,
                scope,
                exported,
                command_env,
            ));
            rest = &rest[span..];
            continue;
        }
        if let Some(name) = bare_parameter(rest) {
            push_eval_literal(&mut literal, &mut pieces);
            let original = format!("${name}");
            pieces.push(parameter_text(
                ParamRef::Name(name),
                &original,
                in_eval,
                scope,
                exported,
                command_env,
            ));
            rest = &rest[name.len()..];
            continue;
        }
        literal.push('$');
    }
    literal.push_str(rest);
    push_eval_literal(&mut literal, &mut pieces);
    if pieces.is_empty() {
        return vec![String::new()];
    }
    let mut acc = vec![String::new()];
    for piece in &pieces {
        acc = concat_eval_pieces(&acc, piece);
    }
    acc
}

fn push_eval_literal(literal: &mut String, pieces: &mut Vec<Vec<String>>) {
    if !literal.is_empty() {
        pieces.push(vec![std::mem::take(literal)]);
    }
}

fn parameter_text(
    spec: ParamRef<'_>,
    original: &str,
    in_eval: bool,
    scope: &ShellScope,
    exported: &[(String, String)],
    command_env: &[(String, String)],
) -> Vec<String> {
    let name = spec.name();
    if in_eval {
        let prefixed = last_assignment(command_env, name);
        let inherited = last_assignment(exported, name);
        // A single-quoted `$x` expands inside eval, after the prefix.
        // That value is definite, so the other branches are not scored.
        if let Some(value) = prefixed
            && prefixed != inherited
        {
            return vec![scalar_param(spec, value)];
        }
    }
    if let Some(values) = scope.visible_array(name) {
        return vec![array_param(spec, values)];
    }
    let mut values = Vec::new();
    if let Some(value) = scope.value(name, exported) {
        values.push(value.to_string());
    }
    // `then` and `else` can each hold a value. `"$x"` and `'$x'` both
    // read that name. A double-quoted word still ignores a prefix.
    for alt in scope.alt_values(name) {
        if values.iter().all(|seen| seen != &alt) {
            values.push(alt);
        }
    }
    if values.is_empty() {
        vec![original.to_string()]
    } else if matches!(spec, ParamRef::Index(_, index) if index > 0) {
        vec![String::new()]
    } else {
        values
    }
}

/// `$name` on a scalar is that text. `${name[0]}` is too. A later index
/// is empty. `${name[@]}` is the text.
fn scalar_param(spec: ParamRef<'_>, value: &str) -> String {
    match spec {
        ParamRef::Index(_, index) if index > 0 => String::new(),
        ParamRef::Name(_) | ParamRef::Index(_, _) | ParamRef::All(_) => value.to_string(),
    }
}

fn array_param(spec: ParamRef<'_>, values: &[String]) -> String {
    match spec {
        ParamRef::Name(_) | ParamRef::Index(_, 0) => values.first().cloned().unwrap_or_default(),
        ParamRef::Index(_, index) => values.get(index).cloned().unwrap_or_default(),
        ParamRef::All(_) => {
            if values.len() == 1 {
                values[0].clone()
            } else {
                values.join(" ")
            }
        }
    }
}

fn join_eval_words(prefixes: &[String], words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for prefix in prefixes {
        for word in words {
            if out.len() == EVAL_VARIANT_CAP {
                return out;
            }
            if prefix.is_empty() {
                out.push(word.clone());
            } else {
                out.push(format!("{prefix} {word}"));
            }
        }
    }
    out
}

fn concat_eval_pieces(prefixes: &[String], parts: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for prefix in prefixes {
        for part in parts {
            if out.len() == EVAL_VARIANT_CAP {
                return out;
            }
            let mut next = String::with_capacity(prefix.len() + part.len());
            next.push_str(prefix);
            next.push_str(part);
            out.push(next);
        }
    }
    out
}

#[derive(Clone, Copy)]
enum ParamRef<'a> {
    Name(&'a str),
    Index(&'a str, usize),
    All(&'a str),
}

impl<'a> ParamRef<'a> {
    fn name(self) -> &'a str {
        match self {
            ParamRef::Name(name) | ParamRef::Index(name, _) | ParamRef::All(name) => name,
        }
    }
}

/// `${name}`, `${name[N]}`, `${name[@]}`, and `${name[*]}`. The span
/// includes both braces. `text` starts at `{`.
fn braced_parameter(text: &str) -> Option<(ParamRef<'_>, usize)> {
    let inner = text.strip_prefix('{')?;
    let end = inner.find('}')?;
    let body = &inner[..end];
    let spec = param_ref(body)?;
    Some((spec, end + 2))
}

fn param_ref(body: &str) -> Option<ParamRef<'_>> {
    if let Some(name) = body
        .strip_suffix("[@]")
        .or_else(|| body.strip_suffix("[*]"))
    {
        return env_name(name).then_some(ParamRef::All(name));
    }
    if let Some(open) = body.find('[') {
        let name = &body[..open];
        let index = body[open + 1..].strip_suffix(']')?;
        if !env_name(name) || index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        let index = index.parse().ok()?;
        return Some(ParamRef::Index(name, index));
    }
    env_name(body).then_some(ParamRef::Name(body))
}

fn bare_parameter(text: &str) -> Option<&str> {
    let mut len = 0;
    for ch in text.chars() {
        let ok = if len == 0 {
            ch.is_ascii_alphabetic() || ch == '_'
        } else {
            ch.is_ascii_alphanumeric() || ch == '_'
        };
        if !ok {
            break;
        }
        len += ch.len_utf8();
    }
    if len == 0 { None } else { Some(&text[..len]) }
}

fn is_shell_name(name: &str) -> bool {
    const SHELLS: &[&str] = &[
        "sh", "bash", "dash", "zsh", "ksh", "ash", "fish", "csh", "tcsh",
    ];
    SHELLS.iter().any(|shell| name.eq_ignore_ascii_case(shell))
}

fn shell_flag_has(argv: &[String], flag: char) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    if !is_shell_name(command_basename(first)) {
        return false;
    }
    let mut index = 1usize;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            return false;
        }
        if shell_opt_takes_value(token) {
            index += 2;
            continue;
        }
        if let Some(flags) = token.strip_prefix('-')
            && !flags.is_empty()
            && !flags.starts_with('-')
        {
            if flags.contains(flag) {
                return true;
            }
            index += 1;
            continue;
        }
        return false;
    }
    false
}

fn is_stdin_script(token: &str) -> bool {
    matches!(token, "/dev/stdin" | "/dev/fd/0" | "-")
}

/// `bash -c 'source /dev/stdin'` and `bash -c 'bash -s'` run the
/// redirected file. `bash -c 'echo hi'` does not.
fn script_reads_stdin(script: &str) -> bool {
    let script = script.trim();
    if matches!(
        script,
        "source /dev/stdin"
            | "source -"
            | "source /dev/fd/0"
            | ". /dev/stdin"
            | ". -"
            | ". /dev/fd/0"
    ) {
        return true;
    }
    tokenize_parts(script).into_iter().any(|part| {
        let ShellPart::Segment(segment) = part else {
            return false;
        };
        executed_argv(&segment.tokens, &segment.in_eval, &[])
            .is_some_and(|executed| shell_reads_stdin(&executed.argv))
    })
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

/// A regular file only. A missing path, a directory, or a fifo is not a
/// script. Open is nonblocking, then `fstat`, so a fifo cannot stall the
/// hook. Past one megabyte the body is `shell.deep` instead of `auto`.
const SCRIPT_FILE_CAP: u64 = 1024 * 1024;

enum ScriptFile {
    Text(String),
    Unscanned,
}

/// `C:\dir\file` and `C:/dir/file` are the same file. A relative
/// `C:/...` under the test root is how Ubuntu opens that spelling.
fn openable_path(path: &str) -> std::borrow::Cow<'_, str> {
    let mut chars = path.chars();
    let drive = matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && path.contains('\\');
    if drive {
        std::borrow::Cow::Owned(path.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(path)
    }
}

fn load_script_file(path: &str) -> Option<ScriptFile> {
    let owned = openable_path(path);
    let path = owned.as_ref();
    let mut file = open_script_candidate(path)?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    if meta.len() > SCRIPT_FILE_CAP {
        return Some(ScriptFile::Unscanned);
    }
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = std::io::Read::read(&mut file, &mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() as u64 > SCRIPT_FILE_CAP {
            return Some(ScriptFile::Unscanned);
        }
    }
    Some(ScriptFile::Text(String::from_utf8_lossy(&buf).into_owned()))
}

/// `O_NONBLOCK` so `open` of a fifo returns. The bit is not the same on
/// every Unix. An unknown target stats first and does not pass a flag
/// that means something else (on MIPS, 2048 is `O_NOCTTY`).
#[cfg(unix)]
fn open_script_candidate(path: &str) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let Some(flag) = unix_nonblock_flag() else {
        return open_regular_file(path);
    };
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flag)
        .open(path)
        .ok()
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn unix_nonblock_flag() -> Option<i32> {
    Some(0x4)
}

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    any(
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6"
    )
))]
fn unix_nonblock_flag() -> Option<i32> {
    Some(128)
}

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    any(target_arch = "sparc", target_arch = "sparc64")
))]
fn unix_nonblock_flag() -> Option<i32> {
    Some(0x4000)
}

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    target_env = "uclibc",
    target_arch = "arm"
))]
fn unix_nonblock_flag() -> Option<i32> {
    Some(0x800)
}

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    not(any(
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "sparc",
        target_arch = "sparc64"
    )),
    not(all(target_env = "uclibc", target_arch = "arm"))
))]
fn unix_nonblock_flag() -> Option<i32> {
    // asm-generic, including the x86_64 and aarch64 values.
    Some(2048)
}

#[cfg(all(
    unix,
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "linux",
        target_os = "android"
    ))
))]
fn unix_nonblock_flag() -> Option<i32> {
    None
}

#[cfg(not(unix))]
fn open_script_candidate(path: &str) -> Option<std::fs::File> {
    open_regular_file(path)
}

fn open_regular_file(path: &str) -> Option<std::fs::File> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    std::fs::File::open(path).ok()
}

/// `source path` and `. path`. `source -- path` skips the option.
/// `/dev/stdin`, `-`, and `/dev/fd/0` are the heredoc path, not a file.
fn source_operand(argv: &[String]) -> Option<&str> {
    let base = command_basename(argv.first()?);
    if base != "source" && base != "." {
        return None;
    }
    let mut index = 1;
    if argv.get(index).is_some_and(|token| token == "--") {
        index += 1;
    }
    let token = argv.get(index)?;
    if is_stdin_script(token) {
        return None;
    }
    Some(token.as_str())
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
        // Words after -File belong to the script. The caller reads the
        // file. A later -Command is an argument, not the engine command.
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
// stops. The caller reads the first operand.
fn is_ps_file_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.is_empty() && "file".starts_with(&name)
}

fn cmd_run_tail(argv: &[String]) -> Option<&[String]> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("cmd") {
        return None;
    }
    let mut index = 1usize;
    while index < argv.len() {
        if is_cmd_run_switch(&argv[index]) {
            let tail = &argv[index + 1..];
            return (!tail.is_empty()).then_some(tail);
        }
        index += 1;
    }
    None
}

/// The tail of `/c` or `/k` when cmd runs a batch file. `call` of that
/// path is the same file. A command string such as `rm -rf /tmp/x` is
/// not, including when the last argument ends in `.bat`.
fn cmd_batch_operand(argv: &[String]) -> Option<&str> {
    let tail = cmd_run_tail(argv)?;
    if tail.len() == 1 {
        return batch_token(&tail[0]);
    }
    if tail[0].eq_ignore_ascii_case("call") {
        let path = tail.get(1)?;
        return is_batch_file(path).then_some(path.as_str());
    }
    is_batch_file(&tail[0]).then_some(tail[0].as_str())
}

/// One quoted tail. `C:\Program Files\payload.bat` is a path. `rm -rf
/// /tmp/x.bat` is a command whose last word happens to end in `.bat`.
fn batch_token(token: &str) -> Option<&str> {
    let token = token.trim();
    if token.is_empty()
        || token
            .chars()
            .any(|ch| matches!(ch, '&' | '|' | '<' | '>' | ';'))
    {
        return None;
    }
    if !token.contains(char::is_whitespace) {
        return is_batch_file(token).then_some(token);
    }
    let first = token.split_whitespace().next()?;
    if !first.contains(['/', '\\']) && !first.starts_with('.') {
        return None;
    }
    is_batch_file(token).then_some(token)
}

fn is_batch_file(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path);
    let name = name.trim_end_matches([' ', '\t']);
    std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"))
}

/// `-File` / `-f` operand, or the bare word `pwsh` treats as `-File`.
/// Windows PowerShell's bare word stays `-Command`.
fn powershell_file_operand(argv: &[String]) -> Option<&str> {
    let base = command_basename(argv.first()?);
    let pwsh = base.eq_ignore_ascii_case("pwsh");
    if !pwsh && !base.eq_ignore_ascii_case("powershell") {
        return None;
    }
    let mut index = 1usize;
    while index < argv.len() {
        let token = argv[index].as_str();
        let Some((name, inline)) = ps_switch_parts(token) else {
            return if pwsh { Some(token) } else { None };
        };
        if is_ps_command_name(name)
            || is_ps_command_with_args_name(name)
            || is_ps_encoded_command_name(name)
        {
            return None;
        }
        if is_ps_file_name(name) {
            if let Some(value) = inline.filter(|value| !value.is_empty()) {
                return Some(value);
            }
            return argv.get(index + 1).map(String::as_str);
        }
        if ps_name_takes_value(name, base) {
            index += 1;
            if inline.is_none() && index < argv.len() {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    None
}

/// `& path`, a `.ps1` path, and `Invoke-Expression` of `Get-Content -Raw`
/// run that file. `iex` and `Get-Content` without `-Raw` do not.
fn powershell_launched_file(argv: &[String], script: &str) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("powershell") && !base.eq_ignore_ascii_case("pwsh") {
        return None;
    }
    ps_call_file(script)
        .or_else(|| ps_iex_raw_file(script))
        .or_else(|| ps_command_script_path(script))
}

fn is_ps1_file(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path);
    let name = name.trim_end_matches([' ', '\t']);
    std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ps1"))
}

/// `-Command path.ps1` runs that file, including a quoted path that
/// contains spaces. A later `;`, `|`, or `&` is a second statement,
/// so this stays out of the way and the text scan still sees it.
fn ps_command_script_path(script: &str) -> Option<String> {
    let script = script.trim();
    if script
        .chars()
        .any(|ch| matches!(ch, ';' | '|' | '&' | '<' | '>'))
    {
        return None;
    }
    if is_ps1_file(script) {
        return Some(script.to_string());
    }
    let (path, rest) = take_ps_literal(script)?;
    if !is_ps1_file(&path) {
        return None;
    }
    if !rest.trim().is_empty() && path.contains(char::is_whitespace) {
        return None;
    }
    Some(path)
}

fn ps_call_file(script: &str) -> Option<String> {
    let rest = script.trim().strip_prefix('&')?;
    if rest.starts_with('&') {
        return None;
    }
    let (path, rest) = take_ps_literal(rest)?;
    if path.is_empty() || path.starts_with(['$', '(', '{', '&']) {
        return None;
    }
    if rest.chars().any(|ch| matches!(ch, ';' | '|' | '&')) {
        return None;
    }
    Some(path)
}

fn ps_iex_raw_file(script: &str) -> Option<String> {
    ps_iex_call(script).or_else(|| ps_iex_pipeline(script))
}

fn ps_iex_call(script: &str) -> Option<String> {
    let rest = split_ps_name(script, "invoke-expression")?;
    let rest = rest.trim_start().strip_prefix('(')?.trim_start();
    let rest = split_ps_name(rest, "get-content")?;
    let (path, rest) = ps_get_content_raw_args(rest)?;
    let rest = rest.trim_start().strip_prefix(')')?;
    if !rest.trim().is_empty() {
        return None;
    }
    Some(path)
}

fn ps_iex_pipeline(script: &str) -> Option<String> {
    let rest = split_ps_name(script, "get-content")?;
    let (path, rest) = ps_get_content_raw_args(rest)?;
    let rest = rest.trim_start().strip_prefix('|')?.trim_start();
    let rest = split_ps_name(rest, "invoke-expression")?;
    if !rest.trim().is_empty() {
        return None;
    }
    Some(path)
}

/// `-Raw` may sit before the path or after it. Both run the file.
fn ps_get_content_raw_args(text: &str) -> Option<(String, &str)> {
    let text = text.trim_start();
    if let Some(rest) = split_ps_name(text, "-raw") {
        let (path, rest) = take_ps_literal(rest)?;
        if path.is_empty() {
            return None;
        }
        return Some((path, rest));
    }
    let (path, rest) = take_ps_literal(text)?;
    if path.is_empty() || path.starts_with('-') {
        return None;
    }
    let rest = split_ps_name(rest, "-raw")?;
    Some((path, rest))
}

fn split_ps_name<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let text = text.trim_start();
    if text.len() < name.len() || !text[..name.len()].eq_ignore_ascii_case(name) {
        return None;
    }
    let rest = &text[name.len()..];
    if rest.starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_') {
        return None;
    }
    Some(rest)
}

fn take_ps_literal(text: &str) -> Option<(String, &str)> {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('\'') {
        let end = rest.find('\'')?;
        return Some((rest[..end].to_string(), &rest[end + 1..]));
    }
    if let Some(rest) = text.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((rest[..end].to_string(), &rest[end + 1..]));
    }
    if text.starts_with(['$', '(', '{', '&']) {
        return None;
    }
    let end = text
        .find(|ch: char| ch.is_whitespace() || matches!(ch, ')' | ';'))
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    Some((text[..end].to_string(), &text[end..]))
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

/// Bash runs this builtin only for the word `trap`. The action is a
/// script when at least one later word is a signal it accepts. A bad
/// word beside a good one still runs. One word is a reset (`trap EXIT`).
/// `-` and `''` clear or ignore. `-l`, `-p`, and `-P` only print.
fn trap_scripts(tokens: &[String]) -> Vec<String> {
    if tokens.first().map(String::as_str) != Some("trap") {
        return Vec::new();
    }
    let mut index = 1;
    let mut query = false;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token == "--" {
            index += 1;
            break;
        }
        if let Some(flags) = token.strip_prefix('-')
            && !flags.is_empty()
            && flags.chars().all(|ch| matches!(ch, 'l' | 'p' | 'P'))
        {
            query = true;
            index += 1;
            continue;
        }
        break;
    }
    if query || index + 1 >= tokens.len() {
        return Vec::new();
    }
    let action = &tokens[index];
    if action.is_empty() || action == "-" {
        return Vec::new();
    }
    if !tokens[index + 1..].iter().any(|word| is_trap_spec(word)) {
        return Vec::new();
    }
    vec![action.clone()]
}

/// `EXIT`, `DEBUG`, `ERR`, and `RETURN` are not `trap -l` names, and a
/// `SIG` prefix does not make them valid (`SIGEXIT` is rejected).
fn matches_trap_special(word: &str) -> bool {
    word.eq_ignore_ascii_case("exit")
        || word.eq_ignore_ascii_case("debug")
        || word.eq_ignore_ascii_case("err")
        || word.eq_ignore_ascii_case("return")
}

fn sig_suffix(word: &str) -> Option<&str> {
    let head = word.get(..3)?;
    if !head.eq_ignore_ascii_case("sig") {
        return None;
    }
    let body = word.get(3..)?;
    if body.is_empty() { None } else { Some(body) }
}

/// Names from `trap -l` on bash 5.3, without the `SIG` prefix. `KILL`
/// and `STOP` are omitted: bash stores a handler, and the kernel does
/// not deliver it. `PWR` and `RTMIN` are not in this list.
const TRAP_SIGNAL_NAMES: &[&str] = &[
    "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "EMT", "FPE", "BUS", "SEGV", "SYS", "PIPE",
    "ALRM", "TERM", "URG", "TSTP", "CONT", "CHLD", "TTIN", "TTOU", "IO", "XCPU", "XFSZ", "VTALRM",
    "PROF", "WINCH", "INFO", "USR1", "USR2",
];

fn is_trap_spec(word: &str) -> bool {
    if matches_trap_special(word) {
        return true;
    }
    let name = match sig_suffix(word) {
        Some(body) if !matches_trap_special(body) => body,
        Some(_) => return false,
        None => word,
    };
    TRAP_SIGNAL_NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
        || is_trap_number(word)
}

/// `0` through `31`, the numbers this bash accepts, except signals the
/// kernel does not deliver. A leading `+` is still a number (`+15` is
/// `SIGTERM`). `+32` and `64` are not. `9` is `SIGKILL`. `SIGSTOP` is
/// `17` on macOS and `19` on Linux.
fn is_trap_number(word: &str) -> bool {
    let digits = word.strip_prefix('+').unwrap_or(word);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    digits
        .parse::<u32>()
        .is_ok_and(|number| number <= 31 && !uncatchable_signal(number))
}

fn uncatchable_signal(number: u32) -> bool {
    if number == 9 {
        return true;
    }
    if cfg!(target_os = "macos") {
        number == 17
    } else if cfg!(target_os = "linux") {
        number == 19
    } else {
        false
    }
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

struct GitScripts {
    commands: Vec<String>,
    shells: Vec<String>,
    bindings: GitBindings,
}

fn git_alias_scripts(
    argv: &[String],
    env: &[(String, String)],
    inherited: &GitBindings,
    dir: Option<&str>,
) -> GitScripts {
    let bindings = inherited.clone();
    let Some(first) = argv.first() else {
        return GitScripts {
            commands: Vec::new(),
            shells: Vec::new(),
            bindings,
        };
    };
    if !command_basename(first).eq_ignore_ascii_case("git") {
        return GitScripts {
            commands: Vec::new(),
            shells: Vec::new(),
            bindings,
        };
    }
    let mut seen = bindings;
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" || !token.starts_with('-') {
            break;
        }
        if token == "-c" {
            index += 1;
            if let Some(spec) = argv.get(index) {
                seen.note(spec);
            }
            index += 1;
            continue;
        }
        if let Some(spec) = token.strip_prefix("--config-env=") {
            seen.note_env(spec, env);
            index += 1;
            continue;
        }
        if token == "--config-env" {
            index += 1;
            if let Some(spec) = argv.get(index) {
                seen.note_env(spec, env);
            }
            index += 1;
            continue;
        }
        let takes = !token.contains('=') && option_takes_value(token);
        index += 1;
        if takes && index < argv.len() {
            index += 1;
        }
    }
    let mut commands = Vec::new();
    let mut shells = Vec::new();
    seen.collect(argv, env, dir, 0, &mut commands, &mut shells);
    GitScripts {
        commands,
        shells,
        bindings: seen,
    }
}

#[derive(Clone, Default)]
struct GitBindings {
    helpers: Vec<String>,
    ssh: Option<String>,
    external_diff: Option<String>,
    diff_tool: Option<String>,
    merge_tool: Option<String>,
    difftools: Vec<(String, String)>,
    mergetools: Vec<(String, String)>,
    cleans: Vec<(String, String)>,
    smudges: Vec<(String, String)>,
    aliases: Vec<(String, String)>,
    hooks: Option<String>,
}

struct GitKey {
    section: String,
    subsection: Option<String>,
    name: String,
}

impl GitBindings {
    fn note(&mut self, spec: &str) {
        let Some((key, value)) = spec.split_once('=') else {
            return;
        };
        let Some(parsed) = parse_git_key(key) else {
            return;
        };
        match (
            parsed.section.as_str(),
            parsed.subsection.as_deref(),
            parsed.name.as_str(),
        ) {
            ("credential", _, "helper") => note_credential_helper(&mut self.helpers, value),
            ("core", None, "sshcommand") => {
                self.ssh = Some(strip_bang(value).to_string());
            }
            ("core", None, "hookspath") => {
                self.hooks = Some(value.to_string());
            }
            ("diff", None, "external") => {
                self.external_diff = Some(strip_bang(value).to_string());
            }
            ("diff", None, "tool") => self.diff_tool = Some(value.to_string()),
            ("merge", None, "tool") => self.merge_tool = Some(value.to_string()),
            ("difftool", Some(tool), "cmd") if !tool.is_empty() => {
                set_named(&mut self.difftools, tool, strip_bang(value), true);
            }
            ("mergetool", Some(tool), "cmd") if !tool.is_empty() => {
                set_named(&mut self.mergetools, tool, strip_bang(value), true);
            }
            ("filter", Some(name), "clean") if !name.is_empty() => {
                set_named(&mut self.cleans, name, strip_bang(value), true);
            }
            ("filter", Some(name), "smudge") if !name.is_empty() => {
                set_named(&mut self.smudges, name, strip_bang(value), true);
            }
            ("alias", _, _) => {
                if let Some(name) = alias_name(&parsed) {
                    set_named(&mut self.aliases, &name, value, false);
                }
            }
            _ => {}
        }
    }

    /// An explicit `core.hooksPath` wins, including an empty one.
    /// Otherwise git uses the repository's `.git/hooks`.
    fn hook_directory(&self, argv: &[String], dir: Option<&str>) -> Option<String> {
        if self.hooks.is_some() {
            return self.hooks.clone();
        }
        default_hook_directory(argv, dir)
    }

    fn note_env(&mut self, spec: &str, env: &[(String, String)]) {
        let Some((key, var)) = spec.rsplit_once('=') else {
            return;
        };
        if key.is_empty() || var.is_empty() {
            return;
        }
        let Some(value) = last_assignment(env, var) else {
            return;
        };
        let binding = format!("{key}={value}");
        self.note(&binding);
    }

    fn collect(
        &self,
        argv: &[String],
        env: &[(String, String)],
        dir: Option<&str>,
        depth: u32,
        commands: &mut Vec<String>,
        shells: &mut Vec<String>,
    ) {
        if depth > MAX_SHELL_DEPTH {
            return;
        }
        // Git runs these for the subcommand, not for a later path
        // that happens to use the same word (`status credential`).
        let Some(subcommand) = git_subcommand(argv) else {
            return;
        };
        if subcommand == "diff"
            && let Some(command) = self
                .external_diff
                .as_ref()
                .filter(|value| !value.is_empty())
        {
            commands.push(command.clone());
        }
        if subcommand == "add" {
            commands.extend(self.cleans.iter().map(|(_, command)| command.clone()));
        }
        if subcommand == "checkout" {
            commands.extend(self.smudges.iter().map(|(_, command)| command.clone()));
        }
        if subcommand == "difftool" {
            push_selected_tool(commands, argv, &self.diff_tool, &self.difftools);
        }
        if subcommand == "mergetool" {
            push_selected_tool(commands, argv, &self.merge_tool, &self.mergetools);
        }
        commands.extend(ssh_scripts(&self.ssh, argv, env));
        shells.extend(git_hook_scripts(
            self.hook_directory(argv, dir).as_deref(),
            subcommand,
            argv,
        ));
        if subcommand == "credential" {
            commands.extend(self.helpers.iter().cloned());
        }
        let extra = git_args_after_subcommand(argv);
        for (name, value) in &self.aliases {
            if !subcommand.eq_ignore_ascii_case(name) {
                continue;
            }
            // Git ignores an alias that hides a real command (`alias.push`).
            // `alias.p` still expands, because `p` is not a command.
            if git_command(name) {
                continue;
            }
            if let Some(shell) = value.strip_prefix('!') {
                if !shell.is_empty() {
                    shells.push(shell.to_string());
                }
            } else if !value.is_empty() {
                let mut child = command_argv(&format!("git {value}"));
                child.extend(extra.iter().cloned());
                if child.len() > 1 {
                    // Quote so a later rescan sees the same words git executes.
                    // A bare join would turn the argument `a;rm` into a shell.
                    commands.push(quote_argv(&child));
                    self.collect(&child, env, dir, depth + 1, commands, shells);
                }
            }
        }
    }
}

fn note_credential_helper(helpers: &mut Vec<String>, value: &str) {
    if value.is_empty() {
        helpers.clear();
        return;
    }
    let Some(shell) = value.strip_prefix('!') else {
        return;
    };
    if !shell.is_empty() {
        helpers.push(shell.to_string());
    }
}

fn strip_bang(value: &str) -> &str {
    value.strip_prefix('!').unwrap_or(value)
}

fn parse_git_key(key: &str) -> Option<GitKey> {
    let (left, name) = key.rsplit_once('.')?;
    if left.is_empty() || !git_ident(name) {
        return None;
    }
    let (section, subsection) = if let Some((section, sub)) = left.split_once('.') {
        (section, Some(sub.to_string()))
    } else {
        (left, None)
    };
    if !git_ident(section) {
        return None;
    }
    Some(GitKey {
        section: section.to_ascii_lowercase(),
        subsection,
        name: name.to_ascii_lowercase(),
    })
}

fn git_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

fn alias_name(key: &GitKey) -> Option<String> {
    if key.section != "alias" {
        return None;
    }
    match &key.subsection {
        None => Some(key.name.clone()),
        Some(sub) => Some(format!("{sub}.{}", key.name)),
    }
}

/// The last value for this name wins. An empty value removes it.
/// `case_sensitive` is false for alias names, which Git folds.
fn set_named(list: &mut Vec<(String, String)>, name: &str, value: &str, case_sensitive: bool) {
    let same = |existing: &str| {
        if case_sensitive {
            existing == name
        } else {
            existing.eq_ignore_ascii_case(name)
        }
    };
    if let Some(index) = list.iter().rposition(|(existing, _)| same(existing)) {
        if value.is_empty() {
            list.remove(index);
        } else {
            list[index] = (name.to_string(), value.to_string());
        }
        return;
    }
    if !value.is_empty() {
        list.push((name.to_string(), value.to_string()));
    }
}

fn push_selected_tool(
    scripts: &mut Vec<String>,
    argv: &[String],
    configured: &Option<String>,
    commands: &[(String, String)],
) {
    let configured = match configured {
        Some(tool) if !tool.is_empty() => vec![tool.clone()],
        _ => Vec::new(),
    };
    let Some(tool) = selected_difftool(argv, &configured) else {
        return;
    };
    for (name, command) in commands {
        if name == &tool {
            scripts.push(command.clone());
        }
    }
}

/// `GIT_SSH_COMMAND` wins, including an empty value, which suppresses
/// `core.sshCommand` and `GIT_SSH`. The config wins over `GIT_SSH`.
fn ssh_scripts(
    configured: &Option<String>,
    argv: &[String],
    env: &[(String, String)],
) -> Vec<String> {
    if !git_uses_ssh(argv) {
        return Vec::new();
    }
    if let Some(value) = last_assignment(env, "GIT_SSH_COMMAND") {
        return one_script(value);
    }
    if let Some(value) = configured {
        return one_script(value);
    }
    if let Some(value) = last_assignment(env, "GIT_SSH") {
        return git_ssh_program(value);
    }
    Vec::new()
}

/// Program text from `python`, `node`, `ruby`, `perl`, or `php`, plus
/// the `package.json` scripts `npm`, `pnpm`, and `bun` actually run.
struct InterpSource<'a> {
    argv: &'a [String],
    stdin_file: Option<&'a str>,
}

enum LoadedProgram {
    Text(String),
    Unscanned,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HostLang {
    Python,
    Node,
    Ruby,
    Perl,
    Php,
    Lua,
    R,
}

enum ExecKind {
    /// One string is a shell command (`os.system`, `execSync`).
    Shell,
    /// A string list is argv. A string runs only when `shell` is truthy.
    Subprocess,
    /// The command is the first string plus a following string list.
    Spawn,
    /// One string is a shell command. More strings are argv.
    Argv,
    /// `os.execv` takes the argv list as its second argument.
    ExecList,
    /// `os.execl` takes the program path, then one string per argv word.
    ExecArgs,
}

#[allow(clippy::too_many_arguments)]
fn score_swift_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    if let Some(text) = swift_inline(argv) {
        let commands = swift_process_commands(&text);
        scan_isolated(policy, tool_name, &commands, depth, local, git, best);
        return;
    }
    let Some(path) = swift_script_path(argv) else {
        return;
    };
    match load_script_file(&shell_path(&path, dir)) {
        Some(ScriptFile::Text(text)) => {
            let commands = swift_process_commands(&text);
            scan_isolated(policy, tool_name, &commands, depth, local, git, best);
        }
        Some(ScriptFile::Unscanned) => prefer(policy, best, unscanned_action(policy)),
        None => {}
    }
}

fn swift_inline(argv: &[String]) -> Option<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("swift"))
    {
        return None;
    }
    let mut index = 1usize;
    while index < argv.len() {
        if argv[index].as_str() == "-e" {
            return argv.get(index + 1).cloned();
        }
        index += 1;
    }
    None
}

fn swift_script_path(argv: &[String]) -> Option<String> {
    if !command_basename(argv.first()?).eq_ignore_ascii_case("swift") {
        return None;
    }
    argv.iter()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .cloned()
}

fn swift_process_commands(source: &str) -> Vec<String> {
    let code = strip_swift_line_comments(source);
    if !code.contains(".run(") {
        return Vec::new();
    }
    let Some(arguments) = swift_arguments(&code) else {
        return Vec::new();
    };
    if arguments.is_empty() {
        return Vec::new();
    }
    if let Some(executable) = swift_executable(&code)
        && !is_env_program(&executable)
    {
        let mut words = vec![command_basename(&executable).to_string()];
        words.extend(arguments);
        return vec![words.join(" ")];
    }
    vec![arguments.join(" ")]
}

fn is_env_program(path: &str) -> bool {
    command_basename(path).eq_ignore_ascii_case("env")
}

fn swift_arguments(code: &str) -> Option<Vec<String>> {
    let start = code.find(".arguments")?;
    let rest = &code[start + ".arguments".len()..];
    let chars: Vec<char> = rest.chars().collect();
    let open = chars.iter().position(|ch| *ch == '[')?;
    let (_, end) = balanced(&chars, open, '[', ']')?;
    let list: String = chars[open..end].iter().collect();
    arg_list(&list)
}

fn swift_executable(code: &str) -> Option<String> {
    let key = "fileURLWithPath:";
    let start = code.find(key)?;
    let rest = &code[start + key.len()..];
    let chars: Vec<char> = rest.chars().collect();
    let quote = chars.iter().position(|ch| *ch == '"')?;
    read_quoted(&chars, quote).map(|(text, _)| text)
}

fn strip_swift_line_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        if let Some((_, end)) = read_quoted(&chars, index) {
            out.extend(chars[index..end].iter());
            index = end;
            continue;
        }
        if chars[index] == '/' && chars.get(index + 1) == Some(&'/') {
            index = skip_line(&chars, index);
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn score_just_recipe<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(body) = just_recipe_body(argv, dir) else {
        return;
    };
    scan_isolated(
        policy,
        tool_name,
        std::slice::from_ref(&body),
        depth,
        local,
        git,
        best,
    );
}

fn just_recipe_body(argv: &[String], dir: Option<&str>) -> Option<String> {
    if !command_basename(argv.first()?).eq_ignore_ascii_case("just") {
        return None;
    }
    let mut file = None;
    let mut words = Vec::new();
    let mut index = 0usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--justfile" {
            index += 1;
            file = argv.get(index).cloned();
            index += 1;
            continue;
        }
        if let Some(path) = arg.strip_prefix("--justfile=")
            && !path.is_empty()
        {
            file = Some(path.to_string());
            index += 1;
            continue;
        }
        if arg == "-f" {
            index += 1;
            file = argv.get(index).cloned();
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        words.push(arg.to_string());
        index += 1;
    }
    let recipe = words.get(1)?.clone();
    let path = if let Some(path) = file {
        shell_path(&path, dir)
    } else {
        ["justfile", "Justfile", ".justfile"]
            .into_iter()
            .map(|name| shell_path(name, dir))
            .find(|candidate| std::path::Path::new(candidate).is_file())?
    };
    let text = std::fs::read_to_string(path).ok()?;
    let mut body = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if !inside {
            let header = format!("{recipe}:");
            if line == header
                || line
                    .strip_prefix(&header)
                    .is_some_and(|rest| rest.trim().is_empty())
            {
                inside = true;
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            body.push(line.trim().to_string());
            continue;
        }
        break;
    }
    if body.is_empty() {
        None
    } else {
        Some(body.join("\n"))
    }
}

#[allow(clippy::too_many_arguments)]
fn score_uv_run<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(path) = uv_python_script(argv) else {
        return;
    };
    match load_script_file(&shell_path(&path, dir)) {
        Some(ScriptFile::Text(text)) => {
            let commands = interpreter_commands(HostLang::Python, &text);
            scan_isolated(policy, tool_name, &commands, depth, local, git, best);
        }
        Some(ScriptFile::Unscanned) => prefer(policy, best, unscanned_action(policy)),
        None => {}
    }
}

fn uv_python_script(argv: &[String]) -> Option<String> {
    if !command_basename(argv.first()?).eq_ignore_ascii_case("uv") {
        return None;
    }
    let mut words = Vec::new();
    let mut index = 0usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        words.push(arg.to_string());
        index += 1;
    }
    if words.get(1).map(String::as_str) != Some("run") {
        return None;
    }
    words.into_iter().skip(2).find(|word| word.ends_with(".py"))
}

fn score_package_scripts<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let scripts = package_script_bodies(argv);
    scan_isolated(policy, tool_name, &scripts, depth, local, git, best);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExecTool {
    Npx,
    Npm,
    Pnpm,
    Bunx,
}

/// `npx`, `npm exec`, `npm x`, `pnpm exec`, `bunx`, and `bun x` run a
/// command. `npm` and `npx` prefer a `PATH` program over
/// `node_modules/.bin`. `pnpm` and `bunx` prefer the local bin. A bin
/// that is not executable does not run. `pnpm` runs nothing unless a
/// `package.json` is at that directory or a parent. `npm exec -c` and
/// `--call` run that string.
#[allow(clippy::too_many_arguments)]
fn score_package_exec<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(parsed) = package_exec(argv, dir) else {
        return;
    };
    if let Some(call) = parsed.call {
        scan_isolated(
            policy,
            tool_name,
            std::slice::from_ref(&call),
            depth,
            local,
            git,
            best,
        );
        return;
    }
    let Some(name) = parsed.command.first() else {
        return;
    };
    if name.is_empty() {
        return;
    }
    let on_path = command_on_path(name, local);
    let walk = !matches!(parsed.tool, ExecTool::Pnpm);
    let bin = local_bin(&parsed.start, name, walk);
    let script = bin.as_deref().filter(|path| hook_executable(path));
    match parsed.tool {
        ExecTool::Npx | ExecTool::Npm => {
            if on_path {
                score_exec_command(policy, tool_name, &parsed.command, depth, local, git, best);
            } else if let Some(path) = script {
                score_exec_file(policy, tool_name, path, depth, local, git, best);
            }
        }
        ExecTool::Pnpm => {
            if !package_json_above(&parsed.start) {
                return;
            }
            if let Some(path) = script {
                score_exec_file(policy, tool_name, path, depth, local, git, best);
            } else if bin.is_none() && on_path {
                score_exec_command(policy, tool_name, &parsed.command, depth, local, git, best);
            }
        }
        ExecTool::Bunx => {
            if let Some(path) = script {
                score_exec_file(policy, tool_name, path, depth, local, git, best);
            } else if bin.is_none() && on_path {
                score_exec_command(policy, tool_name, &parsed.command, depth, local, git, best);
            }
        }
    }
}

struct PackageExec {
    tool: ExecTool,
    start: std::path::PathBuf,
    call: Option<String>,
    command: Vec<String>,
}

fn package_exec(argv: &[String], dir: Option<&str>) -> Option<PackageExec> {
    let base = command_basename(argv.first()?).to_ascii_lowercase();
    let mode = match base.as_str() {
        "npx" => ExecTool::Npx,
        "bunx" => ExecTool::Bunx,
        "npm" => ExecTool::Npm,
        "pnpm" => ExecTool::Pnpm,
        "bun" => ExecTool::Bunx,
        _ => return None,
    };
    let direct = matches!(base.as_str(), "npx" | "bunx");
    let mut start = shell_dir(dir);
    let mut call = None;
    let mut positionals = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            index += 1;
            positionals.extend(argv[index..].iter().cloned());
            break;
        }
        if let Some(value) = exec_flag_value(mode, arg) {
            apply_exec_flag(mode, dir, &mut start, &mut call, value.0, value.1)?;
            index += 1;
            continue;
        }
        if exec_flag_takes_value(mode, arg) {
            index += 1;
            let value = argv.get(index)?.as_str();
            apply_exec_flag(mode, dir, &mut start, &mut call, arg, value)?;
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            if exec_bool_flag(arg) {
                index += 1;
                continue;
            }
            return None;
        }
        positionals.push(arg.to_string());
        index += 1;
    }
    if !direct {
        let sub = positionals.first()?.as_str();
        let ok = match base.as_str() {
            "npm" => sub == "exec" || sub == "x",
            "pnpm" => sub == "exec",
            "bun" => sub == "x",
            _ => false,
        };
        if !ok {
            return None;
        }
        positionals.remove(0);
    }
    if matches!(mode, ExecTool::Pnpm) {
        call = None;
    }
    Some(PackageExec {
        tool: mode,
        start,
        call,
        command: positionals,
    })
}

fn exec_bool_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--offline" | "--no-install" | "--bun" | "--yes" | "-y" | "--silent" | "--verbose"
    )
}

fn exec_flag_takes_value(mode: ExecTool, flag: &str) -> bool {
    match flag {
        "--prefix" => matches!(mode, ExecTool::Npx | ExecTool::Npm),
        "--dir" => mode == ExecTool::Pnpm,
        "--cwd" => mode == ExecTool::Bunx,
        "-c" | "--call" => matches!(mode, ExecTool::Npx | ExecTool::Npm),
        _ => false,
    }
}

fn exec_flag_value(mode: ExecTool, flag: &str) -> Option<(&str, &str)> {
    let (name, value) = if let Some(value) = flag.strip_prefix("--prefix=") {
        ("--prefix", value)
    } else if let Some(value) = flag.strip_prefix("--dir=") {
        ("--dir", value)
    } else if let Some(value) = flag.strip_prefix("--cwd=") {
        ("--cwd", value)
    } else if let Some(value) = flag.strip_prefix("--call=") {
        ("--call", value)
    } else {
        return None;
    };
    if value.is_empty() || !exec_flag_takes_value(mode, name) {
        return None;
    }
    Some((name, value))
}

fn apply_exec_flag(
    mode: ExecTool,
    dir: Option<&str>,
    start: &mut std::path::PathBuf,
    call: &mut Option<String>,
    name: &str,
    value: &str,
) -> Option<()> {
    if !exec_flag_takes_value(mode, name) {
        return None;
    }
    match name {
        "--prefix" | "--dir" | "--cwd" => {
            *start = std::path::PathBuf::from(shell_path(value, dir));
        }
        "-c" | "--call" => {
            *call = Some(value.to_string());
        }
        _ => return None,
    }
    Some(())
}

fn command_on_path(name: &str, env: &[(String, String)]) -> bool {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return false;
    }
    let owned;
    let path_var = if let Some(value) = last_assignment(env, "PATH") {
        value
    } else if let Ok(value) = std::env::var("PATH") {
        owned = value;
        owned.as_str()
    } else {
        return false;
    };
    std::env::split_paths(path_var).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() && hook_executable(&candidate)
    })
}

fn local_bin(start: &std::path::Path, name: &str, walk: bool) -> Option<std::path::PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return None;
    }
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("node_modules").join(".bin").join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !walk || !dir.pop() {
            return None;
        }
    }
}

fn package_json_above(start: &std::path::Path) -> bool {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("package.json").is_file() {
            return true;
        }
        if !dir.pop() {
            return false;
        }
    }
}

fn score_exec_command<'a>(
    policy: &'a Policy,
    tool_name: &str,
    command: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    if command.is_empty() {
        return;
    }
    let text = quote_argv(command);
    scan_isolated(
        policy,
        tool_name,
        std::slice::from_ref(&text),
        depth,
        local,
        git,
        best,
    );
}

fn score_exec_file<'a>(
    policy: &'a Policy,
    tool_name: &str,
    path: &std::path::Path,
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    match load_script_file(&path.display().to_string()) {
        Some(ScriptFile::Text(text)) => {
            scan_isolated(
                policy,
                tool_name,
                std::slice::from_ref(&text),
                depth,
                local,
                git,
                best,
            );
        }
        Some(ScriptFile::Unscanned) => prefer(policy, best, unscanned_action(policy)),
        None => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn score_interpreter<'a>(
    policy: &'a Policy,
    tool_name: &str,
    source: InterpSource<'_>,
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(lang) = source.argv.first().and_then(|word| host_lang(word)) else {
        return;
    };
    match interpreter_program(lang, &source, dir) {
        Some(LoadedProgram::Text(text)) => {
            let commands = interpreter_commands(lang, &text);
            scan_isolated(policy, tool_name, &commands, depth, local, git, best);
        }
        Some(LoadedProgram::Unscanned) => prefer(policy, best, unscanned_action(policy)),
        None => {}
    }
    for path in preload_files(lang, source.argv) {
        match load_script_file(&shell_path(&path, dir)) {
            Some(ScriptFile::Text(text)) => {
                let commands = interpreter_commands(lang, &text);
                scan_isolated(policy, tool_name, &commands, depth, local, git, best);
            }
            Some(ScriptFile::Unscanned) => prefer(policy, best, unscanned_action(policy)),
            None => {}
        }
    }
}

fn preload_files(lang: HostLang, argv: &[String]) -> Vec<String> {
    match lang {
        HostLang::Node => node_preloads(argv),
        HostLang::Ruby => ruby_requires(argv),
        HostLang::Perl => perl_module_files(argv),
        HostLang::Python => python_module_files(argv),
        _ => Vec::new(),
    }
}

fn node_preloads(argv: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-r" || arg == "--require" || arg == "--import" {
            index += 1;
            if let Some(path) = argv.get(index) {
                paths.push(path.clone());
            }
        } else if let Some(path) = arg
            .strip_prefix("--require=")
            .or_else(|| arg.strip_prefix("--import="))
            && !path.is_empty()
        {
            paths.push(path.to_string());
        }
        index += 1;
    }
    paths
}

fn ruby_requires(argv: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-r" {
            index += 1;
            if let Some(path) = argv.get(index) {
                paths.push(path.clone());
            }
        } else if let Some(path) = arg.strip_prefix("-r")
            && !path.is_empty()
        {
            paths.push(path.to_string());
        }
        index += 1;
    }
    paths
}

fn python_module_files(argv: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        if argv[index].as_str() == "-m" {
            index += 1;
            if let Some(name) = argv.get(index)
                && let Some(path) = python_module_path(name)
            {
                files.push(path);
            }
        }
        index += 1;
    }
    files
}

fn python_module_path(name: &str) -> Option<String> {
    if name.is_empty() || name.split('.').any(|part| part.is_empty() || part == "..") {
        return None;
    }
    let rel = name.replace('.', "/");
    let main = format!("{rel}/__main__.py");
    if std::path::Path::new(&main).is_file() {
        return Some(main);
    }
    let file = format!("{rel}.py");
    if std::path::Path::new(&file).is_file() {
        return Some(file);
    }
    None
}

fn perl_module_files(argv: &[String]) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut names = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-I" {
            index += 1;
            if let Some(dir) = argv.get(index) {
                dirs.push(dir.clone());
            }
        } else if let Some(dir) = arg.strip_prefix("-I")
            && !dir.is_empty()
        {
            dirs.push(dir.to_string());
        } else if arg == "-M" {
            index += 1;
            if let Some(name) = argv.get(index) {
                names.push(perl_module_name(name));
            }
        } else if let Some(name) = arg.strip_prefix("-M")
            && !name.is_empty()
        {
            names.push(perl_module_name(name));
        }
        index += 1;
    }
    let mut files = Vec::new();
    for name in names {
        let rel = name.replace("::", "/");
        for dir in &dirs {
            let path = std::path::Path::new(dir).join(format!("{rel}.pm"));
            if path.is_file() {
                files.push(path.display().to_string());
                break;
            }
        }
    }
    files
}

fn perl_module_name(raw: &str) -> String {
    raw.split(['=', ' ']).next().unwrap_or(raw).to_string()
}

#[allow(clippy::too_many_arguments)]
fn score_bun_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(path) = bun_script_path(argv) else {
        return;
    };
    match load_script_file(&shell_path(&path, dir)) {
        Some(ScriptFile::Text(text)) => {
            let commands = interpreter_commands(HostLang::Node, &text);
            scan_isolated(policy, tool_name, &commands, depth, local, git, best);
        }
        Some(ScriptFile::Unscanned) => prefer(policy, best, unscanned_action(policy)),
        None => {}
    }
}

fn bun_script_path(argv: &[String]) -> Option<String> {
    if !command_basename(argv.first()?).eq_ignore_ascii_case("bun") {
        return None;
    }
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            index += 1;
            let path = argv.get(index)?;
            return script_extension(path).then(|| path.clone());
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if arg.eq_ignore_ascii_case("run") {
            index += 1;
            continue;
        }
        return script_extension(arg).then(|| arg.to_string());
    }
    None
}

fn script_extension(path: &str) -> bool {
    [".js", ".mjs", ".cjs", ".ts", ".mts", ".tsx", ".jsx"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
}

#[allow(clippy::too_many_arguments)]
fn score_expect_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let programs = expect_programs(argv, dir);
    if programs.is_empty() {
        return;
    }
    let mut commands = Vec::new();
    for text in &programs {
        commands.extend(expect_spawns(text));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn expect_programs(argv: &[String], dir: Option<&str>) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("expect"))
    {
        return Vec::new();
    }
    let mut programs = Vec::new();
    let mut index = 1usize;
    let mut saw_inline = false;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-c" {
            saw_inline = true;
            index += 1;
            if let Some(code) = argv.get(index) {
                programs.push(code.clone());
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if !saw_inline && let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(arg, dir))
        {
            programs.push(text);
        }
        break;
    }
    programs
}

fn expect_spawns(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let rest = line.strip_prefix("spawn ")?.trim();
            if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            }
        })
        .collect()
}

fn score_tcl_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let Some(text) = tcl_program(argv) else {
        return;
    };
    let commands = tcl_execs(&text);
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn tcl_program(argv: &[String]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("tclsh") {
        return None;
    }
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        return match load_script_file(arg) {
            Some(ScriptFile::Text(text)) => Some(text),
            _ => None,
        };
    }
    None
}

fn tcl_execs(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut index = 0usize;
        while index + 4 <= chars.len() {
            let word = chars[index..index + 4].iter().collect::<String>();
            let before = index == 0 || !chars[index - 1].is_ascii_alphanumeric();
            let after = index + 4 == chars.len() || !chars[index + 4].is_ascii_alphanumeric();
            if before && after && word == "exec" && tcl_command_position(&chars, index) {
                let command: String = chars[index + 4..]
                    .iter()
                    .take_while(|ch| **ch != ']' && **ch != ';')
                    .collect();
                let command = command.trim();
                if !command.is_empty() {
                    commands.push(command.to_string());
                }
                index += 4;
                continue;
            }
            index += 1;
        }
    }
    commands
}

fn tcl_command_position(chars: &[char], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 && chars[cursor - 1].is_whitespace() {
        cursor -= 1;
    }
    cursor == 0 || matches!(chars[cursor - 1], '[' | ';')
}

fn score_sqlite_shell<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let commands = sqlite_shells(argv);
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn sqlite_shells(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("sqlite3") {
        return Vec::new();
    }
    let mut commands = Vec::new();
    let mut saw_db = false;
    for arg in argv.iter().skip(1) {
        if !saw_db {
            if !arg.starts_with('-') {
                saw_db = true;
            }
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        sqlite_text(&mut commands, arg, 0);
    }
    commands
}

fn sqlite_text(commands: &mut Vec<String>, text: &str, depth: u32) {
    if depth > 2 {
        return;
    }
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(".shell ") {
            let rest = rest.trim();
            if !rest.is_empty() {
                commands.push(rest.to_string());
            }
            continue;
        }
        if let Some(path) = line.strip_prefix(".read ")
            && let Some(ScriptFile::Text(body)) = load_script_file(path.trim())
        {
            sqlite_text(commands, &body, depth + 1);
        }
    }
}

fn score_octave_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for text in octave_programs(argv) {
        commands.extend(quoted_call(&text, "system", &['%', '#']));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn octave_programs(argv: &[String]) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("octave"))
    {
        return Vec::new();
    }
    let mut texts = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--eval" {
            index += 1;
            if let Some(code) = argv.get(index) {
                texts.push(code.clone());
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if let Some(ScriptFile::Text(text)) = load_script_file(arg) {
            texts.push(text);
        }
        break;
    }
    texts
}

#[allow(clippy::too_many_arguments)]
fn score_julia_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    let inline = flag_programs(argv, "julia", &["-e", "--eval"]);
    if inline.is_empty() {
        if let Some(text) = positional_script(argv, "julia", dir) {
            commands.extend(julia_commands(&text));
        }
    } else {
        for text in &inline {
            commands.extend(julia_commands(text));
        }
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn julia_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '#' {
            index = skip_line(&chars, index);
            continue;
        }
        if matches!(chars[index], '"' | '\'')
            && let Some((_, end)) = read_quoted(&chars, index)
        {
            index = end;
            continue;
        }
        if chars[index] == '`' {
            let start = index + 1;
            let end = chars[start..]
                .iter()
                .position(|ch| *ch == '`')
                .map_or(chars.len(), |offset| start + offset);
            let command: String = chars[start..end].iter().collect();
            let command = command.trim();
            if !command.is_empty() {
                commands.push(command.to_string());
            }
            index = if end < chars.len() { end + 1 } else { end };
            continue;
        }
        index += 1;
    }
    commands
}

#[allow(clippy::too_many_arguments)]
fn score_elixir_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    let inline = flag_programs(argv, "elixir", &["-e", "--eval"]);
    if inline.is_empty() {
        if let Some(text) = positional_script(argv, "elixir", dir) {
            commands.extend(elixir_commands(&text));
        }
    } else {
        for text in &inline {
            commands.extend(elixir_commands(text));
        }
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn elixir_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '#' {
            index = skip_line(&chars, index);
            continue;
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        let name = if starts_with(&chars, index, "System.shell") {
            Some("System.shell")
        } else if starts_with(&chars, index, "System.cmd") {
            Some("System.cmd")
        } else {
            None
        };
        if let Some(name) = name
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
        {
            let mut cursor = index + name.len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if cursor < chars.len()
                && chars[cursor] == '('
                && let Some((inner, end)) = balanced(&chars, cursor, '(', ')')
            {
                if let Some(command) = elixir_invocation(name, &inner) {
                    commands.push(command);
                }
                index = end;
                continue;
            }
        }
        index += 1;
    }
    commands
}

fn elixir_invocation(name: &str, inner: &str) -> Option<String> {
    let parts = split_args(inner);
    let program = arg_string(parts.first()?)?;
    if name == "System.shell" {
        return (!program.is_empty()).then_some(program);
    }
    let mut words = vec![program];
    if let Some(args) = parts.get(1).and_then(|part| arg_list(part)) {
        words.extend(args);
    }
    let command = words.join(" ");
    (!command.is_empty()).then_some(command)
}

fn score_erlang_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for text in flag_programs(argv, "erl", &["-eval"]) {
        commands.extend(quoted_call(&text, "os:cmd", &['%']));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

#[allow(clippy::too_many_arguments)]
fn score_go_run<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for text in go_sources(argv, dir) {
        commands.extend(go_commands(&text));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn go_sources(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("go") {
        return Vec::new();
    }
    let mut sources = Vec::new();
    let mut running = false;
    for arg in argv.iter().skip(1) {
        if arg == "--" {
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        if !running {
            if arg == "run" {
                running = true;
            } else {
                return Vec::new();
            }
            continue;
        }
        if arg.ends_with(".go")
            && let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(arg, dir))
        {
            sources.push(text);
        }
    }
    sources
}

fn go_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if starts_with(&chars, index, "//") {
            index = skip_line(&chars, index);
            continue;
        }
        if starts_with(&chars, index, "/*") {
            index = skip_block(&chars, index + 2);
            continue;
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if starts_with(&chars, index, "Command(")
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
        {
            let open = index + "Command".len();
            if let Some((inner, end)) = balanced(&chars, open, '(', ')') {
                let mut words = Vec::new();
                for part in split_args(&inner) {
                    let Some(word) = arg_string(&part) else {
                        break;
                    };
                    words.push(word);
                }
                if !words.is_empty() {
                    commands.push(words.join(" "));
                }
                index = end;
                continue;
            }
        }
        index += 1;
    }
    commands
}

fn cd_target(argv: &[String]) -> Option<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("cd"))
    {
        return None;
    }
    let mut path = None;
    for arg in argv.iter().skip(1) {
        if arg == "--" {
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        path = Some(arg.clone());
    }
    path
}

fn shell_dir(dir: Option<&str>) -> std::path::PathBuf {
    dir.map_or_else(|| std::path::PathBuf::from("."), std::path::PathBuf::from)
}

fn shell_path(path: &str, dir: Option<&str>) -> String {
    let file = std::path::Path::new(path);
    if file.is_absolute() || dir.is_none() {
        path.to_string()
    } else {
        shell_dir(dir).join(file).display().to_string()
    }
}

fn node_run_bodies(argv: &[String], dir: Option<&str>) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("node"))
    {
        return Vec::new();
    }
    let Some(name) = node_run_name(argv) else {
        return Vec::new();
    };
    let path = shell_dir(dir).join("package.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("scripts")
        .and_then(|item| item.get(&name))
        .and_then(|item| item.as_str())
        .filter(|body| !body.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect()
}

fn node_run_name(argv: &[String]) -> Option<String> {
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--run" {
            return argv.get(index + 1).cloned();
        }
        if let Some(name) = arg.strip_prefix("--run=").filter(|name| !name.is_empty()) {
            return Some(name.to_string());
        }
        index += 1;
    }
    None
}

fn bun_test_sources(argv: &[String], dir: Option<&str>) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("bun"))
    {
        return Vec::new();
    }
    let mut saw_test = false;
    let mut paths = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if !saw_test {
            if arg == "test" {
                saw_test = true;
            } else {
                return Vec::new();
            }
            index += 1;
            continue;
        }
        paths.push(arg.to_string());
        index += 1;
    }
    if !saw_test {
        return Vec::new();
    }
    let mut texts = Vec::new();
    let mut files = paths;
    if files.is_empty() {
        bun_test_walk(&shell_dir(dir), &mut files, 0);
    }
    for path in files {
        if let Some(ScriptFile::Text(text)) = load_script_file(&path) {
            texts.extend(interpreter_commands(HostLang::Node, &text));
        }
    }
    texts
}

fn bun_test_walk(dir: &std::path::Path, found: &mut Vec<String>, depth: u32) {
    if depth > 8 || found.len() >= 64 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "node_modules" || name == ".git" || name == "target" || name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            bun_test_walk(&path, found, depth + 1);
            continue;
        }
        if bun_test_name(&name) {
            found.push(path.display().to_string());
        }
    }
}

fn bun_test_name(name: &str) -> bool {
    name.ends_with(".test.js")
        || name.ends_with(".spec.js")
        || name.ends_with("_test.js")
        || name.ends_with("_spec.js")
}

fn java_commands(argv: &[String], dir: Option<&str>) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("java"))
    {
        return Vec::new();
    }
    if argv.iter().any(|arg| arg == "-jar") {
        return Vec::new();
    }
    let mut class = None;
    for arg in argv.iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        class = Some(arg.as_str());
    }
    let Some(class) = class else {
        return Vec::new();
    };
    if class.contains('.') || class.contains('/') || class.contains('\\') {
        return Vec::new();
    }
    let path = shell_dir(dir).join(format!("{class}.java"));
    let Some(ScriptFile::Text(text)) = load_script_file(&path.display().to_string()) else {
        return Vec::new();
    };
    process_builder_commands(&text)
}

fn lua_module_commands(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !matches!(host_lang(first), Some(HostLang::Lua)) {
        return Vec::new();
    }
    let mut commands = Vec::new();
    for name in lua_module_names(argv) {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains('.') {
            continue;
        }
        let path = shell_dir(dir).join(format!("{name}.lua"));
        if let Some(ScriptFile::Text(text)) = load_script_file(&path.display().to_string()) {
            commands.extend(interpreter_commands(HostLang::Lua, &text));
        }
    }
    commands
}

fn inline_loaded_commands(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    let Some(lang) = host_lang(first) else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for text in inline_code(lang, argv) {
        for raw in load_paths(lang, &text) {
            let Some(path) = resolve_script_path(dir, &raw) else {
                continue;
            };
            if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
                commands.extend(interpreter_commands(lang, &body));
            }
        }
    }
    commands
}

fn inline_code(lang: HostLang, argv: &[String]) -> Vec<String> {
    let flags: &[&str] = match lang {
        HostLang::Python => &["-c"],
        HostLang::Php => &["-r"],
        HostLang::Node | HostLang::Ruby | HostLang::Perl | HostLang::Lua | HostLang::R => &["-e"],
    };
    let mut texts = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if flags.contains(&arg) {
            index += 1;
            if let Some(code) = argv.get(index) {
                texts.push(code.clone());
            }
            index += 1;
            continue;
        }
        if matches!(lang, HostLang::Node)
            && let Some(code) = arg.strip_prefix("--eval=").filter(|code| !code.is_empty())
        {
            texts.push(code.to_string());
        }
        index += 1;
    }
    texts
}

fn load_paths(lang: HostLang, text: &str) -> Vec<String> {
    match lang {
        HostLang::Lua => call_strings(text, "dofile"),
        HostLang::Ruby => {
            let mut paths = call_strings(text, "load");
            paths.extend(call_strings(text, "require_relative"));
            paths.extend(
                call_strings(text, "require")
                    .into_iter()
                    .filter(|path| path.contains('/') || path.contains('\\')),
            );
            paths
        }
        HostLang::Node => call_strings(text, "require")
            .into_iter()
            .filter(|path| path.contains('/') || path.contains('\\'))
            .collect(),
        HostLang::Python => {
            let mut paths = python_exec_opens(text);
            paths.extend(python_imports(text));
            paths.extend(python_from_imports(text));
            paths.extend(call_strings(text, "run_path"));
            paths.extend(python_import_modules(text));
            paths
        }
        HostLang::Php => {
            let mut paths = call_strings(text, "include");
            paths.extend(call_strings(text, "include_once"));
            paths.extend(call_strings(text, "require"));
            paths.extend(call_strings(text, "require_once"));
            paths
        }
        HostLang::Perl => {
            let mut paths = call_strings(text, "do");
            paths.extend(call_strings(text, "require"));
            paths
                .into_iter()
                .filter(|path| path.contains('/') || path.contains('\\'))
                .collect()
        }
        HostLang::R => call_strings(text, "source"),
    }
}

fn call_strings(text: &str, word: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut paths = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if haskell_word(&chars, index, word) {
            let mut cursor = index + word.len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if chars.get(cursor) == Some(&'(') {
                cursor += 1;
                while cursor < chars.len() && chars[cursor].is_whitespace() {
                    cursor += 1;
                }
            }
            if let Some((path, _)) = read_quoted(&chars, cursor)
                && !path.is_empty()
            {
                paths.push(path);
            }
        }
        index += 1;
    }
    paths
}

fn python_import_modules(text: &str) -> Vec<String> {
    call_strings(text, "import_module")
        .into_iter()
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
        .map(|name| format!("{name}.py"))
        .collect()
}

fn python_from_imports(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut paths = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if haskell_word(&chars, index, "from") {
            let mut cursor = index + "from".len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            let start = cursor;
            if cursor < chars.len() && (chars[cursor].is_ascii_alphabetic() || chars[cursor] == '_')
            {
                cursor += 1;
                while cursor < chars.len()
                    && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '_')
                {
                    cursor += 1;
                }
                let name: String = chars[start..cursor].iter().collect();
                while cursor < chars.len() && chars[cursor].is_whitespace() {
                    cursor += 1;
                }
                if haskell_word(&chars, cursor, "import") {
                    paths.push(format!("{name}.py"));
                }
            }
        }
        index += 1;
    }
    paths
}

fn python_imports(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut paths = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if haskell_word(&chars, index, "import") {
            let mut cursor = index + "import".len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            let start = cursor;
            if cursor < chars.len() && (chars[cursor].is_ascii_alphabetic() || chars[cursor] == '_')
            {
                cursor += 1;
                while cursor < chars.len()
                    && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '_')
                {
                    cursor += 1;
                }
                let name: String = chars[start..cursor].iter().collect();
                paths.push(format!("{name}.py"));
            }
        }
        index += 1;
    }
    paths
}

fn python_exec_opens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut paths = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if haskell_word(&chars, index, "open") && exec_before_open(&chars, index) {
            let mut cursor = index + "open".len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if chars.get(cursor) == Some(&'(') {
                cursor += 1;
                while cursor < chars.len() && chars[cursor].is_whitespace() {
                    cursor += 1;
                }
            }
            if let Some((path, end)) = read_quoted(&chars, cursor)
                && !path.is_empty()
                && read_call_follows(&chars, end)
            {
                paths.push(path);
            }
        }
        index += 1;
    }
    paths
}

fn exec_before_open(chars: &[char], index: usize) -> bool {
    let mut cursor = index;
    loop {
        while cursor > 0 && chars[cursor - 1].is_whitespace() {
            cursor -= 1;
        }
        if cursor > 0 && chars[cursor - 1] == '(' {
            cursor -= 1;
            continue;
        }
        if cursor >= 7
            && starts_with(chars, cursor - 7, "compile")
            && (cursor == 7 || !chars[cursor - 8].is_ascii_alphanumeric())
        {
            cursor -= 7;
            continue;
        }
        break;
    }
    cursor >= 4
        && starts_with(chars, cursor - 4, "exec")
        && (cursor == 4 || !chars[cursor - 5].is_ascii_alphanumeric())
}

fn read_call_follows(chars: &[char], mut index: usize) -> bool {
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    if chars.get(index) == Some(&')') {
        index += 1;
    }
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    starts_with(chars, index, ".read")
}

fn resolve_script_path(dir: Option<&str>, raw: &str) -> Option<String> {
    if raw.is_empty() || raw.split(['/', '\\']).any(|part| part == "..") {
        return None;
    }
    let path = std::path::Path::new(raw);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        shell_dir(dir).join(path)
    };
    Some(full.display().to_string())
}

fn lua_module_names(argv: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-l" {
            index += 1;
            if let Some(name) = argv.get(index) {
                names.push(name.clone());
            }
            index += 1;
            continue;
        }
        if let Some(name) = arg.strip_prefix("-l").filter(|name| !name.is_empty()) {
            names.push(name.to_string());
        }
        index += 1;
    }
    names
}

fn env_preload_commands(
    argv: &[String],
    env: &[(String, String)],
    dir: Option<&str>,
) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    match host_lang(first) {
        Some(HostLang::Ruby) => {
            let mut commands =
                env_option_files(env, "RUBYOPT", "ruby", dir, HostLang::Ruby, ruby_requires);
            commands.extend(ruby_lib_files(argv, env, dir));
            commands.extend(ruby_inc_files(argv, dir));
            commands
        }
        Some(HostLang::Node) => {
            let mut commands = env_option_files(
                env,
                "NODE_OPTIONS",
                "node",
                dir,
                HostLang::Node,
                node_preloads,
            );
            commands.extend(node_path_files(argv, env, dir));
            commands
        }
        Some(HostLang::Perl) => {
            let mut commands = perl_opt_files(env, dir);
            commands.extend(perl_lib_files(argv, env, dir));
            commands.extend(perl_argv_inc_files(argv, dir));
            commands
        }
        Some(HostLang::Python) => {
            let mut commands = python_path_files(argv, env, dir);
            commands.extend(python_dash_m_files(argv, env, dir));
            commands
        }
        Some(HostLang::Php) => php_include_path_files(argv, dir),
        Some(HostLang::Lua) => lua_path_files(argv, env, dir),
        Some(HostLang::R) => r_profile_commands(argv, env, dir),
        _ => Vec::new(),
    }
}

fn env_option_files(
    env: &[(String, String)],
    name: &str,
    command: &str,
    dir: Option<&str>,
    lang: HostLang,
    paths: fn(&[String]) -> Vec<String>,
) -> Vec<String> {
    let Some(value) = last_assignment(env, name) else {
        return Vec::new();
    };
    let mut argv = vec![command.to_string()];
    argv.extend(value.split_whitespace().map(str::to_string));
    let mut commands = Vec::new();
    for raw in paths(&argv) {
        let Some(path) = resolve_script_path(dir, &raw) else {
            continue;
        };
        if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
            commands.extend(interpreter_commands(lang, &body));
        }
    }
    commands
}

fn perl_opt_files(env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    let Some(value) = last_assignment(env, "PERL5OPT") else {
        return Vec::new();
    };
    let mut argv = vec!["perl".to_string()];
    argv.extend(value.split_whitespace().map(str::to_string));
    let mut dirs = Vec::new();
    let mut names = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-I" {
            index += 1;
            if let Some(inc) = argv.get(index) {
                dirs.push(inc.clone());
            }
        } else if let Some(inc) = arg.strip_prefix("-I")
            && !inc.is_empty()
        {
            dirs.push(inc.to_string());
        } else if arg == "-M" {
            index += 1;
            if let Some(name) = argv.get(index) {
                names.push(perl_module_name(name));
            }
        } else if let Some(name) = arg.strip_prefix("-M")
            && !name.is_empty()
        {
            names.push(perl_module_name(name));
        }
        index += 1;
    }
    let base = shell_dir(dir);
    let mut commands = Vec::new();
    for name in names {
        if name.is_empty() || name.contains("..") {
            continue;
        }
        let rel = name.replace("::", "/");
        for inc in &dirs {
            let root = std::path::Path::new(inc);
            let root = if root.is_absolute() {
                root.to_path_buf()
            } else {
                base.join(root)
            };
            let path = root.join(format!("{rel}.pm"));
            if let Some(ScriptFile::Text(body)) = load_script_file(&path.display().to_string()) {
                commands.extend(interpreter_commands(HostLang::Perl, &body));
                break;
            }
        }
    }
    commands
}

fn resolved_inc(inc: &str, dir: Option<&str>) -> String {
    let path = std::path::Path::new(inc);
    if path.is_absolute() {
        path.display().to_string()
    } else {
        shell_dir(dir).join(path).display().to_string()
    }
}

fn perl_argv_inc_files(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let mut fake = vec!["perl".to_string()];
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-I" {
            index += 1;
            if let Some(inc) = argv.get(index) {
                fake.push(format!("-I{}", resolved_inc(inc, dir)));
            }
        } else if let Some(inc) = arg.strip_prefix("-I").filter(|inc| !inc.is_empty()) {
            fake.push(format!("-I{}", resolved_inc(inc, dir)));
        } else if arg == "-M" || arg.starts_with("-M") {
            fake.push(arg.to_string());
            if arg == "-M" {
                index += 1;
                if let Some(name) = argv.get(index) {
                    fake.push(name.clone());
                }
            }
        }
        index += 1;
    }
    let mut commands = Vec::new();
    for path in perl_module_files(&fake) {
        if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
            commands.extend(interpreter_commands(HostLang::Perl, &body));
        }
    }
    commands
}

fn ruby_inc_files(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-I" {
            index += 1;
            if let Some(inc) = argv.get(index) {
                dirs.push(resolved_inc(inc, dir));
            }
        } else if let Some(inc) = arg.strip_prefix("-I").filter(|inc| !inc.is_empty()) {
            dirs.push(resolved_inc(inc, dir));
        }
        index += 1;
    }
    if dirs.is_empty() {
        return Vec::new();
    }
    let mut names = Vec::new();
    for text in inline_code(HostLang::Ruby, argv) {
        for raw in call_strings(&text, "require") {
            if raw.contains('/') || raw.contains('\\') {
                continue;
            }
            if raw.ends_with(".rb") {
                names.push(raw);
            } else {
                names.push(format!("{raw}.rb"));
            }
        }
    }
    library_scripts(&dirs.join(":"), &names, None, HostLang::Ruby)
}

fn awk_path_commands(argv: &[String], env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    let base = command_basename(first);
    if !base.eq_ignore_ascii_case("awk") && !base.eq_ignore_ascii_case("gawk") {
        return Vec::new();
    }
    let Some(value) = last_assignment(env, "AWKPATH") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-f" || arg == "--file" {
            index += 1;
            if let Some(name) = argv.get(index)
                && !name.contains('/')
                && !name.contains('\\')
            {
                names.push(name.clone());
            }
        } else if let Some(name) = arg.strip_prefix("-f").filter(|name| !name.is_empty())
            && !name.contains('/')
            && !name.contains('\\')
        {
            names.push(name.to_string());
        }
        index += 1;
    }
    let mut commands = Vec::new();
    for entry in value.split(':') {
        if entry.is_empty() {
            continue;
        }
        let root = resolved_inc(entry, dir);
        for name in &names {
            let path = std::path::Path::new(&root).join(name);
            if let Some(ScriptFile::Text(body)) = load_script_file(&path.display().to_string()) {
                commands.extend(awk_systems(&body));
            }
        }
    }
    commands
}

fn perl_lib_files(argv: &[String], env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    let Some(value) = last_assignment(env, "PERL5LIB") else {
        return Vec::new();
    };
    let mut fake = vec!["perl".to_string()];
    for entry in value.split(':') {
        if entry.is_empty() {
            continue;
        }
        let root = std::path::Path::new(entry);
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            shell_dir(dir).join(root)
        };
        fake.push(format!("-I{}", root.display()));
    }
    fake.extend(argv.iter().skip(1).cloned());
    let mut commands = Vec::new();
    for path in perl_module_files(&fake) {
        if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
            commands.extend(interpreter_commands(HostLang::Perl, &body));
        }
    }
    commands
}

fn python_path_files(argv: &[String], env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    let Some(value) = last_assignment(env, "PYTHONPATH") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for text in inline_code(HostLang::Python, argv) {
        names.extend(python_imports(&text));
        names.extend(python_from_imports(&text));
        names.extend(python_import_modules(&text));
    }
    library_scripts(value, &names, dir, HostLang::Python)
}

fn python_dash_m_files(
    argv: &[String],
    env: &[(String, String)],
    dir: Option<&str>,
) -> Vec<String> {
    let Some(value) = last_assignment(env, "PYTHONPATH") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        if argv[index].as_str() == "-m" {
            index += 1;
            if let Some(name) = argv.get(index) {
                names.push(name.clone());
            }
        }
        index += 1;
    }
    let mut commands = Vec::new();
    for entry in value.split(':') {
        if entry.is_empty() {
            continue;
        }
        let root = std::path::Path::new(entry);
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            shell_dir(dir).join(root)
        };
        for name in &names {
            let Some(path) = module_file_in(&root, name) else {
                continue;
            };
            if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
                commands.extend(interpreter_commands(HostLang::Python, &body));
            }
        }
    }
    commands
}

fn module_file_in(root: &std::path::Path, name: &str) -> Option<String> {
    if name.is_empty() || name.split('.').any(|part| part.is_empty() || part == "..") {
        return None;
    }
    let rel = name.replace('.', "/");
    let main = root.join(&rel).join("__main__.py");
    if main.is_file() {
        return Some(main.display().to_string());
    }
    let file = root.join(format!("{rel}.py"));
    if file.is_file() {
        return Some(file.display().to_string());
    }
    None
}

fn node_path_files(argv: &[String], env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    let Some(value) = last_assignment(env, "NODE_PATH") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for text in inline_code(HostLang::Node, argv) {
        for raw in call_strings(&text, "require") {
            if raw.contains('/') || raw.contains('\\') || raw.starts_with('.') {
                continue;
            }
            names.push(format!("{raw}.js"));
        }
    }
    library_scripts(value, &names, dir, HostLang::Node)
}

fn php_include_path_files(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        let spec = if arg == "-d" {
            index += 1;
            argv.get(index).map(String::as_str)
        } else {
            arg.strip_prefix("-d")
        };
        if let Some(path) = spec.and_then(|item| item.strip_prefix("include_path="))
            && !path.is_empty()
        {
            dirs.push(path.to_string());
        }
        index += 1;
    }
    if dirs.is_empty() {
        return Vec::new();
    }
    let mut names = Vec::new();
    for text in inline_code(HostLang::Php, argv) {
        for word in ["include", "include_once", "require", "require_once"] {
            names.extend(call_strings(&text, word));
        }
    }
    library_scripts(&dirs.join(":"), &names, dir, HostLang::Php)
}

fn ruby_lib_files(argv: &[String], env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    let Some(value) = last_assignment(env, "RUBYLIB") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for text in inline_code(HostLang::Ruby, argv) {
        for raw in call_strings(&text, "require") {
            if raw.contains('/') || raw.contains('\\') {
                continue;
            }
            if raw.ends_with(".rb") {
                names.push(raw);
            } else {
                names.push(format!("{raw}.rb"));
            }
        }
    }
    library_scripts(value, &names, dir, HostLang::Ruby)
}

fn r_profile_commands(argv: &[String], env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    if argv
        .iter()
        .any(|arg| arg == "--vanilla" || arg == "--no-init-file")
    {
        return Vec::new();
    }
    let Some(value) = last_assignment(env, "R_PROFILE_USER") else {
        return Vec::new();
    };
    let Some(ScriptFile::Text(body)) = load_script_file(&shell_path(value, dir)) else {
        return Vec::new();
    };
    interpreter_commands(HostLang::R, &body)
}

fn lua_path_files(argv: &[String], env: &[(String, String)], dir: Option<&str>) -> Vec<String> {
    let Some(value) = last_assignment(env, "LUA_PATH") else {
        return Vec::new();
    };
    let names = lua_module_names(argv);
    let mut commands = Vec::new();
    for template in value.split(';') {
        if !template.contains('?') {
            continue;
        }
        for name in &names {
            if name.is_empty() || name.contains('.') || name.contains('/') || name.contains('\\') {
                continue;
            }
            let raw = template.replace('?', name);
            let Some(path) = resolve_script_path(dir, &raw) else {
                continue;
            };
            if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
                commands.extend(interpreter_commands(HostLang::Lua, &body));
            }
        }
    }
    commands
}

fn library_scripts(
    value: &str,
    names: &[String],
    dir: Option<&str>,
    lang: HostLang,
) -> Vec<String> {
    let mut commands = Vec::new();
    for entry in value.split(':') {
        if entry.is_empty() {
            continue;
        }
        let root = std::path::Path::new(entry);
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            shell_dir(dir).join(root)
        };
        for name in names {
            let path = root.join(name);
            if let Some(ScriptFile::Text(body)) = load_script_file(&path.display().to_string()) {
                commands.extend(interpreter_commands(lang, &body));
            }
        }
    }
    commands
}

fn octave_source_commands(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let mut commands = Vec::new();
    for text in octave_programs(argv) {
        for raw in call_strings(&text, "source") {
            let Some(path) = resolve_script_path(dir, &raw) else {
                continue;
            };
            if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
                commands.extend(quoted_call(&body, "system", &['%', '#']));
            }
        }
    }
    commands
}

fn go_generate_commands(argv: &[String], dir: Option<&str>) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("go"))
    {
        return Vec::new();
    }
    let mut saw_generate = false;
    let mut extra = false;
    for arg in argv.iter().skip(1) {
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') {
            continue;
        }
        if !saw_generate && arg == "generate" {
            saw_generate = true;
            continue;
        }
        extra = true;
    }
    if !saw_generate || extra {
        return Vec::new();
    }
    let mut commands = Vec::new();
    let Ok(entries) = std::fs::read_dir(shell_dir(dir)) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("go") {
            continue;
        }
        let Some(ScriptFile::Text(text)) = load_script_file(&path.display().to_string()) else {
            continue;
        };
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("//go:generate") else {
                continue;
            };
            let rest = rest.trim();
            if !rest.is_empty() {
                commands.push(rest.to_string());
            }
        }
    }
    commands
}

fn julia_include_commands(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let mut commands = Vec::new();
    for text in flag_programs(argv, "julia", &["-e", "--eval"]) {
        for raw in call_strings(&text, "include") {
            let Some(path) = resolve_script_path(dir, &raw) else {
                continue;
            };
            if let Some(ScriptFile::Text(body)) = load_script_file(&path) {
                commands.extend(julia_commands(&body));
            }
        }
    }
    commands
}

fn kotlin_commands(argv: &[String], dir: Option<&str>) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("kotlinc"))
    {
        return Vec::new();
    }
    let mut texts = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-script" {
            index += 1;
            if let Some(path) = argv.get(index)
                && let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(path, dir))
            {
                texts.push(text);
            }
            index += 1;
            continue;
        }
        index += 1;
    }
    let mut commands = Vec::new();
    for text in &texts {
        commands.extend(process_builder_commands(text));
    }
    commands
}

fn process_builder_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let marker = "ProcessBuilder";
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if starts_with(&chars, index, "//") {
            index = skip_line(&chars, index);
            continue;
        }
        if starts_with(&chars, index, "/*") {
            index = skip_block(&chars, index + 2);
            continue;
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if starts_with(&chars, index, marker)
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
        {
            let mut cursor = index + marker.len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if chars.get(cursor) == Some(&'(')
                && let Some((inner, end)) = balanced(&chars, cursor, '(', ')')
            {
                let mut words = Vec::new();
                for part in split_args(&inner) {
                    let Some(word) = arg_string(&part) else {
                        break;
                    };
                    words.push(word);
                }
                if !words.is_empty() {
                    commands.push(words.join(" "));
                }
                index = end;
                continue;
            }
        }
        index += 1;
    }
    commands
}

#[allow(clippy::too_many_arguments)]
fn score_coffee_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(text) = coffee_program(argv, dir) else {
        return;
    };
    let commands = interpreter_commands(HostLang::Node, &text);
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn coffee_program(argv: &[String], dir: Option<&str>) -> Option<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("coffee"))
    {
        return None;
    }
    if argv.iter().any(|arg| arg == "-c" || arg == "--compile") {
        return None;
    }
    for arg in argv.iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        if let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(arg, dir)) {
            return Some(text);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn score_haskell_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for command in ["ghci", "ghc"] {
        for text in flag_programs(argv, command, &["-e"]) {
            commands.extend(haskell_commands(&text));
        }
    }
    for command in ["runghc", "runhaskell"] {
        if let Some(text) = positional_script(argv, command, dir) {
            commands.extend(haskell_commands(&text));
        }
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn haskell_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if starts_with(&chars, index, "{-") {
            index = skip_haskell_block(&chars, index + 2);
            continue;
        }
        if starts_with(&chars, index, "--") {
            index = skip_line(&chars, index);
            continue;
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if haskell_word(&chars, index, "system") {
            if let Some(command) = haskell_string_after(&chars, index + "system".len()) {
                commands.push(command);
            }
        } else if haskell_word(&chars, index, "proc")
            && let Some(command) = haskell_proc_after(&chars, index + "proc".len())
        {
            commands.push(command);
        }
        index += 1;
    }
    commands
}

fn haskell_word(chars: &[char], index: usize, word: &str) -> bool {
    starts_with(chars, index, word)
        && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
        && chars
            .get(index + word.len())
            .is_none_or(|ch| !ch.is_ascii_alphanumeric())
}

fn haskell_string_after(chars: &[char], mut index: usize) -> Option<String> {
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    let (text, _) = read_quoted(chars, index)?;
    (!text.is_empty()).then_some(text)
}

fn haskell_proc_after(chars: &[char], mut index: usize) -> Option<String> {
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    let (program, mut cursor) = read_quoted(chars, index)?;
    while cursor < chars.len() && chars[cursor].is_whitespace() {
        cursor += 1;
    }
    let mut words = vec![program];
    if chars.get(cursor) == Some(&'[')
        && let Some((inner, _)) = balanced(chars, cursor, '[', ']')
    {
        let listed: Vec<char> = inner.chars().collect();
        let mut listed_at = 0usize;
        while listed_at < listed.len() {
            while listed_at < listed.len() && listed[listed_at].is_whitespace() {
                listed_at += 1;
            }
            if listed.get(listed_at) == Some(&',') {
                listed_at += 1;
                continue;
            }
            let Some((word, end)) = read_quoted(&listed, listed_at) else {
                break;
            };
            words.push(word);
            listed_at = end;
        }
    }
    let command = words.join(" ");
    (!command.is_empty()).then_some(command)
}

fn skip_haskell_block(chars: &[char], index: usize) -> usize {
    let mut cursor = index;
    while cursor + 1 < chars.len() {
        if chars[cursor] == '-' && chars[cursor + 1] == '}' {
            return cursor + 2;
        }
        cursor += 1;
    }
    chars.len()
}

fn score_mix_run<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for text in mix_inline(argv) {
        commands.extend(elixir_commands(&text));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn mix_inline(argv: &[String]) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("mix"))
    {
        return Vec::new();
    }
    let mut running = false;
    let mut texts = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-e" || arg == "--eval" {
            index += 1;
            if let Some(code) = argv.get(index) {
                texts.push(code.clone());
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if arg == "run" {
            running = true;
        }
        index += 1;
    }
    if running { texts } else { Vec::new() }
}

fn score_clojure_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for command in ["clj", "clojure"] {
        for text in flag_programs(argv, command, &["-e"]) {
            commands.extend(clojure_execs(&text));
        }
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn clojure_execs(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == ';' {
            index = skip_line(&chars, index);
            continue;
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if starts_with(&chars, index, ".exec")
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
            && let Some(command) = first_quoted_after(&chars, index + ".exec".len())
        {
            commands.push(command);
            index += ".exec".len();
            continue;
        }
        index += 1;
    }
    commands
}

fn first_quoted_after(chars: &[char], mut index: usize) -> Option<String> {
    let limit = index.saturating_add(200).min(chars.len());
    while index < limit {
        if let Some((text, _)) = read_quoted(chars, index)
            && !text.is_empty()
        {
            return Some(text);
        }
        index += 1;
    }
    None
}

fn score_cargo_run<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let Some(text) = cargo_main(argv) else {
        return;
    };
    let commands = rust_commands(&text);
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn cargo_main(argv: &[String]) -> Option<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case("cargo"))
    {
        return None;
    }
    let mut manifest = None;
    let mut running = false;
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            break;
        }
        if arg == "--manifest-path" {
            index += 1;
            manifest = argv.get(index).cloned();
            index += 1;
            continue;
        }
        if let Some(path) = arg.strip_prefix("--manifest-path=")
            && !path.is_empty()
        {
            manifest = Some(path.to_string());
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if arg == "run" {
            running = true;
        }
        index += 1;
    }
    if !running {
        return None;
    }
    let manifest = manifest.unwrap_or_else(|| "Cargo.toml".to_string());
    let main = std::path::Path::new(&manifest)
        .parent()
        .unwrap_or(std::path::Path::new(""))
        .join("src/main.rs");
    match load_script_file(&main.display().to_string()) {
        Some(ScriptFile::Text(text)) => Some(text),
        _ => None,
    }
}

fn rust_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let marker = "Command::new";
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if starts_with(&chars, index, "//") {
            index = skip_line(&chars, index);
            continue;
        }
        if starts_with(&chars, index, "/*") {
            index = skip_block(&chars, index + 2);
            continue;
        }
        if matches!(chars[index], '"' | '\'')
            && let Some((_, end)) = read_quoted(&chars, index)
        {
            index = end;
            continue;
        }
        if starts_with(&chars, index, marker)
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
            && let Some((words, end)) = rust_command_at(&chars, index + marker.len())
        {
            if !words.is_empty() {
                commands.push(words.join(" "));
            }
            index = end;
            continue;
        }
        index += 1;
    }
    commands
}

fn rust_command_at(chars: &[char], mut cursor: usize) -> Option<(Vec<String>, usize)> {
    while cursor < chars.len() && chars[cursor].is_whitespace() {
        cursor += 1;
    }
    if chars.get(cursor) != Some(&'(') {
        return None;
    }
    let (inner, mut end) = balanced(chars, cursor, '(', ')')?;
    let mut words = Vec::new();
    if let Some(program) = arg_string(inner.trim()) {
        words.push(program);
    }
    cursor = end;
    while cursor < chars.len() && chars[cursor].is_whitespace() {
        cursor += 1;
    }
    if starts_with(chars, cursor, ".args") {
        cursor += ".args".len();
        while cursor < chars.len() && chars[cursor].is_whitespace() {
            cursor += 1;
        }
        if chars.get(cursor) == Some(&'(')
            && let Some((args_inner, args_end)) = balanced(chars, cursor, '(', ')')
        {
            words.extend(rust_string_list(&args_inner));
            end = args_end;
        }
    }
    Some((words, end))
}

fn rust_string_list(inner: &str) -> Vec<String> {
    let chars: Vec<char> = inner.chars().collect();
    let mut index = 0usize;
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    let Some('[') = chars.get(index).copied() else {
        return Vec::new();
    };
    let Some((list_inner, _)) = balanced(&chars, index, '[', ']') else {
        return Vec::new();
    };
    let listed: Vec<char> = list_inner.chars().collect();
    let mut words = Vec::new();
    let mut listed_at = 0usize;
    while listed_at < listed.len() {
        while listed_at < listed.len() && listed[listed_at].is_whitespace() {
            listed_at += 1;
        }
        if listed.get(listed_at) == Some(&',') {
            listed_at += 1;
            continue;
        }
        let Some((word, end)) = read_quoted(&listed, listed_at) else {
            break;
        };
        words.push(word);
        listed_at = end;
    }
    words
}

#[allow(clippy::too_many_arguments)]
fn score_cmake_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for text in cmake_scripts(argv, dir) {
        commands.extend(cmake_commands(&text));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn cmake_scripts(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("cmake") {
        return Vec::new();
    }
    let mut scripts = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-P" {
            index += 1;
            if let Some(path) = argv.get(index)
                && let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(path, dir))
            {
                scripts.push(text);
            }
            index += 1;
            continue;
        }
        index += 1;
    }
    scripts
}

fn cmake_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '#' {
            index = skip_line(&chars, index);
            continue;
        }
        if starts_with(&chars, index, "execute_process") {
            let mut cursor = index + "execute_process".len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if cursor < chars.len()
                && chars[cursor] == '('
                && let Some((inner, end)) = balanced(&chars, cursor, '(', ')')
            {
                commands.extend(cmake_process_commands(&inner));
                index = end;
                continue;
            }
        }
        index += 1;
    }
    commands
}

fn cmake_process_commands(inner: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut in_command = false;
    for raw in inner.split_whitespace() {
        if raw == "COMMAND" {
            if in_command && !words.is_empty() {
                commands.push(words.join(" "));
            }
            words.clear();
            in_command = true;
            continue;
        }
        if in_command && cmake_keyword(raw) {
            if !words.is_empty() {
                commands.push(words.join(" "));
            }
            words.clear();
            in_command = false;
            continue;
        }
        if in_command {
            words.push(raw.to_string());
        }
    }
    if in_command && !words.is_empty() {
        commands.push(words.join(" "));
    }
    commands
}

fn cmake_keyword(word: &str) -> bool {
    matches!(
        word,
        "WORKING_DIRECTORY"
            | "TIMEOUT"
            | "OUTPUT_VARIABLE"
            | "ERROR_VARIABLE"
            | "RESULT_VARIABLE"
            | "OUTPUT_FILE"
            | "ERROR_FILE"
            | "INPUT_FILE"
            | "ENCODING"
    )
}

fn score_sbcl_script<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    best: &mut Option<&'a ActionId>,
) {
    let mut commands = Vec::new();
    for text in flag_programs(argv, "sbcl", &["--eval"]) {
        commands.extend(sbcl_commands(&text));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn sbcl_commands(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let marker = "sb-ext:run-program";
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == ';' {
            index = skip_line(&chars, index);
            continue;
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if starts_with(&chars, index, marker)
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
        {
            let mut cursor = index + marker.len();
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if let Some((program, end)) = read_quoted(&chars, cursor) {
                cursor = end;
                while cursor < chars.len() && chars[cursor].is_whitespace() {
                    cursor += 1;
                }
                let mut words = vec![program];
                if starts_with(&chars, cursor, "(list")
                    && let Some((inner, _)) = balanced(&chars, cursor, '(', ')')
                {
                    let listed: Vec<char> = inner.chars().collect();
                    let mut listed_at = 0usize;
                    if starts_with(&listed, listed_at, "list") {
                        listed_at += 4;
                    }
                    while listed_at < listed.len() {
                        while listed_at < listed.len() && listed[listed_at].is_whitespace() {
                            listed_at += 1;
                        }
                        let Some((word, end)) = read_quoted(&listed, listed_at) else {
                            break;
                        };
                        words.push(word);
                        listed_at = end;
                    }
                }
                let command = words.join(" ");
                if !command.is_empty() {
                    commands.push(command);
                }
                index = cursor;
                continue;
            }
        }
        index += 1;
    }
    commands
}

fn flag_programs(argv: &[String], command: &str, flags: &[&str]) -> Vec<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case(command))
    {
        return Vec::new();
    }
    let mut texts = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if flags.contains(&arg) {
            index += 1;
            if let Some(code) = argv.get(index) {
                texts.push(code.clone());
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        break;
    }
    texts
}

fn quoted_call(text: &str, name: &str, comments: &[char]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if comments.contains(&chars[index]) {
            index = skip_line(&chars, index);
            continue;
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if starts_with(&chars, index, name)
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
        {
            let mut cursor = index + name.chars().count();
            if cursor < chars.len() && chars[cursor].is_ascii_alphanumeric() {
                index += 1;
                continue;
            }
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if cursor < chars.len()
                && chars[cursor] == '('
                && let Some((inner, end)) = balanced(&chars, cursor, '(', ')')
            {
                if let Some(command) = arg_string(inner.trim())
                    && !command.is_empty()
                {
                    commands.push(command);
                }
                index = end;
                continue;
            }
        }
        index += 1;
    }
    commands
}

#[allow(clippy::too_many_arguments)]
fn score_awk_program<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let programs = awk_programs(argv, dir);
    let mut commands = Vec::new();
    for program in &programs {
        commands.extend(awk_systems(program));
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn awk_programs(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    let base = command_basename(first);
    if !base.eq_ignore_ascii_case("awk") && !base.eq_ignore_ascii_case("gawk") {
        return Vec::new();
    }
    let mut programs = Vec::new();
    let mut saw_file = false;
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            break;
        }
        if arg == "-f" || arg == "--file" {
            saw_file = true;
            index += 1;
            if let Some(path) = argv.get(index)
                && let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(path, dir))
            {
                programs.push(text);
            }
            index += 1;
            continue;
        }
        if let Some(path) = arg.strip_prefix("-f").filter(|path| !path.is_empty()) {
            saw_file = true;
            if let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(path, dir)) {
                programs.push(text);
            }
            index += 1;
            continue;
        }
        if matches!(arg, "-F" | "-v" | "--field-separator") {
            index += 2;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            index += 1;
            continue;
        }
        if !saw_file {
            programs.push(arg.to_string());
        }
        break;
    }
    programs
}

fn awk_systems(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if chars[index] == '#' {
            index = skip_line(&chars, index);
            continue;
        }
        if let Some((name, after)) = read_dotted_name(&chars, index) {
            let mut cursor = after;
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if name == "system"
                && cursor < chars.len()
                && chars[cursor] == '('
                && let Some((inner, end)) = balanced(&chars, cursor, '(', ')')
            {
                if let Some(command) = arg_string(inner.trim())
                    && !command.is_empty()
                {
                    commands.push(command);
                }
                index = end;
                continue;
            }
            index = after;
            continue;
        }
        index += 1;
    }
    commands
}

#[allow(clippy::too_many_arguments)]
fn score_osascript<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let texts = osascript_texts(argv, dir);
    let javascript = osascript_javascript(argv);
    let mut commands = Vec::new();
    for text in &texts {
        commands.extend(applescript_shells(text));
        if javascript {
            commands.extend(quoted_call(text, "system", &[]));
        }
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn osascript_javascript(argv: &[String]) -> bool {
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-l" || arg == "--language" {
            return argv
                .get(index + 1)
                .is_some_and(|lang| lang.eq_ignore_ascii_case("JavaScript"));
        }
        index += 1;
    }
    false
}

fn positional_script(argv: &[String], command: &str, dir: Option<&str>) -> Option<String> {
    if argv
        .first()
        .is_none_or(|word| !command_basename(word).eq_ignore_ascii_case(command))
    {
        return None;
    }
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        return match load_script_file(&shell_path(arg, dir)) {
            Some(ScriptFile::Text(text)) => Some(text),
            _ => None,
        };
    }
    None
}

fn osascript_texts(argv: &[String], dir: Option<&str>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    if !command_basename(first).eq_ignore_ascii_case("osascript") {
        return Vec::new();
    }
    let mut texts = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "-e" {
            index += 1;
            if let Some(text) = argv.get(index) {
                texts.push(text.clone());
            }
            index += 1;
            continue;
        }
        if arg == "-l" || arg == "--language" {
            index += 2;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if let Some(ScriptFile::Text(text)) = load_script_file(&shell_path(arg, dir)) {
            texts.push(text);
        }
        break;
    }
    texts
}

fn applescript_shells(text: &str) -> Vec<String> {
    let marker = "do shell script";
    let folded = text.to_ascii_lowercase();
    let mut commands = Vec::new();
    let mut from = 0usize;
    while let Some(found) = folded[from..].find(marker) {
        let start = from + found + marker.len();
        let rest = &text[start..];
        let chars: Vec<char> = rest.chars().collect();
        if let Some(quote) = chars.iter().position(|ch| *ch == '"')
            && let Some((command, _)) = read_quoted(&chars, quote)
            && !command.is_empty()
        {
            commands.push(command);
        }
        from = start;
        if from >= folded.len() {
            break;
        }
    }
    commands
}

fn host_lang(token: &str) -> Option<HostLang> {
    let base = command_basename(token).to_ascii_lowercase();
    match base.as_str() {
        "python" | "python2" | "python3" => Some(HostLang::Python),
        "node" => Some(HostLang::Node),
        "ruby" => Some(HostLang::Ruby),
        "perl" => Some(HostLang::Perl),
        "php" => Some(HostLang::Php),
        "lua" => Some(HostLang::Lua),
        "rscript" | "r" => Some(HostLang::R),
        other if other.starts_with("python3.") || other.starts_with("python2.") => {
            Some(HostLang::Python)
        }
        _ => None,
    }
}

fn interpreter_program(
    lang: HostLang,
    source: &InterpSource<'_>,
    dir: Option<&str>,
) -> Option<LoadedProgram> {
    let mut index = 1usize;
    let mut inline = None;
    let mut file: Option<&str> = None;
    while index < source.argv.len() {
        let arg = source.argv[index].as_str();
        if arg == "--" {
            index += 1;
            file = source.argv.get(index).map(String::as_str);
            break;
        }
        if lang == HostLang::Node
            && let Some(code) = arg.strip_prefix("--eval=")
        {
            inline = Some(code.to_string());
            break;
        }
        if takes_inline_arg(lang, arg) {
            index += 1;
            return Some(LoadedProgram::Text(
                source.argv.get(index).cloned().unwrap_or_default(),
            ));
        }
        if lang == HostLang::Python && arg == "-m" {
            return None;
        }
        if lang == HostLang::Php && arg == "-f" {
            index += 1;
            file = source.argv.get(index).map(String::as_str);
            break;
        }
        if arg.starts_with('-') && arg != "-" {
            if interp_flag_takes_value(lang, arg) {
                index += 1;
            }
            index += 1;
            continue;
        }
        file = Some(arg);
        break;
    }
    if let Some(text) = inline {
        return Some(LoadedProgram::Text(text));
    }
    let path = match file {
        Some("-") | None => source.stdin_file,
        Some(path) => Some(path),
    }?;
    match load_script_file(&shell_path(path, dir)) {
        Some(ScriptFile::Text(text)) => Some(LoadedProgram::Text(text)),
        Some(ScriptFile::Unscanned) => Some(LoadedProgram::Unscanned),
        None => None,
    }
}

fn takes_inline_arg(lang: HostLang, arg: &str) -> bool {
    match lang {
        HostLang::Python => arg == "-c",
        HostLang::Node => arg == "-e" || arg == "--eval",
        HostLang::Ruby | HostLang::Perl => arg == "-e",
        HostLang::Php => arg == "-r",
        HostLang::Lua | HostLang::R => arg == "-e",
    }
}

fn interp_flag_takes_value(lang: HostLang, arg: &str) -> bool {
    match lang {
        HostLang::Python => matches!(arg, "-W" | "-X"),
        HostLang::Node => matches!(arg, "-r" | "--require" | "--import"),
        HostLang::Ruby => matches!(arg, "-I"),
        HostLang::Perl => matches!(arg, "-I" | "-M" | "-m"),
        HostLang::Php | HostLang::Lua | HostLang::R => false,
    }
}

fn interpreter_commands(lang: HostLang, source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        if line_comment(lang, &chars, index) {
            index = skip_line(&chars, index);
            continue;
        }
        if block_comment(&chars, index) && matches!(lang, HostLang::Node | HostLang::Php) {
            index = skip_block(&chars, index + 2);
            continue;
        }
        if chars[index] == '`' {
            match lang {
                HostLang::Ruby | HostLang::Perl | HostLang::Php => {
                    if let Some((text, end)) = read_quoted(&chars, index) {
                        if !text.is_empty() {
                            commands.push(text);
                        }
                        index = end;
                        continue;
                    }
                }
                HostLang::Node => {
                    if let Some((_, end)) = read_quoted(&chars, index) {
                        index = end;
                        continue;
                    }
                }
                HostLang::Python | HostLang::Lua | HostLang::R => {}
            }
        }
        if let Some((_, end)) = read_quoted(&chars, index) {
            index = end;
            continue;
        }
        if let Some((name, after)) = read_dotted_name(&chars, index) {
            let mut cursor = after;
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if cursor < chars.len()
                && chars[cursor] == '('
                && let Some((found, end)) = parse_exec_call(lang, &name, &chars, cursor)
            {
                commands.extend(found);
                index = end;
                continue;
            }
            index = after;
            continue;
        }
        index += 1;
    }
    commands
}

fn line_comment(lang: HostLang, chars: &[char], index: usize) -> bool {
    match lang {
        HostLang::Python | HostLang::Ruby | HostLang::Perl | HostLang::R => chars[index] == '#',
        HostLang::Php => chars[index] == '#' || starts_with(chars, index, "//"),
        HostLang::Node => starts_with(chars, index, "//"),
        HostLang::Lua => starts_with(chars, index, "--"),
    }
}

fn block_comment(chars: &[char], index: usize) -> bool {
    starts_with(chars, index, "/*")
}

fn starts_with(chars: &[char], index: usize, text: &str) -> bool {
    text.chars()
        .enumerate()
        .all(|(offset, ch)| chars.get(index + offset) == Some(&ch))
}

fn skip_line(chars: &[char], index: usize) -> usize {
    chars
        .iter()
        .skip(index)
        .position(|ch| *ch == '\n')
        .map_or(chars.len(), |offset| index + offset)
}

fn skip_block(chars: &[char], index: usize) -> usize {
    let mut cursor = index;
    while cursor + 1 < chars.len() {
        if chars[cursor] == '*' && chars[cursor + 1] == '/' {
            return cursor + 2;
        }
        cursor += 1;
    }
    chars.len()
}

fn read_dotted_name(chars: &[char], index: usize) -> Option<(String, usize)> {
    if !chars[index].is_ascii_alphabetic() && chars[index] != '_' {
        return None;
    }
    let mut cursor = index + 1;
    while cursor < chars.len()
        && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '_' || chars[cursor] == '.')
    {
        cursor += 1;
    }
    let name: String = chars[index..cursor].iter().collect();
    if name.ends_with('.') || name.is_empty() {
        return None;
    }
    Some((name, cursor))
}

fn parse_exec_call(
    lang: HostLang,
    name: &str,
    chars: &[char],
    open: usize,
) -> Option<(Vec<String>, usize)> {
    let kind = exec_kind(lang, name)?;
    let (inner, end) = balanced(chars, open, '(', ')')?;
    let args = split_args(&inner);
    let commands = match kind {
        ExecKind::Shell => args
            .first()
            .and_then(|arg| arg_string(arg))
            .into_iter()
            .collect(),
        ExecKind::Subprocess => subprocess_command(&args),
        ExecKind::Spawn => spawn_command(&args),
        ExecKind::Argv => argv_command(&args),
        ExecKind::ExecList => exec_list_command(&args),
        ExecKind::ExecArgs => exec_args_command(&args),
    };
    Some((commands, end))
}

fn exec_kind(lang: HostLang, name: &str) -> Option<ExecKind> {
    match lang {
        HostLang::Python => match name {
            "os.system" | "os.popen" | "subprocess.getoutput" | "subprocess.getstatusoutput" => {
                Some(ExecKind::Shell)
            }
            "subprocess.run"
            | "subprocess.call"
            | "subprocess.check_call"
            | "subprocess.check_output"
            | "subprocess.Popen" => Some(ExecKind::Subprocess),
            "os.execv" | "os.execve" | "os.execvp" | "os.execvpe" => Some(ExecKind::ExecList),
            "os.execl" | "os.execlp" => Some(ExecKind::ExecArgs),
            _ => None,
        },
        HostLang::Node => match name.rsplit('.').next().unwrap_or(name) {
            "execSync" | "exec" => Some(ExecKind::Shell),
            "spawnSync" | "spawn" | "execFileSync" | "execFile" => Some(ExecKind::Spawn),
            _ => None,
        },
        HostLang::Ruby | HostLang::Perl => match name.rsplit('.').next().unwrap_or(name) {
            "system" | "exec" => Some(ExecKind::Argv),
            _ => None,
        },
        HostLang::Php => match name.rsplit('.').next().unwrap_or(name) {
            "system" | "exec" | "passthru" | "shell_exec" => Some(ExecKind::Shell),
            _ => None,
        },
        HostLang::Lua => match name {
            "os.execute" => Some(ExecKind::Shell),
            _ => None,
        },
        HostLang::R => match name.rsplit('.').next().unwrap_or(name) {
            "system" => Some(ExecKind::Shell),
            _ => None,
        },
    }
}

fn subprocess_command(args: &[String]) -> Vec<String> {
    let Some(first) = args.first() else {
        return Vec::new();
    };
    if let Some(words) = arg_list(first) {
        return vec![words.join(" ")];
    }
    if let Some(text) = arg_string(first)
        && args.iter().any(|arg| shell_enabled(arg))
    {
        return vec![text];
    }
    Vec::new()
}

fn exec_list_command(args: &[String]) -> Vec<String> {
    let Some(words) = args.get(1).and_then(|arg| arg_list(arg)) else {
        return Vec::new();
    };
    if words.is_empty() {
        Vec::new()
    } else {
        vec![words.join(" ")]
    }
}

fn exec_args_command(args: &[String]) -> Vec<String> {
    let mut words = Vec::new();
    for arg in args.iter().skip(1) {
        let Some(text) = arg_string(arg) else {
            break;
        };
        words.push(text);
    }
    if words.is_empty() {
        Vec::new()
    } else {
        vec![words.join(" ")]
    }
}

fn spawn_command(args: &[String]) -> Vec<String> {
    let Some(command) = args.first().and_then(|arg| arg_string(arg)) else {
        return Vec::new();
    };
    let Some(rest) = args.get(1).and_then(|arg| arg_list(arg)) else {
        return Vec::new();
    };
    let mut words = vec![command];
    words.extend(rest);
    vec![words.join(" ")]
}

fn argv_command(args: &[String]) -> Vec<String> {
    let mut words = Vec::new();
    for arg in args {
        let Some(text) = arg_string(arg) else {
            break;
        };
        words.push(text);
    }
    if words.is_empty() {
        Vec::new()
    } else {
        vec![words.join(" ")]
    }
}

fn shell_enabled(arg: &str) -> bool {
    let compact: String = arg.chars().filter(|ch| !ch.is_whitespace()).collect();
    let Some(value) = compact.strip_prefix("shell=") else {
        return false;
    };
    if let Some(text) = quoted_shell_flag(value) {
        return !text.is_empty();
    }
    match value {
        "True" => true,
        "False" | "0" | "None" => false,
        other => other.parse::<i64>().is_ok_and(|number| number != 0),
    }
}

fn quoted_shell_flag(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut text = String::new();
    for ch in chars {
        if ch == quote {
            return Some(text);
        }
        text.push(ch);
    }
    None
}

fn arg_string(arg: &str) -> Option<String> {
    let chars: Vec<char> = arg.trim().chars().collect();
    let (text, end) = read_quoted(&chars, 0)?;
    if end == chars.len() { Some(text) } else { None }
}

fn arg_list(arg: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = arg.trim().chars().collect();
    let open = *chars.first()?;
    if open != '[' && open != '(' {
        return None;
    }
    let close = if open == '[' { ']' } else { ')' };
    let (inner, end) = balanced(&chars, 0, open, close)?;
    if end != chars.len() {
        return None;
    }
    let mut words = Vec::new();
    for part in split_args(&inner) {
        if part.trim().is_empty() {
            continue;
        }
        words.push(arg_string(&part)?);
    }
    Some(words)
}

fn split_args(inner: &str) -> Vec<String> {
    let chars: Vec<char> = inner.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut index = 0usize;
    while index < chars.len() {
        if let Some((_, end)) = read_quoted(&chars, index) {
            current.extend(chars[index..end].iter());
            index = end;
            continue;
        }
        match chars[index] {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(chars[index]);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                current.push(chars[index]);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            ch => current.push(ch),
        }
        index += 1;
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn balanced(chars: &[char], open: usize, left: char, right: char) -> Option<(String, usize)> {
    if chars.get(open) != Some(&left) {
        return None;
    }
    let mut depth = 0i32;
    let mut inner = String::new();
    let mut index = open;
    while index < chars.len() {
        if depth > 0
            && let Some((_, end)) = read_quoted(chars, index)
        {
            inner.extend(chars[index..end].iter());
            index = end;
            continue;
        }
        if depth > 0 && line_comment_any(chars, index) {
            index = skip_line(chars, index);
            continue;
        }
        let ch = chars[index];
        if ch == left {
            depth += 1;
            if depth > 1 {
                inner.push(ch);
            }
            index += 1;
            continue;
        }
        if ch == right {
            depth -= 1;
            if depth == 0 {
                return Some((inner, index + 1));
            }
            inner.push(ch);
            index += 1;
            continue;
        }
        if depth > 0 {
            inner.push(ch);
        }
        index += 1;
    }
    None
}

fn line_comment_any(chars: &[char], index: usize) -> bool {
    chars[index] == '#' || starts_with(chars, index, "//")
}

fn read_quoted(chars: &[char], index: usize) -> Option<(String, usize)> {
    let quote = *chars.get(index)?;
    if !matches!(quote, '"' | '\'' | '`') {
        return None;
    }
    let triple = matches!(quote, '"' | '\'')
        && index + 2 < chars.len()
        && chars[index + 1] == quote
        && chars[index + 2] == quote;
    let mut cursor = index + if triple { 3 } else { 1 };
    let mut text = String::new();
    if triple {
        while cursor + 2 < chars.len() {
            if chars[cursor] == quote && chars[cursor + 1] == quote && chars[cursor + 2] == quote {
                return Some((text, cursor + 3));
            }
            if chars[cursor] == '\\' && cursor + 1 < chars.len() {
                text.push(unescape(chars[cursor + 1]));
                cursor += 2;
                continue;
            }
            text.push(chars[cursor]);
            cursor += 1;
        }
        return None;
    }
    while cursor < chars.len() {
        if chars[cursor] == '\\' && cursor + 1 < chars.len() {
            text.push(unescape(chars[cursor + 1]));
            cursor += 2;
            continue;
        }
        if chars[cursor] == quote {
            return Some((text, cursor + 1));
        }
        if chars[cursor] == '\n' {
            return None;
        }
        text.push(chars[cursor]);
        cursor += 1;
    }
    None
}

fn unescape(ch: char) -> char {
    match ch {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        other => other,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PackageTool {
    Npm,
    Pnpm,
    Bun,
}

fn package_script_bodies(argv: &[String]) -> Vec<String> {
    let Some(tool) = argv.first().and_then(|word| package_tool(word)) else {
        return Vec::new();
    };
    let Some(name) = package_script_name(tool, argv) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(package_dir(argv).join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(|item| item.as_object()) else {
        return Vec::new();
    };
    let mut bodies = Vec::new();
    let pre = format!("pre{name}");
    let post = format!("post{name}");
    for key in [pre, name, post] {
        if let Some(body) = scripts.get(&key).and_then(|item| item.as_str())
            && !body.is_empty()
        {
            bodies.push(body.to_string());
        }
    }
    bodies
}

fn package_tool(token: &str) -> Option<PackageTool> {
    match command_basename(token).to_ascii_lowercase().as_str() {
        "npm" => Some(PackageTool::Npm),
        "pnpm" => Some(PackageTool::Pnpm),
        "bun" => Some(PackageTool::Bun),
        _ => None,
    }
}

fn package_script_name(tool: PackageTool, argv: &[String]) -> Option<String> {
    let positionals = package_positionals(argv);
    let sub = positionals.get(1)?.as_str();
    match (tool, sub) {
        (PackageTool::Npm | PackageTool::Pnpm, "test" | "start") => Some(sub.to_string()),
        (PackageTool::Npm | PackageTool::Pnpm | PackageTool::Bun, "run") => {
            let name = positionals.get(2)?;
            if name.is_empty() {
                None
            } else {
                Some(name.clone())
            }
        }
        _ => None,
    }
}

fn package_dir(argv: &[String]) -> std::path::PathBuf {
    let mut index = 0usize;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            break;
        }
        if arg == "--prefix"
            && let Some(dir) = argv.get(index + 1)
        {
            return std::path::PathBuf::from(dir);
        }
        if let Some(dir) = arg.strip_prefix("--prefix=")
            && !dir.is_empty()
        {
            return std::path::PathBuf::from(dir);
        }
        index += 1;
    }
    std::path::PathBuf::from(".")
}

fn package_positionals(argv: &[String]) -> Vec<String> {
    let mut words = Vec::new();
    let mut index = 0usize;
    let mut options = true;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if options && arg == "--" {
            options = false;
            index += 1;
            continue;
        }
        if options && (arg == "--prefix" || arg.starts_with("--prefix=")) {
            if arg == "--prefix" {
                index += 1;
            }
            index += 1;
            continue;
        }
        if options && arg.starts_with('-') {
            index += 1;
            continue;
        }
        words.push(arg.to_string());
        index += 1;
    }
    words
}

/// `git -c core.hooksPath=dir commit` runs the executable commit
/// hooks. With no `core.hooksPath`, git reads `.git/hooks` in the
/// repository selected by `cd` and `-C`. `merge` runs
/// `pre-merge-commit`, `prepare-commit-msg`, and `post-merge`.
/// `--no-verify` skips `pre-commit`, `commit-msg`, and
/// `pre-merge-commit`. `checkout`, `switch`, and `clone` run
/// `post-checkout`. `clone -n` and `clone --no-checkout` do not.
/// `cherry-pick` runs `prepare-commit-msg` and `post-commit`.
/// `cherry-pick -n` and `cherry-pick --no-verify` do not.
/// A fast-forward `pull` runs `post-merge`. `pull --no-rebase`
/// also runs the merge commit hooks. `pull --rebase`, `pull -r`,
/// and `pull --rebase=merges` run the rebase hooks. `pull
/// --rebase=false` does not.
/// `git -C path` uses `path` as the repository. A later relative `-C`
/// joins the earlier one. `clone` runs `post-checkout` in the new
/// repository, which is not this directory. A stuck `-Cpath` is not
/// an option on this git.
fn default_hook_directory(argv: &[String], dir: Option<&str>) -> Option<String> {
    if git_subcommand(argv).is_none_or(|name| name == "clone") {
        return None;
    }
    let mut tree = shell_dir(dir);
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" || !token.starts_with('-') {
            break;
        }
        if token == "-C" {
            index += 1;
            if let Some(path) = argv.get(index) {
                tree = tree.join(path.as_str());
                index += 1;
            }
            continue;
        }
        let takes = !token.contains('=') && option_takes_value(token);
        index += 1;
        if takes && index < argv.len() {
            index += 1;
        }
    }
    Some(tree.join(".git").join("hooks").display().to_string())
}

fn git_hook_scripts(dir: Option<&str>, subcommand: &str, argv: &[String]) -> Vec<String> {
    let Some(dir) = dir.filter(|dir| !dir.is_empty()) else {
        return Vec::new();
    };
    let skip_verify = commit_no_verify(argv);
    let names: &[&str] = match subcommand {
        "commit" if skip_verify => &["prepare-commit-msg", "post-commit"],
        "commit" => &[
            "pre-commit",
            "prepare-commit-msg",
            "commit-msg",
            "post-commit",
        ],
        "merge" if skip_verify => &["prepare-commit-msg", "post-merge"],
        "merge" => &["pre-merge-commit", "prepare-commit-msg", "post-merge"],
        "checkout" | "switch" => &["post-checkout"],
        "clone" if clone_checks_out(argv) => &["post-checkout"],
        "cherry-pick" if cherry_pick_commits(argv) => &["prepare-commit-msg", "post-commit"],
        "pull" if pull_rebases(argv) => &["pre-rebase", "post-checkout", "post-rewrite"],
        "pull" if pull_merges(argv) => &[
            "pre-merge-commit",
            "prepare-commit-msg",
            "commit-msg",
            "post-merge",
        ],
        "pull" => &["post-merge"],
        "rebase" if rebase_skips_pre(argv) => &["post-checkout", "post-rewrite"],
        "rebase" => &["pre-rebase", "post-checkout", "post-rewrite"],
        "am" => &["applypatch-msg", "pre-applypatch", "post-applypatch"],
        _ => return Vec::new(),
    };
    names
        .iter()
        .filter_map(|name| hook_body(dir, name))
        .collect()
}

/// `git clone -n` and `--no-checkout` skip the worktree, so
/// `post-checkout` does not run. A short cluster that contains `n`
/// is the same flag.
fn clone_checks_out(argv: &[String]) -> bool {
    !argv.iter().any(|token| {
        token == "--no-checkout"
            || (token.starts_with('-') && !token.starts_with("--") && token.contains('n'))
    })
}

/// A fast-forward `git pull` runs `post-merge` only. `git pull
/// --no-rebase` that merges also runs `pre-merge-commit`,
/// `prepare-commit-msg`, and `commit-msg`.
fn pull_merges(argv: &[String]) -> bool {
    argv.iter().any(|token| token == "--no-rebase")
}

/// `git pull --rebase`, `pull -r`, and `pull --rebase=merges` run the
/// rebase hooks, including with `--no-verify`. `pull --rebase=false`
/// is a merge or fast-forward and does not.
fn pull_rebases(argv: &[String]) -> bool {
    argv.iter().any(|token| {
        if token == "--rebase" {
            return true;
        }
        if let Some(value) = token.strip_prefix("--rebase=") {
            return value != "false";
        }
        token.starts_with('-') && !token.starts_with("--") && token.contains('r')
    })
}

/// `git rebase --no-verify` still runs `post-checkout` and
/// `post-rewrite`. It does not run `pre-rebase`.
fn rebase_skips_pre(argv: &[String]) -> bool {
    argv.iter().any(|token| token == "--no-verify")
}

/// This git runs `prepare-commit-msg` and `post-commit` for a
/// cherry-pick that creates a commit. `-n` is `--no-commit` and does
/// not run them. `--no-verify` is not a cherry-pick flag here, so the
/// command fails before any hook.
fn cherry_pick_commits(argv: &[String]) -> bool {
    !argv.iter().any(|token| {
        token == "--no-verify"
            || token == "--no-commit"
            || (token.starts_with('-') && !token.starts_with("--") && token.contains('n'))
    })
}

fn commit_no_verify(argv: &[String]) -> bool {
    for token in argv {
        if token == "--" {
            break;
        }
        if token == "--no-verify" || token == "-n" {
            return true;
        }
        if token.starts_with('-') && !token.starts_with("--") && token.contains('n') {
            return true;
        }
    }
    false
}

fn hook_body(dir: &str, name: &str) -> Option<String> {
    let path = std::path::Path::new(dir).join(name);
    if !hook_executable(&path) {
        return None;
    }
    match load_script_file(&path.to_string_lossy()) {
        Some(ScriptFile::Text(text)) => Some(text),
        _ => None,
    }
}

fn hook_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn one_script(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value.to_string()]
    }
}

/// `GIT_SSH` is one program path. Git execs it. A space does not start a shell.
fn git_ssh_program(value: &str) -> Vec<String> {
    if value.is_empty() || !is_program_path(value) {
        Vec::new()
    } else {
        vec![value.to_string()]
    }
}

fn is_program_path(value: &str) -> bool {
    !value.chars().any(|ch| {
        ch.is_whitespace()
            || matches!(
                ch,
                ';' | '|' | '&' | '$' | '`' | '(' | ')' | '<' | '>' | '\'' | '"' | '\\'
            )
    })
}

fn last_assignment<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn selected_difftool(argv: &[String], configured: &[String]) -> Option<String> {
    // Options end at `--`. `--tool` after that is a path.
    let mut index = git_option_end(argv);
    if index < argv.len() {
        index += 1;
    }
    let mut from_cli = None;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" {
            break;
        }
        if let Some(name) = token.strip_prefix("--tool=") {
            if !name.is_empty() {
                from_cli = Some(name.to_string());
            }
            index += 1;
            continue;
        }
        if token == "--tool" || token == "-t" {
            index += 1;
            if let Some(name) = argv.get(index).filter(|name| !name.is_empty()) {
                from_cli = Some((*name).clone());
                index += 1;
            }
            continue;
        }
        // The value of `--extcmd` is a command, not a tool name.
        if token == "--extcmd" || token == "-x" {
            index += 1;
            if index < argv.len() {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    from_cli.or_else(|| configured.last().cloned())
}

fn git_uses_ssh(argv: &[String]) -> bool {
    // `status push` is not a push. The remote is the subcommand.
    let Some(subcommand) = git_subcommand(argv) else {
        return false;
    };
    if !matches!(
        subcommand,
        "ls-remote" | "fetch" | "pull" | "push" | "clone"
    ) {
        return false;
    }
    let mut saw_url = false;
    let mut ssh_url = false;
    let mut index = 0;
    while index < argv.len() {
        let word = argv[index].as_str();
        if word == "-c" || word == "--config-env" {
            index += 1;
            if index < argv.len() {
                index += 1;
            }
            continue;
        }
        if let Some(ssh) = remote_is_ssh(word) {
            saw_url = true;
            ssh_url |= ssh;
        }
        index += 1;
    }
    if ssh_url { true } else { !saw_url }
}

/// `Some(true)` is an ssh remote. `Some(false)` is some other URL.
/// A config key such as `credential.https://example.com.helper` is neither.
fn remote_is_ssh(word: &str) -> Option<bool> {
    let body = if word.starts_with("--") {
        word.split_once('=')?.1
    } else {
        word
    };
    if is_local_git_path(body) {
        return Some(false);
    }
    if body.starts_with("git@") && body.contains(':') {
        return Some(true);
    }
    let index = body.find("://")?;
    let scheme = &body[..index];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '-')
    {
        return None;
    }
    Some(scheme == "ssh")
}

/// A filesystem path. `origin` is a remote name, so it stays unset here.
fn is_local_git_path(body: &str) -> bool {
    if body.starts_with('/')
        || body.starts_with("./")
        || body.starts_with("../")
        || body == "."
        || body == ".."
        || body.starts_with("~/")
    {
        return true;
    }
    if body.contains('/') && !body.contains(':') {
        return true;
    }
    let mut chars = body.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(drive), Some(':'), Some('/' | '\\')) if drive.is_ascii_alphabetic()
    )
}

/// Git ignores an alias that hides one of these commands.
fn git_command(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "add",
        "am",
        "apply",
        "archive",
        "bisect",
        "blame",
        "branch",
        "bundle",
        "checkout",
        "cherry",
        "cherry-pick",
        "clean",
        "clone",
        "commit",
        "config",
        "credential",
        "describe",
        "diff",
        "difftool",
        "fetch",
        "format-patch",
        "gc",
        "grep",
        "init",
        "log",
        "ls-files",
        "ls-remote",
        "merge",
        "mergetool",
        "mv",
        "notes",
        "pull",
        "push",
        "range-diff",
        "rebase",
        "reflog",
        "remote",
        "reset",
        "restore",
        "revert",
        "rm",
        "shortlog",
        "show",
        "stash",
        "status",
        "submodule",
        "switch",
        "tag",
        "worktree",
    ];
    let folded = name.to_ascii_lowercase();
    NAMES.binary_search(&folded.as_str()).is_ok()
}

fn git_option_end(argv: &[String]) -> usize {
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" {
            return (index + 1).min(argv.len());
        }
        if !token.starts_with('-') {
            return index;
        }
        let takes = !token.contains('=') && option_takes_value(token);
        index += 1;
        if takes && index < argv.len() {
            index += 1;
        }
    }
    index
}

fn git_subcommand(argv: &[String]) -> Option<&str> {
    argv.get(git_option_end(argv)).map(String::as_str)
}

fn git_args_after_subcommand(argv: &[String]) -> &[String] {
    let index = git_option_end(argv);
    if index < argv.len() {
        &argv[index + 1..]
    } else {
        &[]
    }
}

fn quote_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|word| quote_shell_word(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_shell_word(word: &str) -> String {
    if !word.is_empty()
        && word.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '-' | '_' | '.' | '/' | ':' | '@' | '+' | ',' | '=' | '~'
                )
        })
    {
        return word.to_string();
    }
    let mut quoted = String::from("'");
    for ch in word.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

fn command_argv(command: &str) -> Vec<String> {
    for part in tokenize_parts(command) {
        if let ShellPart::Segment(segment) = part
            && let Some(executed) = executed_argv(&segment.tokens, &segment.in_eval, &[])
        {
            return executed.argv;
        }
    }
    Vec::new()
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

/// `bash path` and `sh path` run that file. `bash -c` and `bash -s` do
/// not. After `--`, the next word is a path, even when it looks like `-c`.
fn shell_file_operand(argv: &[String]) -> Option<&str> {
    let base = command_basename(argv.first()?);
    if !is_shell_name(base) {
        return None;
    }
    let mut index = 1;
    let mut after_dash = false;
    while index < argv.len() {
        let token = &argv[index];
        if !after_dash && token == "--" {
            after_dash = true;
            index += 1;
            continue;
        }
        if after_dash {
            return Some(token.as_str());
        }
        if shell_opt_takes_value(token) {
            index += 2;
            continue;
        }
        if token.starts_with('+') {
            index += 1;
            continue;
        }
        if token.starts_with('-') {
            if c_argument(token, argv.get(index + 1)).is_some() {
                return None;
            }
            if let Some(flags) = token.strip_prefix('-')
                && !flags.is_empty()
                && !flags.starts_with('-')
                && flags.contains('s')
            {
                return None;
            }
            index += 1;
            continue;
        }
        if is_stdin_script(token) {
            return None;
        }
        return Some(token.as_str());
    }
    None
}

/// Non-interactive bash runs `BASH_ENV`. Interactive bash runs
/// `--rcfile` or `--init-file`. A non-interactive `--rcfile` does not
/// run, and an interactive shell does not run `BASH_ENV`.
fn bash_startup_file<'a>(argv: &'a [String], env: &'a [(String, String)]) -> Option<&'a str> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("bash") {
        return None;
    }
    let mut interactive = false;
    let mut rcfile: Option<&str> = None;
    let mut index = 1usize;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            break;
        }
        if shell_opt_takes_value(token) {
            let name = token.strip_prefix("--").unwrap_or(token);
            if name == "rcfile" || name == "init-file" {
                rcfile = argv.get(index + 1).map(String::as_str);
            }
            index += 2;
            continue;
        }
        if let Some(flags) = token.strip_prefix('-')
            && !flags.is_empty()
            && !flags.starts_with('-')
            && flags.contains('i')
        {
            interactive = true;
        }
        if token.starts_with('-') || token.starts_with('+') {
            index += 1;
            continue;
        }
        break;
    }
    if interactive {
        return rcfile.filter(|path| !path.is_empty());
    }
    env.iter()
        .rev()
        .find(|(name, _)| name == "BASH_ENV")
        .map(|(_, value)| value.as_str())
        .filter(|path| !path.is_empty())
}

/// `ZDOTDIR=dir zsh` runs `dir/.zshenv`. `zsh -f` does not.
fn zsh_startup_file(argv: &[String], env: &[(String, String)]) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("zsh") {
        return None;
    }
    let mut index = 1usize;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            break;
        }
        if token == "-f" || token == "--no-rcs" {
            return None;
        }
        if let Some(flags) = token.strip_prefix('-')
            && !flags.is_empty()
            && !flags.starts_with('-')
            && flags.contains('f')
        {
            return None;
        }
        if token.starts_with('-') || token.starts_with('+') {
            index += 1;
            continue;
        }
        break;
    }
    let dir = env
        .iter()
        .rev()
        .find(|(name, _)| name == "ZDOTDIR")
        .map(|(_, value)| value.as_str())?;
    let dir = dir.trim_end_matches(['/', '\\']);
    if dir.is_empty() {
        return None;
    }
    Some(format!("{dir}/.zshenv"))
}

/// `make -f file` runs the tab recipes in that file. `make -n` only
/// prints them. With no `-f`, GNU make reads `GNUmakefile`, then
/// `makefile`, then `Makefile`.
#[allow(clippy::too_many_arguments)]
fn score_makefile<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(path) = make_file(argv, dir) else {
        return;
    };
    let Some(loaded) = load_script_file(&path) else {
        return;
    };
    match loaded {
        ScriptFile::Text(text) => {
            let recipes: Vec<String> = text
                .lines()
                .filter_map(|line| {
                    let line = line.strip_prefix('\t')?.trim();
                    if line.is_empty() || line.starts_with('#') {
                        None
                    } else {
                        Some(line.replace("$$", "$"))
                    }
                })
                .collect();
            scan_isolated(policy, tool_name, &recipes, depth, local, git, best);
        }
        ScriptFile::Unscanned => prefer(policy, best, unscanned_action(policy)),
    }
}

fn make_file(argv: &[String], dir: Option<&str>) -> Option<String> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("make") && !base.eq_ignore_ascii_case("gmake") {
        return None;
    }
    let mut file: Option<&str> = None;
    let mut index = 1usize;
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            break;
        }
        if matches!(
            token.as_str(),
            "-n" | "--dry-run" | "--just-print" | "--recon"
        ) {
            return None;
        }
        if token == "-f" || token == "--file" || token == "--makefile" {
            file = argv.get(index + 1).map(String::as_str);
            index += 2;
            continue;
        }
        if let Some(rest) = token.strip_prefix("-f")
            && !rest.is_empty()
            && !token.starts_with("--")
        {
            file = Some(rest);
            index += 1;
            continue;
        }
        if let Some(flags) = token.strip_prefix('-')
            && !flags.is_empty()
            && !flags.starts_with('-')
            && flags.contains('n')
        {
            return None;
        }
        index += 1;
    }
    if let Some(path) = file.filter(|path| !path.is_empty()) {
        return Some(shell_path(path, dir));
    }
    ["GNUmakefile", "makefile", "Makefile"]
        .into_iter()
        .map(|name| shell_path(name, dir))
        .find(|name| load_script_file(name).is_some())
}

/// `ninja` runs the `command` of each build it schedules. With no
/// target, a `default` line chooses the roots; otherwise every build
/// runs. A named target also runs the builds it depends on. `-n`
/// only prints. An unknown target runs nothing.
#[allow(clippy::too_many_arguments)]
fn score_ninja<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    let Some(spec) = ninja_spec(argv, dir) else {
        return;
    };
    if spec.dry {
        return;
    }
    let Some(ScriptFile::Text(text)) = load_script_file(&spec.file) else {
        return;
    };
    let file = parse_ninja(&text);
    let targets = ninja_roots(&file, &spec.targets);
    let Some(targets) = targets else {
        return;
    };
    let commands = ninja_commands(&file, &targets);
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

struct NinjaSpec {
    file: String,
    targets: Vec<String>,
    dry: bool,
}

fn ninja_spec(argv: &[String], dir: Option<&str>) -> Option<NinjaSpec> {
    let base = command_basename(argv.first()?);
    if !base.eq_ignore_ascii_case("ninja") {
        return None;
    }
    let mut tree = shell_dir(dir);
    let mut file = None;
    let mut targets = Vec::new();
    let mut dry = false;
    let mut index = 1usize;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "--" {
            targets.extend(argv[index + 1..].iter().cloned());
            break;
        }
        if token == "-n" || token == "--dry-run" {
            dry = true;
            index += 1;
            continue;
        }
        if token == "-C" {
            index += 1;
            tree = tree.join(argv.get(index)?.as_str());
            index += 1;
            continue;
        }
        if token == "-f" {
            index += 1;
            file = Some(argv.get(index)?.as_str());
            index += 1;
            continue;
        }
        if matches!(token, "-t" | "-d" | "-j" | "-k" | "-w") {
            if token == "-t" {
                return None;
            }
            index += 2;
            continue;
        }
        if token.starts_with("-j") && token.len() > 2 {
            index += 1;
            continue;
        }
        if token == "-v" || token == "--verbose" {
            index += 1;
            continue;
        }
        if token.starts_with('-') {
            return None;
        }
        targets.push(token.to_string());
        index += 1;
    }
    let path = match file {
        Some(name) => tree.join(name),
        None => tree.join("build.ninja"),
    };
    Some(NinjaSpec {
        file: path.display().to_string(),
        targets,
        dry,
    })
}

struct NinjaFile {
    rules: Vec<(String, String)>,
    builds: Vec<NinjaEdge>,
    defaults: Vec<String>,
}

struct NinjaEdge {
    outputs: Vec<String>,
    rule: String,
    inputs: Vec<String>,
}

fn parse_ninja(text: &str) -> NinjaFile {
    let mut rules: Vec<(String, String)> = Vec::new();
    let mut builds: Vec<NinjaEdge> = Vec::new();
    let mut defaults: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            let name = current.clone();
            let command = ninja_command(trimmed);
            if let (Some(name), Some(command)) = (name, command)
                && let Some(rule) = rules.iter_mut().rev().find(|rule| rule.0 == name)
            {
                rule.1 = command;
            }
            continue;
        }
        current = None;
        if let Some(rest) = trimmed.strip_prefix("rule ") {
            let Some(name) = rest.split_whitespace().next() else {
                continue;
            };
            let name = name.to_string();
            rules.push((name.clone(), String::new()));
            current = Some(name);
            continue;
        }
        if let Some(edge) = ninja_edge(trimmed) {
            builds.push(edge);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("default ") {
            defaults.extend(rest.split_whitespace().map(str::to_string));
        }
    }
    NinjaFile {
        rules,
        builds,
        defaults,
    }
}

fn ninja_command(line: &str) -> Option<String> {
    let rest = line.strip_prefix("command")?;
    let rest = rest.trim_start().strip_prefix('=')?;
    Some(rest.trim().to_string())
}

fn ninja_edge(line: &str) -> Option<NinjaEdge> {
    let rest = line.strip_prefix("build ")?;
    let (outputs, right) = rest.split_once(':')?;
    let outputs: Vec<String> = outputs.split_whitespace().map(str::to_string).collect();
    if outputs.is_empty() {
        return None;
    }
    let mut words = right
        .split_whitespace()
        .filter(|word| *word != "|" && *word != "||");
    let rule = words.next()?.to_string();
    let inputs = words.map(str::to_string).collect();
    Some(NinjaEdge {
        outputs,
        rule,
        inputs,
    })
}

fn ninja_roots(file: &NinjaFile, requested: &[String]) -> Option<Vec<String>> {
    let known = |name: &str| {
        file.builds
            .iter()
            .any(|edge| edge.outputs.iter().any(|output| output == name))
    };
    if !requested.is_empty() {
        if requested.iter().any(|name| !known(name)) {
            return None;
        }
        return Some(requested.to_vec());
    }
    if !file.defaults.is_empty() {
        return Some(file.defaults.clone());
    }
    let mut roots = Vec::new();
    for edge in &file.builds {
        for output in &edge.outputs {
            if !roots.iter().any(|have| have == output) {
                roots.push(output.clone());
            }
        }
    }
    Some(roots)
}

fn ninja_commands(file: &NinjaFile, roots: &[String]) -> Vec<String> {
    let mut commands = Vec::new();
    let mut seen = Vec::new();
    for root in roots {
        ninja_walk(file, root, &mut commands, &mut seen);
    }
    commands
}

fn ninja_walk(file: &NinjaFile, target: &str, commands: &mut Vec<String>, seen: &mut Vec<String>) {
    if seen.iter().any(|name| name == target) {
        return;
    }
    let Some((outputs, rule, inputs)) = file
        .builds
        .iter()
        .find(|edge| edge.outputs.iter().any(|output| output == target))
        .map(|edge| (edge.outputs.clone(), edge.rule.clone(), edge.inputs.clone()))
    else {
        seen.push(target.to_string());
        return;
    };
    seen.extend(outputs);
    for input in inputs {
        ninja_walk(file, &input, commands, seen);
    }
    if let Some(command) = file
        .rules
        .iter()
        .rev()
        .find(|item| item.0 == rule)
        .map(|item| item.1.clone())
        .filter(|command| !command.is_empty())
    {
        commands.push(command);
    }
}

/// `vim -c '!cmd'`, `--cmd '!cmd'`, `+!cmd`, and a `-S` script run
/// `cmd` in a shell. `qa!` and `:echo` do not. A leading `:` is
/// optional.
#[allow(clippy::too_many_arguments)]
fn score_vim<'a>(
    policy: &'a Policy,
    tool_name: &str,
    argv: &[String],
    depth: u32,
    local: &[(String, String)],
    git: &GitBindings,
    dir: Option<&str>,
    best: &mut Option<&'a ActionId>,
) {
    if !command_basename(argv.first().unwrap_or(&String::new())).eq_ignore_ascii_case("vim") {
        return;
    }
    let mut commands = Vec::new();
    let mut index = 1usize;
    while index < argv.len() {
        let token = argv[index].as_str();
        if token == "-c" || token == "--cmd" || token == "-S" {
            index += 1;
            let Some(value) = argv.get(index) else {
                break;
            };
            if token == "-S" {
                commands.extend(vim_script_shells(&shell_path(value, dir)));
            } else if let Some(shell) = vim_shell(value) {
                commands.push(shell);
            }
            index += 1;
            continue;
        }
        if let Some(shell) = token.strip_prefix('+').filter(|rest| !rest.is_empty())
            && let Some(shell) = vim_shell(shell)
        {
            commands.push(shell);
        }
        index += 1;
    }
    scan_isolated(policy, tool_name, &commands, depth, local, git, best);
}

fn vim_shell(command: &str) -> Option<String> {
    let trimmed = command
        .trim()
        .strip_prefix(':')
        .unwrap_or(command.trim())
        .trim();
    let rest = trimmed.strip_prefix('!')?.trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

fn vim_script_shells(path: &str) -> Vec<String> {
    let Some(ScriptFile::Text(text)) = load_script_file(path) else {
        return Vec::new();
    };
    text.lines().filter_map(vim_shell).collect()
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

/// `NAME=value` replaces the value. `NAME+=value` appends, and a name
/// this command has not set yet becomes that text.
fn apply_assignment(env: &mut Vec<(String, String)>, token: &str) -> bool {
    if let Some((name, value)) = plain_assignment(token) {
        set_env(env, name, value);
        return true;
    }
    if let Some((name, value)) = append_assignment(token) {
        append_env(env, name, value);
        return true;
    }
    false
}

fn apply_assignment_token(token: &str) -> bool {
    plain_assignment(token).is_some() || append_assignment(token).is_some() || is_assignment(token)
}

fn is_assignment_statement(tokens: &[String]) -> bool {
    !tokens.is_empty() && tokens.iter().all(|token| apply_assignment_token(token))
}

/// `{`, `!`, and keyword `time` run the assignment. `then`, `else`,
/// `elif`, and `do` run only on some paths, so both values stay visible
/// to `eval`.
fn is_assignment_leader(token: &str) -> bool {
    matches!(token, "{" | "then" | "else" | "elif" | "do" | "!" | "time")
}

fn assignment_suffix(tokens: &[String]) -> Option<(&[String], bool)> {
    let mut index = 0;
    let mut conditional = false;
    while tokens
        .get(index)
        .is_some_and(|token| is_assignment_leader(token))
    {
        if matches!(tokens[index].as_str(), "then" | "else" | "elif" | "do") {
            conditional = true;
        }
        index += 1;
    }
    let rest = &tokens[index..];
    is_assignment_statement(rest).then_some((rest, conditional))
}

fn assignment_name(token: &str) -> Option<&str> {
    if let Some((name, _)) = plain_assignment(token) {
        return Some(name);
    }
    append_assignment(token).map(|(name, _)| name)
}

fn wrapper_sets_env(wrapper: &str) -> bool {
    matches!(wrapper, "env" | "sudo" | "doas")
}

fn unset_env(env: &mut Vec<(String, String)>, name: &str) {
    env.retain(|(key, _)| key != name);
}

fn set_env(env: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some(slot) = env.iter_mut().rev().find(|(key, _)| key == name) {
        slot.1 = value.to_string();
    } else {
        env.push((name.to_string(), value.to_string()));
    }
}

fn append_env(env: &mut Vec<(String, String)>, name: &str, extra: &str) {
    if let Some(slot) = env.iter_mut().rev().find(|(key, _)| key == name) {
        slot.1.push_str(extra);
    } else {
        env.push((name.to_string(), extra.to_string()));
    }
}

fn append_assignment(token: &str) -> Option<(&str, &str)> {
    let (name, value) = token.split_once("+=")?;
    if !env_name(name) {
        return None;
    }
    Some((name, value))
}

fn env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn plain_assignment(token: &str) -> Option<(&str, &str)> {
    if !is_assignment(token) {
        return None;
    }
    let (name, value) = token.split_once('=')?;
    if name.ends_with('+') {
        return None;
    }
    Some((name, value))
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
/// An unescaped backtick or `$(...)` still runs.
fn read_dollar_double<I>(
    chars: &mut std::iter::Peekable<I>,
    token: &mut String,
    substitutions: &mut Vec<String>,
) where
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
            if ch == '`' {
                fold_quoted_subst(read_backtick(chars), token, substitutions);
                continue;
            }
            if ch == '$' && chars.peek() == Some(&'(') {
                chars.next();
                fold_quoted_subst(read_balanced_parens(chars), token, substitutions);
                continue;
            }
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
    /// Parallel to `tokens`. True when every character of that word came
    /// from single quotes or `$'...'`.
    in_eval: Vec<bool>,
    heredocs: Vec<String>,
    substitutions: Vec<String>,
    /// A pipeline stage or a background command. Its assignments do not
    /// come back to the parent shell.
    isolated: bool,
    /// Bodies of `<(...)` words, in order. `>(...)` is not a script.
    proc_inputs: Vec<String>,
    /// A literal `< path` that is the stdin `read` and `mapfile` use.
    stdin_file: Option<String>,
    /// `read x < <(echo ...)` reads that command's printed text.
    stdin_proc: Option<String>,
    /// A here-string or heredoc is the stdin those builtins use.
    read_heredoc: bool,
}

#[derive(Default)]
struct PendingSegment {
    proc_inputs: Vec<String>,
    stdin_file: Option<String>,
    stdin_proc: Option<String>,
    read_heredoc: bool,
}

impl PendingSegment {
    fn is_empty(&self) -> bool {
        self.proc_inputs.is_empty()
            && self.stdin_file.is_none()
            && self.stdin_proc.is_none()
            && !self.read_heredoc
    }

    fn clear_stdin(&mut self) {
        self.stdin_file = None;
        self.stdin_proc = None;
        self.read_heredoc = false;
    }
}

enum ShellPart {
    Function { name: String, body: String },
    Segment(ShellSegment),
}

struct PendingHeredoc {
    owner: usize,
    delim: String,
    dash: bool,
    /// Any quote in the delimiter word. `<<'EOF'`, `<<E'OF'`, `<<$'EOF'`,
    /// `<<$"EOF"`, and `<<\EOF` do not expand the body.
    quoted: bool,
}

/// Token so far is `C:` or `C:\...`. The next backslash is a separator.
fn drive_separator(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() || chars.next() != Some(':') {
        return false;
    }
    matches!(chars.next(), None | Some('\\'))
}

/// A path character. A quote, expansion, or separator still ends the word.
fn is_drive_path_char(ch: char) -> bool {
    !ch.is_whitespace()
        && !matches!(
            ch,
            ';' | '&'
                | '|'
                | '<'
                | '>'
                | '('
                | ')'
                | '\''
                | '"'
                | '$'
                | '`'
                | '{'
                | '}'
                | '\\'
                | '#'
        )
}

fn tokenize_parts(command: &str) -> Vec<ShellPart> {
    let mut parts = Vec::new();
    let mut current = Vec::new();
    let mut token = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    let mut quoted = false;
    let mut word_single: Option<bool> = None;
    let mut in_eval_words: Vec<bool> = Vec::new();
    let mut heredocs: Vec<PendingHeredoc> = Vec::new();
    let mut substitutions = Vec::new();
    let mut stdin_bodies = Vec::new();
    let mut pending = PendingSegment::default();
    let mut next_isolated = false;
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
                // Same rule as `$"..."`. A newline disappears. `$`, a
                // backtick, `"`, and `\` drop the backslash. Every other
                // character keeps it, so `"C:\dir\file"` stays a path.
                let Some(next) = chars.next() else {
                    token.push('\\');
                    continue;
                };
                if next == '\0' {
                    token.push('\\');
                    continue;
                }
                match next {
                    '\\' | '$' | '`' | '"' => token.push(next),
                    '\n' => {}
                    other => {
                        token.push('\\');
                        token.push(other);
                    }
                }
                continue;
            }
            // `echo "$(rm)"` and `echo "`rm`"` run rm. The backslash arm
            // already kept an escaped backtick literal. Single quotes
            // never reach this branch.
            if open == '"' && ch == '`' {
                fold_quoted_subst(read_backtick(&mut chars), &mut token, &mut substitutions);
                continue;
            }
            if open == '"' && ch == '$' && chars.peek() == Some(&'(') {
                chars.next();
                fold_quoted_subst(
                    read_balanced_parens(&mut chars),
                    &mut token,
                    &mut substitutions,
                );
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
                    note_eval_quote(&mut word_single, true);
                    read_ansi_c(&mut chars, &mut token);
                } else {
                    note_eval_quote(&mut word_single, false);
                    read_dollar_double(&mut chars, &mut token, &mut substitutions);
                }
                continue;
            }
            if next == '(' {
                chars.next();
                fold_unquoted_subst(
                    read_balanced_parens(&mut chars),
                    &mut token,
                    &mut current,
                    &mut quoted,
                    &mut word_single,
                    &mut in_eval_words,
                    &mut substitutions,
                );
                continue;
            }
        }
        match ch {
            '\'' | '"' => {
                note_eval_quote(&mut word_single, ch == '\'');
                quote = Some(ch);
                quoted = true;
            }
            '\\' => {
                // `C:\dir\file` is a Windows path. The backslash is a
                // separator there, not an escape. `rm\ -rf` still escapes.
                if drive_separator(&token)
                    && chars.peek().is_some_and(|next| is_drive_path_char(*next))
                {
                    let next = chars.next().expect("peeked");
                    note_eval_quote(&mut word_single, false);
                    token.push('\\');
                    token.push(next);
                } else if let Some(next) = chars.next()
                    && next != '\n'
                    && next != '\0'
                {
                    note_eval_quote(&mut word_single, false);
                    token.push(next);
                }
            }
            '`' => fold_unquoted_subst(
                read_backtick(&mut chars),
                &mut token,
                &mut current,
                &mut quoted,
                &mut word_single,
                &mut in_eval_words,
                &mut substitutions,
            ),
            '(' | ')' => {
                push_token(
                    &mut token,
                    &mut current,
                    &mut quoted,
                    &mut word_single,
                    &mut in_eval_words,
                );
                current.push(ch.to_string());
                in_eval_words.push(false);
            }
            '{' if token.is_empty() && brace_is_its_own_word(chars.peek().copied()) => {
                if let Some(name) = function_header(&current) {
                    if !substitutions.is_empty() || !pending.is_empty() {
                        parts.push(ShellPart::Segment(ShellSegment {
                            tokens: Vec::new(),
                            in_eval: Vec::new(),
                            heredocs: Vec::new(),
                            substitutions: std::mem::take(&mut substitutions),
                            isolated: false,
                            proc_inputs: std::mem::take(&mut pending.proc_inputs),
                            stdin_file: pending.stdin_file.take(),
                            stdin_proc: pending.stdin_proc.take(),
                            read_heredoc: std::mem::take(&mut pending.read_heredoc),
                        }));
                    }
                    current.clear();
                    in_eval_words.clear();
                    let body = read_brace_body(&mut chars);
                    parts.push(ShellPart::Function { name, body });
                } else {
                    current.push("{".to_string());
                    in_eval_words.push(false);
                }
            }
            '<' | '>' if chars.peek() == Some(&'(') => {
                let input = ch == '<';
                chars.next();
                let body = read_balanced_parens(&mut chars);
                substitutions.push(body.clone());
                if input {
                    pending.proc_inputs.push(body);
                }
            }
            '<' if chars.peek() == Some(&'<') => {
                chars.next();
                push_token(
                    &mut token,
                    &mut current,
                    &mut quoted,
                    &mut word_single,
                    &mut in_eval_words,
                );
                pending.clear_stdin();
                pending.read_heredoc = true;
                if chars.peek() == Some(&'<') {
                    chars.next();
                    if let Some(body) = read_here_string(&mut chars, &mut substitutions) {
                        stdin_bodies.push(body);
                    }
                } else if let Some((delim, dash, quoted_delim)) = read_heredoc_delim(&mut chars) {
                    heredocs.push(PendingHeredoc {
                        owner: parts.len(),
                        delim,
                        dash,
                        quoted: quoted_delim,
                    });
                }
            }
            '<' | '>' => {
                let second = chars.peek().copied();
                let compound = matches!(
                    (ch, second),
                    ('>', Some('>' | '&' | '|')) | ('<', Some('>' | '&'))
                );
                consume_redirect_tail(&mut chars, ch);
                let fd = if !quoted && is_fd_word(&token) {
                    let parsed = token.parse::<i32>().ok();
                    token.clear();
                    parsed
                } else {
                    push_token(
                        &mut token,
                        &mut current,
                        &mut quoted,
                        &mut word_single,
                        &mut in_eval_words,
                    );
                    None
                };
                let fd = fd.unwrap_or(if ch == '<' { 0 } else { 1 });
                if ch == '<' && fd == 0 {
                    if compound {
                        skip_redirect_word(&mut chars, &mut substitutions);
                        pending.clear_stdin();
                    } else {
                        match take_stdin_target(&mut chars, &mut substitutions) {
                            StdinTarget::Path(path) => {
                                pending.clear_stdin();
                                pending.stdin_file = Some(path);
                            }
                            StdinTarget::Proc(body) => {
                                pending.clear_stdin();
                                pending.stdin_proc = Some(body);
                            }
                            StdinTarget::Opaque => pending.clear_stdin(),
                        }
                    }
                } else {
                    skip_redirect_word(&mut chars, &mut substitutions);
                }
            }
            '&' if chars.peek() == Some(&'>') => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
                push_token(
                    &mut token,
                    &mut current,
                    &mut quoted,
                    &mut word_single,
                    &mut in_eval_words,
                );
                skip_redirect_word(&mut chars, &mut substitutions);
            }
            ' ' | '\t' | '\r' => push_token(
                &mut token,
                &mut current,
                &mut quoted,
                &mut word_single,
                &mut in_eval_words,
            ),
            '\n' => {
                push_token(
                    &mut token,
                    &mut current,
                    &mut quoted,
                    &mut word_single,
                    &mut in_eval_words,
                );
                // A newline after `|` or `|&` is still that pipeline, so
                // the next command stays isolated. A finished command is not.
                let continues_pipeline = next_isolated
                    && current.is_empty()
                    && substitutions.is_empty()
                    && stdin_bodies.is_empty()
                    && pending.is_empty();
                if !continues_pipeline {
                    push_shell_segment(
                        &mut current,
                        &mut in_eval_words,
                        &mut substitutions,
                        &mut stdin_bodies,
                        &mut parts,
                        next_isolated,
                        &mut pending,
                    );
                    next_isolated = false;
                }
                for item in std::mem::take(&mut heredocs) {
                    let body = read_heredoc_body(&mut chars, &item.delim, item.dash);
                    if let Some(ShellPart::Segment(segment)) = parts.get_mut(item.owner) {
                        if !item.quoted {
                            scan_heredoc_expansions(&body, &mut segment.substitutions);
                        }
                        segment.heredocs.push(body);
                    }
                }
            }
            '&' | '|' | ';' => {
                // `|&` is one pipe. The `&` is not a background operator.
                let pipe_both = ch == '|' && chars.peek() == Some(&'&');
                if pipe_both {
                    chars.next();
                }
                let doubled = !pipe_both && matches!(ch, '&' | '|') && chars.peek() == Some(&ch);
                if doubled {
                    chars.next();
                }
                // A pipeline stage and a background command do not publish
                // assignments. `&&`, `||`, and `;` stay in this shell.
                let own_shell = pipe_both || (!doubled && matches!(ch, '&' | '|'));
                let isolated = next_isolated || own_shell;
                next_isolated = pipe_both || (!doubled && ch == '|');
                push_token(
                    &mut token,
                    &mut current,
                    &mut quoted,
                    &mut word_single,
                    &mut in_eval_words,
                );
                push_shell_segment(
                    &mut current,
                    &mut in_eval_words,
                    &mut substitutions,
                    &mut stdin_bodies,
                    &mut parts,
                    isolated,
                    &mut pending,
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
            _ => {
                note_eval_quote(&mut word_single, false);
                token.push(ch);
            }
        }
    }
    push_token(
        &mut token,
        &mut current,
        &mut quoted,
        &mut word_single,
        &mut in_eval_words,
    );
    push_shell_segment(
        &mut current,
        &mut in_eval_words,
        &mut substitutions,
        &mut stdin_bodies,
        &mut parts,
        next_isolated,
        &mut pending,
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

fn note_eval_quote(word_single: &mut Option<bool>, single: bool) {
    *word_single = Some(match *word_single {
        None => single,
        Some(true) if single => true,
        _ => false,
    });
}

fn push_token(
    token: &mut String,
    current: &mut Vec<String>,
    quoted: &mut bool,
    word_single: &mut Option<bool>,
    in_eval: &mut Vec<bool>,
) {
    if *quoted || !token.is_empty() {
        current.push(std::mem::take(token));
        in_eval.push(*word_single == Some(true));
    }
    *quoted = false;
    *word_single = None;
}

/// Printed text of a command substitution. `echo`, `printf`, `true`,
/// `false`, `:`, and `cat` of a regular file are that text. A pipeline's
/// stdout is the last stage only. `cat` with no files copies the previous
/// stage. A printer mixed with another command still inserts the printed
/// text, and the other command is scored on its own. Anything unknown
/// stays a substitution so the body is scored once. One pass: the printed
/// text is not scanned again here. `eval` tokenizes it later.
const FOLD_DEPTH: u32 = 8;

/// A file past one megabyte has no bytes to insert. The tokenizer cannot
/// see the policy, so the substitution list carries this marker and the
/// scorer matches `shell.deep`.
const UNSCANNED_SUBST: &str = "\0snapif-unscanned";

enum Printed {
    Text(String),
    Unscanned,
    Unknown,
}

/// `while read` then `printf` of that name prints the redirected file.
/// `mapfile -t` then `printf` of the first element prints that line.
fn read_back_print(body: &str, depth: u32, substitutions: &mut Vec<String>) -> Option<Printed> {
    let words = loose_words(body)?;
    if let Some(path) = while_read_path(&words) {
        return Some(loaded_print(path, depth, substitutions, false));
    }
    let peeled = peel_simple_prefix(&words);
    if let Some((path, first_only)) = mapfile_path(peeled) {
        return Some(loaded_print(path, depth, substitutions, first_only));
    }
    if let Some(path) = scalar_read_path(peeled) {
        return Some(loaded_print(path, depth, substitutions, true));
    }
    None
}

fn loaded_print(
    path: &str,
    depth: u32,
    substitutions: &mut Vec<String>,
    first_line: bool,
) -> Printed {
    match load_text_operand(path, depth, substitutions) {
        Loaded::Text(text) => {
            if first_line {
                Printed::Text(text.lines().next().unwrap_or("").to_string())
            } else {
                Printed::Text(text)
            }
        }
        Loaded::Unscanned => Printed::Unscanned,
        Loaded::Missing
        | Loaded::Dir
        | Loaded::Null
        | Loaded::Fifo
        | Loaded::Hang
        | Loaded::Unknown => Printed::Unknown,
    }
}

fn loose_words(body: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = body.chars().collect();
    let mut words = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        while index < chars.len()
            && (chars[index].is_whitespace()
                || chars[index] == ';'
                || (chars[index] == '&' && chars.get(index + 1) == Some(&'&'))
                || (chars[index] == '|' && chars.get(index + 1) == Some(&'|')))
        {
            if chars[index] == '&' || chars[index] == '|' {
                index += 2;
            } else {
                index += 1;
            }
        }
        if index >= chars.len() || chars[index] == '#' {
            break;
        }
        let mut word = String::new();
        if chars[index] == '\'' || chars[index] == '"' {
            let quote = chars[index];
            index += 1;
            while index < chars.len() && chars[index] != quote {
                if quote == '"' && chars[index] == '\\' {
                    index += 1;
                    if index >= chars.len() {
                        return None;
                    }
                    let next = chars[index];
                    // Bash keeps the backslash unless the next character is
                    // `$`, `` ` ``, `"`, `\`, or a newline.
                    if !matches!(next, '$' | '`' | '"' | '\\' | '\n') {
                        word.push('\\');
                    }
                }
                word.push(chars[index]);
                index += 1;
            }
            if index >= chars.len() {
                return None;
            }
            index += 1;
        } else {
            while index < chars.len()
                && !chars[index].is_whitespace()
                && chars[index] != ';'
                && chars[index] != '\''
                && chars[index] != '"'
                && !(chars[index] == '&' && chars.get(index + 1) == Some(&'&'))
                && !(chars[index] == '|' && chars.get(index + 1) == Some(&'|'))
            {
                word.push(chars[index]);
                index += 1;
            }
        }
        if !word.is_empty() {
            words.push(word);
        }
    }
    Some(words)
}

fn no_output_tail(words: &[String]) -> bool {
    words
        .iter()
        .all(|word| matches!(word.as_str(), "true" | "false" | ":"))
}

enum PrintedSpan {
    First,
    All,
}

fn printed_span(token: &str, name: &str) -> Option<PrintedSpan> {
    if token == format!("${name}")
        || token == format!("${{{name}}}")
        || token == format!("${{{name}[0]}}")
    {
        Some(PrintedSpan::First)
    } else if token == format!("${{{name}[@]}}") {
        Some(PrintedSpan::All)
    } else {
        None
    }
}

fn peel_simple_prefix(words: &[String]) -> &[String] {
    let mut index = 0usize;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "builtin" || word == "command" || plain_assignment(word).is_some() {
            index += 1;
            continue;
        }
        break;
    }
    &words[index..]
}

fn shell_name(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(ch) if ch == '_' || ch.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn scalar_read_path(words: &[String]) -> Option<&str> {
    if words.first().map(String::as_str) != Some("read") {
        return None;
    }
    let mut index = 1usize;
    if words.get(index).map(String::as_str) == Some("-r") {
        index += 1;
    }
    let name = words.get(index)?;
    if !shell_name(name) {
        return None;
    }
    index += 1;
    let (path, next) = split_redirect(words, index)?;
    let end = match words.get(next).map(String::as_str) {
        Some("printf") => {
            if words.get(next + 1).map(String::as_str) != Some("%s\\n") {
                return None;
            }
            if words
                .get(next + 2)
                .is_none_or(|word| printed_span(word, name).is_none())
            {
                return None;
            }
            next + 3
        }
        Some("echo") => {
            if words
                .get(next + 1)
                .is_none_or(|word| printed_span(word, name).is_none())
            {
                return None;
            }
            next + 2
        }
        _ => return None,
    };
    if no_output_tail(&words[end..]) {
        Some(path)
    } else {
        None
    }
}

fn while_read_path(words: &[String]) -> Option<&str> {
    if words.first().map(String::as_str) != Some("while")
        || words.get(1).map(String::as_str) != Some("read")
    {
        return None;
    }
    let mut index = 2usize;
    if words.get(index).map(String::as_str) == Some("-r") {
        index += 1;
    }
    let name = words.get(index)?;
    if !shell_name(name) {
        return None;
    }
    index += 1;
    if words.get(index).map(String::as_str) != Some("do") {
        return None;
    }
    index += 1;
    match words.get(index).map(String::as_str) {
        Some("printf") => {
            index += 1;
            if words.get(index).map(String::as_str) != Some("%s\\n") {
                return None;
            }
            index += 1;
            if words
                .get(index)
                .is_none_or(|word| printed_span(word, name).is_none())
            {
                return None;
            }
            index += 1;
        }
        Some("echo") => {
            index += 1;
            if words
                .get(index)
                .is_none_or(|word| printed_span(word, name).is_none())
            {
                return None;
            }
            index += 1;
        }
        _ => return None,
    }
    redirect_path(words, index)
}

fn mapfile_path(words: &[String]) -> Option<(&str, bool)> {
    if words.first().map(String::as_str) != Some("mapfile")
        || words.get(1).map(String::as_str) != Some("-t")
    {
        return None;
    }
    let name = words.get(2)?;
    if !shell_name(name) {
        return None;
    }
    let (path, next) = split_redirect(words, 3)?;
    let (end, span) = match words.get(next).map(String::as_str) {
        Some("printf") => {
            if words.get(next + 1).map(String::as_str) != Some("%s\\n") {
                return None;
            }
            let word = words.get(next + 2)?;
            let span = printed_span(word, name)?;
            (next + 3, span)
        }
        Some("echo") => {
            let word = words.get(next + 1)?;
            let span = printed_span(word, name)?;
            (next + 2, span)
        }
        _ => return None,
    };
    if !no_output_tail(&words[end..]) {
        return None;
    }
    Some((path, matches!(span, PrintedSpan::First)))
}

fn redirect_path(words: &[String], index: usize) -> Option<&str> {
    if words.get(index).map(String::as_str) != Some("done") {
        return None;
    }
    let (path, next) = split_redirect(words, index + 1)?;
    if no_output_tail(&words[next..]) {
        return Some(path);
    }
    None
}

fn split_redirect(words: &[String], index: usize) -> Option<(&str, usize)> {
    let token = words.get(index)?;
    if token == "<" {
        let path = words.get(index + 1)?;
        if path.starts_with('<') || path.contains('$') {
            return None;
        }
        return Some((path.as_str(), index + 2));
    }
    let path = token.strip_prefix('<')?;
    if path.is_empty() || path.contains('$') {
        return None;
    }
    Some((path, index + 1))
}

enum Run {
    Produced { text: String, ok: bool },
    Unscanned,
    Unknown,
}

enum Join {
    Semi,
    And,
    Or,
}

struct Piece {
    command: String,
    next: Join,
}

fn printed_output(body: &str, depth: u32, substitutions: &mut Vec<String>) -> Printed {
    if let Some(found) = read_back_print(body, depth, substitutions) {
        return match found {
            Printed::Text(text) => Printed::Text(strip_trailing_newlines(text)),
            other => other,
        };
    }
    match fold_print(body, depth, substitutions) {
        Printed::Text(text) => Printed::Text(strip_trailing_newlines(text)),
        other => other,
    }
}

fn strip_trailing_newlines(mut text: String) -> String {
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

/// Printed bytes, including trailing newlines. Command substitution uses
/// [`printed_output`], which strips those newlines.
fn fold_print(body: &str, depth: u32, substitutions: &mut Vec<String>) -> Printed {
    if depth > FOLD_DEPTH {
        return Printed::Unknown;
    }
    if let Some(path) = command_subst_file(body) {
        return printed_file(&path);
    }
    let Some(pieces) = split_list(body) else {
        return Printed::Unknown;
    };
    let mut extra = Vec::new();
    let mut out = String::new();
    let mut any_known = false;
    // `None` is an unknown exit. `&&` and `||` then still keep a later
    // printer, because that side runs for one of the two exits.
    let mut exit_ok: Option<bool> = Some(true);
    let mut should_run = true;
    for piece in pieces {
        if should_run {
            match run_command(&piece.command, depth, substitutions) {
                Run::Produced { text, ok } => {
                    any_known = true;
                    out.push_str(&text);
                    exit_ok = Some(ok);
                }
                Run::Unscanned => return Printed::Unscanned,
                Run::Unknown => match read_back_print(&piece.command, depth, substitutions) {
                    Some(Printed::Text(text)) => {
                        any_known = true;
                        out.push_str(&text);
                        exit_ok = Some(true);
                    }
                    Some(Printed::Unscanned) => return Printed::Unscanned,
                    Some(Printed::Unknown) | None => {
                        extra.push(piece.command);
                        exit_ok = None;
                    }
                },
            }
        }
        should_run = match piece.next {
            Join::Semi => true,
            Join::And => exit_ok != Some(false),
            Join::Or => exit_ok != Some(true),
        };
    }
    if !any_known {
        return Printed::Unknown;
    }
    substitutions.extend(extra);
    Printed::Text(out)
}

/// `$(<path>)` reads that file. It is not a command named `<`.
fn command_subst_file(body: &str) -> Option<String> {
    let trimmed = body.trim_matches([' ', '\t', '\n', '\r']);
    let rest = trimmed.strip_prefix('<')?;
    let rest = rest.trim_start_matches([' ', '\t']);
    if rest.is_empty()
        || rest.chars().any(|ch| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    ';' | '&'
                        | '|'
                        | '<'
                        | '>'
                        | '('
                        | ')'
                        | '*'
                        | '?'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '~'
                        | '\''
                        | '"'
                        | '\\'
                        | '$'
                        | '`'
                        | '#'
                )
        })
    {
        return None;
    }
    Some(rest.to_string())
}

fn printed_file(path: &str) -> Printed {
    match load_script_file(path) {
        Some(ScriptFile::Text(mut text)) => {
            while text.ends_with('\n') {
                text.pop();
            }
            Printed::Text(text)
        }
        Some(ScriptFile::Unscanned) => Printed::Unscanned,
        None => Printed::Unknown,
    }
}

fn run_command(command: &str, depth: u32, substitutions: &mut Vec<String>) -> Run {
    let Some(stages) = split_pipeline(command) else {
        return Run::Unknown;
    };
    let mut stdout = String::new();
    let mut ok = true;
    for (index, stage) in stages.iter().enumerate() {
        let pipe = if index == 0 {
            None
        } else {
            Some(stdout.as_str())
        };
        match run_stage(stage, pipe, depth, substitutions) {
            Run::Produced { text, ok: stage_ok } => {
                stdout = text;
                ok = stage_ok;
            }
            other => return other,
        }
    }
    Run::Produced { text: stdout, ok }
}

/// Stages of one pipeline. `||` or a glob is not a stage list we can
/// print. A redirect stays in its stage.
fn split_pipeline(command: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = command.chars().collect();
    let mut stages = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut quote: Option<char> = None;
    let mut word_start = true;
    while index < chars.len() {
        let ch = chars[index];
        if let Some(open) = quote {
            if open == '"' && ch == '\\' {
                index += 1;
                if index < chars.len() {
                    index += 1;
                }
                continue;
            }
            if ch == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                word_start = false;
                index += 1;
            }
            '\\' => {
                word_start = false;
                index += 1;
                if index < chars.len() {
                    index += 1;
                }
            }
            ' ' | '\t' | '\n' | '\r' => {
                word_start = true;
                index += 1;
            }
            '#' if word_start => break,
            '|' => {
                if chars.get(index + 1) == Some(&'|') {
                    return None;
                }
                stages.push(chars[start..index].iter().collect());
                index += 1;
                start = index;
                word_start = true;
            }
            '$' if chars.get(index + 1) == Some(&'(') => {
                word_start = false;
                index = skip_command_subst(&chars, index)?;
            }
            '`' => {
                word_start = false;
                index = skip_backtick(&chars, index)?;
            }
            '&' if chars.get(index + 1) == Some(&'>') => {
                word_start = false;
                index += 1;
            }
            '<' | '>' if chars.get(index + 1) == Some(&'(') => {
                word_start = false;
                index = take_proc_subst(&chars, index)?.1;
            }
            '<' | '>' => {
                word_start = false;
                index += 1;
            }
            '&' | '(' | ')' | '*' | '?' | '[' | ']' | '{' | '}' | '~' => return None,
            _ => {
                word_start = false;
                index += 1;
            }
        }
    }
    stages.push(chars[start..].iter().collect());
    Some(stages)
}

fn run_stage(stage: &str, pipe: Option<&str>, depth: u32, substitutions: &mut Vec<String>) -> Run {
    let Some(peeled) = peel_redirects(stage) else {
        return Run::Unknown;
    };
    if peeled.blocked {
        return Run::Unknown;
    }
    if peeled.cancelled {
        return Run::Produced {
            text: String::new(),
            ok: false,
        };
    }
    let mut unscanned = false;
    let Some(mut words) = parse_fields(&peeled.command, depth, substitutions, &mut unscanned)
    else {
        if unscanned {
            return Run::Unscanned;
        }
        return Run::Unknown;
    };
    let mut saw_prefix = false;
    if let Some(first) = words.first() {
        let base = command_basename(first);
        if base == "command" || base == "builtin" {
            saw_prefix = true;
            if words.get(1).is_some_and(|word| word.starts_with('-')) {
                return Run::Unknown;
            }
            words.remove(0);
        }
    }
    if words.is_empty() {
        return if saw_prefix {
            Run::Unknown
        } else {
            Run::Produced {
                text: String::new(),
                ok: true,
            }
        };
    }
    peel_stage_wrappers(&mut words);
    if words.is_empty() {
        return Run::Unknown;
    }
    let ready = match stdin_for(&words, peeled.stdin, pipe) {
        StdinFor::Ready(text) => text,
        StdinFor::Failed => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        StdinFor::Unscanned => return Run::Unscanned,
        StdinFor::Unknown => return Run::Unknown,
    };
    mask_stdout(
        run_builtin(&words, ready.as_deref(), depth, substitutions),
        peeled.stdout_lost,
    )
}

fn mask_stdout(run: Run, stdout_lost: bool) -> Run {
    if !stdout_lost {
        return run;
    }
    match run {
        Run::Produced { ok, .. } => Run::Produced {
            text: String::new(),
            ok,
        },
        other => other,
    }
}

fn peel_stage_wrappers(words: &mut Vec<String>) {
    for _ in 0..8 {
        if words.is_empty() {
            return;
        }
        let name = stage_command_name(&words[0]);
        if !matches!(
            name.as_str(),
            "stdbuf" | "timeout" | "env" | "nice" | "nohup" | "time" | "exec" | "sudo"
        ) {
            if name != words[0] {
                words[0] = name;
            }
            return;
        }
        let Some((next, script)) = skip_wrapper_flags(&name, words, 1, &mut Vec::new()) else {
            return;
        };
        if script.is_some() || next >= words.len() {
            return;
        }
        words.drain(0..next);
    }
}

enum ScriptBytes {
    Bytes(Vec<u8>),
    Unscanned,
}

fn load_script_bytes(path: &str) -> Option<ScriptBytes> {
    let mut file = open_script_candidate(path)?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    if meta.len() > SCRIPT_FILE_CAP {
        return Some(ScriptBytes::Unscanned);
    }
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = std::io::Read::read(&mut file, &mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() as u64 > SCRIPT_FILE_CAP {
            return Some(ScriptBytes::Unscanned);
        }
    }
    Some(ScriptBytes::Bytes(buf))
}

struct GzipQuery {
    file: Option<String>,
}

struct LsbBits<'a> {
    rest: &'a [u8],
    acc: u32,
    acc_bits: usize,
}

impl<'a> LsbBits<'a> {
    fn code(&mut self, n_bits: usize) -> Option<usize> {
        while self.acc_bits < n_bits {
            let byte = *self.rest.first()?;
            self.rest = &self.rest[1..];
            self.acc |= u32::from(byte) << self.acc_bits;
            self.acc_bits += 8;
        }
        let code = (self.acc & ((1u32 << n_bits) - 1)) as usize;
        self.acc >>= n_bits;
        self.acc_bits -= n_bits;
        Some(code)
    }
}

fn unix_compress_decode(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 3 || bytes[0] != 0x1f || bytes[1] != 0x9d {
        return None;
    }
    let maxbits = (bytes[2] & 0x1f) as usize;
    if !(9..=16).contains(&maxbits) {
        return None;
    }
    let block_mode = bytes[2] & 0x80 != 0;
    let limit = 1usize << maxbits;
    let mut prefix = vec![0u16; limit];
    let mut suffix = vec![0u8; limit];
    for (index, slot) in suffix.iter_mut().enumerate().take(256) {
        *slot = index as u8;
    }
    let mut bits = LsbBits {
        rest: &bytes[3..],
        acc: 0,
        acc_bits: 0,
    };
    let mut n_bits = 9usize;
    let mut maxcode = (1usize << n_bits) - 1;
    let mut free_ent = if block_mode { 257 } else { 256 };
    let mut out = Vec::new();
    let mut stack = Vec::new();
    let Some(first) = bits.code(n_bits) else {
        return Some(out);
    };
    if first > 255 {
        return None;
    }
    out.push(first as u8);
    let mut finchar = first;
    let mut oldcode = first;
    while let Some(incode) = bits.code(n_bits) {
        if block_mode && incode == 256 {
            free_ent = 257;
            n_bits = 9;
            maxcode = (1usize << n_bits) - 1;
            bits.acc = 0;
            bits.acc_bits = 0;
            let Some(reset) = bits.code(n_bits) else {
                break;
            };
            if reset > 255 {
                return None;
            }
            out.push(reset as u8);
            finchar = reset;
            oldcode = reset;
            continue;
        }
        let mut code = incode;
        if code == free_ent {
            stack.push(finchar as u8);
            code = oldcode;
        } else if code > free_ent {
            return None;
        }
        while code >= 256 {
            stack.push(suffix[code]);
            code = prefix[code] as usize;
        }
        finchar = code;
        stack.push(finchar as u8);
        while let Some(byte) = stack.pop() {
            out.push(byte);
        }
        if free_ent < limit {
            prefix[free_ent] = oldcode as u16;
            suffix[free_ent] = finchar as u8;
            free_ent += 1;
            if free_ent > maxcode && n_bits < maxbits {
                n_bits += 1;
                maxcode = (1usize << n_bits) - 1;
            }
        }
        oldcode = incode;
    }
    Some(out)
}

fn gzip_flate_decode(bytes: &[u8]) -> Option<String> {
    let mut decoder = flate2::read::GzDecoder::new(bytes);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn gzip_decode(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0x1f, 0x9d]) {
        let raw = unix_compress_decode(bytes)?;
        String::from_utf8(raw).ok()
    } else {
        gzip_flate_decode(bytes)
    }
}

fn deflate_decode(bytes: &[u8]) -> Option<String> {
    let mut decoder = flate2::read::DeflateDecoder::new(bytes);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn zlib_decode(bytes: &[u8]) -> Option<String> {
    let mut decoder = flate2::read::ZlibDecoder::new(bytes);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn gzip_or_zlib_decode(bytes: &[u8]) -> Option<String> {
    gzip_flate_decode(bytes).or_else(|| zlib_decode(bytes))
}

fn any_inflate_decode(bytes: &[u8]) -> Option<String> {
    gzip_flate_decode(bytes)
        .or_else(|| zlib_decode(bytes))
        .or_else(|| deflate_decode(bytes))
}

fn bzip2_decode(bytes: &[u8]) -> Option<String> {
    let mut decoder = bzip2::read::BzDecoder::new(bytes);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn xz_decode(bytes: &[u8]) -> Option<String> {
    let mut input = std::io::Cursor::new(bytes);
    let mut out = Vec::new();
    lzma_rs::xz_decompress(&mut input, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn lzma_decode(bytes: &[u8]) -> Option<String> {
    let mut input = std::io::Cursor::new(bytes);
    let mut out = Vec::new();
    lzma_rs::lzma_decompress(&mut input, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn xz_or_lzma_decode(bytes: &[u8]) -> Option<String> {
    xz_decode(bytes).or_else(|| lzma_decode(bytes))
}

fn zstd_frame(bytes: &[u8]) -> &[u8] {
    let mut rest = bytes;
    while rest.len() >= 8 {
        let magic = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        if !(0x184D_2A50..=0x184D_2A5F).contains(&magic) {
            break;
        }
        let size = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        let next = 8 + size;
        if rest.len() < next {
            break;
        }
        rest = &rest[next..];
    }
    rest
}

fn zstd_decode(bytes: &[u8]) -> Option<String> {
    let bytes = zstd_frame(bytes);
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(bytes).ok()?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn lz4_decode(bytes: &[u8]) -> Option<String> {
    let mut decoder = lz4_flex::frame::FrameDecoder::new(bytes);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn brotli_decode(bytes: &[u8]) -> Option<String> {
    let mut input = std::io::Cursor::new(bytes);
    let mut out = Vec::new();
    brotli_decompressor::BrotliDecompress(&mut input, &mut out).ok()?;
    String::from_utf8(out).ok()
}

fn gzip_query(name: &str, args: &[String]) -> Option<GzipQuery> {
    decompress_stdout_query(
        args,
        matches!(name, "gunzip" | "gzcat" | "zcat"),
        matches!(name, "gzcat" | "zcat"),
    )
}

fn bzip2_query(name: &str, args: &[String]) -> Option<GzipQuery> {
    decompress_stdout_query(args, matches!(name, "bunzip2" | "bzcat"), name == "bzcat")
}

struct XzQuery {
    file: Option<String>,
    lzma: bool,
}

fn xz_query(name: &str, args: &[String]) -> Option<XzQuery> {
    let mut lzma = matches!(name, "lzma" | "unlzma" | "lzcat");
    let mut kept = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--format=lzma" || arg == "-Flzma" {
            lzma = true;
            index += 1;
            continue;
        }
        if arg == "--format=xz" || arg == "-Fxz" {
            lzma = false;
            index += 1;
            continue;
        }
        if arg == "--format" || arg == "-F" {
            match args.get(index + 1).map(String::as_str) {
                Some("lzma" | "alone") => lzma = true,
                Some("xz" | "auto") => lzma = false,
                _ => return None,
            }
            index += 2;
            continue;
        }
        kept.push(arg.clone());
        index += 1;
    }
    let inner = decompress_stdout_query(
        &kept,
        matches!(name, "unxz" | "xzcat" | "unlzma" | "lzcat"),
        matches!(name, "xzcat" | "lzcat"),
    )?;
    Some(XzQuery {
        file: inner.file,
        lzma,
    })
}

fn zstd_query(name: &str, args: &[String]) -> Option<GzipQuery> {
    decompress_stdout_query(
        args,
        matches!(name, "unzstd" | "zstdcat"),
        name == "zstdcat",
    )
}

fn lz4_query(name: &str, args: &[String]) -> Option<GzipQuery> {
    if let Some(query) =
        decompress_stdout_query(args, matches!(name, "unlz4" | "lz4cat"), name == "lz4cat")
    {
        return Some(query);
    }
    if name != "lz4" {
        return None;
    }
    let query = decompress_stdout_query(args, true, false)?;
    let path = query.file.as_deref()?;
    let ext = path.rsplit_once('.').map(|(_, ext)| ext)?;
    if ext.eq_ignore_ascii_case("lz4") {
        Some(query)
    } else {
        None
    }
}

fn uncompress_query(args: &[String]) -> Option<GzipQuery> {
    decompress_stdout_query(args, true, false)
}

fn brotli_query(name: &str, args: &[String]) -> Option<GzipQuery> {
    if name != "brotli" {
        return None;
    }
    decompress_stdout_query(args, false, false)
}

fn decompress_stdout_query(
    args: &[String],
    mut decode: bool,
    mut stdout: bool,
) -> Option<GzipQuery> {
    let mut index = 0usize;
    let mut file = None;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if matches!(arg.as_str(), "-d" | "--decompress" | "--uncompress") {
            decode = true;
            index += 1;
            continue;
        }
        if matches!(arg.as_str(), "-c" | "--stdout" | "--to-stdout") {
            stdout = true;
            index += 1;
            continue;
        }
        if matches!(
            arg.as_str(),
            "-f" | "--force"
                | "-k"
                | "--keep"
                | "-q"
                | "--quiet"
                | "-n"
                | "--no-name"
                | "-N"
                | "--name"
        ) {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-')
            && !rest.is_empty()
            && !rest.starts_with('-')
        {
            for byte in rest.bytes() {
                match byte {
                    b'd' => decode = true,
                    b'c' => stdout = true,
                    b'f' | b'k' | b'q' | b'n' | b'N' => {}
                    _ => return None,
                }
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') || file.is_some() {
            return None;
        }
        file = Some(arg.clone());
        index += 1;
    }
    if index < args.len() {
        if file.is_some() || args.len() - index != 1 {
            return None;
        }
        file = Some(args[index].clone());
    }
    if !decode || !stdout {
        return None;
    }
    Some(GzipQuery { file })
}

fn gzip_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    decoded_bytes_run(
        gzip_query(name, args),
        input,
        depth,
        substitutions,
        gzip_decode,
    )
}

fn bzip2_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    decoded_bytes_run(
        bzip2_query(name, args),
        input,
        depth,
        substitutions,
        bzip2_decode,
    )
}

fn xz_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = xz_query(name, args) else {
        return Run::Unknown;
    };
    let decode = if query.lzma { lzma_decode } else { xz_decode };
    decoded_bytes_run(
        Some(GzipQuery { file: query.file }),
        input,
        depth,
        substitutions,
        decode,
    )
}

fn zstd_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    decoded_bytes_run(
        zstd_query(name, args),
        input,
        depth,
        substitutions,
        zstd_decode,
    )
}

fn lz4_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    decoded_bytes_run(
        lz4_query(name, args),
        input,
        depth,
        substitutions,
        lz4_decode,
    )
}

fn brotli_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    decoded_bytes_run(
        brotli_query(name, args),
        input,
        depth,
        substitutions,
        brotli_decode,
    )
}

fn decoded_bytes_run(
    query: Option<GzipQuery>,
    input: Option<&str>,
    _depth: u32,
    _substitutions: &mut Vec<String>,
    decode: fn(&[u8]) -> Option<String>,
) -> Run {
    let Some(query) = query else {
        return Run::Unknown;
    };
    let bytes = if let Some(path) = query.file.as_deref() {
        match load_script_bytes(path) {
            Some(ScriptBytes::Bytes(bytes)) => bytes,
            Some(ScriptBytes::Unscanned) => return Run::Unscanned,
            None => return Run::Unknown,
        }
    } else if let Some(text) = input {
        text.as_bytes().to_vec()
    } else {
        return Run::Unknown;
    };
    let Some(text) = decode(&bytes) else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TarCompress {
    Auto,
    Lz4,
    Brotli,
}

fn tar_program(program: &str) -> TarCompress {
    let token = program.split_whitespace().next().unwrap_or(program);
    let base = command_basename(token).to_ascii_lowercase();
    match base.as_str() {
        "lz4" | "lz4cat" | "unlz4" => TarCompress::Lz4,
        "brotli" => TarCompress::Brotli,
        _ => TarCompress::Auto,
    }
}

fn set_tar_program(current: &mut TarCompress, program: &str) -> bool {
    let next = tar_program(program);
    if next == TarCompress::Auto {
        return true;
    }
    if *current != TarCompress::Auto && *current != next {
        return false;
    }
    *current = next;
    true
}

struct ArchiveQuery {
    archive: String,
    member: Option<String>,
    compress: TarCompress,
}

fn tar_query(args: &[String]) -> Option<ArchiveQuery> {
    let mut extract = false;
    let mut stdout = false;
    let mut archive = None;
    let mut index = 0usize;
    let mut members = Vec::new();
    let mut compress = TarCompress::Auto;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-I" || arg == "--use-compress-program" {
            let program = args.get(index + 1)?;
            if program.is_empty() || !set_tar_program(&mut compress, program) {
                return None;
            }
            index += 2;
            continue;
        }
        if let Some(program) = arg.strip_prefix("--use-compress-program=") {
            if program.is_empty() || !set_tar_program(&mut compress, program) {
                return None;
            }
            index += 1;
            continue;
        }
        if arg == "-f" || arg == "--file" {
            archive = Some(args.get(index + 1)?.clone());
            index += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--file=") {
            if rest.is_empty() {
                return None;
            }
            archive = Some(rest.to_string());
            index += 1;
            continue;
        }
        if matches!(arg.as_str(), "-x" | "--extract" | "--get") {
            extract = true;
            index += 1;
            continue;
        }
        if matches!(arg.as_str(), "-O" | "--to-stdout") {
            stdout = true;
            index += 1;
            continue;
        }
        if matches!(arg.as_str(), "-v" | "--verbose") {
            index += 1;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--gzip" | "--gunzip" | "--bzip2" | "--xz" | "--zstd" | "--compress"
        ) {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-')
            && !rest.is_empty()
            && !rest.starts_with('-')
        {
            let bytes = rest.as_bytes();
            let mut cursor = 0usize;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'x' => extract = true,
                    b'O' => stdout = true,
                    b'v' | b'z' | b'j' | b'J' | b'Z' => {}
                    b'f' => {
                        let glued = &rest[cursor + 1..];
                        if glued.is_empty() {
                            archive = Some(args.get(index + 1)?.clone());
                            index += 1;
                        } else {
                            archive = Some(glued.to_string());
                        }
                        break;
                    }
                    _ => return None,
                }
                cursor += 1;
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        members.push(arg.clone());
        index += 1;
    }
    members.extend(args[index..].iter().cloned());
    if !extract || !stdout || members.len() > 1 {
        return None;
    }
    let archive = archive?;
    Some(ArchiveQuery {
        archive,
        member: members.into_iter().next(),
        compress,
    })
}

fn member_matches(path: &str, member: &str) -> bool {
    path == member || path.rsplit('/').next() == Some(member)
}

fn inflate_all(mut decoder: impl std::io::Read) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    Some(out)
}

fn lz4_frame_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = lz4_flex::frame::FrameDecoder::new(bytes);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
    Some(out)
}

fn brotli_raw_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut input = std::io::Cursor::new(bytes);
    let mut out = Vec::new();
    brotli_decompressor::BrotliDecompress(&mut input, &mut out).ok()?;
    Some(out)
}

fn tar_payload(bytes: &[u8], compress: TarCompress) -> Option<Vec<u8>> {
    match compress {
        TarCompress::Lz4 => tar_magic(&lz4_frame_bytes(bytes)?),
        TarCompress::Brotli => tar_magic(&brotli_raw_bytes(bytes)?),
        TarCompress::Auto => tar_magic(bytes),
    }
}

fn tar_magic(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(&[0x1f, 0x9d]) {
        return unix_compress_decode(bytes);
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return inflate_all(flate2::read::GzDecoder::new(bytes));
    }
    if bytes.starts_with(b"BZh") {
        return inflate_all(bzip2::read::BzDecoder::new(bytes));
    }
    if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        let mut input = std::io::Cursor::new(bytes);
        let mut out = Vec::new();
        lzma_rs::xz_decompress(&mut input, &mut out).ok()?;
        return Some(out);
    }
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) || bytes.starts_with(&[0x50, 0x2a, 0x4d, 0x18])
    {
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(zstd_frame(bytes)).ok()?;
        return inflate_all(&mut decoder);
    }
    if let Some(out) = lzma_alone_tar(bytes) {
        return Some(out);
    }
    Some(bytes.to_vec())
}

fn lzma_alone_tar(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 13 || bytes[0] > 224 {
        return None;
    }
    let dict = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    if !(4096..=1 << 27).contains(&dict) {
        return None;
    }
    let mut input = std::io::Cursor::new(bytes);
    let mut out = Vec::new();
    lzma_rs::lzma_decompress(&mut input, &mut out).ok()?;
    if out.len() > 262 && out[257..262] == *b"ustar" {
        Some(out)
    } else {
        None
    }
}

fn tar_text(bytes: &[u8], member: Option<&str>, compress: TarCompress) -> Option<String> {
    let bytes = tar_payload(bytes, compress)?;
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut out = Vec::new();
    let mut found = false;
    for entry in archive.entries().ok()? {
        let Ok(mut entry) = entry else {
            break;
        };
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().ok()?;
        let path = path.to_string_lossy();
        if path.rsplit('/').next().unwrap_or(&path).starts_with("._") {
            continue;
        }
        if let Some(member) = member
            && !member_matches(&path, member)
        {
            continue;
        }
        std::io::Read::read_to_end(&mut entry, &mut out).ok()?;
        found = true;
        if member.is_some() {
            break;
        }
    }
    if !found {
        return None;
    }
    String::from_utf8(out).ok()
}

fn unzip_query(args: &[String]) -> Option<ArchiveQuery> {
    let mut stdout = false;
    let mut index = 0usize;
    let mut positionals = Vec::new();
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-p" {
            stdout = true;
            index += 1;
            continue;
        }
        if matches!(arg.as_str(), "-q" | "-qq" | "-o") {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-')
            && !rest.is_empty()
            && !rest.starts_with('-')
            && rest.bytes().all(|byte| matches!(byte, b'p' | b'q' | b'o'))
            && rest.as_bytes().contains(&b'p')
        {
            stdout = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        positionals.push(arg.clone());
        index += 1;
    }
    positionals.extend(args[index..].iter().cloned());
    if !stdout || positionals.is_empty() || positionals.len() > 2 {
        return None;
    }
    Some(ArchiveQuery {
        archive: positionals.remove(0),
        member: positionals.into_iter().next(),
        compress: TarCompress::Auto,
    })
}

fn unzip_first(bytes: &[u8]) -> Option<String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).ok()?;
        if file.is_dir() {
            continue;
        }
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut out).ok()?;
        return String::from_utf8(out).ok();
    }
    None
}

fn unzip_text(bytes: &[u8], member: Option<&str>) -> Option<String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut out = Vec::new();
    if let Some(member) = member {
        let mut file = archive.by_name(member).ok()?;
        std::io::Read::read_to_end(&mut file, &mut out).ok()?;
    } else {
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).ok()?;
            if file.is_dir() {
                continue;
            }
            std::io::Read::read_to_end(&mut file, &mut out).ok()?;
        }
    }
    if out.is_empty() {
        return None;
    }
    String::from_utf8(out).ok()
}

fn produced_archive(path: &str, text: impl FnOnce(&[u8]) -> Option<String>) -> Run {
    let bytes = match load_script_bytes(path) {
        Some(ScriptBytes::Bytes(bytes)) => bytes,
        Some(ScriptBytes::Unscanned) => return Run::Unscanned,
        None => return Run::Unknown,
    };
    let Some(text) = text(&bytes) else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

fn sevenz_query(args: &[String]) -> Option<ArchiveQuery> {
    let mut index = 0usize;
    let mut command = None;
    let mut stdout = false;
    let mut positionals = Vec::new();
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if command.is_none() && !arg.starts_with('-') {
            if arg == "e" || arg == "x" {
                command = Some(());
                index += 1;
                continue;
            }
            return None;
        }
        if arg == "-so" {
            stdout = true;
            index += 1;
            continue;
        }
        if matches!(arg.as_str(), "-bd" | "-y" | "-bso0" | "-bse0" | "-bsp0") {
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        positionals.push(arg.clone());
        index += 1;
    }
    positionals.extend(args[index..].iter().cloned());
    if command.is_none() || !stdout || positionals.is_empty() || positionals.len() > 2 {
        return None;
    }
    Some(ArchiveQuery {
        archive: positionals.remove(0),
        member: positionals.into_iter().next(),
        compress: TarCompress::Auto,
    })
}

fn sevenz_text(bytes: &[u8], member: Option<&str>) -> Option<String> {
    let mut out = Vec::new();
    let mut found = false;
    sevenz_rust2::decompress_with_extract_fn(
        std::io::Cursor::new(bytes),
        std::env::temp_dir(),
        |entry, reader, _dest| {
            if entry.is_directory() {
                return Ok(true);
            }
            let name = entry.name();
            if let Some(member) = member
                && !member_matches(name, member)
            {
                std::io::copy(reader, &mut std::io::sink()).map_err(sevenz_rust2::Error::from)?;
                return Ok(true);
            }
            std::io::Read::read_to_end(reader, &mut out).map_err(sevenz_rust2::Error::from)?;
            found = true;
            Ok(true)
        },
    )
    .ok()?;
    if !found {
        return None;
    }
    String::from_utf8(out).ok()
}

fn sevenz_run(
    args: &[String],
    _input: Option<&str>,
    _depth: u32,
    _substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = sevenz_query(args) else {
        return Run::Unknown;
    };
    produced_archive(&query.archive, |bytes| {
        sevenz_text(bytes, query.member.as_deref())
    })
}

fn tar_run(
    args: &[String],
    _input: Option<&str>,
    _depth: u32,
    _substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = tar_query(args) else {
        return Run::Unknown;
    };
    produced_archive(&query.archive, |bytes| {
        tar_text(bytes, query.member.as_deref(), query.compress)
    })
}

fn unzip_run(
    args: &[String],
    _input: Option<&str>,
    _depth: u32,
    _substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = unzip_query(args) else {
        return Run::Unknown;
    };
    produced_archive(&query.archive, |bytes| {
        unzip_text(bytes, query.member.as_deref())
    })
}

fn run_builtin(
    words: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let name = stage_command_name(&words[0]);
    let args = &words[1..];
    match name.as_str() {
        "echo" => match echo_output(args) {
            Some(text) => Run::Produced { text, ok: true },
            None => Run::Unknown,
        },
        "printf" => match printf_stdout(args) {
            Some(text) => Run::Produced { text, ok: true },
            None => Run::Unknown,
        },
        "true" | ":" => Run::Produced {
            text: String::new(),
            ok: true,
        },
        "false" => Run::Produced {
            text: String::new(),
            ok: false,
        },
        "cat" => cat_run(args, input, depth, substitutions),
        "split" => split_run(args, input, depth, substitutions),
        "stdbuf" => stdbuf_run(args, input, depth, substitutions),
        "xargs" => xargs_run(words, input, depth, substitutions),
        "tee" => tee_run(args, input),
        "tr" => tr_run(args, input),
        "iconv" => iconv_run(args, input, depth, substitutions),
        "join" => join_run(args, input, depth, substitutions),
        "comm" => comm_run(args, input, depth, substitutions),
        "grep" | "egrep" | "fgrep" | "ggrep" => grep_run(&name, args, input, depth, substitutions),
        "head" => head_run(args, input, depth, substitutions),
        "tail" => tail_run(args, input, depth, substitutions),
        "sed" | "gsed" => sed_run(args, input, depth, substitutions),
        "awk" | "gawk" => awk_run(args, input, depth, substitutions),
        "jq" => jq_run(args, input, depth, substitutions),
        "cut" => cut_run(args, input, depth, substitutions),
        "sort" => sort_run(args, input, depth, substitutions),
        "uniq" => uniq_run(args, input, depth, substitutions),
        "tac" => tac_run(args, input, depth, substitutions),
        "base64" => base64_run(args, input, depth, substitutions),
        "basenc" => basenc_run(args, input, depth, substitutions),
        "uudecode" => uudecode_run(args, input, depth, substitutions),
        "xxd" => xxd_run(args, input, depth, substitutions),
        "openssl" => openssl_b64_run(args, input, depth, substitutions),
        "base32" => base32_run(args, input, depth, substitutions),
        "gzip" | "gunzip" | "gzcat" | "zcat" => gzip_run(&name, args, input, depth, substitutions),
        "bzip2" | "bunzip2" | "bzcat" => bzip2_run(&name, args, input, depth, substitutions),
        "xz" | "unxz" | "xzcat" | "lzma" | "unlzma" | "lzcat" => {
            xz_run(&name, args, input, depth, substitutions)
        }
        "zstd" | "unzstd" | "zstdcat" | "pzstd" => {
            zstd_run(&name, args, input, depth, substitutions)
        }
        "tar" => tar_run(args, input, depth, substitutions),
        "unzip" => unzip_run(args, input, depth, substitutions),
        "7z" | "7za" | "7zz" => sevenz_run(args, input, depth, substitutions),
        "lz4" | "unlz4" | "lz4cat" => lz4_run(&name, args, input, depth, substitutions),
        "brotli" => brotli_run(&name, args, input, depth, substitutions),
        "uncompress" => decoded_bytes_run(
            uncompress_query(args),
            input,
            depth,
            substitutions,
            gzip_decode,
        ),
        "dd" => dd_run(args, input, depth, substitutions),
        "perl" | "ruby" => line_print_run(&words[0], args, input, depth, substitutions),
        "python" | "python3" => python_cat_run(args, input, depth, substitutions),
        "node" => node_cat_run(args, input, depth, substitutions),
        "php" => php_cat_run(args, input, depth, substitutions),
        "paste" | "expand" | "unexpand" | "column" | "fold" | "fmt" | "pr" | "strings" => {
            identity_run(&words[0], args, input, depth, substitutions)
        }
        _ => Run::Unknown,
    }
}

fn printf_stdout(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    if let Some(arg) = args.first() {
        if arg == "--" {
            index = 1;
        } else if arg == "-v" {
            // `-v` assigns in the subshell and prints nothing.
            return Some(String::new());
        } else if arg.starts_with('-') {
            return None;
        }
    }
    printf_literal(&args[index..])
}

fn xargs_run(
    words: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some((next, script)) = skip_wrapper_flags("xargs", words, 1, &mut Vec::new()) else {
        return Run::Unknown;
    };
    if script.is_some() || next >= words.len() {
        return Run::Unknown;
    }
    let mut command = words[next..].to_vec();
    if stage_command_name(&command[0]) == "xargs" {
        return Run::Unknown;
    }
    if let Some(text) = input {
        for arg in text.split_whitespace() {
            command.push(arg.to_string());
        }
    }
    run_builtin(&command, None, depth, substitutions)
}

fn cat_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(operands) = cat_operands(args) else {
        return Run::Unknown;
    };
    if operands.is_empty() {
        return match input {
            Some(text) => Run::Produced {
                text: text.to_string(),
                ok: true,
            },
            None => Run::Unknown,
        };
    }
    let mut out = String::new();
    let mut failed = false;
    for operand in operands {
        if operand == "-" {
            let Some(text) = input else {
                return Run::Unknown;
            };
            out.push_str(text);
            continue;
        }
        match load_text_operand(&operand, depth, substitutions) {
            Loaded::Text(text) => out.push_str(&text),
            Loaded::Unscanned => return Run::Unscanned,
            Loaded::Missing | Loaded::Dir => failed = true,
            Loaded::Fifo | Loaded::Hang | Loaded::Unknown => return Run::Unknown,
            Loaded::Null => {}
        }
    }
    Run::Produced {
        text: out,
        ok: !failed,
    }
}

enum SplitNumber {
    Files,
    Bytes(usize, usize),
    Lines(usize, usize),
    Robin(usize, usize),
}

struct SplitPlan {
    filter: Option<String>,
    number: Option<SplitNumber>,
    file: Option<String>,
}

fn split_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(plan) = split_plan(args) else {
        return Run::Unknown;
    };
    if let Some(number) = &plan.number
        && split_number_is_stdout(number)
    {
        // GNU split refuses --filter when -n already writes the chunk to stdout.
        if plan.filter.is_some() {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        return match load_split_file(&plan.file, input, depth, substitutions) {
            SplitText::Text(text) => Run::Produced {
                text: split_chunk(&text, number),
                ok: true,
            },
            SplitText::Empty => Run::Produced {
                text: String::new(),
                ok: false,
            },
            SplitText::Unscanned => Run::Unscanned,
            SplitText::Unknown => Run::Unknown,
        };
    }
    if plan.filter.as_deref().is_some_and(filter_is_cat) {
        return match load_split_file(&plan.file, input, depth, substitutions) {
            SplitText::Text(text) => Run::Produced { text, ok: true },
            SplitText::Empty => Run::Produced {
                text: String::new(),
                ok: false,
            },
            SplitText::Unscanned => Run::Unscanned,
            SplitText::Unknown => Run::Unknown,
        };
    }
    Run::Unknown
}

enum SplitText {
    Text(String),
    Empty,
    Unscanned,
    Unknown,
}

fn load_split_file(
    file: &Option<String>,
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> SplitText {
    let Some(path) = file else {
        return SplitText::Unknown;
    };
    if path == "-" {
        return match input {
            Some(text) => SplitText::Text(text.to_string()),
            None => SplitText::Unknown,
        };
    }
    match load_text_operand(path, depth, substitutions) {
        Loaded::Text(text) => SplitText::Text(text),
        Loaded::Missing | Loaded::Dir | Loaded::Null => SplitText::Empty,
        Loaded::Unscanned => SplitText::Unscanned,
        Loaded::Fifo | Loaded::Hang | Loaded::Unknown => SplitText::Unknown,
    }
}

fn filter_is_cat(command: &str) -> bool {
    let mut words = command.split_whitespace();
    words.next() == Some("cat") && words.next().is_none()
}

fn split_number_is_stdout(number: &SplitNumber) -> bool {
    !matches!(number, SplitNumber::Files)
}

fn split_chunk(text: &str, number: &SplitNumber) -> String {
    match *number {
        SplitNumber::Files => String::new(),
        SplitNumber::Bytes(k, n) => byte_chunk(text, k, n),
        SplitNumber::Lines(k, n) => line_chunk(text, k, n),
        SplitNumber::Robin(k, n) => robin_chunk(text, k, n),
    }
}

fn byte_range(len: usize, k: usize, n: usize) -> Option<(usize, usize)> {
    if n == 0 || k == 0 || k > n {
        return None;
    }
    let base = len / n;
    let rem = len % n;
    let mut start = 0usize;
    for i in 1..k {
        start += base + usize::from(i <= rem);
    }
    let size = base + usize::from(k <= rem);
    Some((start.min(len), start.saturating_add(size).min(len)))
}

fn byte_chunk(text: &str, k: usize, n: usize) -> String {
    let Some((start, end)) = byte_range(text.len(), k, n) else {
        return String::new();
    };
    let bytes = text.as_bytes();
    String::from_utf8_lossy(&bytes[start..end]).into_owned()
}

fn kept_lines(text: &str) -> Vec<(usize, &str)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            lines.push((start, &text[start..=index]));
            start = index + 1;
        }
    }
    if start < bytes.len() {
        lines.push((start, &text[start..]));
    }
    lines
}

fn line_chunk(text: &str, k: usize, n: usize) -> String {
    let Some((start, end)) = byte_range(text.len(), k, n) else {
        return String::new();
    };
    let mut out = String::new();
    for (line_start, line) in kept_lines(text) {
        if line_start >= start && line_start < end {
            out.push_str(line);
        }
    }
    out
}

fn robin_chunk(text: &str, k: usize, n: usize) -> String {
    if n == 0 || k == 0 || k > n {
        return String::new();
    }
    let mut out = String::new();
    for (index, (_, line)) in kept_lines(text).into_iter().enumerate() {
        if index % n == k - 1 {
            out.push_str(line);
        }
    }
    out
}

fn parse_split_number(spec: &str) -> Option<SplitNumber> {
    let positive = |token: &str| {
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        token.parse::<usize>().ok().filter(|value| *value >= 1)
    };
    let parts: Vec<&str> = spec.split('/').collect();
    match parts.as_slice() {
        [n] if positive(n).is_some() => Some(SplitNumber::Files),
        ["l", n] if positive(n).is_some() => Some(SplitNumber::Files),
        ["r", n] if positive(n).is_some() => Some(SplitNumber::Files),
        ["l", k, n] => Some(SplitNumber::Lines(positive(k)?, positive(n)?)),
        ["r", k, n] => Some(SplitNumber::Robin(positive(k)?, positive(n)?)),
        [k, n] => Some(SplitNumber::Bytes(positive(k)?, positive(n)?)),
        _ => None,
    }
}

fn split_plan(args: &[String]) -> Option<SplitPlan> {
    let mut index = 0usize;
    let mut filter = None;
    let mut number = None;
    let mut operands = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            operands.extend(args[index + 1..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            operands.push(arg.to_string());
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (rest, None),
            };
            match name {
                "verbose" | "elide-empty-files" | "unbuffered" => {
                    if inline.is_some() {
                        return None;
                    }
                    index += 1;
                }
                "numeric-suffixes" | "hex-suffixes" => {
                    index += 1;
                }
                "filter" => {
                    let command = match inline {
                        Some(value) if !value.is_empty() => value.to_string(),
                        Some(_) => return None,
                        None => {
                            index += 1;
                            let command = args.get(index)?.clone();
                            if command.is_empty() {
                                return None;
                            }
                            command
                        }
                    };
                    if filter.is_some() {
                        return None;
                    }
                    filter = Some(command);
                    index += 1;
                }
                "lines" | "bytes" | "line-bytes" | "suffix-length" | "additional-suffix"
                | "separator" => {
                    if inline.is_none() {
                        index += 1;
                        if args.get(index)?.is_empty() {
                            return None;
                        }
                    } else if inline.is_some_and(str::is_empty) {
                        return None;
                    }
                    index += 1;
                }
                "number" => {
                    let spec = match inline {
                        Some(value) if !value.is_empty() => value.to_string(),
                        Some(_) => return None,
                        None => {
                            index += 1;
                            args.get(index)?.clone()
                        }
                    };
                    if number.is_some() {
                        return None;
                    }
                    number = Some(parse_split_number(&spec)?);
                    index += 1;
                }
                _ => return None,
            }
            continue;
        }
        let mut chars = arg[1..].chars();
        while let Some(ch) = chars.next() {
            match ch {
                'd' | 'e' | 'u' | 'x' => {}
                'a' | 'b' | 'C' | 'l' | 't' => {
                    let rest: String = chars.collect();
                    if rest.is_empty() {
                        index += 1;
                        if args.get(index)?.is_empty() {
                            return None;
                        }
                    }
                    break;
                }
                'n' => {
                    let rest: String = chars.collect();
                    let spec = if rest.is_empty() {
                        index += 1;
                        args.get(index)?.clone()
                    } else {
                        rest
                    };
                    if number.is_some() {
                        return None;
                    }
                    number = Some(parse_split_number(&spec)?);
                    break;
                }
                _ => return None,
            }
        }
        index += 1;
    }
    if operands.len() > 2 {
        return None;
    }
    Some(SplitPlan {
        filter,
        number,
        file: operands.first().cloned(),
    })
}

fn split_filter_commands(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    let base = command_basename(first);
    if !base.eq_ignore_ascii_case("split") && !base.eq_ignore_ascii_case("gsplit") {
        return Vec::new();
    }
    let Some(plan) = split_plan(&argv[1..]) else {
        return Vec::new();
    };
    if plan.number.as_ref().is_some_and(split_number_is_stdout) {
        return Vec::new();
    }
    plan.filter.into_iter().collect()
}

fn stdbuf_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(command) = stdbuf_command(args) else {
        return Run::Unknown;
    };
    run_builtin(command, input, depth, substitutions)
}

fn stdbuf_command(args: &[String]) -> Option<&[String]> {
    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            index += 1;
            break;
        }
        if matches!(arg, "-i" | "-o" | "-e" | "--input" | "--output" | "--error") {
            index += 2;
            if index > args.len() {
                return None;
            }
            continue;
        }
        if let Some((name, _)) = arg.split_once('=')
            && matches!(name, "--input" | "--output" | "--error")
        {
            index += 1;
            continue;
        }
        if let Some(rest) = ["-i", "-o", "-e"]
            .iter()
            .find_map(|prefix| arg.strip_prefix(prefix))
            && !rest.is_empty()
            && rest.chars().all(|ch| ch.is_ascii_digit() || ch == 'L')
        {
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        break;
    }
    if index >= args.len() {
        return None;
    }
    Some(&args[index..])
}

fn cat_operands(args: &[String]) -> Option<Vec<String>> {
    let mut operands = Vec::new();
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options && arg.starts_with("--") {
            match arg.as_str() {
                "--show-nonprinting" | "--show-ends" | "--show-tabs" | "--squeeze-blank"
                | "--show-all" => continue,
                _ => return None,
            }
        }
        if options && arg.starts_with('-') && arg != "-" {
            // `-n` and `-b` number the line, so it is no longer the command.
            // `-v`, `-e`, `-E`, `-t`, `-T`, `-s`, and `-A` still print it.
            for ch in arg.chars().skip(1) {
                if !matches!(ch, 'u' | 'v' | 'e' | 'E' | 't' | 'T' | 's' | 'A') {
                    return None;
                }
            }
            continue;
        }
        operands.push(arg.clone());
    }
    Some(operands)
}

fn tee_run(args: &[String], input: Option<&str>) -> Run {
    if !tee_query(args) {
        return Run::Unknown;
    }
    match input {
        Some(text) => Run::Produced {
            text: text.to_string(),
            ok: true,
        },
        None => Run::Unknown,
    }
}

/// `tee` copies stdin to stdout. Later operands are output files.
fn tee_query(args: &[String]) -> bool {
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options
            && (arg == "-a" || arg == "--append" || arg == "-i" || arg == "--ignore-interrupts")
        {
            continue;
        }
        if options && arg.starts_with('-') && arg != "-" {
            return false;
        }
    }
    true
}

fn tr_run(args: &[String], input: Option<&str>) -> Run {
    if args.is_empty() {
        return Run::Unknown;
    }
    match input {
        Some(text) => Run::Produced {
            text: text.to_string(),
            ok: true,
        },
        None => Run::Unknown,
    }
}

fn iconv_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(file) = iconv_query(args) else {
        return Run::Unknown;
    };
    source_text(file.as_deref(), input, depth, substitutions)
}

fn iconv_query(args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut from = None;
    let mut to = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "--" {
            index += 1;
            files.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "-f" || arg == "--from-code" {
            index += 1;
            from = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--from-code=") {
            if value.is_empty() {
                return None;
            }
            from = Some(value.to_string());
            index += 1;
            continue;
        }
        if arg == "-t" || arg == "--to-code" {
            index += 1;
            to = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--to-code=") {
            if value.is_empty() {
                return None;
            }
            to = Some(value.to_string());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(arg);
        index += 1;
    }
    if files.len() > 1 {
        return None;
    }
    if let (Some(from), Some(to)) = (from.as_deref(), to.as_deref()) {
        if !same_charset(from, to) {
            return None;
        }
    } else if from.is_some() || to.is_some() {
        return None;
    }
    Some(files.first().cloned())
}

fn same_charset(left: &str, right: &str) -> bool {
    charset_key(left) == charset_key(right)
}

fn charset_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn comm_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some((hide1, hide2, hide3, left, right)) = comm_query(args) else {
        return Run::Unknown;
    };
    let left_text = match operand_text(&left, input, depth, substitutions) {
        Run::Produced { text, ok: true } => text,
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        other => return other,
    };
    let right_text = match operand_text(&right, input, depth, substitutions) {
        Run::Produced { text, ok: true } => text,
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        other => return other,
    };
    Run::Produced {
        text: comm_text(&left_text, &right_text, hide1, hide2, hide3),
        ok: true,
    }
}

fn operand_text(
    operand: &str,
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    if operand == "-" {
        source_text(None, input, depth, substitutions)
    } else {
        source_text(Some(operand), None, depth, substitutions)
    }
}

fn comm_query(args: &[String]) -> Option<(bool, bool, bool, String, String)> {
    let mut hide = [false; 3];
    let mut files = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            files.extend(args[index + 1..].iter().cloned());
            break;
        }
        if arg == "--nocheck-order" || arg == "--check-order" {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-') {
            if rest.is_empty() {
                files.push(arg.clone());
                index += 1;
                continue;
            }
            if rest.starts_with('-') || !rest.chars().all(|ch| matches!(ch, '1' | '2' | '3')) {
                return None;
            }
            for ch in rest.chars() {
                hide[usize::from(ch as u8 - b'1')] = true;
            }
            index += 1;
            continue;
        }
        files.push(arg.clone());
        index += 1;
    }
    if files.len() != 2 {
        return None;
    }
    Some((
        hide[0],
        hide[1],
        hide[2],
        files[0].clone(),
        files[1].clone(),
    ))
}

fn comm_text(left: &str, right: &str, hide1: bool, hide2: bool, hide3: bool) -> String {
    let left_lines = comm_lines(left);
    let right_lines = comm_lines(right);
    let mut used = vec![false; right_lines.len()];
    let mut out = String::new();
    for line in &left_lines {
        if let Some(pos) = right_lines
            .iter()
            .enumerate()
            .find(|(index, other)| !used[*index] && *other == line)
            .map(|(index, _)| index)
        {
            used[pos] = true;
            if !hide3 {
                let tabs = usize::from(!hide1) + usize::from(!hide2);
                push_comm_line(&mut out, line, tabs);
            }
        } else if !hide1 {
            push_comm_line(&mut out, line, 0);
        }
    }
    if !hide2 {
        let tabs = usize::from(!hide1);
        for (index, line) in right_lines.iter().enumerate() {
            if !used[index] {
                push_comm_line(&mut out, line, tabs);
            }
        }
    }
    out
}

fn comm_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines.into_iter().map(str::to_string).collect()
}

fn push_comm_line(out: &mut String, line: &str, tabs: usize) {
    for _ in 0..tabs {
        out.push('\t');
    }
    out.push_str(line);
    out.push('\n');
}

fn join_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(files) = join_query(args) else {
        return Run::Unknown;
    };
    load_pieces(&files, input, depth, substitutions, true).map_or_else(
        |run| run,
        |pieces| Run::Produced {
            text: pieces.text,
            ok: pieces.ok,
        },
    )
}

/// Two files, no flags. `join` of a file with itself still prints the line.
fn join_query(args: &[String]) -> Option<Vec<String>> {
    if args.iter().any(|arg| arg.starts_with('-') && arg != "-") {
        return None;
    }
    if args.len() != 2 {
        return None;
    }
    Some(args.to_vec())
}

fn grep_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(mut query) = grep_query(args) else {
        return Run::Unknown;
    };
    match command_basename(name) {
        "egrep" => query.extended = true,
        "fgrep" => query.fixed = true,
        _ => {}
    }
    let text = match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        Run::Produced { text, ok: true } => text,
        other => return other,
    };
    let Some(matched) = grep_any(
        &text,
        &query.patterns,
        query.fixed,
        query.ignore_case,
        query.word,
        query.only,
        query.extended,
    ) else {
        return Run::Unknown;
    };
    let ok = !matched.is_empty();
    Run::Produced { text: matched, ok }
}

struct GrepQuery {
    patterns: Vec<String>,
    fixed: bool,
    ignore_case: bool,
    word: bool,
    only: bool,
    extended: bool,
    file: Option<String>,
}

fn grep_query(args: &[String]) -> Option<GrepQuery> {
    let mut index = 0usize;
    let mut fixed = false;
    let mut ignore_case = false;
    let mut word = false;
    let mut only = false;
    let mut extended = false;
    let mut patterns = Vec::new();
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-F" || arg == "--fixed-strings" {
            fixed = true;
            index += 1;
            continue;
        }
        if arg == "-h" || arg == "--no-filename" {
            index += 1;
            continue;
        }
        if arg == "-i" || arg == "--ignore-case" {
            ignore_case = true;
            index += 1;
            continue;
        }
        if arg == "-w" || arg == "--word-regexp" {
            word = true;
            index += 1;
            continue;
        }
        if arg == "-o" || arg == "--only-matching" {
            only = true;
            index += 1;
            continue;
        }
        if arg == "-E" || arg == "--extended-regexp" {
            extended = true;
            index += 1;
            continue;
        }
        if arg == "-G" || arg == "--basic-regexp" {
            index += 1;
            continue;
        }
        if arg == "-P" || arg == "--perl-regexp" {
            index += 1;
            continue;
        }
        if arg == "-a" || arg == "--text" || arg == "-I" {
            index += 1;
            continue;
        }
        if arg == "-e" {
            patterns.push(args.get(index + 1)?.clone());
            index += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-')
            && !arg.starts_with("--")
            && !rest.is_empty()
            && rest
                .chars()
                .all(|ch| matches!(ch, 'E' | 'F' | 'a' | 'I' | 'h' | 'i' | 'w' | 'o'))
        {
            if rest.contains('E') {
                extended = true;
            }
            if rest.contains('F') {
                fixed = true;
            }
            if rest.contains('i') {
                ignore_case = true;
            }
            if rest.contains('w') {
                word = true;
            }
            if rest.contains('o') {
                only = true;
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        break;
    }
    if patterns.is_empty() {
        patterns.push(args.get(index)?.clone());
        index += 1;
    }
    if args.len() - index > 1 {
        return None;
    }
    Some(GrepQuery {
        patterns,
        fixed,
        ignore_case,
        word,
        only,
        extended,
        file: args.get(index).cloned(),
    })
}

fn grep_any(
    text: &str,
    patterns: &[String],
    fixed: bool,
    ignore_case: bool,
    word: bool,
    only: bool,
    extended: bool,
) -> Option<String> {
    if patterns.is_empty() {
        return None;
    }
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        if line.is_empty() {
            continue;
        }
        for pattern in patterns {
            let matched = grep_lines(line, pattern, fixed, ignore_case, word, only, extended)?;
            out.push_str(&matched);
        }
    }
    Some(out)
}

fn grep_lines(
    text: &str,
    pattern: &str,
    fixed: bool,
    ignore_case: bool,
    word: bool,
    only: bool,
    extended: bool,
) -> Option<String> {
    let dot = !fixed && pattern == ".";
    let star = !fixed && pattern == ".*";
    let plus = extended && !fixed && plus_prefix(pattern).is_some();
    let literal = fixed || literal_pattern(pattern);
    if only && dot {
        return None;
    }
    if word && !literal {
        return None;
    }
    if only && plus {
        return None;
    }
    if !dot && !star && !literal && !plus {
        return None;
    }
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        if star || (plus && plus_matches(body, pattern)) {
            out.push_str(body);
            out.push('\n');
            continue;
        }
        if dot {
            if !body.is_empty() {
                out.push_str(body);
                out.push('\n');
            }
            continue;
        }
        let Some(ranges) = literal_hits(body, pattern, ignore_case, word) else {
            continue;
        };
        if only {
            for range in ranges {
                out.push_str(&body[range]);
                out.push('\n');
            }
        } else {
            out.push_str(body);
            out.push('\n');
        }
    }
    Some(out)
}

fn plus_prefix(pattern: &str) -> Option<&str> {
    let prefix = pattern.strip_suffix('+')?;
    if prefix.is_empty() || !literal_pattern(prefix) {
        return None;
    }
    Some(prefix)
}

fn plus_matches(line: &str, pattern: &str) -> bool {
    let Some(prefix) = plus_prefix(pattern) else {
        return false;
    };
    let Some(last) = prefix.chars().next_back() else {
        return false;
    };
    let head = &prefix[..prefix.len() - last.len_utf8()];
    let mut rest = line;
    loop {
        let Some(rel) = rest.find(head) else {
            return false;
        };
        let mut tail = &rest[rel + head.len()..];
        let mut saw = false;
        while tail.starts_with(last) {
            saw = true;
            tail = &tail[last.len_utf8()..];
        }
        if saw {
            return true;
        }
        let Some(next) = rest[rel..].chars().next() else {
            return false;
        };
        rest = &rest[rel + next.len_utf8()..];
    }
}

fn literal_hits(
    line: &str,
    pattern: &str,
    ignore_case: bool,
    word: bool,
) -> Option<Vec<std::ops::Range<usize>>> {
    if pattern.is_empty() {
        return None;
    }
    let hay = if ignore_case {
        line.to_ascii_lowercase()
    } else {
        line.to_string()
    };
    let needle = if ignore_case {
        pattern.to_ascii_lowercase()
    } else {
        pattern.to_string()
    };
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < hay.len() {
        let Some(rel) = hay[start..].find(&needle) else {
            break;
        };
        let at = start + rel;
        let end = at + needle.len();
        let before = at == 0 || !is_grep_word(hay.as_bytes()[at - 1]);
        let after = end >= hay.len() || !is_grep_word(hay.as_bytes()[end]);
        if !word || (before && after) {
            ranges.push(at..end);
            start = end.max(at + 1);
        } else {
            start = at + 1;
        }
    }
    if ranges.is_empty() {
        None
    } else {
        Some(ranges)
    }
}

fn is_grep_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn literal_pattern(pattern: &str) -> bool {
    pattern.chars().all(|ch| {
        !matches!(
            ch,
            '.' | '^' | '$' | '*' | '+' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '\\'
        )
    })
}

fn head_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = head_query(args) else {
        return Run::Unknown;
    };
    match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { text, ok } => Run::Produced {
            text: match query.bytes {
                Some(count) => first_bytes(&text, count),
                None => first_lines(&text, query.lines),
            },
            ok,
        },
        other => other,
    }
}

struct HeadQuery {
    lines: usize,
    bytes: Option<usize>,
    file: Option<String>,
}

fn head_query(args: &[String]) -> Option<HeadQuery> {
    let mut index = 0usize;
    let mut lines = None;
    let mut bytes = None;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-n" {
            lines = Some(line_count(args.get(index + 1)?)?);
            index += 2;
            continue;
        }
        if arg == "-c" || arg == "--bytes" {
            bytes = Some(line_count(args.get(index + 1)?)?);
            index += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--bytes=")
            && !rest.is_empty()
        {
            bytes = Some(line_count(rest)?);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-c")
            && !rest.is_empty()
            && !rest.starts_with('-')
        {
            bytes = Some(line_count(rest)?);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-n")
            && !rest.is_empty()
        {
            lines = Some(line_count(rest)?);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-') {
            if !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()) {
                lines = Some(line_count(rest)?);
                index += 1;
                continue;
            }
            if !rest.is_empty() {
                return None;
            }
        }
        break;
    }
    if args.len() - index > 1 {
        return None;
    }
    if lines.is_some() && bytes.is_some() {
        return None;
    }
    Some(HeadQuery {
        lines: lines.unwrap_or(10),
        bytes,
        file: args.get(index).cloned(),
    })
}

fn first_bytes(text: &str, count: usize) -> String {
    if count == 0 {
        return String::new();
    }
    if count >= text.len() {
        return text.to_string();
    }
    let mut end = count;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn line_count(text: &str) -> Option<usize> {
    if text.is_empty() || !text.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn first_lines(text: &str, count: usize) -> String {
    if count == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index >= count {
            break;
        }
        out.push_str(line);
    }
    out
}

enum Loaded {
    Text(String),
    Unscanned,
    Missing,
    Dir,
    Fifo,
    Null,
    Hang,
    Unknown,
}

/// `<(body)` is the printed text of `body`. `>(body)` is not a file.
/// Anything else is a path.
fn load_text_operand(operand: &str, depth: u32, substitutions: &mut Vec<String>) -> Loaded {
    if let Some((body, input)) = proc_word(operand) {
        if !input {
            return Loaded::Unknown;
        }
        return match fold_print(body, depth + 1, substitutions) {
            Printed::Text(text) => Loaded::Text(text),
            Printed::Unscanned => Loaded::Unscanned,
            Printed::Unknown => Loaded::Unknown,
        };
    }
    match path_body(operand) {
        PathBody::Text(text) => Loaded::Text(text),
        PathBody::Unscanned => Loaded::Unscanned,
        PathBody::Missing => Loaded::Missing,
        PathBody::Dir => Loaded::Dir,
        PathBody::Fifo => Loaded::Fifo,
        PathBody::Null => Loaded::Null,
        PathBody::Hang => Loaded::Hang,
    }
}

fn proc_word(operand: &str) -> Option<(&str, bool)> {
    let (input, rest) = if let Some(rest) = operand.strip_prefix("<(") {
        (true, rest)
    } else if let Some(rest) = operand.strip_prefix(">(") {
        (false, rest)
    } else {
        return None;
    };
    let body = rest.strip_suffix(')')?;
    Some((body, input))
}

fn source_text(
    file: Option<&str>,
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    // `-` is stdin, the same way `cat -` is. Opening a file named `-`
    // is not what head or tail runs.
    let loaded = if let Some(path) = file.filter(|path| *path != "-") {
        load_text_operand(path, depth, substitutions)
    } else if let Some(text) = input {
        Loaded::Text(text.to_string())
    } else {
        return Run::Unknown;
    };
    match loaded {
        Loaded::Text(text) => Run::Produced { text, ok: true },
        Loaded::Unscanned => Run::Unscanned,
        Loaded::Missing | Loaded::Dir => Run::Produced {
            text: String::new(),
            ok: false,
        },
        Loaded::Null => Run::Produced {
            text: String::new(),
            ok: true,
        },
        Loaded::Fifo | Loaded::Hang | Loaded::Unknown => Run::Unknown,
    }
}

enum TailMode {
    Last(usize),
    From(usize),
    BytesLast(usize),
    BytesFrom(usize),
    /// `tail +N file` prints the whole file. GNU and BSD disagree on the
    /// start line, and both still emit every later line.
    Whole,
}

struct TailQuery {
    mode: TailMode,
    file: Option<String>,
}

fn tail_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = tail_query(args) else {
        return Run::Unknown;
    };
    match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { text, ok } => Run::Produced {
            text: tail_lines(&text, &query.mode),
            ok,
        },
        other => other,
    }
}

fn tail_query(args: &[String]) -> Option<TailQuery> {
    let mut index = 0usize;
    let mut mode = None;
    let mut legacy = false;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-n" {
            mode = Some(tail_count(args.get(index + 1)?)?);
            index += 2;
            continue;
        }
        if arg == "-c" {
            mode = Some(tail_bytes(args.get(index + 1)?)?);
            index += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-c")
            && !rest.is_empty()
        {
            mode = Some(tail_bytes(rest)?);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-n")
            && !rest.is_empty()
        {
            mode = Some(tail_count(rest)?);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix('-') {
            if !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()) {
                mode = Some(TailMode::Last(line_count(rest)?));
                index += 1;
                continue;
            }
            if !rest.is_empty() {
                return None;
            }
        }
        if let Some(rest) = arg.strip_prefix('+')
            && !rest.is_empty()
            && rest.chars().all(|ch| ch.is_ascii_digit())
        {
            legacy = true;
            mode = Some(TailMode::Whole);
            index += 1;
            continue;
        }
        break;
    }
    if args.len() - index > 1 {
        return None;
    }
    let file = args.get(index).cloned();
    if legacy && file.is_none() {
        return None;
    }
    Some(TailQuery {
        mode: mode.unwrap_or(TailMode::Last(10)),
        file,
    })
}

fn tail_bytes(text: &str) -> Option<TailMode> {
    if let Some(rest) = text.strip_prefix('+') {
        let count = line_count(rest)?;
        let start = if count == 0 { 1 } else { count };
        return Some(TailMode::BytesFrom(start));
    }
    if let Some(rest) = text.strip_prefix('-') {
        return Some(TailMode::BytesLast(line_count(rest)?));
    }
    Some(TailMode::BytesLast(line_count(text)?))
}

fn last_bytes(text: &str, count: usize) -> String {
    if count == 0 || text.is_empty() {
        return String::new();
    }
    if count >= text.len() {
        return text.to_string();
    }
    let mut index = text.len() - count;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    text[index..].to_string()
}

fn from_byte(text: &str, start: usize) -> String {
    if start <= 1 {
        return text.to_string();
    }
    let mut index = start - 1;
    if index >= text.len() {
        return String::new();
    }
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    text[index..].to_string()
}

fn tail_count(text: &str) -> Option<TailMode> {
    if let Some(rest) = text.strip_prefix('+') {
        let count = line_count(rest)?;
        let from = if count == 0 { 1 } else { count };
        return Some(TailMode::From(from));
    }
    if let Some(rest) = text.strip_prefix('-') {
        return Some(TailMode::Last(line_count(rest)?));
    }
    Some(TailMode::Last(line_count(text)?))
}

fn tail_lines(text: &str, mode: &TailMode) -> String {
    match mode {
        TailMode::Whole => text.to_string(),
        TailMode::Last(count) => last_lines(text, *count),
        TailMode::From(line) => from_line(text, *line),
        TailMode::BytesLast(count) => last_bytes(text, *count),
        TailMode::BytesFrom(start) => from_byte(text, *start),
    }
}

fn last_lines(text: &str, count: usize) -> String {
    if count == 0 || text.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].concat()
}

fn from_line(text: &str, line: usize) -> String {
    if line <= 1 {
        return text.to_string();
    }
    let skip = line - 1;
    text.split_inclusive('\n').skip(skip).collect()
}

#[derive(Clone, Copy)]
enum SedEnd {
    Line(usize),
    Last,
}

enum SedPrint {
    All,
    Empty,
    Double,
    First,
    FirstPlus,
    /// Quiet `Np`, `N,Mp`, or `$p`. Not quiet still prints every line.
    Range {
        start: SedEnd,
        end: SedEnd,
    },
    /// Several quiet print commands. A selected line is kept once.
    Many(Vec<(SedEnd, SedEnd)>),
    /// Quiet `/text/p`. A line is kept when it contains `text`.
    Contains(String),
    /// `s/from/to/`. Quiet `p` prints a line that changed. Not quiet prints every line.
    Subst {
        pattern: String,
        replacement: String,
        global: bool,
        print_all: bool,
    },
}

struct SedQuery {
    kind: SedPrint,
    file: Option<String>,
}

fn sed_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = sed_query(args) else {
        return Run::Unknown;
    };
    match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { text, ok } => Run::Produced {
            text: sed_apply(&text, query.kind),
            ok,
        },
        other => other,
    }
}

fn sed_query(args: &[String]) -> Option<SedQuery> {
    let mut index = 0usize;
    let mut quiet = false;
    let mut script = None;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-n" {
            quiet = true;
            index += 1;
            continue;
        }
        if arg == "-e" {
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index + 1)?.clone());
            index += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-e")
            && !rest.is_empty()
        {
            if script.is_some() {
                return None;
            }
            script = Some(rest.to_string());
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        break;
    }
    if script.is_none() {
        script = Some(args.get(index)?.clone());
        index += 1;
    }
    if args.len() - index > 1 {
        return None;
    }
    Some(SedQuery {
        kind: sed_program(script.as_deref()?, quiet)?,
        file: args.get(index).cloned(),
    })
}

fn sed_program(script: &str, quiet: bool) -> Option<SedPrint> {
    match (script, quiet) {
        ("", true) => Some(SedPrint::Empty),
        ("", false) => Some(SedPrint::All),
        ("p", true) => Some(SedPrint::All),
        ("p", false) => Some(SedPrint::Double),
        ("1p" | "1,1p", true) => Some(SedPrint::First),
        ("1p" | "1,1p", false) => Some(SedPrint::FirstPlus),
        _ => sed_script(script, quiet),
    }
}

fn sed_script(script: &str, quiet: bool) -> Option<SedPrint> {
    let pieces: Vec<&str> = script
        .split(';')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .collect();
    if pieces.is_empty() {
        return None;
    }
    if let [piece] = pieces.as_slice()
        && let Some(kind) = sed_subst(piece, quiet)
    {
        return Some(kind);
    }
    if let [piece] = pieces.as_slice()
        && let Some(pattern) = sed_slash(piece)
    {
        return Some(if quiet {
            SedPrint::Contains(pattern.to_string())
        } else {
            SedPrint::All
        });
    }
    let mut spans = Vec::new();
    for piece in pieces {
        if piece == "p" || piece == "l" {
            // `l` prints the line and a `$`. Eval expands that `$` away.
            return Some(if quiet || piece == "l" {
                SedPrint::All
            } else {
                SedPrint::Double
            });
        }
        let (start, end) = sed_address(piece)?;
        spans.push((start, end));
    }
    if !quiet {
        // Default sed still prints every line.
        return Some(SedPrint::All);
    }
    if let [(start, end)] = spans.as_slice() {
        return Some(SedPrint::Range {
            start: *start,
            end: *end,
        });
    }
    Some(SedPrint::Many(spans))
}

fn sed_slash(piece: &str) -> Option<&str> {
    let body = piece.strip_suffix('p').unwrap_or(piece);
    if body.is_empty() {
        return None;
    }
    let inner = body.strip_prefix('/')?.strip_suffix('/')?;
    if inner.is_empty() || inner.contains(['/', '\\', '[', '*', '.']) {
        return None;
    }
    Some(inner)
}

fn sed_address(script: &str) -> Option<(SedEnd, SedEnd)> {
    // `l` prints the line and a `$`. Eval expands that `$` away.
    let body = script
        .strip_suffix('p')
        .or_else(|| script.strip_suffix('l'))?;
    if body.is_empty() {
        return None;
    }
    if let Some((left, right)) = body.split_once(',') {
        return Some((sed_end(left)?, sed_end(right)?));
    }
    let end = sed_end(body)?;
    Some((end, end))
}

fn sed_end(text: &str) -> Option<SedEnd> {
    if text == "$" {
        return Some(SedEnd::Last);
    }
    let line = line_count(text)?;
    if line == 0 {
        return None;
    }
    Some(SedEnd::Line(line))
}

fn sed_apply(text: &str, kind: SedPrint) -> String {
    match kind {
        SedPrint::Empty => String::new(),
        SedPrint::All => text.to_string(),
        SedPrint::First => first_lines(text, 1),
        SedPrint::FirstPlus => {
            let mut out = first_lines(text, 1);
            out.push_str(text);
            out
        }
        SedPrint::Double => {
            let mut out = String::new();
            for line in text.split_inclusive('\n') {
                if line.is_empty() {
                    continue;
                }
                let body = line.trim_end_matches('\n');
                out.push_str(body);
                out.push('\n');
                out.push_str(body);
                out.push('\n');
            }
            out
        }
        SedPrint::Range { start, end } => line_span(text, start, end),
        SedPrint::Many(spans) => union_lines(text, &spans),
        SedPrint::Contains(pattern) => {
            let mut out = String::new();
            for line in text.split_inclusive('\n') {
                let body = line.trim_end_matches('\n');
                if body.contains(pattern.as_str()) {
                    out.push_str(body);
                    out.push('\n');
                }
            }
            out
        }
        SedPrint::Subst {
            pattern,
            replacement,
            global,
            print_all,
        } => sed_subst_apply(text, &pattern, &replacement, global, print_all),
    }
}

fn sed_subst(piece: &str, quiet: bool) -> Option<SedPrint> {
    let rest = piece.strip_prefix("s/")?;
    let (pattern, rest) = rest.split_once('/')?;
    let (replacement, flags) = rest.split_once('/')?;
    if pattern.is_empty() || pattern.contains(['\\', '&', '*', '.', '[']) {
        return None;
    }
    if replacement.contains(['\\', '&', '/']) {
        return None;
    }
    let mut print = false;
    let mut global = false;
    for ch in flags.chars() {
        match ch {
            'p' => print = true,
            'g' => global = true,
            _ => return None,
        }
    }
    if quiet && !print {
        return Some(SedPrint::Empty);
    }
    Some(SedPrint::Subst {
        pattern: pattern.to_string(),
        replacement: replacement.to_string(),
        global,
        print_all: !quiet,
    })
}

fn sed_subst_apply(
    text: &str,
    pattern: &str,
    replacement: &str,
    global: bool,
    print_all: bool,
) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        if line.is_empty() {
            continue;
        }
        let body = line.trim_end_matches('\n');
        let (next, hit) = sed_replace_line(body, pattern, replacement, global);
        if print_all || hit {
            out.push_str(&next);
            out.push('\n');
        }
    }
    out
}

fn sed_replace_line(line: &str, pattern: &str, replacement: &str, global: bool) -> (String, bool) {
    let Some(pos) = line.find(pattern) else {
        return (line.to_string(), false);
    };
    if global {
        return (line.replace(pattern, replacement), true);
    }
    let mut out = String::new();
    out.push_str(&line[..pos]);
    out.push_str(replacement);
    out.push_str(&line[pos + pattern.len()..]);
    (out, true)
}

fn union_lines(text: &str, spans: &[(SedEnd, SedEnd)]) -> String {
    let lines: Vec<&str> = text
        .split_inclusive('\n')
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let last = lines.len() - 1;
    let mut picked = vec![false; lines.len()];
    for (start, end) in spans {
        let start = sed_index(*start, last);
        let end = sed_index(*end, last);
        if start > end || start >= lines.len() {
            continue;
        }
        let stop = end.min(last);
        for slot in picked.iter_mut().take(stop + 1).skip(start) {
            *slot = true;
        }
    }
    let mut out = String::new();
    for (slot, line) in lines.iter().enumerate() {
        if picked[slot] {
            out.push_str(line);
        }
    }
    out
}

fn sed_index(end: SedEnd, last: usize) -> usize {
    match end {
        SedEnd::Line(line) => line.saturating_sub(1),
        SedEnd::Last => last,
    }
}

fn line_span(text: &str, start: SedEnd, end: SedEnd) -> String {
    union_lines(text, &[(start, end)])
}

enum AwkSel {
    All,
    Range {
        start: usize,
        end: usize,
    },
    Many(Vec<(usize, usize)>),
    /// Rules that print some lines. A line is kept when any rule prints it.
    Script(Vec<AwkAtom>),
    /// `{printf "%s", $0}` joins line bodies with no newline between them.
    Concat,
    /// `END {print}`. The last line is kept.
    Last,
}

enum AwkAtom {
    /// `{print}` or an unmodeled pattern. Every line is kept.
    All,
    Range {
        start: usize,
        end: usize,
    },
    Match(AwkPat),
}

enum AwkPat {
    /// `index($0,"text")`. The text is literal.
    Literal(String),
    /// `/text/` or `$0 ~ /text/`. `.`, `*`, and `+` are the modeled regex.
    Regex(String),
}

struct AwkQuery {
    sel: AwkSel,
    file: Option<String>,
}

fn awk_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = awk_query(args) else {
        return Run::Unknown;
    };
    match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { text, ok } => Run::Produced {
            text: match query.sel {
                AwkSel::All => text,
                AwkSel::Range { start, end } => {
                    line_span(text.as_str(), SedEnd::Line(start), SedEnd::Line(end))
                }
                AwkSel::Many(spans) => {
                    let spans: Vec<(SedEnd, SedEnd)> = spans
                        .iter()
                        .map(|(start, end)| (SedEnd::Line(*start), SedEnd::Line(*end)))
                        .collect();
                    union_lines(text.as_str(), &spans)
                }
                AwkSel::Script(atoms) => awk_script(text.as_str(), &atoms),
                AwkSel::Concat => awk_concat(text.as_str()),
                AwkSel::Last => last_line(text.as_str()),
            },
            ok,
        },
        other => other,
    }
}

fn jq_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(file) = jq_query(args) else {
        return Run::Unknown;
    };
    source_text(file.as_deref(), input, depth, substitutions)
}

fn jq_query(args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut raw_in = false;
    let mut raw_out = false;
    let mut program = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-R" || arg == "--raw-input" {
            raw_in = true;
            index += 1;
            continue;
        }
        if arg == "-r" || arg == "--raw-output" {
            raw_out = true;
            index += 1;
            continue;
        }
        if arg == "-Rr" || arg == "-rR" {
            raw_in = true;
            raw_out = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        if program.is_none() {
            program = Some(args[index].clone());
        } else {
            files.push(args[index].clone());
        }
        index += 1;
    }
    if !raw_in || !raw_out || program.as_deref() != Some(".") || files.len() != 1 {
        return None;
    }
    Some(files.first().cloned())
}

fn awk_query(args: &[String]) -> Option<AwkQuery> {
    let mut index = 0usize;
    if args.first().is_some_and(|arg| arg == "--") {
        index = 1;
    }
    if matches!(args.get(index).map(String::as_str), Some("-e" | "--source")) {
        index += 1;
    }
    let program = args.get(index)?;
    let sel = awk_program(program)?;
    index += 1;
    if index < args.len() && args[index..].iter().any(|arg| arg.starts_with('-')) {
        return None;
    }
    if args.len() - index > 1 {
        return None;
    }
    Some(AwkQuery {
        sel,
        file: args.get(index).cloned(),
    })
}

fn awk_program(program: &str) -> Option<AwkSel> {
    let compact = awk_compact(program);
    if matches!(
        compact.as_str(),
        "{print}" | "1" | "{print$0}" | "{print$0;}"
    ) {
        return Some(AwkSel::All);
    }
    if let Some(sel) = awk_printf(&compact) {
        return Some(sel);
    }
    if matches!(
        compact.as_str(),
        "END{print}" | "END{print;}" | "END{print$0}" | "END{print$0;}"
    ) {
        return Some(AwkSel::Last);
    }
    let pieces = split_statements(&compact);
    if pieces.is_empty() {
        return None;
    }
    if let Some(sel) = awk_script_sel(&pieces) {
        return Some(sel);
    }
    let mut spans = Vec::new();
    for piece in pieces {
        match awk_one(piece)? {
            AwkSel::All => return Some(AwkSel::All),
            AwkSel::Range { start, end } => spans.push((start, end)),
            AwkSel::Many(_) | AwkSel::Script(_) | AwkSel::Concat | AwkSel::Last => return None,
        }
    }
    if let [(start, end)] = spans.as_slice() {
        return Some(AwkSel::Range {
            start: *start,
            end: *end,
        });
    }
    Some(AwkSel::Many(spans))
}

/// Drop spaces that awk ignores. Spaces inside `/.../` and quotes stay,
/// because `/push origin/` is not `/pushorigin/`.
fn awk_compact(program: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut slash = false;
    for ch in program.chars() {
        if let Some(mark) = quote {
            out.push(ch);
            if ch == mark {
                quote = None;
            }
            continue;
        }
        if slash {
            out.push(ch);
            if ch == '/' {
                slash = false;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            out.push(ch);
            continue;
        }
        if ch == '/' {
            slash = true;
            out.push(ch);
            continue;
        }
        if ch.is_whitespace() {
            continue;
        }
        out.push(ch);
    }
    out
}

fn awk_printf(compact: &str) -> Option<AwkSel> {
    match compact {
        "{printf\"%s\\n\",$0}" | "{printf\"%s\\n\",$0;}" => Some(AwkSel::All),
        "{printf\"%s\",$0}" | "{printf\"%s\",$0;}" | "{printf$0}" | "{printf$0;}" => {
            Some(AwkSel::Concat)
        }
        _ => None,
    }
}

fn last_line(text: &str) -> String {
    let mut chosen = "";
    let mut saw = false;
    for line in text.split_inclusive('\n') {
        if line.is_empty() {
            continue;
        }
        chosen = line;
        saw = true;
    }
    if !saw {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(chosen);
    if !chosen.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn awk_concat(text: &str) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        if line.is_empty() {
            continue;
        }
        out.push_str(line.trim_end_matches('\n'));
    }
    out
}

fn awk_script_sel(pieces: &[&str]) -> Option<AwkSel> {
    let mut atoms = Vec::new();
    let mut patterned = false;
    for piece in pieces {
        let atom = awk_atom(piece)?;
        if !matches!(atom, AwkAtom::Range { .. }) {
            patterned = true;
        }
        atoms.push(atom);
    }
    if !patterned {
        return None;
    }
    Some(AwkSel::Script(atoms))
}

fn awk_atom(piece: &str) -> Option<AwkAtom> {
    if matches!(piece, "{print}" | "1" | "{print$0}" | "{print$0;}") {
        return Some(AwkAtom::All);
    }
    let body = awk_drop_print(piece);
    if let Some(atom) = awk_slash_atom(body) {
        return Some(atom);
    }
    if let Some(pat) = awk_gsub_pat(body) {
        return Some(AwkAtom::Match(pat));
    }
    if let Some(pat) = awk_index_pat(body) {
        return Some(AwkAtom::Match(pat));
    }
    match awk_one(piece)? {
        AwkSel::All => Some(AwkAtom::All),
        AwkSel::Range { start, end } => Some(AwkAtom::Range { start, end }),
        AwkSel::Many(_) | AwkSel::Script(_) | AwkSel::Concat | AwkSel::Last => None,
    }
}

fn awk_slash_atom(body: &str) -> Option<AwkAtom> {
    let inner = if let Some(rest) = body.strip_prefix("$0~/") {
        rest.strip_suffix('/')?
    } else {
        let rest = body.strip_prefix('/')?;
        rest.strip_suffix('/')?
    };
    if parse_awk_regex(inner).is_none() {
        return Some(AwkAtom::All);
    }
    Some(AwkAtom::Match(AwkPat::Regex(inner.to_string())))
}

fn awk_gsub_pat(body: &str) -> Option<AwkPat> {
    let rest = body.strip_prefix("gsub(/")?;
    let (pattern, rest) = rest.split_once("/,\"")?;
    let replacement = rest.strip_suffix("\")")?;
    if pattern.is_empty()
        || pattern.contains(['/', '\\', '[', '*', '.', '+', '"'])
        || replacement != pattern
    {
        return None;
    }
    Some(AwkPat::Literal(pattern.to_string()))
}

fn awk_index_pat(body: &str) -> Option<AwkPat> {
    let rest = body.strip_prefix("index($0,\"")?;
    let inner = rest.strip_suffix("\")")?;
    if inner.contains('"') {
        return None;
    }
    Some(AwkPat::Literal(inner.to_string()))
}

fn awk_script(text: &str, atoms: &[AwkAtom]) -> String {
    let lines: Vec<&str> = text
        .split_inclusive('\n')
        .filter(|line| !line.is_empty())
        .collect();
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        let nr = index + 1;
        let body = line.trim_end_matches('\n');
        let hit = atoms.iter().any(|atom| match atom {
            AwkAtom::All => true,
            AwkAtom::Range { start, end } => nr >= *start && nr <= *end,
            AwkAtom::Match(pat) => awk_pat_hit(body, pat),
        });
        if hit {
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

fn awk_pat_hit(line: &str, pat: &AwkPat) -> bool {
    match pat {
        AwkPat::Literal(text) => line.contains(text),
        AwkPat::Regex(source) => awk_regex_hit(line, source),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AwkQuant {
    One,
    Star,
    Plus,
}

#[derive(Clone, Copy)]
enum AwkRx {
    Any,
    Char(char),
}

struct AwkPiece {
    atom: AwkRx,
    quant: AwkQuant,
}

/// `.`, a literal, and one `*` or `+` on the previous atom.
/// Anything else is unmodeled (`None`), and the caller keeps every line.
fn parse_awk_regex(source: &str) -> Option<Vec<AwkPiece>> {
    let chars: Vec<char> = source.chars().collect();
    let mut index = 0usize;
    let mut parts = Vec::new();
    while index < chars.len() {
        let atom = match chars[index] {
            '.' => AwkRx::Any,
            '*' | '+' | '?' | '[' | ']' | '\\' | '(' | ')' | '|' | '{' | '}' | '^' | '$' => {
                return None;
            }
            ch => AwkRx::Char(ch),
        };
        index += 1;
        let quant = if index < chars.len() {
            match chars[index] {
                '*' => {
                    index += 1;
                    AwkQuant::Star
                }
                '+' => {
                    index += 1;
                    AwkQuant::Plus
                }
                _ => AwkQuant::One,
            }
        } else {
            AwkQuant::One
        };
        parts.push(AwkPiece { atom, quant });
    }
    Some(parts)
}

fn awk_regex_hit(line: &str, source: &str) -> bool {
    let Some(parts) = parse_awk_regex(source) else {
        return true;
    };
    if parts.is_empty() {
        return true;
    }
    if parts
        .iter()
        .filter(|part| part.quant != AwkQuant::One)
        .count()
        > 1
    {
        return true;
    }
    let chars: Vec<char> = line.chars().collect();
    (0..=chars.len()).any(|start| awk_match_at(&chars, &parts, start))
}

fn awk_match_at(chars: &[char], parts: &[AwkPiece], start: usize) -> bool {
    let mut ci = start;
    for (index, part) in parts.iter().enumerate() {
        match part.quant {
            AwkQuant::One => {
                if !awk_eat(chars, ci, part.atom) {
                    return false;
                }
                ci += 1;
            }
            AwkQuant::Star | AwkQuant::Plus => {
                let min = if part.quant == AwkQuant::Plus { 1 } else { 0 };
                let mut count = 0usize;
                while awk_eat(chars, ci + count, part.atom) {
                    count += 1;
                }
                if count < min {
                    return false;
                }
                let suffix = &parts[index + 1..];
                return (min..=count)
                    .rev()
                    .any(|take| awk_match_fixed(chars, suffix, ci + take));
            }
        }
    }
    true
}

fn awk_match_fixed(chars: &[char], parts: &[AwkPiece], mut ci: usize) -> bool {
    for part in parts {
        if part.quant != AwkQuant::One || !awk_eat(chars, ci, part.atom) {
            return false;
        }
        ci += 1;
    }
    true
}

fn awk_eat(chars: &[char], index: usize, atom: AwkRx) -> bool {
    let Some(ch) = chars.get(index) else {
        return false;
    };
    match atom {
        AwkRx::Any => true,
        AwkRx::Char(want) => *ch == want,
    }
}

fn split_statements(program: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    for (index, ch) in program.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => {
                let piece = program[start..index].trim();
                if !piece.is_empty() {
                    pieces.push(piece);
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    let piece = program[start..].trim();
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

fn awk_drop_print(text: &str) -> &str {
    text.strip_suffix("{print;}")
        .or_else(|| text.strip_suffix("{print}"))
        .or_else(|| text.strip_suffix("{print$0;}"))
        .or_else(|| text.strip_suffix("{print$0}"))
        .unwrap_or(text)
}

fn awk_one(program: &str) -> Option<AwkSel> {
    if matches!(program, "{print}" | "1" | "{print$0}" | "{print$0;}") {
        return Some(AwkSel::All);
    }
    let rest = program.strip_prefix("NR==")?;
    if let Some((left, right)) = rest.split_once(",NR==") {
        let start = line_count(awk_drop_print(left))?;
        let end = line_count(awk_drop_print(right))?;
        if start == 0 || end == 0 {
            return None;
        }
        return Some(AwkSel::Range { start, end });
    }
    let body = awk_drop_print(rest);
    let line = line_count(body)?;
    if line == 0 {
        return None;
    }
    Some(AwkSel::Range {
        start: line,
        end: line,
    })
}

struct CutQuery {
    file: Option<String>,
}

fn cut_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = cut_query(args) else {
        return Run::Unknown;
    };
    source_text(query.file.as_deref(), input, depth, substitutions)
}

fn cut_query(args: &[String]) -> Option<CutQuery> {
    let mut index = 0usize;
    let mut identity = false;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-d" || arg == "--delimiter" {
            index += 1;
            args.get(index)?;
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-d")
            && !rest.is_empty()
            && !rest.starts_with('-')
        {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--delimiter=") {
            if rest.is_empty() {
                return None;
            }
            index += 1;
            continue;
        }
        if arg == "-f" {
            if args.get(index + 1).is_some_and(|spec| spec == "1-") {
                identity = true;
                index += 2;
                continue;
            }
            return None;
        }
        if let Some(rest) = arg.strip_prefix("-f")
            && !rest.is_empty()
        {
            if rest != "1-" {
                return None;
            }
            identity = true;
            index += 1;
            continue;
        }
        if arg == "-c" {
            if args.get(index + 1).is_some_and(|spec| spec == "1-") {
                identity = true;
                index += 2;
                continue;
            }
            return None;
        }
        if let Some(rest) = arg.strip_prefix("-c")
            && !rest.is_empty()
        {
            if rest != "1-" {
                return None;
            }
            identity = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        break;
    }
    if !identity || args.len() - index > 1 {
        return None;
    }
    Some(CutQuery {
        file: args.get(index).cloned(),
    })
}

struct SortQuery {
    files: Vec<String>,
    from0: Option<String>,
}

fn sort_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    // GNU sort execs this program once the input no longer fits in memory.
    if let Some(program) = sort_compress_program(args) {
        substitutions.push(program);
    }
    let Some(query) = sort_query(args) else {
        return Run::Unknown;
    };
    let files = if let Some(list) = &query.from0 {
        match nul_names(list, input, depth, substitutions) {
            NulNames::Paths(paths) => paths,
            NulNames::Empty => {
                return Run::Produced {
                    text: String::new(),
                    ok: false,
                };
            }
            NulNames::Unscanned => return Run::Unscanned,
            NulNames::Unknown => return Run::Unknown,
        }
    } else {
        query.files
    };
    if query.from0.is_some() && files.is_empty() {
        return Run::Produced {
            text: String::new(),
            ok: false,
        };
    }
    // A missing path makes GNU sort print nothing, including earlier files.
    load_pieces(&files, input, depth, substitutions, true).map_or_else(
        |run| run,
        |pieces| Run::Produced {
            text: pieces.text,
            ok: pieces.ok,
        },
    )
}

fn sort_query(args: &[String]) -> Option<SortQuery> {
    let mut index = 0usize;
    let mut files = Vec::new();
    let mut from0 = None;
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "--" {
            index += 1;
            files.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            files.push(arg);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (rest, None),
            };
            if matches!(name, "check" | "output" | "help" | "version") {
                return None;
            }
            if name == "files0-from" {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        index += 1;
                        args.get(index)?.clone()
                    }
                };
                if value.is_empty() || from0.is_some() {
                    return None;
                }
                from0 = Some(value);
                index += 1;
                continue;
            }
            if takes_sort_value(name) {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        index += 1;
                        args.get(index)?.clone()
                    }
                };
                if value.is_empty() {
                    return None;
                }
                index += 1;
                continue;
            }
            if is_sort_flag(name) {
                if inline.is_some() {
                    return None;
                }
                index += 1;
                continue;
            }
            return None;
        }
        let flags: Vec<char> = arg[1..].chars().collect();
        let mut cursor = 0usize;
        while cursor < flags.len() {
            let ch = flags[cursor];
            cursor += 1;
            match ch {
                'c' | 'C' | 'o' => return None,
                'b' | 'd' | 'f' | 'g' | 'h' | 'i' | 'M' | 'm' | 'n' | 'r' | 'R' | 's' | 'u'
                | 'V' | 'z' => {}
                'k' | 't' | 'S' | 'T' => {
                    let attached: String = flags[cursor..].iter().collect();
                    if attached.is_empty() {
                        index += 1;
                        args.get(index)?;
                    }
                    break;
                }
                _ => return None,
            }
        }
        index += 1;
    }
    if from0.is_some() && !files.is_empty() {
        return None;
    }
    Some(SortQuery { files, from0 })
}

fn takes_sort_value(name: &str) -> bool {
    matches!(
        name,
        "key"
            | "field-separator"
            | "buffer-size"
            | "temporary-directory"
            | "parallel"
            | "batch-size"
            | "random-source"
            | "compress-program"
            | "sort"
    )
}

fn is_sort_flag(name: &str) -> bool {
    matches!(
        name,
        "stable"
            | "unique"
            | "reverse"
            | "zero-terminated"
            | "numeric-sort"
            | "dictionary-order"
            | "ignore-case"
            | "ignore-leading-blanks"
            | "ignore-nonprinting"
            | "month-sort"
            | "human-numeric-sort"
            | "version-sort"
            | "general-numeric-sort"
            | "merge"
            | "random-sort"
            | "debug"
    )
}

/// Program GNU sort executes to compress a temporary file.
fn sort_compress_program(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if let Some(program) = arg.strip_prefix("--compress-program=") {
            if program.is_empty() {
                return None;
            }
            return Some(program.to_string());
        }
        if arg == "--compress-program" {
            return args
                .get(index + 1)
                .cloned()
                .filter(|program| !program.is_empty());
        }
        index += 1;
    }
    None
}

fn sort_compress_commands(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return Vec::new();
    };
    let base = command_basename(first);
    if !base.eq_ignore_ascii_case("sort") && !base.eq_ignore_ascii_case("gsort") {
        return Vec::new();
    }
    sort_compress_program(&argv[1..]).into_iter().collect()
}

enum NulNames {
    Paths(Vec<String>),
    Empty,
    Unscanned,
    Unknown,
}

fn nul_names(
    list: &str,
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> NulNames {
    let text = if list == "-" {
        match input {
            Some(text) => text.to_string(),
            None => return NulNames::Unknown,
        }
    } else {
        match load_text_operand(list, depth, substitutions) {
            Loaded::Text(text) => text,
            Loaded::Missing | Loaded::Dir | Loaded::Null => return NulNames::Empty,
            Loaded::Unscanned => return NulNames::Unscanned,
            Loaded::Fifo | Loaded::Hang | Loaded::Unknown => return NulNames::Unknown,
        }
    };
    let paths = text
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    NulNames::Paths(paths)
}

struct LoadedPieces {
    text: String,
    ok: bool,
}

/// `drop_all_if_missing` matches GNU sort: one unreadable path prints nothing.
/// Tac still prints the files it could read.
fn load_pieces(
    files: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
    drop_all_if_missing: bool,
) -> Result<LoadedPieces, Run> {
    if files.is_empty() {
        return match source_text(None, input, depth, substitutions) {
            Run::Produced { text, ok } => Ok(LoadedPieces { text, ok }),
            other => Err(other),
        };
    }
    let mut parts = Vec::new();
    let mut missing = false;
    for file in files {
        if file == "-" {
            let Some(text) = input else {
                return Err(Run::Unknown);
            };
            parts.push(text.to_string());
            continue;
        }
        match load_text_operand(file, depth, substitutions) {
            Loaded::Text(text) => parts.push(text),
            Loaded::Unscanned => return Err(Run::Unscanned),
            Loaded::Missing | Loaded::Dir => {
                if drop_all_if_missing {
                    return Ok(LoadedPieces {
                        text: String::new(),
                        ok: false,
                    });
                }
                missing = true;
            }
            Loaded::Null => {}
            Loaded::Fifo | Loaded::Hang | Loaded::Unknown => return Err(Run::Unknown),
        }
    }
    Ok(LoadedPieces {
        text: parts.concat(),
        ok: !missing,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UniqMode {
    Each,
    Once,
    Dups,
    EveryDup,
    /// `--group` keeps every line.
    Group,
}

struct UniqQuery {
    mode: UniqMode,
    fold: bool,
    zero: bool,
    file: Option<String>,
}

fn uniq_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = uniq_query(args) else {
        return Run::Unknown;
    };
    match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { text, ok } => Run::Produced {
            text: uniq_lines(&text, query.mode, query.fold, query.zero),
            ok,
        },
        other => other,
    }
}

fn uniq_query(args: &[String]) -> Option<UniqQuery> {
    let mut index = 0usize;
    let mut mode = None;
    let mut fold = false;
    let mut zero = false;
    let mut positional = Vec::new();
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "--" {
            index += 1;
            positional.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            positional.push(arg);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (rest, None),
            };
            match name {
                "count" | "help" | "version" => return None,
                "repeated" => set_uniq_mode(&mut mode, UniqMode::Dups)?,
                "all-repeated" => {
                    if let Some(value) = inline
                        && !matches!(value, "none" | "prepend" | "separate")
                    {
                        return None;
                    }
                    set_uniq_mode(&mut mode, UniqMode::EveryDup)?;
                }
                "unique" => set_uniq_mode(&mut mode, UniqMode::Once)?,
                "group" => {
                    if let Some(value) = inline
                        && !matches!(value, "separate" | "prepend" | "append" | "both")
                    {
                        return None;
                    }
                    set_uniq_mode(&mut mode, UniqMode::Group)?;
                }
                "ignore-case" => {
                    if inline.is_some() {
                        return None;
                    }
                    fold = true;
                }
                "zero-terminated" => {
                    if inline.is_some() {
                        return None;
                    }
                    zero = true;
                }
                "skip-fields" | "skip-chars" | "check-chars" => {
                    let value = match inline {
                        Some(value) => value.to_string(),
                        None => {
                            index += 1;
                            args.get(index)?.clone()
                        }
                    };
                    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
                        return None;
                    }
                }
                _ => return None,
            }
            index += 1;
            continue;
        }
        let flags: Vec<char> = arg[1..].chars().collect();
        let mut cursor = 0usize;
        while cursor < flags.len() {
            let ch = flags[cursor];
            cursor += 1;
            match ch {
                'c' => return None,
                'd' => set_uniq_mode(&mut mode, UniqMode::Dups)?,
                'D' => set_uniq_mode(&mut mode, UniqMode::EveryDup)?,
                'u' => set_uniq_mode(&mut mode, UniqMode::Once)?,
                'i' => fold = true,
                'z' => zero = true,
                'f' | 's' | 'w' => {
                    let attached: String = flags[cursor..].iter().collect();
                    let value = if attached.is_empty() {
                        index += 1;
                        args.get(index)?.clone()
                    } else {
                        attached
                    };
                    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
                        return None;
                    }
                    break;
                }
                _ => return None,
            }
        }
        index += 1;
    }
    // A second operand is an output file. Stdout stays empty.
    if positional.len() > 1 {
        return None;
    }
    Some(UniqQuery {
        mode: mode.unwrap_or(UniqMode::Each),
        fold,
        zero,
        file: positional.first().cloned(),
    })
}

fn set_uniq_mode(mode: &mut Option<UniqMode>, next: UniqMode) -> Option<()> {
    if mode.is_some() {
        return None;
    }
    *mode = Some(next);
    Some(())
}

fn uniq_lines(text: &str, mode: UniqMode, fold: bool, zero: bool) -> String {
    if mode == UniqMode::Group {
        return text.to_string();
    }
    let sep = if zero { "\0" } else { "\n" };
    let ended = text.ends_with(sep);
    let mut lines: Vec<&str> = text.split(sep).collect();
    if ended && lines.last() == Some(&"") {
        lines.pop();
    }
    if text.is_empty() {
        return String::new();
    }
    let mut kept = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let mut next = index + 1;
        while next < lines.len() && line_eq(lines[index], lines[next], fold) {
            next += 1;
        }
        let count = next - index;
        let copies = match mode {
            UniqMode::Each => 1,
            UniqMode::Once => usize::from(count == 1),
            UniqMode::Dups => usize::from(count > 1),
            UniqMode::EveryDup => {
                if count > 1 {
                    count
                } else {
                    0
                }
            }
            UniqMode::Group => 0,
        };
        for _ in 0..copies {
            kept.push(lines[index]);
        }
        index = next;
    }
    if kept.is_empty() {
        return String::new();
    }
    let mut joined = kept.join(sep);
    if ended {
        joined.push_str(sep);
    }
    joined
}

fn line_eq(left: &str, right: &str, fold: bool) -> bool {
    if fold {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

struct TacQuery {
    files: Vec<String>,
    before: bool,
    regex: bool,
    separator: Option<String>,
}

fn tac_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = tac_query(args) else {
        return Run::Unknown;
    };
    let sep = query.separator.unwrap_or_else(|| "\n".to_string());
    if sep.is_empty() {
        return Run::Unknown;
    }
    if query.regex && sep_is_regex(&sep) {
        return Run::Unknown;
    }
    let loaded = match load_pieces(&query.files, input, depth, substitutions, false) {
        Ok(pieces) => pieces,
        Err(run) => return run,
    };
    Run::Produced {
        text: tac_text(&loaded.text, &sep, query.before),
        ok: loaded.ok,
    }
}

fn tac_query(args: &[String]) -> Option<TacQuery> {
    let mut index = 0usize;
    let mut files = Vec::new();
    let mut before = false;
    let mut regex = false;
    let mut separator = None;
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "--" {
            index += 1;
            files.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            files.push(arg);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (rest, None),
            };
            match name {
                "before" => {
                    if inline.is_some() {
                        return None;
                    }
                    before = true;
                }
                "regex" => {
                    if inline.is_some() {
                        return None;
                    }
                    regex = true;
                }
                "separator" => {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            index += 1;
                            args.get(index)?.clone()
                        }
                    };
                    if separator.is_some() {
                        return None;
                    }
                    separator = Some(value);
                }
                "help" | "version" => return None,
                _ => return None,
            }
            index += 1;
            continue;
        }
        let flags: Vec<char> = arg[1..].chars().collect();
        let mut cursor = 0usize;
        while cursor < flags.len() {
            let ch = flags[cursor];
            cursor += 1;
            match ch {
                'b' => before = true,
                'r' => regex = true,
                's' => {
                    let attached: String = flags[cursor..].iter().collect();
                    let value = if attached.is_empty() {
                        index += 1;
                        args.get(index)?.clone()
                    } else {
                        attached
                    };
                    if separator.is_some() {
                        return None;
                    }
                    separator = Some(value);
                    break;
                }
                _ => return None,
            }
        }
        index += 1;
    }
    Some(TacQuery {
        files,
        before,
        regex,
        separator,
    })
}

fn sep_is_regex(sep: &str) -> bool {
    sep.chars().any(|ch| {
        matches!(
            ch,
            '.' | '*' | '+' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '\\' | '^' | '$'
        )
    })
}

/// GNU tac keeps the separator on the record, then reverses the records.
fn tac_text(text: &str, sep: &str, before: bool) -> String {
    if text.is_empty() || sep.is_empty() {
        return String::new();
    }
    if !before {
        let mut records = Vec::new();
        let mut rest = text;
        loop {
            if let Some(pos) = rest.find(sep) {
                let end = pos + sep.len();
                records.push(rest[..end].to_string());
                rest = &rest[end..];
                if rest.is_empty() {
                    break;
                }
            } else {
                records.push(rest.to_string());
                break;
            }
        }
        records.reverse();
        return records.concat();
    }
    let mut records = Vec::new();
    let mut rest = text;
    loop {
        if let Some(pos) = rest.find(sep) {
            let end = pos + sep.len();
            records.push((true, rest[..pos].to_string()));
            rest = &rest[end..];
            if rest.is_empty() {
                break;
            }
        } else {
            records.push((false, rest.to_string()));
            break;
        }
    }
    records.reverse();
    let mut out = String::new();
    for (has_sep, content) in records {
        if has_sep {
            out.push_str(sep);
        }
        out.push_str(&content);
    }
    out
}

fn identity_run(
    command: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(file) = identity_query(command, args) else {
        return Run::Unknown;
    };
    source_text(file.as_deref(), input, depth, substitutions)
}

/// Printers whose output still contains the file text.
fn identity_query(command: &str, args: &[String]) -> Option<Option<String>> {
    let name = command_basename(command);
    let mut index = 0usize;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "--" {
            index += 1;
            files.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            files.push(arg);
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            let (long, inline) = match rest.split_once('=') {
                Some((long, value)) => (long, Some(value)),
                None => (rest, None),
            };
            let takes = long_identity_value(name, long)?;
            if takes {
                match inline {
                    Some(value) if !value.is_empty() => {}
                    Some(_) => return None,
                    None => {
                        index += 1;
                        args.get(index)?;
                    }
                }
            } else if inline.is_some() {
                return None;
            }
            index += 1;
            continue;
        }
        let flags: Vec<char> = arg[1..].chars().collect();
        let mut cursor = 0usize;
        while cursor < flags.len() {
            let ch = flags[cursor];
            cursor += 1;
            if short_identity_value(name, ch)? {
                let attached: String = flags[cursor..].iter().collect();
                if attached.is_empty() {
                    index += 1;
                    args.get(index)?;
                }
                break;
            }
        }
        index += 1;
    }
    if files.len() > 1 {
        return None;
    }
    Some(files.first().cloned())
}

fn short_identity_value(name: &str, flag: char) -> Option<bool> {
    Some(match (name, flag) {
        ("paste", 's')
        | ("unexpand", 'a')
        | ("fold", 's')
        | ("fold", 'b')
        | ("column", 't')
        | ("pr", 't')
        | ("expand", 'i')
        | ("strings", 'a')
        | ("fmt", 's') => false,
        ("paste", 'd')
        | ("expand", 't')
        | ("fold", 'w')
        | ("fmt", 'w')
        | ("strings", 'n')
        | ("column", 'c') => true,
        _ => return None,
    })
}

fn long_identity_value(name: &str, flag: &str) -> Option<bool> {
    Some(match (name, flag) {
        ("paste", "serial")
        | ("unexpand", "all")
        | ("fold", "spaces")
        | ("fold", "bytes")
        | ("column", "table")
        | ("expand", "initial")
        | ("strings", "all")
        | ("fmt", "split-only") => false,
        ("paste", "delimiters")
        | ("expand", "tabs")
        | ("fold", "width")
        | ("fmt", "width")
        | ("strings", "bytes")
        | ("column", "output-width") => true,
        _ => return None,
    })
}

struct Base64Query {
    file: Option<String>,
}

fn base32_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = base32_query(args) else {
        return Run::Unknown;
    };
    let raw = match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        Run::Produced { text, ok: true } => text,
        other => return other,
    };
    let Some(text) = b32_decode(&raw) else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

struct CoreutilsShort {
    decode: bool,
    wrap_needs_arg: bool,
}

/// GNU `base64` and `base32` cluster `-d`, `-i`, and `-w` COLS.
/// `-dw0` is decode plus a wrap size. `-wd0` is not a wrap size.
fn coreutils_short_opts(arg: &str, allow_upper_d: bool) -> Option<CoreutilsShort> {
    let rest = arg.strip_prefix('-')?;
    if rest.len() < 2 || rest.starts_with('-') {
        return None;
    }
    let mut decode = false;
    let bytes = rest.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'd' => decode = true,
            b'D' if allow_upper_d => decode = true,
            b'i' => {}
            b'w' => {
                let cols = &rest[index + 1..];
                if cols.is_empty() {
                    return Some(CoreutilsShort {
                        decode,
                        wrap_needs_arg: true,
                    });
                }
                if !cols.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                return Some(CoreutilsShort {
                    decode,
                    wrap_needs_arg: false,
                });
            }
            _ => return None,
        }
        index += 1;
    }
    Some(CoreutilsShort {
        decode,
        wrap_needs_arg: false,
    })
}

fn apply_coreutils_short(
    args: &[String],
    index: &mut usize,
    decode: &mut bool,
    allow_upper_d: bool,
) -> Option<bool> {
    let arg = args.get(*index)?;
    let Some(opts) = coreutils_short_opts(arg, allow_upper_d) else {
        return Some(false);
    };
    if opts.decode {
        *decode = true;
    }
    if opts.wrap_needs_arg {
        let next = args.get(*index + 1)?;
        if next.is_empty() || !next.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *index += 2;
    } else {
        *index += 1;
    }
    Some(true)
}

fn base32_query(args: &[String]) -> Option<Base64Query> {
    let mut index = 0usize;
    let mut decode = false;
    let mut file = None;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg == "-d" || arg == "--decode" {
            decode = true;
            index += 1;
            continue;
        }
        if arg == "-w" || arg == "--wrap" {
            index += 1;
            args.get(index)?;
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-w")
            && !rest.is_empty()
        {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--wrap=") {
            if rest.is_empty() {
                return None;
            }
            index += 1;
            continue;
        }
        if arg == "-i" || arg == "--ignore-garbage" {
            index += 1;
            continue;
        }
        match apply_coreutils_short(args, &mut index, &mut decode, false) {
            None => return None,
            Some(true) => continue,
            Some(false) => {}
        }
        if arg.starts_with('-') {
            return None;
        }
        break;
    }
    if !decode || args.len() - index > 1 {
        return None;
    }
    if file.is_none() {
        file = args.get(index).cloned();
    }
    Some(Base64Query { file })
}

fn b32_decode(text: &str) -> Option<String> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a'),
            b'2'..=b'7' => Some(byte - b'2' + 26),
            _ => None,
        }
    }
    let filtered: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
        .collect();
    if filtered.is_empty() {
        return Some(String::new());
    }
    let mut out = Vec::new();
    let mut buffer = 0u64;
    let mut bits = 0u32;
    for byte in filtered {
        buffer = (buffer << 5) | u64::from(value(byte)?);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1u64 << bits) - 1;
        }
    }
    String::from_utf8(out).ok()
}

fn dd_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(file) = dd_query(args) else {
        return Run::Unknown;
    };
    source_text(file.as_deref(), input, depth, substitutions)
}

fn dd_query(args: &[String]) -> Option<Option<String>> {
    let mut file = None;
    let mut saw_if = false;
    for arg in args {
        if let Some(path) = arg.strip_prefix("if=") {
            if saw_if {
                return None;
            }
            saw_if = true;
            if path != "-" {
                file = Some(path.to_string());
            }
            continue;
        }
        if arg.starts_with("of=")
            || arg.starts_with("count=")
            || arg.starts_with("skip=")
            || arg.starts_with("seek=")
            || arg.starts_with("iseek=")
            || arg.starts_with("oseek=")
            || arg.starts_with("conv=")
        {
            return None;
        }
        if arg.starts_with("status=")
            || arg.starts_with("bs=")
            || arg.starts_with("ibs=")
            || arg.starts_with("obs=")
            || arg.starts_with("cbs=")
        {
            continue;
        }
        return None;
    }
    Some(file)
}

struct PerlTarQuery {
    archive: String,
    member: Option<String>,
}

fn perl_tar_query(name: &str, args: &[String]) -> Option<PerlTarQuery> {
    if command_basename(name) != "perl" {
        return None;
    }
    let mut index = 0usize;
    let mut script = None;
    let mut module = false;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-MArchive::Tar" {
            module = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    let compact = compact_ws(&script);
    if !module
        || files.len() != 2
        || (compact != "printArchive::Tar->new($ARGV[0])->get_content($ARGV[1])"
            && compact != "printArchive::Tar->new($ARGV[0])->get_content($ARGV[1]);")
    {
        return None;
    }
    Some(PerlTarQuery {
        archive: files[0].clone(),
        member: Some(files[1].clone()),
    })
}

fn perl_gzip_query(name: &str, args: &[String]) -> Option<String> {
    if command_basename(name) != "perl" {
        return None;
    }
    let mut index = 0usize;
    let mut script = None;
    let mut module = false;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-MIO::Uncompress::Gunzip=gunzip" {
            module = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if !module || files.len() != 1 || compact_ws(&script) != "gunzip$ARGV[0]=>\\*STDOUT" {
        return None;
    }
    Some(files.remove(0))
}

fn perl_bzip_query(name: &str, args: &[String]) -> Option<String> {
    if command_basename(name) != "perl" {
        return None;
    }
    let mut index = 0usize;
    let mut script = None;
    let mut module = false;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-MIO::Uncompress::Bunzip2=bunzip2" {
            module = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if !module || files.len() != 1 || compact_ws(&script) != "bunzip2$ARGV[0]=>\\*STDOUT" {
        return None;
    }
    Some(files.remove(0))
}

fn perl_raw_query(name: &str, args: &[String]) -> Option<String> {
    if command_basename(name) != "perl" {
        return None;
    }
    let mut index = 0usize;
    let mut script = None;
    let mut module = false;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-MIO::Uncompress::RawInflate=rawinflate" {
            module = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if !module || files.len() != 1 || compact_ws(&script) != "rawinflate$ARGV[0]=>\\*STDOUT" {
        return None;
    }
    Some(files.remove(0))
}

fn perl_any_query(name: &str, args: &[String]) -> Option<String> {
    if command_basename(name) != "perl" {
        return None;
    }
    let mut index = 0usize;
    let mut script = None;
    let mut module = false;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-MIO::Uncompress::AnyInflate=anyinflate" {
            module = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if !module || files.len() != 1 || compact_ws(&script) != "anyinflate$ARGV[0]=>\\*STDOUT" {
        return None;
    }
    Some(files.remove(0))
}

fn perl_zip_query(name: &str, args: &[String]) -> Option<(String, String)> {
    if command_basename(name) != "perl" {
        return None;
    }
    let mut index = 0usize;
    let mut script = None;
    let mut module = false;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-MArchive::Zip" {
            module = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if !module
        || files.len() != 2
        || compact_ws(&script) != "printArchive::Zip->new($ARGV[0])->contents($ARGV[1])"
    {
        return None;
    }
    let member = files.pop()?;
    let archive = files.pop()?;
    Some((archive, member))
}

fn line_print_run(
    name: &str,
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    if let Some(query) = perl_tar_query(name, args) {
        return produced_archive(&query.archive, |bytes| {
            tar_text(bytes, query.member.as_deref(), TarCompress::Auto)
        });
    }
    if let Some(path) = perl_gzip_query(name, args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            gzip_flate_decode,
        );
    }
    if let Some((kind, path)) = ruby_gzip_query(name, args) {
        let decode = match kind {
            RubyInflate::Gzip => gzip_flate_decode,
            RubyInflate::Zlib => zlib_decode,
        };
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            decode,
        );
    }
    if let Some(path) = perl_bzip_query(name, args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            bzip2_decode,
        );
    }
    if let Some(path) = perl_raw_query(name, args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            deflate_decode,
        );
    }
    if let Some(path) = perl_any_query(name, args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            any_inflate_decode,
        );
    }
    if let Some((archive, member)) = perl_zip_query(name, args) {
        return produced_archive(&archive, |bytes| unzip_text(bytes, Some(&member)));
    }
    let Some(file) = line_print_query(name, args) else {
        return Run::Unknown;
    };
    let produced = source_text(file.as_deref(), input, depth, substitutions);
    if ruby_b64_script(name, args) || perl_b64_script(name, args) {
        decode_produced(produced)
    } else {
        produced
    }
}

/// `perl -pe ''` and `ruby -pe ''` print each line unchanged.
/// `perl -ne 'print'` and `ruby -ne 'print'` print the current line.
fn line_print_query(name: &str, args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut print_lines = false;
    let mut loop_lines = false;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "--" {
            index += 1;
            files.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "-p" {
            print_lines = true;
            index += 1;
            continue;
        }
        if arg == "-n" {
            loop_lines = true;
            index += 1;
            continue;
        }
        if arg == "-l" || arg == "-w" || arg == "-W" || arg == "-X" {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-E")
            && !rest.is_empty()
            && rest
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '-' | '_' | '.'))
        {
            index += 1;
            continue;
        }
        if let Some(level) = arg.strip_prefix("-W")
            && level.chars().all(|ch| ch.is_ascii_digit())
        {
            index += 1;
            continue;
        }
        if let Some(flags) = arg.strip_prefix('-')
            && flags.len() > 1
            && flags
                .chars()
                .all(|ch| matches!(ch, 'l' | 'n' | 'p' | 'e' | 'w'))
            && flags.contains('e')
        {
            if flags.contains('p') {
                print_lines = true;
            }
            if flags.contains('n') {
                loop_lines = true;
            }
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-ne" || arg == "-en" {
            loop_lines = true;
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-e" || arg == "-pe" || arg == "-ep" {
            if arg != "-e" {
                print_lines = true;
            }
            index += 1;
            let text = args.get(index)?.clone();
            if script.is_some() {
                return None;
            }
            script = Some(text);
            index += 1;
            continue;
        }
        if arg.starts_with("-0") && arg.chars().skip(2).all(|ch| matches!(ch, '0'..='7')) {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-pe")
            && !rest.is_empty()
        {
            print_lines = true;
            if script.is_some() {
                return None;
            }
            script = Some(rest.to_string());
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-r")
            && !rest.is_empty()
            && rest
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '/'))
        {
            index += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-M")
            && !rest.is_empty()
            && rest
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_'))
        {
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(arg);
        index += 1;
    }
    if files.len() > 1 {
        return None;
    }
    match script {
        Some(text) if print_lines && !pe_clears_line(&text) => Some(files.first().cloned()),
        Some(text) if loop_lines && line_prints_current(&text) => Some(files.first().cloned()),
        Some(text)
            if command_basename(name) == "ruby" && ruby_reads_argv0(&text) && files.len() == 1 =>
        {
            Some(files.first().cloned())
        }
        Some(text)
            if command_basename(name) == "perl" && perl_b64_text(&text) && files.len() == 1 =>
        {
            Some(files.first().cloned())
        }
        Some(text)
            if command_basename(name) == "perl"
                && perl_diamond_prints(&text)
                && files.len() == 1 =>
        {
            Some(files.first().cloned())
        }
        _ => None,
    }
}

fn pe_clears_line(script: &str) -> bool {
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "$_=\"\"" | "$_=''" | "$_=\"\";" | "$_='';"
    )
}

fn line_prints_current(script: &str) -> bool {
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "print" | "print;" | "print$_" | "print$_;"
    )
}

fn perl_diamond_prints(script: &str) -> bool {
    let compact = compact_ws(script);
    matches!(
        compact.as_str(),
        "print<>"
            | "printwhile<>"
            | "printwhile<>;"
            | "while(<>){print}"
            | "while(<>){print;}"
            | "print$_while<>"
            | "print$_while<>;"
    )
}

fn ruby_reads_argv0(script: &str) -> bool {
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "printFile.read(ARGV[0])"
            | "printFile.binread(ARGV[0])"
            | "printFile.open(ARGV[0]).read"
            | "printFile.readlines(ARGV[0]).join"
            | "printFile.readlines(ARGV[0]).join(\"\")"
            | "printFile.readlines(ARGV[0]).join(\"\\n\")"
            | "File.foreach(ARGV[0]){|l|printl}"
            | "IO.foreach(ARGV[0]){|l|printl}"
            | "putsFile.readlines(ARGV[0])"
            | "File.read(ARGV[0]).each_line{|l|printl}"
            | "printFile.read(ARGV[0],encoding:\"UTF-8\")"
            | "printFile.read(ARGV[0],mode:\"r\")"
            | "putsFile.readlines(ARGV[0]).join"
            | "putsFile.read(ARGV[0])"
            | "printIO.read(ARGV[0])"
            | "printIO.binread(ARGV[0])"
            | "printARGF.read"
            | "putsARGF.read"
            | "putsIO.read(ARGV[0])"
            | "STDOUT.writeIO.read(ARGV[0])"
            | "IO.copy_stream(ARGV[0],STDOUT)"
            | "system(File.read(ARGV[0]))"
            | "printFile.binread(ARGV[0]).unpack1(\"m\")"
            | "printFile.binread(ARGV[0]).unpack1('m')"
            | "printBase64.decode64(File.binread(ARGV[0]))"
    )
}

fn perl_b64_text(script: &str) -> bool {
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "printdecode_base64(<>)" | "printdecode_base64(<>);"
    )
}

fn perl_b64_script(name: &str, args: &[String]) -> bool {
    if command_basename(name) != "perl" {
        return false;
    }
    flag_script(args).is_some_and(perl_b64_text)
}

fn ruby_b64_script(name: &str, args: &[String]) -> bool {
    if command_basename(name) != "ruby" {
        return false;
    }
    let Some(script) = flag_script(args) else {
        return false;
    };
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "printFile.binread(ARGV[0]).unpack1(\"m\")"
            | "printFile.binread(ARGV[0]).unpack1('m')"
            | "printBase64.decode64(File.binread(ARGV[0]))"
    )
}

fn flag_script(args: &[String]) -> Option<&str> {
    let mut index = 0usize;
    while index < args.len() {
        if args[index] == "-e" || args[index] == "-c" {
            return args.get(index + 1).map(String::as_str);
        }
        index += 1;
    }
    None
}

fn decode_produced(produced: Run) -> Run {
    match produced {
        Run::Produced { text, ok: true } => {
            let Some(text) = b64_decode(&text) else {
                return Run::Unknown;
            };
            Run::Produced { text, ok: true }
        }
        other => other,
    }
}

struct PyModuleB64 {
    file: Option<String>,
}

fn python_module_b64(args: &[String]) -> Option<PyModuleB64> {
    let mut index = 0usize;
    while index < args.len() && matches!(args[index].as_str(), "-u" | "-E" | "-B" | "-s" | "-I") {
        index += 1;
    }
    if args.get(index).map(String::as_str) != Some("-m") {
        return None;
    }
    index += 1;
    if args.get(index).map(String::as_str) != Some("base64") {
        return None;
    }
    index += 1;
    let mut decode = false;
    let mut file = None;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-d" || arg == "-u" {
            decode = true;
            index += 1;
            continue;
        }
        if arg == "-" || !arg.starts_with('-') {
            if file.is_some() {
                return None;
            }
            if arg != "-" {
                file = Some(args[index].clone());
            }
            index += 1;
            continue;
        }
        return None;
    }
    if !decode {
        return None;
    }
    Some(PyModuleB64 { file })
}

enum PyCodec {
    Gzip,
    Bzip2,
    Lzma,
    Zlib,
}

fn python_codec_query(args: &[String]) -> Option<(PyCodec, String)> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-c" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if matches!(arg, "-u" | "-E" | "-B" | "-s" | "-I") {
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    let compact = compact_ws(&script);
    let codec = match compact.as_str() {
        "importsys,gzip;sys.stdout.buffer.write(gzip.open(sys.argv[1],\"rb\").read())"
        | "importsys,gzip;sys.stdout.buffer.write(gzip.open(sys.argv[1],'rb').read())"
        | "importgzip,sys;sys.stdout.buffer.write(gzip.decompress(open(sys.argv[1],\"rb\").read()))"
        | "importgzip,sys;sys.stdout.buffer.write(gzip.decompress(open(sys.argv[1],'rb').read()))"
        | "importsys,gzip;sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1]).read())"
        | "importsys,gzip;sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1],\"rb\").read())"
        | "importsys,gzip;sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1],'rb').read())"
        | "importsys,gzip;sys.stdout.write(gzip.open(sys.argv[1],\"rt\").read())"
        | "importsys,gzip;sys.stdout.write(gzip.open(sys.argv[1],'rt').read())"
        | "importsys,gzip;sys.stdout.write(gzip.open(sys.argv[1],\"rt\",encoding=\"utf-8\").read())"
        | "importsys,gzip;sys.stdout.write(gzip.open(sys.argv[1],'rt',encoding='utf-8').read())" => {
            PyCodec::Gzip
        }
        "importsys,bz2;sys.stdout.buffer.write(bz2.open(sys.argv[1],\"rb\").read())"
        | "importsys,bz2;sys.stdout.buffer.write(bz2.open(sys.argv[1],'rb').read())"
        | "importbz2,sys;sys.stdout.buffer.write(bz2.decompress(open(sys.argv[1],\"rb\").read()))"
        | "importbz2,sys;sys.stdout.buffer.write(bz2.decompress(open(sys.argv[1],'rb').read()))"
        | "importsys,bz2;sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1]).read())"
        | "importsys,bz2;sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1],\"rb\").read())"
        | "importsys,bz2;sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1],'rb').read())"
        | "importbz2,sys;sys.stdout.buffer.write(bz2.BZ2Decompressor().decompress(open(sys.argv[1],\"rb\").read()))"
        | "importbz2,sys;sys.stdout.buffer.write(bz2.BZ2Decompressor().decompress(open(sys.argv[1],'rb').read()))"
        | "importsys,bz2;sys.stdout.write(bz2.open(sys.argv[1],\"rt\").read())"
        | "importsys,bz2;sys.stdout.write(bz2.open(sys.argv[1],'rt').read())" => PyCodec::Bzip2,
        "importsys,lzma;sys.stdout.buffer.write(lzma.open(sys.argv[1],\"rb\").read())"
        | "importsys,lzma;sys.stdout.buffer.write(lzma.open(sys.argv[1],'rb').read())"
        | "importlzma,sys;sys.stdout.buffer.write(lzma.decompress(open(sys.argv[1],\"rb\").read()))"
        | "importlzma,sys;sys.stdout.buffer.write(lzma.decompress(open(sys.argv[1],'rb').read()))"
        | "importsys,lzma;sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1]).read())"
        | "importsys,lzma;sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1],\"rb\").read())"
        | "importsys,lzma;sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1],'rb').read())"
        | "importlzma,sys;sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],\"rb\").read()))"
        | "importlzma,sys;sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],'rb').read()))"
        | "importsys,lzma;sys.stdout.write(lzma.open(sys.argv[1],\"rt\").read())"
        | "importsys,lzma;sys.stdout.write(lzma.open(sys.argv[1],'rt').read())" => PyCodec::Lzma,
        "importsys,zlib;sys.stdout.buffer.write(zlib.decompress(open(sys.argv[1],\"rb\").read()))"
        | "importsys,zlib;sys.stdout.buffer.write(zlib.decompress(open(sys.argv[1],'rb').read()))" => {
            PyCodec::Zlib
        }
        _ => return None,
    };
    if files.len() != 1 {
        return None;
    }
    Some((codec, files.remove(0)))
}

enum RubyInflate {
    Gzip,
    Zlib,
}

fn ruby_gzip_query(name: &str, args: &[String]) -> Option<(RubyInflate, String)> {
    if command_basename(name) != "ruby" {
        return None;
    }
    let mut index = 0usize;
    let mut script = None;
    let mut zlib = false;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg == "-rzlib" {
            zlib = true;
            index += 1;
            continue;
        }
        if arg == "-r" && args.get(index + 1).map(String::as_str) == Some("zlib") {
            zlib = true;
            index += 2;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if !zlib || files.len() != 1 {
        return None;
    }
    let kind = match compact_ws(&script).as_str() {
        "printZlib::GzipReader.open(ARGV[0]).read"
        | "printZlib::GzipReader.open(ARGV[0]).readlines.join"
        | "Zlib::GzipReader.open(ARGV[0]){|f|printf.read}"
        | "printZlib::GzipReader.wrap(File.open(ARGV[0])){|f|f.read}"
        | "printZlib::GzipReader.new(File.open(ARGV[0])).read"
        | "printZlib::GzipReader.open(ARGV[0],&:read)"
        | "printZlib.gunzip(File.binread(ARGV[0]))" => RubyInflate::Gzip,
        "printZlib::Inflate.new.inflate(File.binread(ARGV[0]))"
        | "printZlib.inflate(File.binread(ARGV[0]))" => RubyInflate::Zlib,
        _ => return None,
    };
    Some((kind, files.remove(0)))
}

fn node_gzip_query(args: &[String]) -> Option<(NodeInflate, String)> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-e" || arg == "--eval" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    let kind = node_inflate_kind(&compact_ws(&script))?;
    if files.len() != 1 {
        return None;
    }
    Some((kind, files.remove(0)))
}

enum NodeInflate {
    Gzip,
    Zlib,
    GzipOrZlib,
    Brotli,
    Raw,
}

fn node_inflate_kind(compact: &str) -> Option<NodeInflate> {
    match compact {
        "constfs=require(\"fs\");constz=require(\"zlib\");process.stdout.write(z.gunzipSync(fs.readFileSync(process.argv[1])))"
        | "constfs=require('fs');constz=require('zlib');process.stdout.write(z.gunzipSync(fs.readFileSync(process.argv[1])))" => {
            Some(NodeInflate::Gzip)
        }
        "constfs=require(\"fs\");constz=require(\"zlib\");process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))"
        | "constfs=require('fs');constz=require('zlib');process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))" => {
            Some(NodeInflate::GzipOrZlib)
        }
        "constfs=require(\"fs\");constz=require(\"zlib\");process.stdout.write(z.inflateSync(fs.readFileSync(process.argv[1])))"
        | "constfs=require('fs');constz=require('zlib');process.stdout.write(z.inflateSync(fs.readFileSync(process.argv[1])))" => {
            Some(NodeInflate::Zlib)
        }
        "constfs=require(\"fs\");constz=require(\"zlib\");process.stdout.write(z.brotliDecompressSync(fs.readFileSync(process.argv[1])))"
        | "constfs=require('fs');constz=require('zlib');process.stdout.write(z.brotliDecompressSync(fs.readFileSync(process.argv[1])))" => {
            Some(NodeInflate::Brotli)
        }
        "constfs=require(\"fs\");constz=require(\"zlib\");process.stdout.write(z.inflateRawSync(fs.readFileSync(process.argv[1])))"
        | "constfs=require('fs');constz=require('zlib');process.stdout.write(z.inflateRawSync(fs.readFileSync(process.argv[1])))" => {
            Some(NodeInflate::Raw)
        }
        _ => None,
    }
}

fn python_cat_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    if let Some((codec, path)) = python_codec_query(args) {
        let decode = match codec {
            PyCodec::Gzip => gzip_flate_decode,
            PyCodec::Bzip2 => bzip2_decode,
            PyCodec::Lzma => xz_or_lzma_decode,
            PyCodec::Zlib => zlib_decode,
        };
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            decode,
        );
    }
    if let Some(query) = python_module_b64(args) {
        return decode_produced(source_text(
            query.file.as_deref(),
            input,
            depth,
            substitutions,
        ));
    }
    if let Some(query) = python_archive_query(args) {
        return produced_archive(&query.archive, |bytes| match query.kind {
            PyArchive::Tar => tar_text(bytes, query.member.as_deref(), TarCompress::Auto),
            PyArchive::Zip => {
                if query.member.is_none() {
                    unzip_first(bytes)
                } else {
                    unzip_text(bytes, query.member.as_deref())
                }
            }
        });
    }
    let Some(file) = python_cat_query(args) else {
        return Run::Unknown;
    };
    let produced = source_text(file.as_deref(), input, depth, substitutions);
    if python_b64_script(args) {
        decode_produced(produced)
    } else {
        produced
    }
}

enum PyArchive {
    Tar,
    Zip,
}

struct PyArchiveQuery {
    kind: PyArchive,
    archive: String,
    member: Option<String>,
}

fn compact_ws(script: &str) -> String {
    script.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn script_file_text(path: &str) -> Option<String> {
    match load_script_bytes(path)? {
        ScriptBytes::Bytes(bytes) => String::from_utf8(bytes).ok(),
        ScriptBytes::Unscanned => None,
    }
}

fn python_archive_query(args: &[String]) -> Option<PyArchiveQuery> {
    let mut index = 0usize;
    let mut inline = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-c" {
            index += 1;
            if inline.is_some() {
                return None;
            }
            inline = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if matches!(arg, "-u" | "-E" | "-B" | "-s" | "-I") {
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = if let Some(script) = inline {
        script
    } else {
        let path = files.first()?.clone();
        files.remove(0);
        script_file_text(&path)?
    };
    let compact = compact_ws(&script);
    if compact
        == "importsys,zipfile;z=zipfile.ZipFile(sys.argv[1]);sys.stdout.buffer.write(z.read(z.namelist()[0]))"
    {
        if files.len() != 1 {
            return None;
        }
        return Some(PyArchiveQuery {
            kind: PyArchive::Zip,
            archive: files.remove(0),
            member: None,
        });
    }
    let (kind, archive_at, member_at) = match compact.as_str() {
        "importsys,tarfile;sys.stdout.buffer.write(tarfile.open(sys.argv[1]).extractfile(sys.argv[2]).read())" => {
            (PyArchive::Tar, 0usize, 1usize)
        }
        "importsysimporttarfilemode=sys.argv[1]path=sys.argv[2]member=sys.argv[3]withtarfile.open(path,mode)asarchive:extracted=archive.extractfile(member)sys.stdout.buffer.write(extracted.read())" => {
            (PyArchive::Tar, 1, 2)
        }
        "importsysimportzipfilewithzipfile.ZipFile(sys.argv[1])asarchive:sys.stdout.buffer.write(archive.read(sys.argv[2]))" => {
            (PyArchive::Zip, 0, 1)
        }
        "importsys,zipfile;sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))"
        | "importsys,zipfile;sys.stdout.buffer.write(zipfile.Path(sys.argv[1],sys.argv[2]).read_bytes())"
        | "importsys,zipfile;sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).open(sys.argv[2]).read())" => {
            (PyArchive::Zip, 0, 1)
        }
        _ => return None,
    };
    if files.len() != member_at + 1 {
        return None;
    }
    Some(PyArchiveQuery {
        kind,
        archive: files.get(archive_at)?.clone(),
        member: Some(files.get(member_at)?.clone()),
    })
}

fn python_cat_query(args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "-c" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if matches!(arg.as_str(), "-u" | "-E" | "-B" | "-s" | "-I") {
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(arg);
        index += 1;
    }
    let script = script?;
    if !python_reads_argv1(&script) || files.len() != 1 {
        return None;
    }
    Some(files.first().cloned())
}

fn python_reads_argv1(script: &str) -> bool {
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "importpathlib,sys;sys.stdout.write(pathlib.Path(sys.argv[1]).read_text())"
            | "importpathlib,sys;sys.stdout.buffer.write(pathlib.Path(sys.argv[1]).read_bytes())"
            | "importsys;sys.stdout.write(open(sys.argv[1]).read())"
            | "importsys;sys.stdout.buffer.write(open(sys.argv[1],\"rb\").read())"
            | "importsys;sys.stdout.buffer.write(open(sys.argv[1],'rb').read())"
            | "importsys;sys.stdout.write(open(sys.argv[1],\"rb\").read().decode())"
            | "importsys;sys.stdout.write(open(sys.argv[1],'rb').read().decode())"
            | "importsys;print(open(sys.argv[1]).read())"
            | "importsys;sys.stdout.writelines(open(sys.argv[1]))"
            | "importsys;sys.stdout.writelines(open(sys.argv[1],encoding='utf-8'))"
            | "importsys;print(open(sys.argv[1],encoding=\"utf-8\").read())"
            | "importsys;print(open(sys.argv[1],encoding='utf-8').read())"
            | "importsys;sys.stdout.write(open(sys.argv[1],encoding='utf-8').read())"
            | "importsys;print(''.join(open(sys.argv[1]).readlines()))"
            | "frompathlibimportPath;importsys;print(Path(sys.argv[1]).read_text())"
            | "importsys;frompathlibimportPath;print(Path(sys.argv[1]).read_text())"
            | "importsys;frompathlibimportPath;sys.stdout.write(Path(sys.argv[1]).read_text())"
            | "importsys;frompathlibimportPath;sys.stdout.write(Path(sys.argv[1]).read_text(encoding='utf-8'))"
            | "importsys;frompathlibimportPath;print(Path(sys.argv[1]).read_text(encoding='utf-8'))"
            | "importsys;frompathlibimportPath;sys.stdout.buffer.write(Path(sys.argv[1]).read_bytes())"
            | "importsys;print(open(sys.argv[1]).read(),end=\"\")"
            | "importos,sys;os.system(open(sys.argv[1]).read())"
            | "importbase64,sys;sys.stdout.buffer.write(base64.b64decode(open(sys.argv[1],\"rb\").read()))"
            | "importbase64,sys;sys.stdout.buffer.write(base64.b64decode(open(sys.argv[1],'rb').read()))"
            | "importbinascii,sys;sys.stdout.buffer.write(binascii.a2b_base64(open(sys.argv[1],\"rb\").read()))"
            | "importbinascii,sys;sys.stdout.buffer.write(binascii.a2b_base64(open(sys.argv[1],'rb').read()))"
    )
}

fn python_b64_script(args: &[String]) -> bool {
    let Some(script) = flag_script(args) else {
        return false;
    };
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "importbase64,sys;sys.stdout.buffer.write(base64.b64decode(open(sys.argv[1],\"rb\").read()))"
            | "importbase64,sys;sys.stdout.buffer.write(base64.b64decode(open(sys.argv[1],'rb').read()))"
            | "importbinascii,sys;sys.stdout.buffer.write(binascii.a2b_base64(open(sys.argv[1],\"rb\").read()))"
            | "importbinascii,sys;sys.stdout.buffer.write(binascii.a2b_base64(open(sys.argv[1],'rb').read()))"
    )
}

fn node_cat_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    if let Some((kind, path)) = node_gzip_query(args) {
        let decode = match kind {
            NodeInflate::Gzip => gzip_flate_decode,
            NodeInflate::Zlib => zlib_decode,
            NodeInflate::GzipOrZlib => gzip_or_zlib_decode,
            NodeInflate::Brotli => brotli_decode,
            NodeInflate::Raw => deflate_decode,
        };
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            decode,
        );
    }
    let Some(file) = node_cat_query(args) else {
        return Run::Unknown;
    };
    let produced = source_text(file.as_deref(), input, depth, substitutions);
    if node_b64_script(args) {
        decode_produced(produced)
    } else {
        produced
    }
}

fn node_cat_query(args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "-e" || arg == "--eval" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(arg);
        index += 1;
    }
    let script = script?;
    if !node_reads_argv1(&script) || files.len() != 1 {
        return None;
    }
    Some(files.first().cloned())
}

fn node_reads_argv1(script: &str) -> bool {
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "process.stdout.write(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))"
            | "console.log(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))"
            | "constfs=require(\"fs\");process.stdout.write(fs.readFileSync(process.argv[1],\"utf8\"))"
            | "constfs=require('fs');process.stdout.write(fs.readFileSync(process.argv[1]))"
            | "constfs=require('fs');process.stdout.write(fs.readFileSync(process.argv[1],'utf8'))"
            | "constfs=require('fs');console.log(fs.readFileSync(process.argv[1],'utf8'))"
            | "constfs=require('fs');process.stdout.write(fs.readFileSync(process.argv[1],'utf-8'))"
            | "constfs=require(\"fs\");process.stdout.write(fs.readFileSync(process.argv[1],\"utf-8\"))"
            | "process.stdout.write(require('fs').readFileSync(process.argv[1]))"
            | "process.stdout.write(require('fs').readFileSync(process.argv[1],'utf8'))"
            | "const{readFileSync}=require('fs');process.stdout.write(readFileSync(process.argv[1],'utf8'))"
            | "fs=require('fs');process.stdout.write(fs.readFileSync(process.argv[1]))"
            | "constfs=require('fs');process.stdout.write(fs.readFileSync(process.argv[1],{encoding:'utf8'}))"
            | "constfs=require(\"fs\");process.stdout.write(fs.readFileSync(process.argv[1]))"
            | "require(\"child_process\").execSync(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))"
            | "process.stdout.write(Buffer.from(require(\"fs\").readFileSync(process.argv[1]).toString(),\"base64\"))"
    )
}

fn node_b64_script(args: &[String]) -> bool {
    let Some(script) = flag_script(args) else {
        return false;
    };
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    compact
        == "process.stdout.write(Buffer.from(require(\"fs\").readFileSync(process.argv[1]).toString(),\"base64\"))"
}

fn php_gzip_query(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-r" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    let compact = compact_ws(&script);
    if files.len() != 1
        || (compact != "echogzdecode(file_get_contents($argv[1]));"
            && compact != "echoimplode(\"\",gzfile($argv[1]));"
            && compact != "echofile_get_contents(\"compress.zlib://\".$argv[1]);"
            && compact != "echostream_get_contents(gzopen($argv[1],\"r\"));"
            && compact != "echogzread(gzopen($argv[1],\"r\"),4096);"
            && compact != "echogzgets(gzopen($argv[1],\"r\"));"
            && compact != "echogzpassthru(gzopen($argv[1],\"r\"));"
            && compact != "echofread(gzopen($argv[1],\"r\"),4096);"
            && compact != "echoreadgzfile($argv[1]);")
    {
        return None;
    }
    Some(files.remove(0))
}

fn php_bz_query(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-r" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    let compact = compact_ws(&script);
    if files.len() != 1
        || (compact != "echobzdecompress(file_get_contents($argv[1]));"
            && compact != "echobzread(bzopen($argv[1],\"r\"));"
            && compact != "echostream_get_contents(bzopen($argv[1],\"r\"));"
            && compact != "echobzread(bzopen($argv[1],\"r\"),4096);"
            && compact != "echofile_get_contents(\"compress.bzip2://\".$argv[1]);")
    {
        return None;
    }
    Some(files.remove(0))
}

fn php_raw_query(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-r" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if files.len() != 1 || compact_ws(&script) != "echogzinflate(file_get_contents($argv[1]));" {
        return None;
    }
    Some(files.remove(0))
}

fn php_unc_query(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-r" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    if files.len() != 1 || compact_ws(&script) != "echogzuncompress(file_get_contents($argv[1]));" {
        return None;
    }
    Some(files.remove(0))
}

fn php_zip_query(args: &[String]) -> Option<(String, String)> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-r" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(args[index].clone());
        index += 1;
    }
    let script = script?;
    let compact = compact_ws(&script);
    if compact == "echofile_get_contents(\"zip://\".$argv[1].\"#\".$argv[2]);" && files.len() == 2 {
        return Some((files.remove(0), files.remove(0)));
    }
    if compact == "echofile_get_contents(\"zip://\".$argv[1].\"#git.txt\");" && files.len() == 1 {
        return Some((files.remove(0), "git.txt".to_string()));
    }
    None
}

fn php_cat_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    if let Some(path) = php_gzip_query(args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            gzip_flate_decode,
        );
    }
    if let Some(path) = php_bz_query(args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            bzip2_decode,
        );
    }
    if let Some(path) = php_raw_query(args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            deflate_decode,
        );
    }
    if let Some(path) = php_unc_query(args) {
        return decoded_bytes_run(
            Some(GzipQuery { file: Some(path) }),
            input,
            depth,
            substitutions,
            zlib_decode,
        );
    }
    if let Some((archive, member)) = php_zip_query(args) {
        return produced_archive(&archive, |bytes| unzip_text(bytes, Some(&member)));
    }
    let Some(file) = php_cat_query(args) else {
        return Run::Unknown;
    };
    let produced = limit_php_plain(
        args,
        source_text(file.as_deref(), input, depth, substitutions),
    );
    if php_b64_script(args) {
        decode_produced(produced)
    } else {
        produced
    }
}

/// `fgets` prints one line. `fread` of length 4096 prints that many bytes.
fn limit_php_plain(args: &[String], produced: Run) -> Run {
    let Run::Produced { text, ok } = produced else {
        return produced;
    };
    let Some(script) = php_r_script(args) else {
        return Run::Produced { text, ok };
    };
    let text = match compact_ws(script).as_str() {
        "echofgets(fopen($argv[1],\"r\"));" => first_lines(&text, 1),
        "echofread(fopen($argv[1],\"r\"),4096);" => first_bytes(&text, 4096),
        _ => text,
    };
    Run::Produced { text, ok }
}

fn php_r_script(args: &[String]) -> Option<&str> {
    let mut index = 0usize;
    while index < args.len() {
        if args[index] == "-r" {
            return args.get(index + 1).map(String::as_str);
        }
        index += 1;
    }
    None
}

fn php_cat_query(args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut script = None;
    let mut files = Vec::new();
    while index < args.len() {
        let arg = args[index].clone();
        if arg == "-r" {
            index += 1;
            if script.is_some() {
                return None;
            }
            script = Some(args.get(index)?.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            return None;
        }
        files.push(arg);
        index += 1;
    }
    let script = script?;
    if !php_reads_argv1(&script) || files.len() != 1 {
        return None;
    }
    Some(files.first().cloned())
}

fn php_reads_argv1(script: &str) -> bool {
    let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "echofile_get_contents($argv[1]);"
            | "printfile_get_contents($argv[1]);"
            | "readfile($argv[1]);"
            | "echoreadfile($argv[1]);"
            | "system(file_get_contents($argv[1]));"
            | "echoimplode(\"\",file($argv[1]));"
            | "printimplode(\"\",file($argv[1]));"
            | "echofread(fopen($argv[1],\"r\"),4096);"
            | "echostream_get_contents(fopen($argv[1],\"r\"));"
            | "echofgets(fopen($argv[1],\"r\"));"
            | "foreach(file($argv[1])as$line){echo$line;}"
            | "echofile_get_contents($argv[1],false);"
            | "echofile_get_contents($argv[1],true);"
            | "echobase64_decode(file_get_contents($argv[1]));"
    )
}

fn php_b64_script(args: &[String]) -> bool {
    let mut index = 0usize;
    while index < args.len() {
        if args[index] == "-r" {
            let Some(script) = args.get(index + 1) else {
                return false;
            };
            let compact: String = script.chars().filter(|ch| !ch.is_whitespace()).collect();
            return compact == "echobase64_decode(file_get_contents($argv[1]));";
        }
        index += 1;
    }
    false
}

enum BasencKind {
    Base64,
    Base64Url,
    Base16,
    Base32,
    Base32Hex,
    Base2Msb,
    Base2Lsb,
    Z85,
    Base58,
}

struct BasencQuery {
    file: Option<String>,
    kind: BasencKind,
}

fn base64url_decode(text: &str) -> Option<String> {
    let mapped: String = text
        .chars()
        .map(|ch| match ch {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    b64_decode(&mapped)
}

fn base32hex_decode(text: &str) -> Option<String> {
    const FROM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    const TO: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut mapped = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'=' {
            mapped.push('=');
            continue;
        }
        let pos = FROM
            .iter()
            .position(|ch| *ch == byte.to_ascii_uppercase())?;
        mapped.push(TO[pos] as char);
    }
    b32_decode(&mapped)
}

fn base2_decode(text: &str, msb_first: bool) -> Option<String> {
    let bits: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if bits.is_empty() || !bits.len().is_multiple_of(8) {
        return None;
    }
    let mut out = Vec::new();
    for chunk in bits.chunks_exact(8) {
        let mut value = 0u8;
        for (index, bit) in chunk.iter().enumerate() {
            let one = match bit {
                b'1' => 1u8,
                b'0' => 0,
                _ => return None,
            };
            if msb_first {
                value |= one << (7 - index);
            } else {
                value |= one << index;
            }
        }
        out.push(value);
    }
    String::from_utf8(out).ok()
}

fn z85_decode(text: &str) -> Option<String> {
    const ALPHA: &[u8] =
        b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";
    let mut digits = Vec::new();
    for byte in text.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let pos = ALPHA.iter().position(|ch| *ch == byte)? as u64;
        digits.push(pos);
    }
    if digits.is_empty() || !digits.len().is_multiple_of(5) {
        return None;
    }
    let mut out = Vec::new();
    for chunk in digits.chunks_exact(5) {
        let mut value = 0u64;
        for digit in chunk {
            value = value * 85 + digit;
        }
        if value > u32::MAX as u64 {
            return None;
        }
        out.push((value >> 24) as u8);
        out.push((value >> 16) as u8);
        out.push((value >> 8) as u8);
        out.push(value as u8);
    }
    String::from_utf8(out).ok()
}

fn basenc_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = basenc_query(args) else {
        return Run::Unknown;
    };
    let raw = match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        Run::Produced { text, ok: true } => text,
        other => return other,
    };
    let text = match query.kind {
        BasencKind::Base64 => b64_decode(&raw),
        BasencKind::Base64Url => base64url_decode(&raw),
        BasencKind::Base16 => hex_decode(&raw),
        BasencKind::Base32 => b32_decode(&raw),
        BasencKind::Base32Hex => base32hex_decode(&raw),
        BasencKind::Base2Msb => base2_decode(&raw, true),
        BasencKind::Base2Lsb => base2_decode(&raw, false),
        BasencKind::Z85 => z85_decode(&raw),
        BasencKind::Base58 => base58_decode(&raw),
    };
    let Some(text) = text else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

fn base58_decode(text: &str) -> Option<String> {
    const ALPHA: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut zeros = 0usize;
    let mut digits = Vec::new();
    let mut number = false;
    for byte in text.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if !number && byte == b'1' {
            zeros += 1;
            continue;
        }
        number = true;
        let pos = ALPHA.iter().position(|ch| *ch == byte)?;
        digits.push(pos as u32);
    }
    let mut acc = Vec::new();
    for digit in digits {
        let mut carry = digit;
        for byte in acc.iter_mut().rev() {
            let current = u32::from(*byte) * 58 + carry;
            *byte = (current & 0xff) as u8;
            carry = current >> 8;
        }
        while carry > 0 {
            acc.insert(0, (carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    let mut out = vec![0u8; zeros];
    out.extend(acc);
    String::from_utf8(out).ok()
}

fn basenc_query(args: &[String]) -> Option<BasencQuery> {
    let mut index = 0usize;
    let mut decode = false;
    let mut kind = None;
    let mut file = None;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if matches!(arg.as_str(), "-d" | "--decode") {
            decode = true;
            index += 1;
            continue;
        }
        if arg == "-i" || arg == "--ignore-garbage" {
            index += 1;
            continue;
        }
        if arg == "--wrap" {
            let next = args.get(index + 1)?;
            if next.is_empty() || !next.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            index += 2;
            continue;
        }
        if let Some(cols) = arg.strip_prefix("--wrap=") {
            if cols.is_empty() || !cols.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            index += 1;
            continue;
        }
        if let Some(cols) = arg.strip_prefix("-w") {
            if cols.is_empty() {
                let next = args.get(index + 1)?;
                if next.is_empty() || !next.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                index += 2;
            } else if !cols.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            } else {
                index += 1;
            }
            continue;
        }
        let next_kind = match arg.as_str() {
            "--base64" => Some(BasencKind::Base64),
            "--base64url" => Some(BasencKind::Base64Url),
            "--base16" => Some(BasencKind::Base16),
            "--base32" => Some(BasencKind::Base32),
            "--base32hex" => Some(BasencKind::Base32Hex),
            "--base2msbf" => Some(BasencKind::Base2Msb),
            "--base2lsbf" => Some(BasencKind::Base2Lsb),
            "--z85" => Some(BasencKind::Z85),
            "--base58" => Some(BasencKind::Base58),
            _ => None,
        };
        if let Some(next) = next_kind {
            if kind.is_some() {
                return None;
            }
            kind = Some(next);
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        if file.is_some() {
            return None;
        }
        file = Some(arg.clone());
        index += 1;
    }
    let kind = kind?;
    if !decode || args.len() - index > 1 {
        return None;
    }
    if file.is_none() {
        file = args.get(index).cloned();
    } else if index < args.len() {
        return None;
    }
    Some(BasencQuery { file, kind })
}

fn base64_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = base64_query(args) else {
        return Run::Unknown;
    };
    let raw = match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        Run::Produced { text, ok: true } => text,
        other => return other,
    };
    let Some(text) = b64_decode(&raw) else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

fn uudecode_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(file) = uudecode_query(args) else {
        return Run::Unknown;
    };
    let raw = match source_text(file.as_deref(), input, depth, substitutions) {
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        Run::Produced { text, ok: true } => text,
        other => return other,
    };
    let Some(text) = uu_decode(&raw) else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

fn uudecode_query(args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut to_stdout = false;
    let mut file = None;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-p" {
            to_stdout = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        if file.is_some() {
            return None;
        }
        file = Some(args[index].clone());
        index += 1;
    }
    if !to_stdout {
        return None;
    }
    Some(file)
}

fn xxd_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(file) = xxd_query(args) else {
        return Run::Unknown;
    };
    let raw = match source_text(file.as_deref(), input, depth, substitutions) {
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        Run::Produced { text, ok: true } => text,
        other => return other,
    };
    let Some(text) = hex_decode(&raw) else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

fn xxd_query(args: &[String]) -> Option<Option<String>> {
    let mut index = 0usize;
    let mut reverse = false;
    let mut plain = false;
    let mut file = None;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-r" || arg == "--reverse" {
            reverse = true;
            index += 1;
            continue;
        }
        if matches!(arg, "-p" | "-ps" | "--postscript") {
            plain = true;
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        if file.is_some() {
            return None;
        }
        file = Some(args[index].clone());
        index += 1;
    }
    if !reverse || !plain {
        return None;
    }
    Some(file)
}

fn hex_decode(text: &str) -> Option<String> {
    let mut nibbles = Vec::new();
    for byte in text.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        nibbles.push(value);
    }
    if !nibbles.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::new();
    for pair in nibbles.chunks_exact(2) {
        out.push((pair[0] << 4) | pair[1]);
    }
    String::from_utf8(out).ok()
}

fn uu_value(byte: u8) -> u8 {
    byte.wrapping_sub(32) & 0x3f
}

fn uu_decode(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let header = lines.next()?;
    if !header.starts_with("begin ") {
        return None;
    }
    let mut out = Vec::new();
    for line in lines {
        if line == "end" || line.starts_with("end") {
            break;
        }
        let bytes = line.as_bytes();
        if bytes.is_empty() {
            continue;
        }
        let len = uu_value(bytes[0]) as usize;
        if len == 0 {
            continue;
        }
        let mut data = bytes[1..].to_vec();
        while data.len() % 4 != 0 {
            data.push(b' ');
        }
        let mut decoded = Vec::new();
        for chunk in data.chunks_exact(4) {
            let a = uu_value(chunk[0]);
            let b = uu_value(chunk[1]);
            let c = uu_value(chunk[2]);
            let d = uu_value(chunk[3]);
            decoded.push((a << 2) | (b >> 4));
            decoded.push((b << 4) | (c >> 2));
            decoded.push((c << 6) | d);
        }
        if decoded.len() < len {
            return None;
        }
        out.extend_from_slice(&decoded[..len]);
    }
    String::from_utf8(out).ok()
}

fn base64_query(args: &[String]) -> Option<Base64Query> {
    let mut index = 0usize;
    let mut decode = false;
    let mut file = None;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if matches!(arg.as_str(), "-d" | "-D" | "--decode") {
            decode = true;
            index += 1;
            continue;
        }
        if arg == "-i" || arg == "--ignore-garbage" {
            index += 1;
            continue;
        }
        if arg == "-w" || arg == "--wrap" {
            let next = args.get(index + 1)?;
            if next.is_empty() || !next.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            index += 2;
            continue;
        }
        if let Some(cols) = arg.strip_prefix("--wrap=") {
            if cols.is_empty() || !cols.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            index += 1;
            continue;
        }
        if let Some(cols) = arg.strip_prefix("-w") {
            if !cols.is_empty() && cols.bytes().all(|byte| byte.is_ascii_digit()) {
                index += 1;
                continue;
            }
            return None;
        }
        match apply_coreutils_short(args, &mut index, &mut decode, true) {
            None => return None,
            Some(true) => continue,
            Some(false) => {}
        }
        if arg.starts_with('-') {
            return None;
        }
        break;
    }
    if !decode || args.len() - index > 1 {
        return None;
    }
    if file.is_none() {
        file = args.get(index).cloned();
    } else if index < args.len() {
        return None;
    }
    Some(Base64Query { file })
}

struct OpensslDecode {
    file: Option<String>,
}

fn openssl_b64_run(
    args: &[String],
    input: Option<&str>,
    depth: u32,
    substitutions: &mut Vec<String>,
) -> Run {
    let Some(query) = openssl_b64_query(args) else {
        return Run::Unknown;
    };
    let raw = match source_text(query.file.as_deref(), input, depth, substitutions) {
        Run::Produced { ok: false, .. } => {
            return Run::Produced {
                text: String::new(),
                ok: false,
            };
        }
        Run::Produced { text, ok: true } => text,
        other => return other,
    };
    let Some(text) = b64_decode(&raw) else {
        return Run::Unknown;
    };
    Run::Produced { text, ok: true }
}

fn openssl_b64_query(args: &[String]) -> Option<OpensslDecode> {
    let sub = args.first().map(String::as_str)?;
    if sub != "base64" && sub != "enc" {
        return None;
    }
    let mut index = 1usize;
    let mut decode = false;
    let mut base64_mode = sub == "base64";
    let mut file = None;
    while index < args.len() {
        let arg = args[index].as_str();
        if matches!(arg, "-d" | "-D" | "--decode") {
            decode = true;
            index += 1;
            continue;
        }
        if matches!(arg, "-a" | "-base64" | "--base64") {
            base64_mode = true;
            index += 1;
            continue;
        }
        if arg == "-A" {
            index += 1;
            continue;
        }
        if arg == "-in" {
            if file.is_some() {
                return None;
            }
            file = Some(args.get(index + 1)?.clone());
            index += 2;
            continue;
        }
        return None;
    }
    if !decode || !base64_mode {
        return None;
    }
    Some(OpensslDecode { file })
}

fn b64_decode(text: &str) -> Option<String> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let filtered: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if filtered.is_empty() {
        return Some(String::new());
    }
    if !filtered.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::new();
    for chunk in filtered.chunks(4) {
        let mut parts = [0u8; 4];
        let mut pad = 0u8;
        for (slot, byte) in chunk.iter().copied().enumerate() {
            if byte == b'=' {
                if slot < 2 {
                    return None;
                }
                pad += 1;
            } else if pad > 0 {
                return None;
            } else {
                parts[slot] = value(byte)?;
            }
        }
        let packed = ((parts[0] as u32) << 18)
            | ((parts[1] as u32) << 12)
            | ((parts[2] as u32) << 6)
            | (parts[3] as u32);
        out.push((packed >> 16) as u8);
        if pad < 2 {
            out.push((packed >> 8) as u8);
        }
        if pad < 1 {
            out.push(packed as u8);
        }
    }
    String::from_utf8(out).ok()
}

enum StdinFor {
    Ready(Option<String>),
    Failed,
    Unscanned,
    Unknown,
}

enum InSrc {
    Text(String),
    Unscanned,
    Error,
    Eof,
    Hang,
}

fn stdin_for(words: &[String], redirect: Option<InSrc>, pipe: Option<&str>) -> StdinFor {
    if !builtin_reads_stdin(words) {
        return StdinFor::Ready(None);
    }
    if let Some(source) = redirect {
        return match source {
            InSrc::Text(text) => StdinFor::Ready(Some(text)),
            InSrc::Unscanned => StdinFor::Unscanned,
            InSrc::Error => StdinFor::Failed,
            InSrc::Eof => StdinFor::Ready(Some(String::new())),
            InSrc::Hang => StdinFor::Unknown,
        };
    }
    match pipe {
        Some(text) => StdinFor::Ready(Some(text.to_string())),
        // `xargs cat file` still runs `cat file` when stdin is empty.
        // GNU xargs does that unless `-r` is set. A missing pipe is that EOF.
        None if stage_command_name(&words[0]) == "xargs" => StdinFor::Ready(Some(String::new())),
        None => StdinFor::Unknown,
    }
}

fn builtin_reads_stdin(words: &[String]) -> bool {
    if words.is_empty() {
        return false;
    }
    let name = stage_command_name(&words[0]);
    let args = &words[1..];
    match name.as_str() {
        "cat" => {
            cat_operands(args).is_none_or(|ops| ops.is_empty() || ops.iter().any(|op| op == "-"))
        }
        "stdbuf" => stdbuf_command(args).is_some_and(builtin_reads_stdin),
        "xargs" => true,
        "tee" | "tr" => true,
        "base32" => base32_query(args).is_none_or(|query| query.file.is_none()),
        "dd" => dd_query(args).is_none_or(|query| query.is_none()),
        "perl" | "ruby" => {
            if perl_tar_query(&words[0], args).is_some()
                || perl_gzip_query(&words[0], args).is_some()
                || perl_bzip_query(&words[0], args).is_some()
                || perl_raw_query(&words[0], args).is_some()
                || perl_any_query(&words[0], args).is_some()
                || perl_zip_query(&words[0], args).is_some()
                || ruby_gzip_query(&words[0], args).is_some()
            {
                false
            } else {
                line_print_query(&words[0], args).is_none_or(|query| query.is_none())
            }
        }
        "python" | "python3" => match python_module_b64(args) {
            Some(query) => query.file.is_none(),
            None => {
                if python_archive_query(args).is_some() || python_codec_query(args).is_some() {
                    false
                } else {
                    python_cat_query(args).is_none_or(|query| query.is_none())
                }
            }
        },
        "node" => {
            if node_gzip_query(args).is_some() {
                false
            } else {
                node_cat_query(args).is_none_or(|query| query.is_none())
            }
        }
        "php" => {
            if php_gzip_query(args).is_some()
                || php_bz_query(args).is_some()
                || php_raw_query(args).is_some()
                || php_unc_query(args).is_some()
                || php_zip_query(args).is_some()
            {
                false
            } else {
                php_cat_query(args).is_none_or(|query| query.is_none())
            }
        }
        "iconv" => iconv_query(args).is_none_or(|query| query.is_none()),
        "join" => join_query(args).is_none_or(|files| files.iter().any(|file| file == "-")),
        "comm" => comm_query(args).is_none_or(|(_, _, _, left, right)| left == "-" || right == "-"),
        "grep" | "egrep" | "fgrep" | "ggrep" => {
            grep_query(args).is_none_or(|query| query.file.is_none())
        }
        "head" => head_query(args)
            .is_none_or(|query| query.file.as_deref().is_none_or(|file| file == "-")),
        "tail" => tail_query(args)
            .is_none_or(|query| query.file.as_deref().is_none_or(|file| file == "-")),
        "sed" | "gsed" => sed_query(args).is_none_or(|query| query.file.is_none()),
        "awk" | "gawk" => awk_query(args).is_none_or(|query| query.file.is_none()),
        "jq" => jq_query(args).is_none_or(|file| file.is_none()),
        "cut" => cut_query(args).is_none_or(|query| query.file.is_none()),
        "sort" => sort_query(args).is_none_or(|query| {
            query.from0.as_deref() == Some("-")
                || (query.from0.is_none()
                    && (query.files.is_empty() || query.files.iter().any(|file| file == "-")))
        }),
        "uniq" => uniq_query(args).is_none_or(|query| query.file.is_none()),
        "tac" => tac_query(args).is_none_or(|query| {
            query.files.is_empty() || query.files.iter().any(|file| file == "-")
        }),
        "base64" => base64_query(args).is_none_or(|query| query.file.is_none()),
        "basenc" => basenc_query(args).is_some_and(|query| query.file.is_none()),
        "uudecode" => uudecode_query(args).is_some_and(|file| file.is_none()),
        "xxd" => xxd_query(args).is_some_and(|file| file.is_none()),
        "openssl" => openssl_b64_query(args).is_some_and(|query| query.file.is_none()),
        "gzip" | "gunzip" | "gzcat" | "zcat" => {
            gzip_query(&name, args).is_some_and(|query| query.file.is_none())
        }
        "bzip2" | "bunzip2" | "bzcat" => {
            bzip2_query(&name, args).is_some_and(|query| query.file.is_none())
        }
        "xz" | "unxz" | "xzcat" | "lzma" | "unlzma" | "lzcat" => {
            xz_query(&name, args).is_some_and(|query| query.file.is_none())
        }
        "zstd" | "unzstd" | "zstdcat" | "pzstd" => {
            zstd_query(&name, args).is_some_and(|query| query.file.is_none())
        }
        "tar" => tar_query(args).is_some_and(|query| query.archive == "-"),
        "unzip" => unzip_query(args).is_some_and(|query| query.archive == "-"),
        "7z" | "7za" | "7zz" => sevenz_query(args).is_some_and(|query| query.archive == "-"),
        "lz4" | "unlz4" | "lz4cat" => {
            lz4_query(&name, args).is_some_and(|query| query.file.is_none())
        }
        "brotli" => brotli_query(&name, args).is_some_and(|query| query.file.is_none()),
        "uncompress" => uncompress_query(args).is_some_and(|query| query.file.is_none()),
        "paste" | "expand" | "unexpand" | "column" | "fold" | "fmt" | "pr" | "strings" => {
            identity_query(&words[0], args).is_none_or(|query| query.is_none())
        }
        _ => false,
    }
}

struct Peeled {
    command: String,
    stdin: Option<InSrc>,
    stdout_lost: bool,
    cancelled: bool,
    blocked: bool,
}

fn peel_redirects(stage: &str) -> Option<Peeled> {
    let chars: Vec<char> = stage.chars().collect();
    let mut kept = String::new();
    let mut index = 0usize;
    let mut quote: Option<char> = None;
    let mut word_start = true;
    let mut fd_digits: Option<String> = Some(String::new());
    let mut state = RedirState {
        stdin: None,
        saw_stdin: false,
        stdout_lost: false,
        cancelled: false,
        blocked: false,
    };
    while index < chars.len() {
        let ch = chars[index];
        if let Some(open) = quote {
            fd_digits = None;
            if open == '"' && ch == '\\' {
                kept.push(ch);
                index += 1;
                if index < chars.len() {
                    kept.push(chars[index]);
                    index += 1;
                }
                continue;
            }
            kept.push(ch);
            if ch == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if ch == '#' && word_start {
            break;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                word_start = false;
                fd_digits = None;
                kept.push(ch);
                index += 1;
            }
            '\\' => {
                word_start = false;
                fd_digits = None;
                kept.push(ch);
                index += 1;
                if index < chars.len() {
                    kept.push(chars[index]);
                    index += 1;
                }
            }
            ' ' | '\t' | '\n' | '\r' => {
                word_start = true;
                fd_digits = Some(String::new());
                kept.push(ch);
                index += 1;
            }
            '<' | '>' | '&' => {
                if matches!(ch, '<' | '>') && chars.get(index + 1) == Some(&'(') {
                    let end = take_proc_subst(&chars, index)?.1;
                    kept.extend(chars[index..end].iter());
                    index = end;
                    word_start = false;
                    fd_digits = None;
                    continue;
                }
                let fd = take_fd(&mut fd_digits, &mut kept)?;
                apply_redirect(&chars, &mut index, fd, &mut state)?;
                word_start = true;
                fd_digits = Some(String::new());
            }
            _ => {
                word_start = false;
                if let Some(digits) = fd_digits.as_mut() {
                    if ch.is_ascii_digit() {
                        digits.push(ch);
                    } else {
                        fd_digits = None;
                    }
                }
                kept.push(ch);
                index += 1;
            }
        }
    }
    Some(Peeled {
        command: kept,
        stdin: state.stdin,
        stdout_lost: state.stdout_lost,
        cancelled: state.cancelled,
        blocked: state.blocked,
    })
}

struct RedirState {
    stdin: Option<InSrc>,
    saw_stdin: bool,
    stdout_lost: bool,
    cancelled: bool,
    blocked: bool,
}

fn take_fd(fd_digits: &mut Option<String>, kept: &mut String) -> Option<Option<i32>> {
    let Some(digits) = fd_digits.as_ref() else {
        return Some(None);
    };
    if digits.is_empty() {
        return Some(None);
    }
    let fd = digits.parse().ok()?;
    kept.truncate(kept.len().saturating_sub(digits.len()));
    Some(Some(fd))
}

fn apply_redirect(
    chars: &[char],
    index: &mut usize,
    fd: Option<i32>,
    state: &mut RedirState,
) -> Option<()> {
    match chars.get(*index)? {
        '&' => {
            if chars.get(*index + 1) != Some(&'>') {
                return None;
            }
            *index += 2;
            if chars.get(*index) == Some(&'>') {
                *index += 1;
            }
            let target = redirect_word(chars, index)?;
            note_output(&target, true, state);
            Some(())
        }
        '<' => {
            if matches!(chars.get(*index + 1), Some('<' | '(' | '&' | '>')) {
                return None;
            }
            *index += 1;
            let target = redirect_word(chars, index)?;
            if fd.unwrap_or(0) == 0 {
                if state.saw_stdin {
                    return None;
                }
                state.saw_stdin = true;
                match classify_input(&target) {
                    InputClass::Cancel => state.cancelled = true,
                    InputClass::Block => state.blocked = true,
                    InputClass::Src(source) => state.stdin = Some(source),
                }
            } else {
                note_read_open(&target, state);
            }
            Some(())
        }
        '>' => {
            *index += 1;
            if chars.get(*index) == Some(&'>') || chars.get(*index) == Some(&'|') {
                *index += 1;
                let target = redirect_word(chars, index)?;
                note_output(&target, fd.unwrap_or(1) == 1, state);
                return Some(());
            }
            if chars.get(*index) == Some(&'&') {
                *index += 1;
                let target = redirect_word(chars, index)?;
                if !target.chars().all(|ch| ch.is_ascii_digit()) {
                    return None;
                }
                let dest: i32 = target.parse().ok()?;
                if fd.unwrap_or(1) == 1 && dest != 1 {
                    state.stdout_lost = true;
                }
                return Some(());
            }
            let target = redirect_word(chars, index)?;
            note_output(&target, fd.unwrap_or(1) == 1, state);
            Some(())
        }
        _ => None,
    }
}

fn redirect_word(chars: &[char], index: &mut usize) -> Option<String> {
    while matches!(chars.get(*index), Some(' ' | '\t')) {
        *index += 1;
    }
    let start = *index;
    while *index < chars.len() {
        let ch = chars[*index];
        if matches!(
            ch,
            ' ' | '\t' | '\n' | '\r' | '|' | '&' | '<' | '>' | ';' | '#'
        ) {
            break;
        }
        if matches!(
            ch,
            '\'' | '"' | '\\' | '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '~' | '(' | ')'
        ) {
            return None;
        }
        *index += 1;
    }
    if *index == start {
        return None;
    }
    Some(chars[start..*index].iter().collect())
}

enum InputClass {
    Cancel,
    Block,
    Src(InSrc),
}

fn classify_input(path: &str) -> InputClass {
    match path_body(path) {
        PathBody::Missing => InputClass::Cancel,
        PathBody::Fifo => InputClass::Block,
        PathBody::Dir => InputClass::Src(InSrc::Error),
        PathBody::Text(text) => InputClass::Src(InSrc::Text(text)),
        PathBody::Unscanned => InputClass::Src(InSrc::Unscanned),
        PathBody::Null => InputClass::Src(InSrc::Eof),
        PathBody::Hang => InputClass::Src(InSrc::Hang),
    }
}

fn note_output(path: &str, lose_stdout: bool, state: &mut RedirState) {
    match output_open(path) {
        OpenCheck::Ok => {
            if lose_stdout {
                state.stdout_lost = true;
            }
        }
        OpenCheck::Fail => state.cancelled = true,
        OpenCheck::Block => state.blocked = true,
    }
}

fn note_read_open(path: &str, state: &mut RedirState) {
    match read_open(path) {
        OpenCheck::Ok => {}
        OpenCheck::Fail => state.cancelled = true,
        OpenCheck::Block => state.blocked = true,
    }
}

enum OpenCheck {
    Ok,
    Fail,
    Block,
}

fn output_open(path: &str) -> OpenCheck {
    match std::fs::metadata(path) {
        Ok(meta) if metadata_is_fifo(&meta) => OpenCheck::Block,
        Ok(meta) if meta.is_dir() => OpenCheck::Fail,
        Ok(_) => OpenCheck::Ok,
        Err(_) => {
            if parent_is_dir(path) {
                OpenCheck::Ok
            } else {
                OpenCheck::Fail
            }
        }
    }
}

fn read_open(path: &str) -> OpenCheck {
    match std::fs::metadata(path) {
        Err(_) => OpenCheck::Fail,
        Ok(meta) if metadata_is_fifo(&meta) => OpenCheck::Block,
        Ok(_) => OpenCheck::Ok,
    }
}

fn parent_is_dir(path: &str) -> bool {
    match std::path::Path::new(path).parent() {
        None => false,
        Some(parent) if parent.as_os_str().is_empty() => true,
        Some(parent) => parent.is_dir(),
    }
}

enum PathBody {
    Text(String),
    Unscanned,
    Missing,
    Dir,
    Fifo,
    Null,
    Hang,
}

fn path_body(path: &str) -> PathBody {
    let owned = openable_path(path);
    let path = owned.as_ref();
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return PathBody::Missing,
    };
    if meta.is_dir() {
        return PathBody::Dir;
    }
    if metadata_is_fifo(&meta) {
        return PathBody::Fifo;
    }
    if meta.is_file() {
        return match load_script_file(path) {
            Some(ScriptFile::Text(text)) => PathBody::Text(text),
            Some(ScriptFile::Unscanned) => PathBody::Unscanned,
            None => PathBody::Missing,
        };
    }
    match read_immediate(path) {
        Immediate::Eof => PathBody::Null,
        Immediate::Data | Immediate::Block => PathBody::Hang,
    }
}

enum Immediate {
    Eof,
    Data,
    Block,
}

fn read_immediate(path: &str) -> Immediate {
    let Some(mut file) = open_script_candidate(path) else {
        return Immediate::Block;
    };
    let mut buf = [0u8; 1];
    match std::io::Read::read(&mut file, &mut buf) {
        Ok(0) => Immediate::Eof,
        Ok(_) => Immediate::Data,
        Err(_) => Immediate::Block,
    }
}

fn metadata_is_fifo(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        meta.file_type().is_fifo()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

fn split_list(body: &str) -> Option<Vec<Piece>> {
    let chars: Vec<char> = body.chars().collect();
    let mut pieces = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut quote: Option<char> = None;
    let mut word_start = true;
    while index < chars.len() {
        let ch = chars[index];
        if let Some(open) = quote {
            if open == '"' && ch == '\\' {
                index += 1;
                if index < chars.len() {
                    index += 1;
                }
                continue;
            }
            if ch == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                word_start = false;
                index += 1;
            }
            '\\' => {
                word_start = false;
                index += 1;
                if index < chars.len() {
                    index += 1;
                }
            }
            ' ' | '\t' | '\r' => {
                word_start = true;
                index += 1;
            }
            '#' if word_start => {
                index += 1;
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            ';' | '\n' => {
                let command = chars[start..index].iter().collect();
                push_piece(&mut pieces, command, Join::Semi)?;
                index += 1;
                start = index;
                word_start = true;
            }
            '|' if chars.get(index + 1) == Some(&'|') => {
                let command = chars[start..index].iter().collect();
                push_piece(&mut pieces, command, Join::Or)?;
                index += 2;
                start = index;
                word_start = true;
            }
            '|' if chars.get(index + 1) == Some(&'&') => return None,
            '|' => {
                word_start = true;
                index += 1;
            }
            '&' if chars.get(index + 1) == Some(&'&') => {
                let command = chars[start..index].iter().collect();
                push_piece(&mut pieces, command, Join::And)?;
                index += 2;
                start = index;
                word_start = true;
            }
            '&' if chars.get(index + 1) == Some(&'>') => {
                word_start = false;
                index += 1;
            }
            '&' => return None,
            '$' if chars.get(index + 1) == Some(&'(') => {
                word_start = false;
                index = skip_command_subst(&chars, index)?;
            }
            '`' => {
                word_start = false;
                index = skip_backtick(&chars, index)?;
            }
            '<' | '>' if chars.get(index + 1) == Some(&'(') => {
                word_start = false;
                index = take_proc_subst(&chars, index)?.1;
            }
            '(' | ')' | '*' | '?' | '[' | ']' | '{' | '}' | '~' => return None,
            _ => {
                word_start = false;
                index += 1;
            }
        }
    }
    let command = chars[start..].iter().collect();
    push_piece(&mut pieces, command, Join::Semi)?;
    Some(pieces)
}

fn push_piece(pieces: &mut Vec<Piece>, command: String, next: Join) -> Option<()> {
    let blank = command.trim_matches([' ', '\t', '\r']).is_empty();
    let prev_logical = pieces
        .last()
        .is_some_and(|piece| matches!(piece.next, Join::And | Join::Or));
    if blank && (prev_logical || matches!(next, Join::And | Join::Or)) {
        return None;
    }
    pieces.push(Piece { command, next });
    Some(())
}

fn skip_command_subst(chars: &[char], start: usize) -> Option<usize> {
    take_command_subst(chars, start).map(|(_, next)| next)
}

fn skip_backtick(chars: &[char], start: usize) -> Option<usize> {
    take_backtick_chars(chars, start).map(|(_, next)| next)
}

/// Body of `$'...'`. `index` starts after the opening quote.
fn take_ansi_c_slice(chars: &[char], index: &mut usize) -> Option<String> {
    let rest = chars.get(*index..)?;
    let total = rest.len();
    let mut out = String::new();
    let mut iter = rest.iter().copied().peekable();
    read_ansi_c(&mut iter, &mut out);
    let unread = iter.count();
    let consumed = total - unread;
    if consumed == 0 || chars.get(*index + consumed - 1) != Some(&'\'') {
        return None;
    }
    *index += consumed;
    Some(out)
}

fn parse_fields(
    command: &str,
    depth: u32,
    substitutions: &mut Vec<String>,
    unscanned: &mut bool,
) -> Option<Vec<String>> {
    let chars: Vec<char> = command.chars().collect();
    let mut index = 0usize;
    let mut words = Vec::new();
    loop {
        while index < chars.len() && matches!(chars[index], ' ' | '\t' | '\n' | '\r') {
            index += 1;
        }
        if index >= chars.len() || chars[index] == '#' {
            break;
        }
        let before = index;
        words.extend(read_word_fields(
            &chars,
            &mut index,
            depth,
            substitutions,
            unscanned,
        )?);
        if index == before {
            return None;
        }
    }
    Some(words)
}

fn read_word_fields(
    chars: &[char],
    index: &mut usize,
    depth: u32,
    substitutions: &mut Vec<String>,
    unscanned: &mut bool,
) -> Option<Vec<String>> {
    let mut fields = vec![String::new()];
    let mut saw = false;
    let mut quoted_word = false;
    while *index < chars.len() {
        let ch = chars[*index];
        if matches!(ch, '<' | '>') && chars.get(*index + 1) == Some(&'(') {
            let (body, next) = take_proc_subst(chars, *index)?;
            let mut word = String::new();
            word.push(ch);
            word.push('(');
            word.push_str(&body);
            word.push(')');
            push_field(&mut fields, &word);
            *index = next;
            saw = true;
            continue;
        }
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | ';' | '|' | '&' | '<' | '>') {
            break;
        }
        if ch == '#' && !saw {
            break;
        }
        match ch {
            '\'' => {
                *index += 1;
                push_field(&mut fields, &take_single(chars, index)?);
                saw = true;
                quoted_word = true;
            }
            '"' => {
                *index += 1;
                push_field(
                    &mut fields,
                    &take_double(chars, index, depth, substitutions, unscanned)?,
                );
                saw = true;
                quoted_word = true;
            }
            '$' if chars.get(*index + 1) == Some(&'(') => {
                let (body, next) = take_command_subst(chars, *index)?;
                *index = next;
                splice_fields(
                    &mut fields,
                    &nested_printed(&body, depth + 1, substitutions, unscanned)?,
                );
                saw = true;
            }
            // `$'\\n'` is one quoted argument. A bare `$` still stops the parse.
            '$' if chars.get(*index + 1) == Some(&'\'') => {
                *index += 2;
                push_field(&mut fields, &take_ansi_c_slice(chars, index)?);
                saw = true;
                quoted_word = true;
            }
            '`' => {
                let (body, next) = take_backtick_chars(chars, *index)?;
                *index = next;
                splice_fields(
                    &mut fields,
                    &nested_printed(&body, depth + 1, substitutions, unscanned)?,
                );
                saw = true;
            }
            '\\' => {
                *index += 1;
                let current = fields.last().map(String::as_str).unwrap_or("");
                if drive_separator(current)
                    && *index < chars.len()
                    && is_drive_path_char(chars[*index])
                {
                    let next = chars[*index];
                    *index += 1;
                    push_field(&mut fields, "\\");
                    push_field(&mut fields, &next.to_string());
                } else if *index < chars.len() {
                    let next = chars[*index];
                    *index += 1;
                    if next != '\n' {
                        push_field(&mut fields, &next.to_string());
                    }
                }
                saw = true;
            }
            '*' | '?' | '[' | ']' | '{' | '}' | '~' | '(' | ')' | '$' => return None,
            _ => {
                push_field(&mut fields, &ch.to_string());
                *index += 1;
                saw = true;
            }
        }
    }
    if fields.len() == 1 && fields[0].is_empty() && !quoted_word {
        return Some(Vec::new());
    }
    Some(fields)
}

fn push_field(fields: &mut [String], text: &str) {
    if let Some(last) = fields.last_mut() {
        last.push_str(text);
    }
}

fn splice_fields(fields: &mut Vec<String>, text: &str) {
    let parts = split_ifs(text);
    let Some((first, rest)) = parts.split_first() else {
        return;
    };
    push_field(fields, first);
    fields.extend(rest.iter().cloned());
}

fn take_single(chars: &[char], index: &mut usize) -> Option<String> {
    let mut out = String::new();
    while *index < chars.len() {
        let ch = chars[*index];
        *index += 1;
        if ch == '\'' {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

/// Double quotes with no parameter expansion. A bare `$` or `\` is not
/// a literal, so the caller keeps the substitution.
fn take_double(
    chars: &[char],
    index: &mut usize,
    depth: u32,
    substitutions: &mut Vec<String>,
    unscanned: &mut bool,
) -> Option<String> {
    let mut out = String::new();
    while *index < chars.len() {
        let ch = chars[*index];
        if ch == '"' {
            *index += 1;
            return Some(out);
        }
        if ch == '\\' || (ch == '$' && chars.get(*index + 1) != Some(&'(')) {
            return None;
        }
        if ch == '$' {
            let (body, next) = take_command_subst(chars, *index)?;
            out.push_str(&nested_printed(&body, depth + 1, substitutions, unscanned)?);
            *index = next;
            continue;
        }
        if ch == '`' {
            let (body, next) = take_backtick_chars(chars, *index)?;
            out.push_str(&nested_printed(&body, depth + 1, substitutions, unscanned)?);
            *index = next;
            continue;
        }
        out.push(ch);
        *index += 1;
    }
    None
}

fn nested_printed(
    body: &str,
    depth: u32,
    substitutions: &mut Vec<String>,
    unscanned: &mut bool,
) -> Option<String> {
    match printed_output(body, depth, substitutions) {
        Printed::Text(text) => Some(text),
        Printed::Unscanned => {
            *unscanned = true;
            None
        }
        Printed::Unknown => None,
    }
}

/// `$(...)` starting at `$`. Returns the inside and the index after `)`.
fn take_command_subst(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start) != Some(&'$') || chars.get(start + 1) != Some(&'(') {
        return None;
    }
    take_balanced_parens(chars, start + 1)
}

/// `<(...)` or `>(...)` starting at `<` or `>`.
fn take_proc_subst(chars: &[char], start: usize) -> Option<(String, usize)> {
    if !matches!(chars.get(start), Some('<' | '>')) || chars.get(start + 1) != Some(&'(') {
        return None;
    }
    take_balanced_parens(chars, start + 1)
}

fn take_balanced_parens(chars: &[char], open: usize) -> Option<(String, usize)> {
    if chars.get(open) != Some(&'(') {
        return None;
    }
    let mut index = open + 1;
    let mut depth = 1usize;
    let mut quote: Option<char> = None;
    let body_start = index;
    while index < chars.len() {
        let ch = chars[index];
        if let Some(open) = quote {
            if open == '"' && ch == '\\' {
                index += 1;
                if index < chars.len() {
                    index += 1;
                }
                continue;
            }
            if ch == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                index += 1;
            }
            '\\' => {
                index += 1;
                if index < chars.len() {
                    index += 1;
                }
            }
            '(' => {
                depth += 1;
                index += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let body = chars[body_start..index].iter().collect();
                    return Some((body, index + 1));
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

fn take_backtick_chars(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start) != Some(&'`') {
        return None;
    }
    let mut index = start + 1;
    let mut inner = String::new();
    while index < chars.len() {
        let ch = chars[index];
        if ch == '`' {
            return Some((inner, index + 1));
        }
        if ch == '\\' {
            index += 1;
            if index >= chars.len() {
                inner.push('\\');
                break;
            }
            let next = chars[index];
            index += 1;
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
        index += 1;
    }
    None
}

fn echo_output(args: &[String]) -> Option<String> {
    let mut newline = true;
    let mut interpret = false;
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if let Some(flags) = arg.strip_prefix('-') {
            if flags.is_empty() || !flags.chars().all(|ch| matches!(ch, 'n' | 'e' | 'E')) {
                return None;
            }
            for ch in flags.chars() {
                match ch {
                    'n' => newline = false,
                    'e' => interpret = true,
                    'E' => interpret = false,
                    _ => return None,
                }
            }
            index += 1;
            continue;
        }
        break;
    }
    let mut parts = Vec::new();
    for arg in &args[index..] {
        if interpret {
            parts.push(echo_escapes(arg)?);
        } else {
            parts.push(arg.clone());
        }
    }
    let mut out = parts.join(" ");
    if newline {
        out.push('\n');
    }
    Some(out)
}

fn echo_escapes(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] != '\\' {
            out.push(chars[index]);
            index += 1;
            continue;
        }
        let next = *chars.get(index + 1)?;
        match next {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '\\' => out.push('\\'),
            _ => return None,
        }
        index += 2;
    }
    Some(out)
}

fn printf_literal(args: &[String]) -> Option<String> {
    let format = args.first()?;
    let chars: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    let mut arg_index = 1usize;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                let next = *chars.get(index + 1)?;
                match next {
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    _ => return None,
                }
                index += 2;
            }
            '%' => {
                let next = *chars.get(index + 1)?;
                match next {
                    '%' => out.push('%'),
                    's' => {
                        if let Some(arg) = args.get(arg_index) {
                            out.push_str(arg);
                            arg_index += 1;
                        }
                    }
                    _ => return None,
                }
                index += 2;
            }
            ch => {
                out.push(ch);
                index += 1;
            }
        }
    }
    Some(out)
}

/// One `read` name keeps the line. Later names take one field each, and
/// the last name keeps the remainder. Leading and trailing space, tab,
/// and newline are removed. Internal spaces stay.
fn read_ifs_fields(line: &str, count: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut start = 0usize;
    let mut end = chars.len();
    while start < end && is_default_ifs(chars[start]) {
        start += 1;
    }
    while end > start && is_default_ifs(chars[end - 1]) {
        end -= 1;
    }
    if count == 1 {
        return vec![chars[start..end].iter().collect()];
    }
    let mut fields = Vec::new();
    let mut index = start;
    for nth in 0..count {
        if nth + 1 == count {
            while index < end && is_default_ifs(chars[index]) {
                index += 1;
            }
            fields.push(chars[index..end].iter().collect());
            break;
        }
        while index < end && is_default_ifs(chars[index]) {
            index += 1;
        }
        let field_start = index;
        while index < end && !is_default_ifs(chars[index]) {
            index += 1;
        }
        fields.push(chars[field_start..index].iter().collect());
    }
    while fields.len() < count {
        fields.push(String::new());
    }
    fields
}

fn is_default_ifs(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n')
}

fn split_ifs(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut field = String::new();
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t' | '\n') {
            if !field.is_empty() {
                parts.push(std::mem::take(&mut field));
            }
        } else {
            field.push(ch);
        }
    }
    if !field.is_empty() {
        parts.push(field);
    }
    parts
}

fn fold_quoted_subst(body: String, token: &mut String, substitutions: &mut Vec<String>) {
    match printed_output(&body, 0, substitutions) {
        Printed::Text(text) => token.push_str(&text),
        Printed::Unscanned => substitutions.push(UNSCANNED_SUBST.to_string()),
        Printed::Unknown => substitutions.push(body),
    }
}

fn fold_unquoted_subst(
    body: String,
    token: &mut String,
    current: &mut Vec<String>,
    quoted: &mut bool,
    word_single: &mut Option<bool>,
    in_eval: &mut Vec<bool>,
    substitutions: &mut Vec<String>,
) {
    match printed_output(&body, 0, substitutions) {
        Printed::Text(text) => {
            splice_unquoted_output(&text, token, current, quoted, word_single, in_eval);
        }
        Printed::Unscanned => substitutions.push(UNSCANNED_SUBST.to_string()),
        Printed::Unknown => substitutions.push(body),
    }
}

/// An assignment keeps the whole value. A bare substitution splits on
/// space, tab, and newline. A carriage return stays in the word.
fn splice_unquoted_output(
    text: &str,
    token: &mut String,
    current: &mut Vec<String>,
    quoted: &mut bool,
    word_single: &mut Option<bool>,
    in_eval: &mut Vec<bool>,
) {
    if is_assignment(token) {
        note_eval_quote(word_single, false);
        token.push_str(text);
        return;
    }
    let parts = split_ifs(text);
    let Some((first, rest)) = parts.split_first() else {
        return;
    };
    note_eval_quote(word_single, false);
    token.push_str(first);
    let Some((last, middle)) = rest.split_last() else {
        return;
    };
    push_token(token, current, quoted, word_single, in_eval);
    for part in middle {
        current.push(part.clone());
        in_eval.push(false);
    }
    token.push_str(last);
    note_eval_quote(word_single, false);
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

/// Rest of an unquoted here-string word. `$(...)` and backticks run
/// before the command starts. The character after a backslash stays
/// literal, so an escaped backtick does not.
fn read_unquoted_here_body<I>(
    chars: &mut std::iter::Peekable<I>,
    substitutions: &mut Vec<String>,
) -> String
where
    I: Iterator<Item = char>,
{
    let mut body = String::new();
    let mut literal_next = false;
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '<' | '>') {
            break;
        }
        chars.next();
        if ch == '\0' {
            literal_next = false;
            continue;
        }
        if literal_next {
            body.push(ch);
            literal_next = false;
            continue;
        }
        if ch == '\\' {
            body.push('\\');
            literal_next = true;
            continue;
        }
        if ch == '`' {
            let inner = read_backtick(chars);
            body.push('`');
            body.push_str(&inner);
            body.push('`');
            substitutions.push(inner);
            continue;
        }
        if ch == '$' && chars.peek() == Some(&'(') {
            chars.next();
            let inner = read_balanced_parens(chars);
            body.push_str("$(");
            body.push_str(&inner);
            body.push(')');
            substitutions.push(inner);
            continue;
        }
        body.push(ch);
    }
    body
}

fn read_here_string<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
    substitutions: &mut Vec<String>,
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
                read_dollar_double(chars, &mut body, substitutions);
                return Some(body);
            }
            Some('(') => {
                chars.next();
                let inner = read_balanced_parens(chars);
                let mut body = String::from("$(");
                body.push_str(&inner);
                body.push(')');
                substitutions.push(inner);
                body.push_str(&read_unquoted_here_body(chars, substitutions));
                return Some(body);
            }
            _ => {
                let mut body = String::from("$");
                body.push_str(&read_unquoted_here_body(chars, substitutions));
                return Some(body);
            }
        }
    }
    if chars.peek() == Some(&'\'') {
        chars.next();
        let mut body = String::new();
        for ch in chars.by_ref() {
            if ch == '\'' {
                break;
            }
            if ch != '\0' {
                body.push(ch);
            }
        }
        return Some(body);
    }
    if chars.peek() == Some(&'"') {
        chars.next();
        let mut body = String::new();
        let mut literal_next = false;
        while let Some(ch) = chars.next() {
            if ch == '\0' {
                literal_next = false;
                continue;
            }
            // A backslash does not protect the closing quote in this reader.
            if ch == '"' {
                break;
            }
            if literal_next {
                body.push(ch);
                literal_next = false;
                continue;
            }
            if ch == '\\' {
                body.push('\\');
                literal_next = true;
                continue;
            }
            if ch == '`' {
                let inner = read_backtick(chars);
                body.push('`');
                body.push_str(&inner);
                body.push('`');
                substitutions.push(inner);
                continue;
            }
            if ch == '$' && chars.peek() == Some(&'(') {
                chars.next();
                let inner = read_balanced_parens(chars);
                body.push_str("$(");
                body.push_str(&inner);
                body.push(')');
                substitutions.push(inner);
                continue;
            }
            body.push(ch);
        }
        return Some(body);
    }
    let body = read_unquoted_here_body(chars, substitutions);
    if body.is_empty() { None } else { Some(body) }
}

/// Bash removes quotes from the delimiter word and does not expand `$name`
/// or `$(...)` there. One quote in the word quotes the body. `$'...'` is
/// ANSI-C. `$"..."` and `"..."` keep `\$`, `` \` ``, `\"`, `\\`, and a
/// continued line, and they leave other backslashes in place.
fn read_heredoc_delim<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
) -> Option<(String, bool, bool)> {
    let dash = chars.peek() == Some(&'-');
    if dash {
        chars.next();
    }
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
    }
    let mut delim = String::new();
    let mut quoted = false;
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            break;
        }
        chars.next();
        if ch == '\\' {
            quoted = true;
            match chars.next() {
                Some('\n') | None => {}
                Some(next) => delim.push(next),
            }
            continue;
        }
        if ch == '\'' {
            quoted = true;
            for inner in chars.by_ref() {
                if inner == '\'' {
                    break;
                }
                delim.push(inner);
            }
            continue;
        }
        if ch == '"' {
            quoted = true;
            push_double_quoted_delim(chars, &mut delim);
            continue;
        }
        if ch == '$' {
            match chars.peek().copied() {
                Some('\'') => {
                    quoted = true;
                    chars.next();
                    read_ansi_c(chars, &mut delim);
                    continue;
                }
                Some('"') => {
                    quoted = true;
                    chars.next();
                    push_double_quoted_delim(chars, &mut delim);
                    continue;
                }
                _ => {}
            }
        }
        delim.push(ch);
    }
    if delim.is_empty() {
        None
    } else {
        Some((delim, dash, quoted))
    }
}

fn push_double_quoted_delim<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
    delim: &mut String,
) {
    while let Some(ch) = chars.next() {
        if ch == '"' {
            return;
        }
        if ch != '\\' {
            delim.push(ch);
            continue;
        }
        match chars.next() {
            Some('\n') | None => {}
            Some(next @ ('\\' | '$' | '`' | '"')) => delim.push(next),
            Some(next) => {
                delim.push('\\');
                delim.push(next);
            }
        }
    }
}

/// An unquoted heredoc expands `$(...)` and backticks before `cat` starts.
/// A backslash quotes `$`, backtick, `\`, and a newline. `$'...'` is not
/// ANSI-C quoting in that body, so a backtick there still runs. A quoted
/// delimiter never reaches this function.
fn scan_heredoc_expansions(body: &str, substitutions: &mut Vec<String>) {
    let mut chars = body.chars().peekable();
    let mut ignored = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some('\n') => {
                    chars.next();
                }
                Some('$' | '`' | '\\') => {
                    chars.next();
                }
                _ => {}
            }
            continue;
        }
        if ch == '`' {
            substitutions.push(read_backtick(&mut chars));
            continue;
        }
        if ch != '$' {
            continue;
        }
        match chars.peek().copied() {
            Some('"') => {
                chars.next();
                read_dollar_double(&mut chars, &mut ignored, substitutions);
                ignored.clear();
            }
            Some('(') => {
                chars.next();
                substitutions.push(read_balanced_parens(&mut chars));
            }
            _ => {}
        }
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
    in_eval: &mut Vec<bool>,
    substitutions: &mut Vec<String>,
    stdin_bodies: &mut Vec<String>,
    parts: &mut Vec<ShellPart>,
    isolated: bool,
    pending: &mut PendingSegment,
) {
    if current.is_empty()
        && substitutions.is_empty()
        && stdin_bodies.is_empty()
        && pending.is_empty()
    {
        in_eval.clear();
        return;
    }
    let tokens = std::mem::take(current);
    let mut flags = std::mem::take(in_eval);
    if flags.len() < tokens.len() {
        flags.resize(tokens.len(), false);
    } else if flags.len() > tokens.len() {
        flags.truncate(tokens.len());
    }
    parts.push(ShellPart::Segment(ShellSegment {
        tokens,
        in_eval: flags,
        heredocs: std::mem::take(stdin_bodies),
        substitutions: std::mem::take(substitutions),
        isolated,
        proc_inputs: std::mem::take(&mut pending.proc_inputs),
        stdin_file: pending.stdin_file.take(),
        stdin_proc: pending.stdin_proc.take(),
        read_heredoc: std::mem::take(&mut pending.read_heredoc),
    }));
}

enum StdinTarget {
    Path(String),
    Proc(String),
    Opaque,
}

/// The word after `<`. A literal path is kept. `<(...)` is the printed
/// text of that command. A single-quoted path, and a double-quoted
/// path with no `$`, backtick, or backslash, are that literal path.
/// Globs and expansions are not a path.
fn take_stdin_target<I>(
    chars: &mut std::iter::Peekable<I>,
    substitutions: &mut Vec<String>,
) -> StdinTarget
where
    I: Iterator<Item = char> + Clone,
{
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
    }
    let mut look = chars.clone();
    if look.next() == Some('<') && look.next() == Some('(') {
        chars.next();
        chars.next();
        let body = read_balanced_parens(chars);
        substitutions.push(body.clone());
        return StdinTarget::Proc(body);
    }
    match chars.peek().copied() {
        None => return StdinTarget::Opaque,
        Some('\'') => {
            chars.next();
            let mut path = String::new();
            for ch in chars.by_ref() {
                if ch == '\'' {
                    break;
                }
                path.push(ch);
            }
            return if path.is_empty() {
                StdinTarget::Opaque
            } else {
                StdinTarget::Path(path)
            };
        }
        Some('"') => {
            let mut look = chars.clone();
            look.next();
            let mut path = String::new();
            let mut literal = true;
            for ch in look.by_ref() {
                if ch == '"' {
                    break;
                }
                if matches!(ch, '\\' | '$' | '`') {
                    literal = false;
                    break;
                }
                path.push(ch);
            }
            if literal && !path.is_empty() {
                chars.next();
                for _ in 0..path.chars().count() {
                    chars.next();
                }
                if chars.peek() == Some(&'"') {
                    chars.next();
                }
                return StdinTarget::Path(path);
            }
            skip_redirect_word(chars, substitutions);
            return StdinTarget::Opaque;
        }
        Some(
            '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '~' | '(' | ')' | '\\' | '#' | '&'
            | '|' | ';' | '>' | '<',
        ) => {
            skip_redirect_word(chars, substitutions);
            return StdinTarget::Opaque;
        }
        Some(_) => {}
    }
    let mut path = String::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '<' | '>' | '#') {
            break;
        }
        if matches!(
            ch,
            '\'' | '"' | '\\' | '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '~' | '(' | ')'
        ) {
            skip_unquoted_target(chars, substitutions);
            return StdinTarget::Opaque;
        }
        chars.next();
        path.push(ch);
    }
    if path.is_empty() {
        StdinTarget::Opaque
    } else {
        StdinTarget::Path(path)
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
    use super::{HarmClass, Policy, effective_gates, matched_action, verdict_with_blocks};
    use crate::ids::ActionId;
    use crate::verdict::Verdict;

    fn drive_letter_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn nested_bash(inner: &str, layers: u32) -> String {
        let mut command = inner.to_string();
        for _ in 0..layers {
            command = format!("bash -c '{}'", command.replace('\'', "'\\''"));
        }
        command
    }

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
    fn nine_nested_shells_do_not_fall_through_to_the_default_row() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let eight = nested_bash("rm -rf /tmp/x", 8);
        let nine = nested_bash("rm -rf /tmp/x", 9);
        assert_eq!(
            matched_action(&policy, "Bash", Some(&eight)).map(|id| id.0.as_str()),
            Some("bash.rm")
        );
        assert_eq!(
            matched_action(&policy, "Bash", Some(&nine)).map(|id| id.0.as_str()),
            Some("shell.deep")
        );
        assert_eq!(
            matched_action(&policy, "Bash", Some(&nested_bash("git push origin", 8)))
                .map(|id| id.0.as_str()),
            Some("git.push")
        );
        assert_eq!(
            matched_action(&policy, "Bash", Some(&nested_bash("git push origin", 9)))
                .map(|id| id.0.as_str()),
            Some("shell.deep")
        );
        let blank = nested_bash("   ", 9);
        assert!(matched_action(&policy, "Bash", Some(&blank)).is_none());
        assert!(matched_action(&policy, "Bash", Some("ls")).is_none());

        let bare = Policy::from_toml_str(
            r#"
            schema_version = 1
            fail = "closed"
            [choice]
            escalate_below = 0.8
            review_below = 1.0
            [default_action]
            auto = 0.8
            review = 0.8
            when_unsure = "review_guess"
            class = "read"
            [actions."bash.rm"]
            tool = "Bash"
            prefixes = ["rm "]
            auto = 0.99
            review = 0.90
            when_unsure = "escalate"
            class = "exec"
            "#,
        )
        .expect("policy without shell.deep");
        assert_eq!(
            matched_action(&bare, "Bash", Some(&eight)).map(|id| id.0.as_str()),
            Some("bash.rm")
        );
        assert!(matched_action(&bare, "Bash", Some(&nine)).is_none());
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
    fn double_quoted_continuation_is_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let wrapped = "bash -c \"git \\\npush origin main\"";
        assert_eq!(
            matched_action(&policy, "Bash", Some(wrapped)).map(|id| id.0.as_str()),
            Some("git.push"),
            "{wrapped:?}"
        );
        let single = "bash -c 'git \\\npush origin main'";
        assert_eq!(
            matched_action(&policy, "Bash", Some(single)).map(|id| id.0.as_str()),
            Some("git.push"),
            "{single:?}"
        );
    }

    #[test]
    fn double_quoted_command_substitution_is_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let push = [
            "echo \"`git push origin main`\"",
            "echo $\"`git push origin main`\"",
            "echo \"$(git push origin main)\"",
            "echo $\"$(git push origin main)\"",
            "cat <<< \"`git push origin main`\"",
            "cat <<< \"$(git push origin main)\"",
            "cat <<< $\"`git push origin main`\"",
            "cat <<< $\"$(git push origin main)\"",
            "cat <<< $(git push origin main)",
            "cat <<< `git push origin main`",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        assert_eq!(
            matched_action(&policy, "Bash", Some("echo \"`rm -rf /tmp/x`\""))
                .map(|id| id.0.as_str()),
            Some("bash.rm")
        );
        let neither = [
            "echo \"\\`git push origin main\\`\"",
            "echo $\"\\`git push origin main\\`\"",
            "echo '`git push origin main`'",
            "echo '$(git push origin main)'",
            "cat <<< '$(git push origin main)'",
            "cat <<< '`git push origin main`'",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    #[test]
    fn unquoted_heredoc_expands_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let push = [
            "cat <<EOF\n$(git push origin main)\nEOF",
            "cat <<EOF\n`git push origin main`\nEOF",
            "cat <<-EOF\n\t$(git push origin main)\nEOF",
            "cat <<EOF\n$\"$(git push origin main)\"\nEOF",
            "cat <<EOF\n\\\\$(git push origin main)\nEOF",
            "cat <<EOF\n${missing:-$(git push origin main)}\nEOF",
            "cat <<EOF\n$'`git push origin main`'\nEOF",
            "cat <<EOF\n$'$(git push origin main)'\nEOF",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        assert_eq!(
            matched_action(&policy, "Bash", Some("cat <<EOF\n$(rm -rf /tmp/x)\nEOF"))
                .map(|id| id.0.as_str()),
            Some("bash.rm")
        );
        let neither = [
            "cat <<'EOF'\n$(git push origin main)\nEOF",
            "cat <<\"EOF\"\n$(git push origin main)\nEOF",
            "cat <<\\EOF\n$(git push origin main)\nEOF",
            "cat <<EOF\n\\$(git push origin main)\nEOF",
            "cat <<EOF\nrm -rf /tmp/x\nEOF",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    #[test]
    fn quoted_heredoc_word_does_not_hide_the_next_command() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let push = [
            "cat <<E'OF'\necho hello\nEOF\ngit push origin main",
            "cat <<E\"OF\"\necho hello\nEOF\ngit push origin main",
            "cat <<'EO'F\necho hello\nEOF\ngit push origin main",
            "cat <<\"EO\"F\necho hello\nEOF\ngit push origin main",
            "cat <<E'O'F\necho hello\nEOF\ngit push origin main",
            "cat <<$'EOF'\necho hello\nEOF\ngit push origin main",
            "cat <<$\"EOF\"\necho hello\nEOF\ngit push origin main",
            "cat <<$'EO\\x46'\necho hello\nEOF\ngit push origin main",
            "cat <<$'EO\\106'\necho hello\nEOF\ngit push origin main",
            "cat <<-E'OF'\necho hello\n\tEOF\ngit push origin main",
            "cat <<E\\OF\necho hello\nEOF\ngit push origin main",
            "cat <<E\"O\\\"F\"\necho hello\nEO\"F\ngit push origin main",
            "cat <<''\necho hello\n\ngit push origin main",
        ];
        for command in push {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        let neither = [
            "cat <<E'OF'\n$(git push origin main)\nEOF",
            "cat <<$'EOF'\n$(git push origin main)\nEOF",
            "cat <<$\"EOF\"\n$(git push origin main)\nEOF",
            "cat <<$'EO\\x46'\n$(git push origin main)\nEOF",
            "EOF=END; cat <<$EOF\necho hello\nEND\ngit push origin main",
            "cat <<\"$(git push origin DELIM)\"\necho hello\nEOF\ngit push origin AFTER",
            "cat <<$\"EO\\x46\"\necho hello\nEOF\ngit push origin main",
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
    fn assignment_then_eval_matches_the_command_bash_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let push = [
            "x='git push origin main'; eval \"$x\"",
            "x='git push origin main'; eval '$x'",
            "x='git push origin main'; eval $x",
            "x='git push origin main'; eval \"${x}\"",
            "x='git push origin main'; eval '${x}'",
            "x='git push origin main'; eval \"$x extra\"",
            "export x='git push origin main'; eval \"$x\"",
            "x='git push origin main'; export x; eval \"$x\"",
            "x=old; export x='git push origin main'; eval \"$x\"",
            "x='git push origin main'; command eval \"$x\"",
            "x='git push origin main'; eval -- \"$x\"",
            "x='git push origin main' && true; eval \"$x\"",
            "x='git push'; x+=' origin main'; eval \"$x\"",
            "y='git push origin main'; eval \"$y\"",
            "declare x='git push origin main'; eval \"$x\"",
            "typeset x='git push origin main'; eval \"$x\"",
            "declare -x x='git push origin main'; eval \"$x\"",
            "eval \"x='git push origin main'\"; eval \"$x\"",
            "f() { local x='git push origin main'; eval \"$x\"; }; f",
            "f() { declare x='git push origin main'; eval \"$x\"; }; f",
            "bash -c \"x='git push origin main'; eval \\\"$x\\\"\"",
            "x='rm -rf /tmp/x'; eval \"$x\"",
            "{ x='git push origin main'; }; eval \"$x\"",
            "{ x='git push origin main'; eval \"$x\"; }",
            "if true; then x='git push origin main'; fi; eval \"$x\"",
            "if true; then x='git push origin main'; eval \"$x\"; fi",
            "if true; then x='git push origin main'; else x='echo hello'; fi; eval \"$x\"",
            "if false; then x='echo hello'; else x='git push origin main'; fi; eval \"$x\"",
            "if false; then true; elif true; then x='git push origin main'; fi; eval \"$x\"",
            "! x='git push origin main'; eval \"$x\"",
            "time x='git push origin main'; eval \"$x\"",
            "for i in 1; do x='git push origin main'; done; eval \"$x\"",
            "while true; do x='git push origin main'; break; done; eval \"$x\"",
            "until false; do x='git push origin main'; break; done; eval \"$x\"",
            "x='git push origin main' eval '$x'",
            "x='git push origin main' command eval '$x'",
            "x='git push origin main'; f() { local x; x='echo ok'; }; f; eval \"$x\"",
            "readonly x='git push origin main'; eval \"$x\"",
            "readonly x='git push origin main'; eval '$x'",
            "readonly -- x='git push origin main'; eval \"$x\"",
            "command readonly x='git push origin main'; eval \"$x\"",
            "builtin readonly x='git push origin main'; eval \"$x\"",
            "f() { readonly x='git push origin main'; }; f; eval \"$x\"",
            "f() { readonly x='git push origin main'; eval \"$x\"; }; f",
            "readonly -a x='git push origin main'; eval \"$x\"",
            "readonly -A x='git push origin main'; eval \"$x\"",
            "readonly -p x='git push origin main'; eval \"$x\"",
            "eval \"$(echo 'git push origin main')\"",
            "eval \"$(echo -n 'git push origin main')\"",
            "eval \"$(echo -e 'git push origin main')\"",
            "eval \"$(printf '%s' 'git push origin main')\"",
            "eval \"$(printf '%s\\n' 'git push origin main')\"",
            "eval \"`echo 'git push origin main'`\"",
            "x=$(echo 'git push origin main'); eval \"$x\"",
            "$(echo 'git push origin main')",
            "eval $(echo 'git push origin main')",
            "eval \"$(echo 'git push origin main'; true)\"",
            "eval \"$(echo $(echo 'git push origin main'))\"",
            "eval \"$(command echo 'git push origin main')\"",
            "eval \"$(printf 'echo a\\ngit push origin main')\"",
            "bash <<EOF\n$(echo 'git push origin main')\nEOF",
            "bash <<< \"$(echo 'git push origin main')\"",
            "eval \"$(echo 'rm -rf /tmp/x')\"",
            "$(echo 'rm -rf /tmp/x')",
            "eval \"$(echo 'git push origin main'; git status)\"",
            "eval \"$(echo 'git push origin main' | cat)\"",
            "eval \"$(echo -n 'git push origin main' | cat)\"",
            "eval \"$(echo 'git push origin main' | cat | cat)\"",
            "eval \"$(echo 'git push origin main' | stdbuf -o0 cat)\"",
            "eval \"$(printf '%s\\n' 'git push origin main' | stdbuf -oL cat)\"",
            "eval \"$(echo 'git push origin main' | gstdbuf -o0 cat)\"",
            "eval \"$(echo 'git push origin main' | timeout 1 cat)\"",
            "eval \"$(echo 'git push origin main' | gtimeout 1 cat)\"",
            "eval \"$(echo 'rm -rf /tmp/x' | cat)\"",
            "$(echo 'rm -rf /tmp/x' | cat)",
            "printf -v x '%s' 'git push origin main'; eval \"$x\"",
            "printf -v x '%s\\n' 'git push origin main'; eval \"$x\"",
            "printf -v x -- '%s' 'git push origin main'; eval \"$x\"",
            "command printf -v x '%s' 'git push origin main'; eval \"$x\"",
            "f() { printf -v x '%s' 'git push origin main'; }; f; eval \"$x\"",
            "read x <<< 'git push origin main'; eval \"$x\"",
            "read -r x <<< 'git push origin main'; eval \"$x\"",
            "read -- x <<< 'git push origin main'; eval \"$x\"",
            "command read x <<< 'git push origin main'; eval \"$x\"",
            "f() { read x <<< 'git push origin main'; }; f; eval \"$x\"",
            "read x <<EOF\ngit push origin main\nEOF\neval \"$x\"",
            "read x <<'EOF'\ngit push origin main\nEOF\neval \"$x\"",
            "eval \"$(echo 'git push origin main' && true)\"",
            "eval \"$(echo 'git push origin main' && git status)\"",
            "eval \"$(echo 'git push origin main' && false)\"",
            "eval \"$(echo 'git push origin main' || true)\"",
            "eval \"$(echo 'git push origin main' || git status)\"",
            "eval \"$(false || echo 'git push origin main')\"",
            "eval \"$(git status && echo 'git push origin main')\"",
            "eval \"$(git status || echo 'git push origin main')\"",
            "eval \"$(echo 'git push origin main && true')\"",
            "eval \"$(echo 'git push origin main' | head -n 1)\"",
            "eval \"$(echo 'git push origin main' | grep .)\"",
            "eval \"$(echo 'git push origin main' < /dev/null)\"",
            "eval \"$(echo 'git push origin main' 2>/dev/null)\"",
            "eval \"$(echo hidden > /dev/null && echo 'git push origin main')\"",
            "mapfile -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "mapfile -t x <<< 'git push origin main'; eval \"$x\"",
            "mapfile -t x <<< 'git push origin main'; eval \"${x}\"",
            "mapfile -t x <<< 'git push origin main'; eval \"${x[@]}\"",
            "mapfile -t x <<< 'git push origin main'; eval \"${x[*]}\"",
            "mapfile x <<< 'git push origin main'; eval \"${x[0]}\"",
            "readarray -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "mapfile -n 0 -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "mapfile -s 0 -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "mapfile -O 0 -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "mapfile -u 0 -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "mapfile -d '' -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "mapfile -C 'git push origin main' -c 1 -t x <<< hello",
            "f() { mapfile -t x <<< 'git push origin main'; }; f; eval \"${x[0]}\"",
            "f() { local x; mapfile -t x <<< 'git push origin main'; eval \"${x[0]}\"; }; f",
            "read <<< 'git push origin main'; eval \"$REPLY\"",
            "mapfile -t <<< 'git push origin main'; eval \"${MAPFILE[0]}\"",
            "source <(echo 'git push origin main')",
            ". <(echo 'git push origin main')",
            "bash <(echo 'git push origin main')",
            "sh <(echo 'git push origin main')",
            "source <(printf '%s\\n' 'git push origin main')",
            "eval \"$(cat <(echo 'git push origin main'))\"",
            "eval \"$(tail -n 1 <(echo 'git push origin main'))\"",
            "read x < <(echo 'git push origin main'); eval \"$x\"",
            "eval \"$(printf '%s' 'Z2l0IHB1c2ggb3JpZ2luIG1haW4K' | base64 -d)\"",
            "eval \"$(printf '%s' 'Z2l0IHB1c2ggb3JpZ2luIG1haW4K' | base64 --decode)\"",
            "eval \"$(printf '%s\\n' 'git push origin main' | sort -s)\"",
            "eval \"$(printf '%s\\n' 'git push origin main' | uniq -u)\"",
            "eval \"$(printf '%s\\n' 'git push origin main' | tac -b)\"",
            "eval \"$(printf '%s\\n' 'git push origin main' | paste)\"",
            "eval \"$(printf '%s\\n' 'git push origin main' | expand)\"",
        ];
        for command in push {
            let want = if command.contains("rm -rf") {
                "bash.rm"
            } else {
                "git.push"
            };
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some(want),
                "{command:?}"
            );
        }
        let neither = [
            "echo 'git push origin main'",
            "eval 'echo rm'",
            "x='git push origin main' echo hello",
            "x='git push origin main' echo hello; eval \"$x\"",
            "x='git push origin main' | true; eval \"$x\"",
            "x='echo hello'; eval \"$x\"",
            "x='git push origin main' eval \"$x\"",
            "x='git push origin main' eval true; eval \"$x\"",
            "x='git push origin main'; eval \"echo $x\"",
            "x='git push origin main' & eval \"$x\"",
            "(x='git push origin main'); eval \"$x\"",
            "x='git push origin main'; bash -c 'eval \"$x\"'",
            "f() { local x='git push origin main'; }; f; eval \"$x\"",
            "f() { declare x='git push origin main'; }; f; eval \"$x\"",
            "f() { local x; x='git push origin main'; }; f; eval \"$x\"",
            "f() { declare x; x='git push origin main'; }; f; eval \"$x\"",
            "echo \"$(echo 'git push origin main')\"",
            "eval \"echo $(echo 'git push origin main')\"",
            "readonly -x x='git push origin main'; eval \"$x\"",
            "readonly -g x='git push origin main'; eval \"$x\"",
            "readonly -f x='git push origin main'; eval \"$x\"",
            "cat <<< \"$(echo 'git push origin main')\"",
            "cat <<EOF\n$(echo 'git push origin main')\nEOF",
            "eval $(printf 'echo a\\ngit push origin main')",
            "eval \"$(echo 'git push origin main' | true)\"",
            "eval \"$(echo 'git push origin main' | echo safe)\"",
            "eval \"$(echo hello | stdbuf -o0 cat)\"",
            "eval \"$(echo hello | gstdbuf -o0 cat)\"",
            "eval \"$(echo hello | timeout 1 cat)\"",
            "eval \"$(printf -v x '%s' 'git push origin main')\"",
            "echo \"$(printf -v x '%s' 'git push origin main')\"",
            "printf -v x '%s' 'echo hello'; eval \"$x\"",
            "read x <<< 'git push origin main'",
            "read x <<< 'echo hello'; eval \"$x\"",
            "read x; eval \"$x\"",
            "read -n 1 x <<< 'git push origin main'; eval \"$x\"",
            "read x <<< 'git push origin main' | true; eval \"$x\"",
            "(read x <<< 'git push origin main'); eval \"$x\"",
            "f() { local x; printf -v x '%s' 'git push origin main'; }; f; eval \"$x\"",
            "f() { local x; read x <<< 'git push origin main'; }; f; eval \"$x\"",
            "eval \"$(false && echo 'git push origin main')\"",
            "eval \"$(echo safe || echo 'git push origin main')\"",
            "eval \"$(echo 'git push origin main' > /dev/null)\"",
            "eval \"$(echo 'git push origin main' >&2)\"",
            "eval \"$(echo 'git push origin main'2>/dev/null)\"",
            "eval \"$(echo 'git push origin main' &&)\"",
            "mapfile -O 2 -t x <<< 'git push origin main'; eval \"${x[0]}\"",
            "f() { local x; mapfile -t x <<< 'git push origin main'; }; f; eval \"${x[0]}\"",
            "mapfile -t x <<< 'git push origin main' | true; eval \"${x[0]}\"",
            "( mapfile -t x <<< 'git push origin main' ); eval \"${x[0]}\"",
            "source <(echo 'echo hello')",
            "bash -c 'echo hi' <(echo 'git push origin main')",
            "eval \"$(echo <(echo 'git push origin main'))\"",
            "mapfile -C 'git push origin main' -c 5000 -t x <<< hello",
            "mapfile -C git -c 1 -t x <<< 'git push origin main'",
            "mapfile -t x <<< alpha; eval \"${x[0]}\"",
            "read x y <<< 'git push origin main'; eval \"$x\"",
            "read x y <<< 'git push origin main'; eval \"$y\"",
            "mapfile -t x <<< 'git push origin main'; x=echo; eval \"${x[0]}\"",
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
            "sort --compress-program=rm /tmp/snapif-sort-in",
            "sort -S 1k --compress-program=rm /tmp/snapif-sort-in",
            "sort --compress-program rm /tmp/snapif-sort-in",
            "gsort --compress-program=rm /tmp/snapif-sort-in",
            "eval \"$(sort --compress-program=rm /tmp/snapif-sort-in)\"",
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
            "gsplit --filter='git push origin main' -l 1 /tmp/snapif-split-in",
            "split --filter='git push origin main' /tmp/snapif-split-in",
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
            "sort --compress-program=true /tmp/snapif-sort-in",
            "gsplit --filter=echo -l 1 /tmp/snapif-split-in",
            "gsplit --filter='git push origin main' -n 1/1 /tmp/snapif-split-in",
            "gsplit -n 1/1 /tmp/snapif-split-in",
            "sort -S 1k /tmp/snapif-sort-in",
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
            "pwsh -EC cgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=",
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
        // The decoded text uses the same matcher as `-Command`. `&&`
        // keeps the stricter `bash.rm` row, which is what the plain
        // script selects.
        let compound = "git push && rm -rf /tmp/x";
        let compound_encoded =
            "ZwBpAHQAIABwAHUAcwBoACAAJgAmACAAcgBtACAALQByAGYAIAAvAHQAbQBwAC8AeAA=";
        assert_eq!(
            super::decode_ps_encoded_script(compound_encoded).as_deref(),
            Some(compound)
        );
        let wrapped = format!("powershell -EncodedCommand {compound_encoded}");
        let as_command = format!("pwsh -Command {compound}");
        assert_eq!(
            matched_action(&policy, "Bash", Some(&wrapped)).map(|id| id.0.as_str()),
            matched_action(&policy, "Bash", Some(&as_command)).map(|id| id.0.as_str()),
        );
        assert_eq!(
            matched_action(&policy, "Bash", Some(&wrapped)).map(|id| id.0.as_str()),
            Some("bash.rm")
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
            "pwsh -File - <<'EOF'\nrm -rf /tmp/x\nEOF",
            "powershell -File - <<'EOF'\nrm -rf /tmp/x\nEOF",
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

    /// Bash runs the trap action when the signal arrives. The builtin is
    /// the word `trap` only. `trap EXIT` resets that signal, and `-p`
    /// only prints. A signal this bash rejects does not run the action
    /// unless another word is a real signal.
    #[test]
    fn trap_action_matches_the_command_it_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let matched = [
            ("trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("trap $'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("trap 'rm -rf /tmp/x' EXIT INT", "bash.rm"),
            ("trap rm EXIT", "bash.rm"),
            ("trap -- 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("trap 'rm -rf /tmp/x' 0", "bash.rm"),
            ("trap 'rm -rf /tmp/x' 000", "bash.rm"),
            ("trap 'rm -rf /tmp/x' +15", "bash.rm"),
            ("trap 'rm -rf /tmp/x' 31", "bash.rm"),
            ("trap 'rm -rf /tmp/x' exit", "bash.rm"),
            ("trap 'rm -rf /tmp/x' SIGINT", "bash.rm"),
            ("trap 'rm -rf /tmp/x' NOTASIGNAL EXIT", "bash.rm"),
            ("trap 'rm -rf /tmp/x' -rf EXIT", "bash.rm"),
            ("trap 'rm -rf /tmp/x' EXIT; echo ok", "bash.rm"),
            ("command trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("command -p trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("builtin trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("FOO=1 trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("time trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("time -p trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("git push origin main; trap 'rm -rf /tmp/x' EXIT", "bash.rm"),
            ("trap 'git push origin main' EXIT", "git.push"),
            ("trap 'rm -rf /tmp/x' KILL EXIT", "bash.rm"),
        ];
        for (command, expected) in matched {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some(expected),
                "{command:?}"
            );
        }
        let quiet = [
            "trap 'echo ok' EXIT",
            "trap - EXIT",
            "trap -- - EXIT",
            "trap '' EXIT",
            "trap EXIT",
            "trap rm",
            "trap -p",
            "trap -p EXIT",
            "trap -p 'rm -rf /tmp/x' EXIT",
            "trap -l",
            "trap -P EXIT",
            "echo ok",
            "TRAP 'rm -rf /tmp/x' EXIT",
            "Trap 'rm -rf /tmp/x' EXIT",
            "/bin/trap 'rm -rf /tmp/x' EXIT",
            "./trap 'rm -rf /tmp/x' EXIT",
            "trap.exe 'rm -rf /tmp/x' EXIT",
            "sudo trap 'rm -rf /tmp/x' EXIT",
            "env trap 'rm -rf /tmp/x' EXIT",
            "nice trap 'rm -rf /tmp/x' EXIT",
            "timeout 1 trap 'rm -rf /tmp/x' EXIT",
            "exec trap 'rm -rf /tmp/x' EXIT",
            "/usr/bin/time trap 'rm -rf /tmp/x' EXIT",
            "command time trap 'rm -rf /tmp/x' EXIT",
            "TIME trap 'rm -rf /tmp/x' EXIT",
            "trap 'rm -rf /tmp/x' SIGEXIT",
            "trap 'rm -rf /tmp/x' NOTASIGNAL",
            "trap 'rm -rf /tmp/x' 64",
            "trap 'rm -rf /tmp/x' 32",
            "trap 'rm -rf /tmp/x' +32",
            "trap 'rm -rf /tmp/x' --",
            "trap 'rm -rf /tmp/x' PWR",
            "trap 'rm -rf /tmp/x' KILL",
            "trap 'rm -rf /tmp/x' STOP",
            "trap 'rm -rf /tmp/x' SIGKILL",
            "trap 'rm -rf /tmp/x' SIGSTOP",
            "trap 'rm -rf /tmp/x' 9",
            "trap 'rm -rf /tmp/x' +9",
        ];
        for command in quiet {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
        let stop_number = if cfg!(target_os = "macos") {
            "17"
        } else if cfg!(target_os = "linux") {
            "19"
        } else {
            return;
        };
        let stop = format!("trap 'rm -rf /tmp/x' {stop_number}");
        assert!(
            matched_action(&policy, "Bash", Some(&stop)).is_none(),
            "{stop:?}"
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
                "git -c mergetool.x.cmd='rm -rf /tmp/x' mergetool --tool=x file.txt",
                "bash.rm",
            ),
            (
                "git -c mergetool.x.cmd='rm -rf /tmp/x' -c merge.tool=x mergetool file.txt",
                "bash.rm",
            ),
            (
                "git -c filter.probe.clean='rm -rf /tmp/x' add file.txt",
                "bash.rm",
            ),
            (
                "git -c filter.probe.smudge='rm -rf /tmp/x' checkout -- file.txt",
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
                Some("git -c mergetool.x.cmd='rm -rf /tmp/x' status")
            )
            .is_none(),
            "mergetool.cmd is not used by status"
        );
        assert!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c filter.probe.clean='rm -rf /tmp/x' status")
            )
            .is_none(),
            "a clean filter is not used by status"
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

    /// OpenSSH accepts the keyword in any ASCII case, a space, or `=`.
    /// `ssh -G` resolves these to one proxycommand line. The negative
    /// rows are the near misses: empty, `none`, and a longer keyword.
    #[test]
    fn openssh_proxycommand_follows_the_option_grammar() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "ssh -o proxycommand='rm -rf /tmp/x' example.com",
            "ssh -o PROXYCOMMAND='rm -rf /tmp/x' example.com",
            "ssh -o 'ProxyCommand rm -rf /tmp/x' example.com",
            "ssh -o 'ProxyCommand =rm -rf /tmp/x' example.com",
            "ssh -o 'ProxyCommand= rm -rf /tmp/x' example.com",
            "ssh -o 'ProxyCommand = rm -rf /tmp/x' example.com",
            "ssh -o 'ProxyCommand\trm -rf /tmp/x' example.com",
            "scp -o proxycommand='rm -rf /tmp/x' host:/tmp /tmp",
            "sftp -o 'proxycommand rm -rf /tmp/x' host",
            "ssh -oproxycommand='rm -rf /tmp/x' example.com",
            "ssh -o 'ProxyCommand=\"rm\" -rf /tmp/x' example.com",
            "ssh -o StrictHostKeyChecking=no -o proxycommand='rm -rf /tmp/x' example.com",
            "ssh -p 22 -o ProxyCommand='rm -rf /tmp/x' example.com",
            "ssh -i ~/.ssh/id -o ProxyCommand='rm -rf /tmp/x' example.com",
            "scp -P 22 -o ProxyCommand='rm -rf /tmp/x' host:/tmp /tmp",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        let neither = [
            "ssh -o ProxyCommand= example.com",
            "ssh -o ProxyCommand example.com",
            "ssh -o ProxyCommand=none example.com",
            "ssh -o ProxyCommand=None example.com",
            "ssh -o ProxyCommandFoo='rm -rf /tmp/x' example.com",
            "ssh -o MyProxyCommand='rm -rf /tmp/x' example.com",
            "ssh -o '你好ProxyCommand=rm -rf /tmp/x' example.com",
            "ssh -o 'ProxyCommand=\"rm -rf /tmp/x\"' example.com",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    /// Git folds the section and the variable and keeps the subsection.
    /// The rows are the forms `git config` and `credential fill` accept,
    /// plus the near misses those programs reject or do not run.
    #[test]
    fn git_config_matches_the_key_git_stores() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let rm = [
            "git -c Credential.Helper='!rm -rf /tmp/x' credential fill",
            "git -c CREDENTIAL.HELPER='!rm -rf /tmp/x' credential fill",
            "git -c 'credential.https://example.com.helper=!rm -rf /tmp/x' credential fill",
            "git -c 'Credential.HTTPS://Example.com.Helper=!rm -rf /tmp/x' credential fill",
            "git -c 'credential..helper=!rm -rf /tmp/x' credential fill",
            "git -C repo -c Credential.Helper='!rm -rf /tmp/x' credential fill",
            "git -c credential.helper= -c credential.helper='!rm -rf /tmp/x' credential fill",
            "git -c credential.helper='!echo ok' -c credential.helper='!rm -rf /tmp/x' credential fill",
            "HELPER='!rm -rf /tmp/x' git --config-env=credential.helper=HELPER credential fill",
            "git -c Core.sshCommand='rm -rf /tmp/x' ls-remote ssh://example/no.git",
            "git -c core.sshcommand='rm -rf /tmp/x' fetch git@example.com:repo.git",
            "git -c core.sshCommand=echo -c core.sshCommand='rm -rf /tmp/x' ls-remote ssh://example/no.git",
            "git -c core.sshCommand='rm -rf /tmp/x' push",
            "git -c Diff.External='rm -rf /tmp/x' diff --no-index a b",
            "git -c diff.external=echo -c Diff.External='rm -rf /tmp/x' diff --no-index a b",
            "git -c Diff.Tool=x -c Difftool.x.Cmd='rm -rf /tmp/x' difftool a b",
            "git -c Merge.Tool=x -c Mergetool.x.Cmd='rm -rf /tmp/x' mergetool file.txt",
            "git -c Filter.probe.Clean='rm -rf /tmp/x' add file.txt",
            "git -c filter.probe.CLEAN='rm -rf /tmp/x' add file.txt",
            "git -c 'alias.P=!rm -rf /tmp/x' p",
            "git -c 'alias.P=!rm -rf /tmp/x' P",
            "git -c 'alias.Foo.Bar=!rm -rf /tmp/x' foo.bar",
            "GIT_SSH_COMMAND='rm -rf /tmp/x' git ls-remote ssh://example/no.git",
            "env GIT_SSH_COMMAND='rm -rf /tmp/x' git ls-remote ssh://example/no.git",
            "GIT_SSH=rm git ls-remote ssh://example/no.git",
            "GIT_SSH=/bin/rm git ls-remote ssh://example/no.git",
            "SSH='rm -rf /tmp/x' git --config-env=core.sshCommand=SSH ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND='rm -rf /tmp/x' bash -c 'git ls-remote ssh://example/no.git'",
            "export GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "builtin export GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "command builtin export GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND=rm builtin export GIT_SSH_COMMAND; git ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND='rm -rf /tmp/x' nice git ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND+=rm git ls-remote ssh://example/no.git",
            "git -c alias.p=push -c core.sshCommand='rm -rf /tmp/x' p",
            "git -c alias.c=credential -c credential.helper='!rm -rf /tmp/x' c fill",
            "git -c alias.p='!git ls-remote ssh://example/no.git' -c core.sshCommand='rm -rf /tmp/x' p",
            "HELPER='!rm -rf /tmp/x' bash -c 'git --config-env=credential.helper=HELPER credential fill'",
            "git -c 'credential.https://example.com.helper=!echo' -c core.sshCommand='rm -rf /tmp/x' push",
            "GIT_SSH_COMMAND=rm export GIT_SSH_COMMAND; git ls-remote ssh://example/no.git",
            "eval 'export GIT_SSH_COMMAND=rm'; git ls-remote ssh://example/no.git",
            "export GIT_SSH_COMMAND=rm && git ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND='rm -rf /tmp/x'; declare -x GIT_SSH_COMMAND; git ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND='rm -rf /tmp/x'; typeset -x GIT_SSH_COMMAND; git ls-remote ssh://example/no.git",
            "f() { declare -x GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git; }; f",
            "f() { local -x GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git; }; f",
            "export -- GIT_SSH_COMMAND=rm; git ls-remote ssh://example/no.git",
            "export() { rm -rf /tmp/x; }; export",
            "export() { true; }; builtin export GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "git -c 'alias.push=!true' -c core.sshCommand='rm -rf /tmp/x' push",
            "git -c alias.credential=status -c credential.helper='!rm -rf /tmp/x' credential fill",
        ];
        for command in rm {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        assert_eq!(
            matched_action(&policy, "Bash", Some("git -c alias.P=push P")).map(|id| id.0.as_str()),
            Some("git.push"),
        );
        assert_eq!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c core.sshCommand='rm -rf /tmp/x' push --repo=https://example/no.git")
            )
            .map(|id| id.0.as_str()),
            Some("git.push"),
            "an https remote does not run core.sshCommand"
        );
        assert_eq!(
            matched_action(
                &policy,
                "Bash",
                Some(
                    "git -c alias.p='push --repo=https://example/no.git' -c core.sshCommand='rm -rf /tmp/x' p"
                )
            )
            .map(|id| id.0.as_str()),
            Some("git.push"),
            "an alias expands before the ssh command is chosen"
        );
        assert_eq!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c \"alias.p=push 'a;rm -rf /tmp/x'\" p")
            )
            .map(|id| id.0.as_str()),
            Some("git.push"),
            "an alias argument is not a shell"
        );
        assert_eq!(
            matched_action(
                &policy,
                "Bash",
                Some("git -c 'alias.push=!rm -rf /tmp/x' push --repo=https://example/no.git")
            )
            .map(|id| id.0.as_str()),
            Some("git.push"),
            "git ignores an alias that hides push"
        );
        for command in [
            "git -c core.sshCommand='rm -rf /tmp/x' push /tmp/bare.git",
            "git -c core.sshCommand='rm -rf /tmp/x' push --repo=/tmp/bare.git",
        ] {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "a local path does not run core.sshCommand: {command:?}"
            );
        }
        let neither = [
            "git -c Credential.Helper='!rm -rf /tmp/x' status",
            "git -c 'credential.https://example.com.helper=!rm -rf /tmp/x' status",
            "git -c credential.helper='rm -rf /tmp/x' credential fill",
            "git -c 'credential.helper =!rm -rf /tmp/x' credential fill",
            "git --config credential.helper='!rm -rf /tmp/x' credential fill",
            "git -ccredential.helper='!rm -rf /tmp/x' credential fill",
            "git -c credential.helper='!rm -rf /tmp/x' -c credential.helper= credential fill",
            "git -c 'credential.https://example.com.helper=!rm -rf /tmp/x' -c credential.helper= credential fill",
            "git --config-env=credential.helper=HELPER credential fill",
            "git -c difftool.X.cmd='rm -rf /tmp/x' difftool --tool=x a b",
            "git -c Difftool.x.Cmd='rm -rf /tmp/x' diff --no-index a b",
            "git -c Core.sshCommand='rm -rf /tmp/x' status",
            "git -c Core.sshCommand='rm -rf /tmp/x' ls-remote https://example/no.git",
            "git -c core.sshCommand='rm -rf /tmp/x' -c core.sshCommand=echo ls-remote ssh://example/no.git",
            "git -c diff.external='rm -rf /tmp/x' -c diff.external=echo diff --no-index a b",
            "git -c alias.P='!rm -rf /tmp/x' status",
            "git -c alias.p='!rm -rf /tmp/x' -c alias.p=status p",
            "GIT_SSH_COMMAND=echo git -c core.sshCommand='rm -rf /tmp/x' ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND= git -c core.sshCommand='rm -rf /tmp/x' ls-remote ssh://example/no.git",
            "GIT_SSH=rm git -c core.sshCommand=echo ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND='rm -rf /tmp/x' git status",
            "GIT_SSH_COMMAND='rm -rf /tmp/x' git ls-remote https://example/no.git",
            "nice GIT_SSH_COMMAND='rm -rf /tmp/x' git ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "f() { local -x GIT_SSH_COMMAND='rm -rf /tmp/x'; }; f; git ls-remote ssh://example/no.git",
            "f() { declare -x GIT_SSH_COMMAND='rm -rf /tmp/x'; }; f; git ls-remote ssh://example/no.git",
            "git -c alias.p='!rm -rf /tmp/x' status P",
            "GIT_SSH='rm -rf /tmp/x' git ls-remote ssh://example/no.git",
            "GIT_SSH_COMMAND=rm GIT_SSH_COMMAND+=.safe git ls-remote ssh://example/no.git",
            "git -c core.sshCommand='rm -rf /tmp/x' ls-remote git://example/no.git",
            "git -c core.sshCommand='rm -rf /tmp/x' ls-remote ftps://example/no.git",
            "git -c core.sshCommand='rm -rf /tmp/x' ls-remote SSH://example/no.git",
            "git -c core.sshCommand='rm -rf /tmp/x' ls-remote https://example/ssh://no.git",
            "env export GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "nice -- GIT_SSH_COMMAND='rm -rf /tmp/x' git ls-remote ssh://example/no.git",
            "( export GIT_SSH_COMMAND=rm ); git ls-remote ssh://example/no.git",
            "export GIT_SSH_COMMAND=rm | true; git ls-remote ssh://example/no.git",
            "export GIT_SSH_COMMAND=rm & git ls-remote ssh://example/no.git",
            "git -c credential.helper='!rm -rf /tmp/x' status credential",
            "git -C diff -c diff.external='rm -rf /tmp/x' status",
            "git -c filter.probe.clean='rm -rf /tmp/x' status add",
            "git -c Diff.Tool=x -c Difftool.x.Cmd='rm -rf /tmp/x' status difftool",
            "git -c core.sshCommand='rm -rf /tmp/x' status push",
            "env builtin export GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "true |\nexport GIT_SSH_COMMAND='rm -rf /tmp/x'\ngit ls-remote ssh://example/no.git",
            "true |\n\nexport GIT_SSH_COMMAND='rm -rf /tmp/x'\ngit ls-remote ssh://example/no.git",
            "true | # note\nexport GIT_SSH_COMMAND='rm -rf /tmp/x'\ngit ls-remote ssh://example/no.git",
            "export GIT_SSH_COMMAND='rm -rf /tmp/x' |& true; git ls-remote ssh://example/no.git",
            "true |&\nexport GIT_SSH_COMMAND='rm -rf /tmp/x'\ngit ls-remote ssh://example/no.git",
            "git -c core.sshCommand='rm -rf /tmp/x' clone /tmp/src",
            "git -c core.sshCommand='rm -rf /tmp/x' fetch ./repo",
            "git -c core.sshCommand='rm -rf /tmp/x' clone ~/src",
            "git -c core.sshCommand='rm -rf /tmp/x' clone C:/repo",
            "git -c core.sshCommand='rm -rf /tmp/x' clone foo/bar",
            "git -c diff.tool=vimdiff -c difftool.x.cmd='rm -rf /tmp/x' difftool -- --tool=x",
            "export GIT_SSH_COMMAND=rm; export -n GIT_SSH_COMMAND; git ls-remote ssh://example/no.git",
            "export -n GIT_SSH_COMMAND=rm; git ls-remote ssh://example/no.git",
            "export GIT_SSH_COMMAND=rm; export -n -- GIT_SSH_COMMAND; git ls-remote ssh://example/no.git",
            "export() { true; }; export GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
            "readonly GIT_SSH_COMMAND='rm -rf /tmp/x'; git ls-remote ssh://example/no.git",
        ];
        for command in neither {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
    }

    /// `source` and `.` run a regular file in this shell. `bash` and `sh`
    /// run it in a new process. A missing path stays unmatched. `bash -c`
    /// and `bash -s` do not read a later path.
    #[test]
    fn script_file_matches_the_command_bash_runs() {
        fn gzip_bytes(text: &str) -> Vec<u8> {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(text.as_bytes()).expect("gzip write");
            encoder.finish().expect("gzip finish")
        }
        fn zlib_bytes(text: &str) -> Vec<u8> {
            use std::io::Write;
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(text.as_bytes()).expect("zlib write");
            encoder.finish().expect("zlib finish")
        }
        fn deflate_bytes(text: &str) -> Vec<u8> {
            use std::io::Write;
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(text.as_bytes()).expect("deflate write");
            encoder.finish().expect("deflate finish")
        }
        fn gzip_bytes_raw(bytes: &[u8]) -> Vec<u8> {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(bytes).expect("gzip raw");
            encoder.finish().expect("gzip raw finish")
        }
        fn bzip_bytes_raw(bytes: &[u8]) -> Vec<u8> {
            use std::io::Write;
            let mut encoder =
                bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
            encoder.write_all(bytes).expect("bzip raw");
            encoder.finish().expect("bzip raw finish")
        }
        fn zstd_bytes_raw(bytes: &[u8]) -> Vec<u8> {
            ruzstd::encoding::compress_to_vec(bytes, ruzstd::encoding::CompressionLevel::Fastest)
        }
        fn compress_bytes(kind: &str) -> &'static [u8] {
            match kind {
                "git" => &[
                    31, 157, 144, 103, 210, 208, 1, 1, 167, 206, 28, 52, 32, 222, 200, 73, 19, 208,
                    13, 136, 54, 97, 210, 184, 81, 0,
                ],
                "echo" => &[
                    31, 157, 144, 101, 198, 160, 121, 3, 2, 77, 25, 54, 108, 222, 40, 0,
                ],
                _ => &[],
            }
        }
        fn wrapped_tar(kind: &str) -> &'static [u8] {
            match kind {
                "git.Z" => &[
                    31, 157, 144, 103, 210, 208, 113, 65, 7, 15, 29, 0, 8, 19, 42, 92, 200, 176,
                    161, 195, 135, 16, 35, 74, 156, 136, 16, 134, 69, 24, 54, 104, 208, 0, 112,
                    177, 35, 199, 142, 22, 63, 130, 180, 40, 163, 134, 200, 145, 33, 45, 222, 144,
                    33, 195, 6, 0, 16, 48, 40, 202, 156, 73, 179, 166, 77, 0, 117, 230, 208, 9, 35,
                    7, 4, 136, 155, 64, 131, 10, 29, 74, 180, 168, 209, 163, 72, 19, 6, 164, 3, 2,
                    78, 78, 52, 32, 222, 200, 73, 19, 208, 13, 136, 54, 97, 210, 184, 81, 144, 180,
                    171, 215, 175, 96, 195, 138, 29, 75, 182, 172, 217, 179, 104, 211, 170, 93,
                    203, 182, 173, 219, 183, 112, 227, 202, 157, 75, 183, 174, 221, 187, 120, 243,
                    234, 221, 203, 183, 175, 223, 191, 128, 3, 11, 30, 76, 184, 176, 225, 195, 136,
                    19, 43, 94, 204, 184, 177, 227, 199, 144, 35, 75, 158, 76, 185, 178, 229, 203,
                    152, 51, 107, 222, 204, 185, 179, 231, 207, 160, 67, 139, 30, 77, 186, 180,
                    233, 211, 168, 83, 171, 94, 205, 186, 181, 235, 215, 176, 99, 203, 158, 77,
                    187, 182, 237, 219, 184, 115, 235, 222, 205, 187, 183, 239, 223, 192, 131, 11,
                    31, 78, 188, 184, 241, 227, 200, 147, 43, 95, 30, 28,
                ],
                "echo.Z" => &[
                    31, 157, 144, 101, 198, 160, 121, 227, 130, 14, 30, 58, 0, 18, 42, 92, 200,
                    176, 161, 195, 135, 16, 35, 74, 156, 72, 17, 0, 140, 139, 48, 108, 208, 160,
                    97, 17, 35, 198, 142, 30, 97, 128, 12, 9, 35, 198, 140, 145, 36, 69, 94, 188,
                    49, 163, 134, 13, 0, 32, 68, 86, 156, 73, 179, 166, 205, 155, 117, 230, 208, 9,
                    35, 7, 4, 136, 155, 64, 131, 10, 29, 74, 180, 168, 209, 163, 72, 41, 6, 28, 8,
                    2, 77, 25, 54, 108, 222, 40, 72, 74, 181, 170, 213, 171, 88, 179, 106, 221,
                    202, 181, 171, 215, 175, 96, 195, 138, 29, 75, 182, 172, 217, 179, 104, 211,
                    170, 93, 203, 182, 173, 219, 183, 112, 227, 202, 157, 75, 183, 174, 221, 187,
                    120, 243, 234, 221, 203, 183, 175, 223, 191, 128, 3, 11, 30, 76, 184, 176, 225,
                    195, 136, 19, 43, 94, 204, 184, 177, 227, 199, 144, 35, 75, 158, 76, 185, 178,
                    229, 203, 152, 51, 107, 222, 204, 185, 179, 231, 207, 160, 67, 139, 30, 77,
                    186, 180, 233, 211, 168, 83, 171, 94, 205, 186, 181, 235, 215, 176, 99, 203,
                    158, 77, 187, 182, 237, 219, 184, 115, 235, 222, 205, 187, 183, 239, 223, 192,
                    131, 11, 31, 110, 26,
                ],
                "git.lz4" => &[
                    4, 34, 77, 24, 100, 64, 167, 142, 0, 0, 0, 143, 103, 105, 116, 46, 116, 120,
                    116, 0, 1, 0, 73, 113, 48, 48, 48, 48, 54, 52, 52, 8, 0, 60, 48, 48, 48, 8, 0,
                    68, 48, 48, 50, 53, 20, 0, 0, 2, 0, 175, 0, 48, 48, 55, 50, 50, 54, 0, 32, 48,
                    149, 0, 73, 4, 2, 0, 116, 117, 115, 116, 97, 114, 32, 32, 15, 0, 15, 2, 0, 221,
                    255, 6, 103, 105, 116, 32, 112, 117, 115, 104, 32, 111, 114, 105, 103, 105,
                    110, 32, 109, 97, 105, 110, 10, 5, 1, 221, 15, 2, 0, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    8, 80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 66, 37, 89, 243,
                ],
                "echo.lz4" => &[
                    4, 34, 77, 24, 100, 64, 167, 131, 0, 0, 0, 159, 101, 99, 104, 111, 46, 116,
                    120, 116, 0, 1, 0, 72, 113, 48, 48, 48, 48, 54, 52, 52, 8, 0, 60, 48, 48, 48,
                    8, 0, 68, 48, 48, 49, 51, 20, 0, 0, 2, 0, 175, 0, 48, 48, 55, 51, 53, 54, 0,
                    32, 48, 148, 0, 72, 5, 2, 0, 117, 117, 115, 116, 97, 114, 32, 32, 16, 0, 15, 2,
                    0, 220, 0, 0, 2, 127, 32, 104, 101, 108, 108, 111, 10, 250, 0, 220, 15, 2, 0,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 19, 80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 219, 52, 204, 225,
                ],
                "git.br" => &[
                    225, 248, 63, 1, 255, 76, 224, 77, 152, 87, 71, 64, 92, 120, 99, 164, 134, 57,
                    3, 169, 139, 108, 178, 199, 204, 16, 155, 14, 192, 5, 100, 165, 225, 233, 228,
                    248, 254, 37, 148, 66, 196, 45, 104, 1, 70, 42, 65, 38, 135, 29, 176, 122, 222,
                    100, 17, 27, 87, 1, 95, 11, 49, 66, 142, 166, 244, 79, 102, 56, 47, 133, 143,
                    13, 27, 147, 189, 6, 107, 62, 73, 31, 105, 7, 0, 0,
                ],
                "echo.br" => &[
                    225, 248, 63, 1, 63, 32, 115, 123, 138, 132, 120, 6, 98, 46, 184, 169, 147,
                    227, 251, 151, 80, 10, 17, 63, 105, 145, 75, 16, 72, 130, 193, 14, 88, 61, 111,
                    178, 232, 167, 242, 234, 254, 37, 208, 167, 27, 137, 134, 33, 11, 11, 95, 191,
                    117, 134, 16, 251, 149, 194, 198, 2, 180, 83, 242, 201, 124, 92, 59, 0, 0,
                ],
                "git.lzma" => &[
                    93, 0, 0, 128, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 51, 154, 74, 235,
                    19, 5, 33, 78, 97, 254, 189, 253, 181, 221, 24, 167, 147, 184, 229, 218, 174,
                    53, 143, 72, 13, 255, 62, 218, 44, 91, 8, 144, 172, 139, 180, 97, 114, 227, 24,
                    208, 195, 113, 46, 143, 230, 57, 142, 47, 21, 201, 184, 199, 175, 138, 90, 86,
                    79, 95, 15, 76, 177, 131, 230, 192, 50, 139, 137, 174, 85, 156, 28, 152, 77,
                    76, 219, 215, 235, 20, 205, 157, 176, 152, 105, 164, 197, 115, 162, 0, 119,
                    134, 111, 81, 109, 252, 152, 92, 104, 64, 223, 178, 198, 116, 228, 63, 181,
                    142, 81, 238, 127, 249, 24, 244, 192,
                ],
                "echo.lzma" => &[
                    93, 0, 0, 128, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 50, 152, 201, 79,
                    139, 152, 159, 47, 49, 49, 28, 82, 151, 13, 122, 209, 64, 66, 7, 64, 70, 74,
                    201, 106, 166, 127, 65, 192, 246, 106, 197, 83, 54, 132, 38, 255, 207, 208, 97,
                    208, 157, 101, 51, 14, 253, 131, 61, 210, 144, 138, 28, 99, 32, 178, 25, 197,
                    33, 235, 73, 162, 239, 115, 31, 179, 5, 85, 120, 37, 228, 29, 58, 161, 207,
                    223, 215, 208, 236, 208, 96, 97, 237, 179, 186, 40, 151, 156, 74, 244, 247,
                    250, 75, 235, 100, 131, 20, 13, 239, 181, 111, 255, 255, 67, 124, 0, 0,
                ],
                _ => &[],
            }
        }
        fn xz_bytes_raw(bytes: &[u8]) -> Vec<u8> {
            let mut input = std::io::Cursor::new(bytes);
            let mut out = Vec::new();
            lzma_rs::xz_compress(&mut input, &mut out).expect("xz raw");
            out
        }
        fn bzip_bytes(text: &str) -> Vec<u8> {
            use std::io::Write;
            let mut encoder =
                bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
            encoder.write_all(text.as_bytes()).expect("bzip write");
            encoder.finish().expect("bzip finish")
        }
        fn xz_bytes(text: &str) -> Vec<u8> {
            let mut input = std::io::Cursor::new(text.as_bytes());
            let mut out = Vec::new();
            lzma_rs::xz_compress(&mut input, &mut out).expect("xz");
            out
        }
        fn lzma_bytes(text: &str) -> Vec<u8> {
            let mut input = std::io::Cursor::new(text.as_bytes());
            let mut out = Vec::new();
            lzma_rs::lzma_compress(&mut input, &mut out).expect("lzma");
            out
        }
        fn pzstd_bytes(text: &str) -> Vec<u8> {
            let mut out = vec![0x50, 0x2a, 0x4d, 0x18, 4, 0, 0, 0, 0x22, 0, 0, 0];
            out.extend(zstd_bytes(text));
            out
        }
        fn zstd_bytes(text: &str) -> Vec<u8> {
            ruzstd::encoding::compress_to_vec(
                text.as_bytes(),
                ruzstd::encoding::CompressionLevel::Fastest,
            )
        }
        fn lz4_bytes(text: &str) -> Vec<u8> {
            use std::io::Write;
            let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
            encoder.write_all(text.as_bytes()).expect("lz4 write");
            encoder.finish().expect("lz4 finish")
        }
        fn tar_bytes(name: &str, text: &str) -> Vec<u8> {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).expect("tar path");
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            let mut builder = tar::Builder::new(Vec::new());
            builder
                .append(&header, text.as_bytes())
                .expect("tar append");
            builder.into_inner().expect("tar finish")
        }
        fn zip_bytes(name: &str, text: &str) -> Vec<u8> {
            use std::io::Write;
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut writer = zip::ZipWriter::new(&mut cursor);
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                writer.start_file(name, options).expect("zip start");
                writer.write_all(text.as_bytes()).expect("zip write");
                writer.finish().expect("zip finish");
            }
            cursor.into_inner()
        }
        fn brotli_bytes(text: &str) -> &'static [u8] {
            match text {
                "git push origin main\n" => &[
                    15, 10, 128, 103, 105, 116, 32, 112, 117, 115, 104, 32, 111, 114, 105, 103,
                    105, 110, 32, 109, 97, 105, 110, 10, 3,
                ],
                "echo hello\n" => &[
                    15, 5, 128, 101, 99, 104, 111, 32, 104, 101, 108, 108, 111, 10, 3,
                ],
                _ => &[],
            }
        }
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let push = dir.join(format!("snapif-c29-push-{pid}.sh"));
        let echo = dir.join(format!("snapif-c29-echo-{pid}.sh"));
        let assign = dir.join(format!("snapif-c29-assign-{pid}.sh"));
        let multi = dir.join(format!("snapif-c29-multi-{pid}.txt"));
        let big = dir.join(format!("snapif-c29-big-{pid}.sh"));
        std::fs::write(&push, "git push origin main\n").expect("push script");
        std::fs::write(&echo, "echo hello\n").expect("echo script");
        std::fs::write(&assign, "x='git push origin main'\n").expect("assign script");
        std::fs::write(&multi, "alpha\nbeta\ngit push origin main\n").expect("multi script");
        let beta_last = dir.join(format!("snapif-c33-beta-last-{pid}.txt"));
        std::fs::write(&beta_last, "git push origin main\nbeta\n").expect("beta last");
        let coded = dir.join(format!("snapif-c30-b64-{pid}.txt"));
        std::fs::write(&coded, "Z2l0IHB1c2ggb3JpZ2luIG1haW4K\n").expect("b64");
        let echo_b64 = dir.join(format!("snapif-c36-echo-b64-{pid}.txt"));
        std::fs::write(&echo_b64, "ZWNobyBoZWxsbwo=\n").expect("echo b64");
        let uu_push = dir.join(format!("snapif-c37-uu-{pid}.txt"));
        std::fs::write(
            &uu_push,
            "begin 644 git.txt\n59VET('!U<V@@;W)I9VEN(&UA:6X*\n`\nend\n",
        )
        .expect("uu");
        let uu_hello = dir.join(format!("snapif-c37-uu-hello-{pid}.txt"));
        std::fs::write(
            &uu_hello,
            "begin 644 hello.txt\n+96-H;R!H96QL;PH`\n`\nend\n",
        )
        .expect("uu hello");
        let hex_push = dir.join(format!("snapif-c38-hex-{pid}.txt"));
        std::fs::write(&hex_push, "6769742070757368206f726967696e206d61696e0a\n").expect("hex");
        let hex_hello = dir.join(format!("snapif-c38-hex-hello-{pid}.txt"));
        std::fs::write(&hex_hello, "6563686f2068656c6c6f0a\n").expect("hex hello");
        let b32 = dir.join(format!("snapif-c31-b32-{pid}.txt"));
        std::fs::write(&b32, "M5UXIIDQOVZWQIDPOJUWO2LOEBWWC2LOBI======\n").expect("b32");
        let b32_hello = dir.join(format!("snapif-c38-b32-hello-{pid}.txt"));
        std::fs::write(&b32_hello, "MVRWQ3ZANBSWY3DPBI======\n").expect("b32 hello");
        let b32hex = dir.join(format!("snapif-c38-b32hex-{pid}.txt"));
        std::fs::write(&b32hex, "CTKN883GELPMG83FE9KMEQBE41MM2QBE18======\n").expect("b32hex");
        let b32hex_hello = dir.join(format!("snapif-c38-b32hex-hello-{pid}.txt"));
        std::fs::write(&b32hex_hello, "CLHMGRP0D1IMOR3F18======\n").expect("b32hex hello");
        let z85 = dir.join(format!("snapif-c38-z85-{pid}.txt"));
        std::fs::write(&z85, "xk8DgAclknaA8pmxk8lazddc@\n").expect("z85");
        let z85_hello = dir.join(format!("snapif-c38-z85-hello-{pid}.txt"));
        std::fs::write(&z85_hello, "wN/y(aztt5y&oT/\n").expect("z85 hello");
        let b58 = dir.join(format!("snapif-c38-b58-{pid}.txt"));
        std::fs::write(&b58, "7MpYVGLu1NHfckZT9eDc9bocU71gu\n").expect("b58");
        let b58_hello = dir.join(format!("snapif-c38-b58-hello-{pid}.txt"));
        std::fs::write(&b58_hello, "S9DsdjxTzkNw3uT\n").expect("b58 hello");
        let gz = dir.join(format!("snapif-c39-gz-{pid}.gz"));
        std::fs::write(&gz, gzip_bytes("git push origin main\n")).expect("gz");
        let gz_hello = dir.join(format!("snapif-c39-gz-hello-{pid}.gz"));
        std::fs::write(&gz_hello, gzip_bytes("echo hello\n")).expect("gz hello");
        let zl = dir.join(format!("snapif-c40-zl-{pid}.zlib"));
        std::fs::write(&zl, zlib_bytes("git push origin main\n")).expect("zlib");
        let zl_hello = dir.join(format!("snapif-c40-zl-hello-{pid}.zlib"));
        std::fs::write(&zl_hello, zlib_bytes("echo hello\n")).expect("zlib hello");
        let df = dir.join(format!("snapif-c41-df-{pid}.deflate"));
        std::fs::write(&df, deflate_bytes("git push origin main\n")).expect("deflate");
        let df_hello = dir.join(format!("snapif-c41-df-hello-{pid}.deflate"));
        std::fs::write(&df_hello, deflate_bytes("echo hello\n")).expect("deflate hello");
        let bz = dir.join(format!("snapif-c39-bz-{pid}.bz2"));
        std::fs::write(&bz, bzip_bytes("git push origin main\n")).expect("bz");
        let bz_hello = dir.join(format!("snapif-c39-bz-hello-{pid}.bz2"));
        std::fs::write(&bz_hello, bzip_bytes("echo hello\n")).expect("bz hello");
        let xz_file = dir.join(format!("snapif-c39-xz-{pid}.xz"));
        std::fs::write(&xz_file, xz_bytes("git push origin main\n")).expect("xz");
        let xz_hello = dir.join(format!("snapif-c39-xz-hello-{pid}.xz"));
        std::fs::write(&xz_hello, xz_bytes("echo hello\n")).expect("xz hello");
        let lzma_file = dir.join(format!("snapif-c39-lzma-{pid}.lzma"));
        std::fs::write(&lzma_file, lzma_bytes("git push origin main\n")).expect("lzma");
        let lzma_hello = dir.join(format!("snapif-c39-lzma-hello-{pid}.lzma"));
        std::fs::write(&lzma_hello, lzma_bytes("echo hello\n")).expect("lzma hello");
        let zst = dir.join(format!("snapif-c39-zst-{pid}.zst"));
        std::fs::write(&zst, zstd_bytes("git push origin main\n")).expect("zst");
        let zst_hello = dir.join(format!("snapif-c39-zst-hello-{pid}.zst"));
        std::fs::write(&zst_hello, zstd_bytes("echo hello\n")).expect("zst hello");
        let pz = dir.join(format!("snapif-c39-pz-{pid}.zst"));
        std::fs::write(&pz, pzstd_bytes("git push origin main\n")).expect("pz");
        let tar_file = dir.join(format!("snapif-c39-tar-{pid}.tar"));
        std::fs::write(&tar_file, tar_bytes("git.txt", "git push origin main\n")).expect("tar");
        let tar_hello = dir.join(format!("snapif-c39-tar-hello-{pid}.tar"));
        std::fs::write(&tar_hello, tar_bytes("echo.txt", "echo hello\n")).expect("tar hello");
        let tgz = dir.join(format!("snapif-c39-tgz-{pid}.tar.gz"));
        std::fs::write(
            &tgz,
            gzip_bytes_raw(&tar_bytes("git.txt", "git push origin main\n")),
        )
        .expect("tgz");
        let tgz_hello = dir.join(format!("snapif-c39-tgz-hello-{pid}.tar.gz"));
        std::fs::write(
            &tgz_hello,
            gzip_bytes_raw(&tar_bytes("echo.txt", "echo hello\n")),
        )
        .expect("tgz hello");
        let txz = dir.join(format!("snapif-c39-txz-{pid}.tar.xz"));
        std::fs::write(
            &txz,
            xz_bytes_raw(&tar_bytes("git.txt", "git push origin main\n")),
        )
        .expect("txz");
        let tbz = dir.join(format!("snapif-c39-tbz-{pid}.tar.bz2"));
        std::fs::write(
            &tbz,
            bzip_bytes_raw(&tar_bytes("git.txt", "git push origin main\n")),
        )
        .expect("tbz");
        let tzst = dir.join(format!("snapif-c39-tzst-{pid}.tar.zst"));
        std::fs::write(
            &tzst,
            zstd_bytes_raw(&tar_bytes("git.txt", "git push origin main\n")),
        )
        .expect("tzst");
        let zed = dir.join(format!("snapif-c39-Z-{pid}.Z"));
        std::fs::write(&zed, compress_bytes("git")).expect("Z");
        let zed_hello = dir.join(format!("snapif-c39-Z-hello-{pid}.Z"));
        std::fs::write(&zed_hello, compress_bytes("echo")).expect("Z hello");
        let tarz = dir.join(format!("snapif-c39-tarz-{pid}.tar.Z"));
        std::fs::write(&tarz, wrapped_tar("git.Z")).expect("tarz");
        let tarz_hello = dir.join(format!("snapif-c39-tarz-hello-{pid}.tar.Z"));
        std::fs::write(&tarz_hello, wrapped_tar("echo.Z")).expect("tarz hello");
        let tlz4 = dir.join(format!("snapif-c39-tlz4-{pid}.tar.lz4"));
        std::fs::write(&tlz4, wrapped_tar("git.lz4")).expect("tlz4");
        let tlz4_hello = dir.join(format!("snapif-c39-tlz4-hello-{pid}.tar.lz4"));
        std::fs::write(&tlz4_hello, wrapped_tar("echo.lz4")).expect("tlz4 hello");
        let tbr = dir.join(format!("snapif-c39-tbr-{pid}.tar.br"));
        std::fs::write(&tbr, wrapped_tar("git.br")).expect("tbr");
        let tbr_hello = dir.join(format!("snapif-c39-tbr-hello-{pid}.tar.br"));
        std::fs::write(&tbr_hello, wrapped_tar("echo.br")).expect("tbr hello");
        let tlzma = dir.join(format!("snapif-c40-tlzma-{pid}.tar.lzma"));
        std::fs::write(&tlzma, wrapped_tar("git.lzma")).expect("tlzma");
        let tlzma_hello = dir.join(format!("snapif-c40-tlzma-hello-{pid}.tar.lzma"));
        std::fs::write(&tlzma_hello, wrapped_tar("echo.lzma")).expect("tlzma hello");
        let zip_file = dir.join(format!("snapif-c39-zip-{pid}.zip"));
        std::fs::write(&zip_file, zip_bytes("git.txt", "git push origin main\n")).expect("zip");
        let zip_hello = dir.join(format!("snapif-c39-zip-hello-{pid}.zip"));
        std::fs::write(&zip_hello, zip_bytes("echo.txt", "echo hello\n")).expect("zip hello");
        let seven_src = dir.join(format!("snapif-c39-7zsrc-{pid}"));
        let seven_hello_src = dir.join(format!("snapif-c39-7zhello-{pid}"));
        std::fs::create_dir_all(&seven_src).expect("7z dir");
        std::fs::create_dir_all(&seven_hello_src).expect("7z hello dir");
        std::fs::write(seven_src.join("git.txt"), "git push origin main\n").expect("7z src");
        std::fs::write(seven_hello_src.join("echo.txt"), "echo hello\n").expect("7z hello src");
        let seven_file = dir.join(format!("snapif-c39-7z-{pid}.7z"));
        let seven_cursor =
            sevenz_rust2::compress(&seven_src, std::io::Cursor::new(Vec::new())).expect("7z");
        std::fs::write(&seven_file, seven_cursor.into_inner()).expect("7z write");
        let seven_hello = dir.join(format!("snapif-c39-7z-hello-{pid}.7z"));
        let seven_hello_cursor =
            sevenz_rust2::compress(&seven_hello_src, std::io::Cursor::new(Vec::new()))
                .expect("7z hello");
        std::fs::write(&seven_hello, seven_hello_cursor.into_inner()).expect("7z hello write");
        std::fs::remove_dir_all(&seven_src).ok();
        std::fs::remove_dir_all(&seven_hello_src).ok();
        let lz4_file = dir.join(format!("snapif-c39-lz4-{pid}.lz4"));
        std::fs::write(&lz4_file, lz4_bytes("git push origin main\n")).expect("lz4");
        let lz4_hello = dir.join(format!("snapif-c39-lz4-hello-{pid}.lz4"));
        std::fs::write(&lz4_hello, lz4_bytes("echo hello\n")).expect("lz4 hello");
        let br = dir.join(format!("snapif-c39-br-{pid}.br"));
        std::fs::write(&br, brotli_bytes("git push origin main\n")).expect("br");
        let br_hello = dir.join(format!("snapif-c39-br-hello-{pid}.br"));
        std::fs::write(&br_hello, brotli_bytes("echo hello\n")).expect("br hello");
        let b2m = dir.join(format!("snapif-c38-b2m-{pid}.txt"));
        std::fs::write(
            &b2m,
            "0110011101101001011101000010000001110000011101010111001101101000001000000110\n1111011100100110100101100111011010010110111000100000011011010110000101101001\n0110111000001010\n",
        )
        .expect("b2m");
        let b2l = dir.join(format!("snapif-c38-b2l-{pid}.txt"));
        std::fs::write(
            &b2l,
            "1110011010010110001011100000010000001110101011101100111000010110000001001111\n0110010011101001011011100110100101100111011000000100101101101000011010010110\n0111011001010000\n",
        )
        .expect("b2l");
        let b2m_hello = dir.join(format!("snapif-c38-b2m-hello-{pid}.txt"));
        std::fs::write(
            &b2m_hello,
            "0110010101100011011010000110111100100000011010000110010101101100011011000110\n111100001010\n",
        )
        .expect("b2m hello");
        let b2l_hello = dir.join(format!("snapif-c38-b2l-hello-{pid}.txt"));
        std::fs::write(
            &b2l_hello,
            "1010011011000110000101101111011000000100000101101010011000110110001101101111\n011001010000\n",
        )
        .expect("b2l hello");
        let dup = dir.join(format!("snapif-c30-dup-{pid}.txt"));
        std::fs::write(&dup, "git push origin main\ngit push origin main\n").expect("dup");
        let mut big_body = b"git push origin main\n".to_vec();
        big_body.resize(super::SCRIPT_FILE_CAP as usize + 1, b' ');
        std::fs::write(&big, &big_body).expect("big script");
        let fread_early = dir.join(format!("snapif-c47-early-{pid}.txt"));
        let mut early_body = b"git push origin main\n".to_vec();
        early_body.extend(std::iter::repeat_n(b'x', 5000));
        std::fs::write(&fread_early, &early_body).expect("fread early");
        let fread_late = dir.join(format!("snapif-c47-late-{pid}.txt"));
        let mut late_body = vec![b'x'; 4096];
        late_body.extend(b"\ngit push origin main\n");
        std::fs::write(&fread_late, &late_body).expect("fread late");
        let push_s = plain_script_path(&push);
        let echo_s = plain_script_path(&echo);
        let early_s = plain_script_path(&fread_early);
        let late_s = plain_script_path(&fread_late);
        let assign_s = plain_script_path(&assign);
        let multi_s = plain_script_path(&multi);
        let beta_last_s = plain_script_path(&beta_last);
        let coded_s = plain_script_path(&coded);
        let echo_b64_s = plain_script_path(&echo_b64);
        let uu_push_s = plain_script_path(&uu_push);
        let uu_hello_s = plain_script_path(&uu_hello);
        let hex_push_s = plain_script_path(&hex_push);
        let hex_hello_s = plain_script_path(&hex_hello);
        let b32_s = plain_script_path(&b32);
        let b32_hello_s = plain_script_path(&b32_hello);
        let b32hex_s = plain_script_path(&b32hex);
        let b32hex_hello_s = plain_script_path(&b32hex_hello);
        let z85_s = plain_script_path(&z85);
        let z85_hello_s = plain_script_path(&z85_hello);
        let b58_s = plain_script_path(&b58);
        let gz_s = plain_script_path(&gz);
        let gz_hello_s = plain_script_path(&gz_hello);
        let zl_s = plain_script_path(&zl);
        let zl_hello_s = plain_script_path(&zl_hello);
        let df_s = plain_script_path(&df);
        let df_hello_s = plain_script_path(&df_hello);
        let bz_s = plain_script_path(&bz);
        let bz_hello_s = plain_script_path(&bz_hello);
        let xz_s = plain_script_path(&xz_file);
        let xz_hello_s = plain_script_path(&xz_hello);
        let lzma_s = plain_script_path(&lzma_file);
        let lzma_hello_s = plain_script_path(&lzma_hello);
        let zst_s = plain_script_path(&zst);
        let zst_hello_s = plain_script_path(&zst_hello);
        let pz_s = plain_script_path(&pz);
        let tar_s = plain_script_path(&tar_file);
        let tar_hello_s = plain_script_path(&tar_hello);
        let tgz_s = plain_script_path(&tgz);
        let tgz_hello_s = plain_script_path(&tgz_hello);
        let txz_s = plain_script_path(&txz);
        let tbz_s = plain_script_path(&tbz);
        let tzst_s = plain_script_path(&tzst);
        let zed_s = plain_script_path(&zed);
        let zed_hello_s = plain_script_path(&zed_hello);
        let tarz_s = plain_script_path(&tarz);
        let tarz_hello_s = plain_script_path(&tarz_hello);
        let tlz4_s = plain_script_path(&tlz4);
        let tlz4_hello_s = plain_script_path(&tlz4_hello);
        let tbr_s = plain_script_path(&tbr);
        let tbr_hello_s = plain_script_path(&tbr_hello);
        let tlzma_s = plain_script_path(&tlzma);
        let tlzma_hello_s = plain_script_path(&tlzma_hello);
        let pytar = dir.join(format!("snapif-c40-pytar-{pid}.py"));
        let pyzip = dir.join(format!("snapif-c40-pyzip-{pid}.py"));
        std::fs::write(
            &pytar,
            "import sys\nimport tarfile\nmode = sys.argv[1]\npath = sys.argv[2]\nmember = sys.argv[3]\nwith tarfile.open(path, mode) as archive:\n    extracted = archive.extractfile(member)\n    sys.stdout.buffer.write(extracted.read())\n",
        )
        .expect("pytar");
        std::fs::write(
            &pyzip,
            "import sys\nimport zipfile\nwith zipfile.ZipFile(sys.argv[1]) as archive:\n    sys.stdout.buffer.write(archive.read(sys.argv[2]))\n",
        )
        .expect("pyzip");
        let pytar_s = plain_script_path(&pytar);
        let pyzip_s = plain_script_path(&pyzip);
        let zip_s = plain_script_path(&zip_file);
        let zip_hello_s = plain_script_path(&zip_hello);
        let seven_s = plain_script_path(&seven_file);
        let seven_hello_s = plain_script_path(&seven_hello);
        let lz4_s = plain_script_path(&lz4_file);
        let lz4_hello_s = plain_script_path(&lz4_hello);
        let br_s = plain_script_path(&br);
        let br_hello_s = plain_script_path(&br_hello);
        let b58_hello_s = plain_script_path(&b58_hello);
        let b2m_s = plain_script_path(&b2m);
        let b2l_s = plain_script_path(&b2l);
        let b2m_hello_s = plain_script_path(&b2m_hello);
        let b2l_hello_s = plain_script_path(&b2l_hello);
        let dup_s = plain_script_path(&dup);
        let big_s = plain_script_path(&big);
        let names = dir.join(format!("snapif-c30-names-{pid}.txt"));
        let mut names_body = push_s.as_bytes().to_vec();
        names_body.push(0);
        std::fs::write(&names, names_body).expect("names");
        let names_s = plain_script_path(&names);
        let tee_out = dir.join(format!("snapif-c31-tee-{pid}.txt"));
        let tee_s = plain_script_path(&tee_out);
        let missing = dir.join(format!("snapif-c29-missing-{pid}.sh"));
        let missing2 = dir.join(format!("snapif-c29-missing2-{pid}.sh"));
        let _ = std::fs::remove_file(&missing);
        let _ = std::fs::remove_file(&missing2);
        let missing_s = plain_script_path(&missing);
        let missing2_s = plain_script_path(&missing2);
        let sub = dir.join(format!("snapif-c29-dir-{pid}"));
        let _ = std::fs::remove_dir(&sub);
        std::fs::create_dir(&sub).expect("dir");
        let sub_s = plain_script_path(&sub);

        let positives = [
            format!("source {push_s}"),
            format!(". {push_s}"),
            format!("source -- {push_s}"),
            format!("bash {push_s}"),
            format!("bash -- {push_s}"),
            format!("cat {push_s}"),
            format!("head {push_s}"),
            format!("tail {push_s}"),
            format!("gcat {push_s}"),
            format!("ghead {push_s}"),
            format!("gtail {push_s}"),
            format!("sh {push_s}"),
            format!("command bash {push_s}"),
            format!("source {assign_s}; eval \"$x\""),
            format!("command source {push_s}"),
            format!("eval \"$(cat {push_s})\""),
            format!("eval \"$(cat -v {push_s})\""),
            format!("eval \"$(cat -e {push_s})\""),
            format!("eval \"$(cat -t {push_s})\""),
            format!("eval \"$(cat -s {push_s})\""),
            format!("eval \"$(cat -A {push_s})\""),
            format!("eval \"$(cat -E {push_s})\""),
            format!("eval \"$(cat -T {push_s})\""),
            format!("eval \"$(cat -vet {push_s})\""),
            format!("eval \"$(tee {tee_s} < {push_s})\""),
            format!("eval \"$(cat {push_s} | tee)\""),
            format!("eval \"$(tr a-z A-Z < {push_s})\""),
            format!("eval \"$(iconv -f utf-8 -t utf-8 {push_s})\""),
            format!("eval \"$(iconv -f UTF-8 -t utf8 {push_s})\""),
            format!("eval \"$(join {push_s} {push_s})\""),
            format!("eval \"$(gjoin {push_s} {push_s})\""),
            format!("eval \"$(gbase32 -d {b32_s})\""),
            format!("eval \"$(comm -12 {push_s} {push_s})\""),
            format!("eval \"$(gcomm -12 {push_s} {push_s})\""),
            format!("eval \"$(gcomm {push_s} {push_s})\""),
            format!("eval \"$(gcomm -1 -2 {push_s} {push_s})\""),
            format!("eval \"$(perl -pe '' {push_s})\""),
            format!("eval \"$(ruby -pe '' {push_s})\""),
            format!("eval \"$(perl -ne 'print' {push_s})\""),
            format!("eval \"$(perl -ne 'print;' {push_s})\""),
            format!("eval \"$(perl -ne 'print $_' {push_s})\""),
            format!("eval \"$(ruby -ne 'print' {push_s})\""),
            format!("eval \"$(ruby -ne 'print $_' {push_s})\""),
            format!("eval \"$(perl -pe 'print' {push_s})\""),
            format!("eval \"$(perl -pe 'print $_' {push_s})\""),
            format!("eval \"$(perl -pe '1' {push_s})\""),
            format!("eval \"$(perl -pe '2' {push_s})\""),
            format!("eval \"$(perl -0777 -pe '' {push_s})\""),
            format!("eval \"$(perl -0 -pe '' {push_s})\""),
            format!("eval \"$(perl -lpe '' {push_s})\""),
            format!("eval \"$(perl -w -pe '' {push_s})\""),
            format!("eval \"$(perl -wpe '' {push_s})\""),
            format!("eval \"$(perl -W -pe '' {push_s})\""),
            format!("eval \"$(ruby -W0 -ne 'print' {push_s})\""),
            format!(
                "eval \"$(python3 -I -c 'import sys; print(open(sys.argv[1]).read())' {push_s})\""
            ),
            format!("eval \"$(perl -l -pe '' {push_s})\""),
            format!("eval \"$(ruby -nle 'print' {push_s})\""),
            format!("eval \"$(perl -0777 -ne 'print' {push_s})\""),
            format!("eval \"$(ruby -pe 'print' {push_s})\""),
            format!("eval \"$(ruby -e 'puts File.read(ARGV[0])' {push_s})\""),
            format!("eval \"$(ruby -e 'print IO.read(ARGV[0])' {push_s})\""),
            format!("eval \"$(ruby -e 'puts IO.read(ARGV[0])' {push_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; from pathlib import Path; print(Path(sys.argv[1]).read_text())' {push_s})\""
            ),
            format!(
                "eval \"$(node -e 'console.log(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))' {push_s})\""
            ),
            format!("eval \"$(php -r 'print file_get_contents($argv[1]);' {push_s})\""),
            format!("eval \"$(php -r 'readfile($argv[1]);' {push_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; print(open(sys.argv[1]).read())' {push_s})\""
            ),
            format!(
                "eval \"$(node -e 'const fs=require(\"fs\"); process.stdout.write(fs.readFileSync(process.argv[1],\"utf8\"))' {push_s})\""
            ),
            format!("eval \"$(awk '{{printf \"%s\\n\", $0}}' {push_s})\""),
            format!("eval \"$(awk '{{printf \"%s\\n\", $0}}' {multi_s})\""),
            format!("eval \"$(awk '{{printf \"%s\", $0}}' {push_s})\""),
            format!("eval \"$(awk '{{printf $0}}' {push_s})\""),
            format!("eval \"$(awk 'END {{print}}' {push_s})\""),
            format!("eval \"$(awk 'END {{print}}' {multi_s})\""),
            format!("eval \"$(awk 'END {{print $0}}' {push_s})\""),
            format!("eval \"$(ruby -e 'STDOUT.write IO.read(ARGV[0])' {push_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; print(open(sys.argv[1], encoding=\"utf-8\").read())' {push_s})\""
            ),
            format!("eval \"$(php -r 'echo readfile($argv[1]);' {push_s})\""),
            format!(
                "eval \"$(python3 -c 'import pathlib,sys; sys.stdout.write(pathlib.Path(sys.argv[1]).read_text())' {push_s})\""
            ),
            format!(
                r#"eval "$(python3 -c "import pathlib,sys; sys.stdout.buffer.write(pathlib.Path(sys.argv[1]).read_bytes())" {push_s})""#
            ),
            format!(r#"eval "$(ruby -e 'print File.binread(ARGV[0])' {push_s})""#),
            format!(r#"eval "$(ruby -e 'print IO.binread(ARGV[0])' {push_s})""#),
            format!(r#"eval "$(ruby -e 'print File.open(ARGV[0]).read' {push_s})""#),
            format!(r#"eval "$(ruby -e 'print File.readlines(ARGV[0]).join' {push_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; sys.stdout.buffer.write(Path(sys.argv[1]).read_bytes())" {push_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1],'utf8'))" {push_s})""#
            ),
            format!(r#"eval "$(perl -e 'print <>' {push_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.write(open(sys.argv[1],'rb').read().decode())" {push_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys; sys.stdout.write(open(sys.argv[1],"rb").read().decode())' {push_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.buffer.write(open(sys.argv[1],'rb').read())" {push_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys; sys.stdout.buffer.write(open(sys.argv[1],"rb").read())' {push_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1]))" {push_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); process.stdout.write(fs.readFileSync(process.argv[1]))' {push_s})""#
            ),
            format!("eval \"$(base32 -d {b32_s})\""),
            format!("eval \"$(base32 -d -i {b32_s})\""),
            format!("eval \"$(base32 -d --ignore-garbage {b32_s})\""),
            format!("eval \"$(base32 -di {b32_s})\""),
            format!("eval \"$(gbase32 -d -i {b32_s})\""),
            format!("eval \"$(cut -d ' ' -f 1- {push_s})\""),
            format!("eval \"$(sed -n l {push_s})\""),
            format!("eval \"$(sed -n '/git/p' {push_s})\""),
            format!("eval \"$(sed -n '/push/p' {push_s})\""),
            format!("eval \"$(sed -n 's/git/git/p' {push_s})\""),
            format!("eval \"$(sed -n 's/push/push/p' {push_s})\""),
            format!("eval \"$(sed 's/zzzz/no/' {push_s})\""),
            format!("eval \"$(sed -n 1l {push_s})\""),
            format!("eval \"$(sed -n '$l' {push_s})\""),
            format!("eval \"$(sed -n 3l {multi_s})\""),
            format!("eval \"$(sed -n '$l' {multi_s})\""),
            format!("eval \"$(gsed -n 1l {push_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; sys.stdout.write(open(sys.argv[1]).read())' {push_s})\""
            ),
            format!(
                "eval \"$(python3 -c 'import sys; print(open(sys.argv[1]).read(), end=\"\")' {push_s})\""
            ),
            format!(
                "eval \"$(node -e 'process.stdout.write(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))' {push_s})\""
            ),
            format!(
                "eval \"$(node --eval 'process.stdout.write(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))' {push_s})\""
            ),
            format!("eval \"$(ruby -e 'print File.read(ARGV[0])' {push_s})\""),
            format!("eval \"$(php -r 'echo file_get_contents($argv[1]);' {push_s})\""),
            format!(
                "eval \"$(python3 -c 'import os,sys; os.system(open(sys.argv[1]).read())' {push_s})\""
            ),
            format!(
                "eval \"$(node -e 'require(\"child_process\").execSync(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))' {push_s})\""
            ),
            format!("eval \"$(ruby -e 'system(File.read(ARGV[0]))' {push_s})\""),
            format!("eval \"$(ruby -e 'print File.binread(ARGV[0]).unpack1(\"m\")' {coded_s})\""),
            format!(
                r#"eval "$(python3 -c 'import base64,sys; sys.stdout.buffer.write(base64.b64decode(open(sys.argv[1],"rb").read()))' {coded_s})""#
            ),
            format!(
                "eval \"$(php -r 'echo base64_decode(file_get_contents($argv[1]));' {coded_s})\""
            ),
            format!(
                r#"eval "$(node -e 'process.stdout.write(Buffer.from(require("fs").readFileSync(process.argv[1]).toString(),"base64"))' {coded_s})""#
            ),
            format!("eval \"$(perl -MMIME::Base64 -0777 -e 'print decode_base64(<>)' {coded_s})\""),
            format!("eval \"$(basenc --base64 -d {coded_s})\""),
            format!("eval \"$(gbasenc --base64 -d {coded_s})\""),
            format!("eval \"$(uudecode -p {uu_push_s})\""),
            format!("eval \"$(xxd -r -p {hex_push_s})\""),
            format!("eval \"$(xxd -p -r {hex_push_s})\""),
            format!("eval \"$(xxd -r -ps {hex_push_s})\""),
            format!("eval \"$(xxd -r -p < {hex_push_s})\""),
            format!("eval \"$(basenc --base16 -d {hex_push_s})\""),
            format!("eval \"$(basenc -d --base16 {hex_push_s})\""),
            format!("eval \"$(basenc --base16 --decode {hex_push_s})\""),
            format!("eval \"$(basenc --base16 -d < {hex_push_s})\""),
            format!("eval \"$(gbasenc --base16 -d {hex_push_s})\""),
            format!("eval \"$(basenc --base32 -d {b32_s})\""),
            format!("eval \"$(basenc -d --base32 {b32_s})\""),
            format!("eval \"$(basenc --base32 --decode {b32_s})\""),
            format!("eval \"$(basenc --base32 -d < {b32_s})\""),
            format!("eval \"$(gbasenc --base32 -d {b32_s})\""),
            format!("eval \"$(basenc --base64url -d {coded_s})\""),
            format!("eval \"$(basenc --base32hex -d {b32hex_s})\""),
            format!("eval \"$(basenc --base2msbf -d {b2m_s})\""),
            format!("eval \"$(basenc --base2lsbf -d {b2l_s})\""),
            format!("eval \"$(basenc --z85 -d {z85_s})\""),
            format!("eval \"$(gbasenc --base64url -d {coded_s})\""),
            format!("eval \"$(basenc --base64 -d -i {coded_s})\""),
            format!("eval \"$(basenc --base64 -d -w0 {coded_s})\""),
            format!("eval \"$(basenc --base64 -d --wrap=0 {coded_s})\""),
            format!("eval \"$(basenc --base64 -d --wrap 0 {coded_s})\""),
            format!("eval \"$(basenc --base58 -d {b58_s})\""),
            format!("eval \"$(gbasenc --base58 -d {b58_s})\""),
            format!(
                "eval \"$(ruby -rbase64 -e 'print Base64.decode64(File.binread(ARGV[0]))' {coded_s})\""
            ),
            format!(
                r#"eval "$(python3 -c 'import binascii,sys; sys.stdout.buffer.write(binascii.a2b_base64(open(sys.argv[1],"rb").read()))' {coded_s})""#
            ),
            format!("eval \"$(basenc --base64 --decode {coded_s})\""),
            format!("eval \"$(basenc -d --base64 {coded_s})\""),
            format!("eval \"$(basenc --base64 -d < {coded_s})\""),
            format!("eval \"$(python3 -m base64 -d {coded_s})\""),
            format!("eval \"$(python3 -m base64 -d < {coded_s})\""),
            format!("eval \"$(python3 -m base64 -u {coded_s})\""),
            format!("eval \"$(gzip -dc {gz_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys,gzip; sys.stdout.buffer.write(gzip.open(sys.argv[1],\"rb\").read())' {gz_s})\""
            ),
            format!(
                "eval \"$(python3 -c 'import sys,bz2; sys.stdout.buffer.write(bz2.open(sys.argv[1],\"rb\").read())' {bz_s})\""
            ),
            format!(
                "eval \"$(python3 -c 'import sys,lzma; sys.stdout.buffer.write(lzma.open(sys.argv[1],\"rb\").read())' {xz_s})\""
            ),
            format!(
                "eval \"$(ruby -rzlib -e 'print Zlib::GzipReader.open(ARGV[0]).read' {gz_s})\""
            ),
            format!(
                "eval \"$(node -e 'const fs=require(\"fs\"); const z=require(\"zlib\"); process.stdout.write(z.gunzipSync(fs.readFileSync(process.argv[1])))' {gz_s})\""
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.buffer.write(gzip.open(sys.argv[1],'rb').read())" {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.buffer.write(bz2.open(sys.argv[1],'rb').read())" {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.open(sys.argv[1],'rb').read())" {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import gzip,sys; sys.stdout.buffer.write(gzip.decompress(open(sys.argv[1],"rb").read()))' {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import gzip,sys; sys.stdout.buffer.write(gzip.decompress(open(sys.argv[1],'rb').read()))" {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import bz2,sys; sys.stdout.buffer.write(bz2.decompress(open(sys.argv[1],"rb").read()))' {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import bz2,sys; sys.stdout.buffer.write(bz2.decompress(open(sys.argv[1],'rb').read()))" {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import lzma,sys; sys.stdout.buffer.write(lzma.decompress(open(sys.argv[1],"rb").read()))' {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import lzma,sys; sys.stdout.buffer.write(lzma.decompress(open(sys.argv[1],'rb').read()))" {xz_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib.gunzip(File.binread(ARGV[0]))' {gz_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib.gunzip(File.binread(ARGV[0]))' {gz_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::Gunzip=gunzip -e 'gunzip $ARGV[0] => \*STDOUT' {gz_s})""#
            ),
            format!(r#"eval "$(php -r 'echo gzdecode(file_get_contents($argv[1]));' {gz_s})""#),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.gunzipSync(fs.readFileSync(process.argv[1])))" {gz_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))' {gz_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))" {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zlib; sys.stdout.buffer.write(zlib.decompress(open(sys.argv[1],"rb").read()))' {zl_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,zlib; sys.stdout.buffer.write(zlib.decompress(open(sys.argv[1],'rb').read()))" {zl_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.inflateSync(fs.readFileSync(process.argv[1])))' {zl_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.inflateSync(fs.readFileSync(process.argv[1])))" {zl_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))' {zl_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))" {zl_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::Bunzip2=bunzip2 -e 'bunzip2 $ARGV[0] => \*STDOUT' {bz_s})""#
            ),
            format!(r#"eval "$(php -r 'echo bzdecompress(file_get_contents($argv[1]));' {bz_s})""#),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.brotliDecompressSync(fs.readFileSync(process.argv[1])))' {br_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.brotliDecompressSync(fs.readFileSync(process.argv[1])))" {br_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1]).read())" {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],'rt').read())" {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.write(bz2.open(sys.argv[1],'rt').read())" {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,bz2; sys.stdout.write(bz2.open(sys.argv[1],"rt").read())' {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.write(lzma.open(sys.argv[1],'rt').read())" {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,lzma; sys.stdout.write(lzma.open(sys.argv[1],"rt").read())' {xz_s})""#
            ),
            format!(r#"eval "$(php -r 'echo stream_get_contents(gzopen($argv[1],"r"));' {gz_s})""#),
            format!(r#"eval "$(php -r 'echo gzread(gzopen($argv[1],"r"), 4096);' {gz_s})""#),
            format!(r#"eval "$(php -r 'echo gzgets(gzopen($argv[1],"r"));' {gz_s})""#),
            format!(r#"eval "$(php -r 'echo gzpassthru(gzopen($argv[1],"r"));' {gz_s})""#),
            format!(r#"eval "$(php -r 'echo fread(gzopen($argv[1],"r"), 4096);' {gz_s})""#),
            format!(r#"eval "$(php -r 'echo readgzfile($argv[1]);' {gz_s})""#),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.wrap(File.open(ARGV[0])) {{ |f| f.read }}' {gz_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.new(File.open(ARGV[0])).read' {gz_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.open(ARGV[0], &:read)' {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).open(sys.argv[2]).read())' {zip_s} git.txt)""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); sys.stdout.buffer.write(z.read(z.namelist()[0]))' {zip_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zipfile; sys.stdout.buffer.write(zipfile.Path(sys.argv[1], sys.argv[2]).read_bytes())' {zip_s} git.txt)""#
            ),
            format!(
                r#"eval "$(perl -MArchive::Zip -e 'print Archive::Zip->new($ARGV[0])->contents($ARGV[1])' {zip_s} git.txt)""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib::GzipReader.wrap(File.open(ARGV[0])) {{ |f| f.read }}' {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],'rt',encoding='utf-8').read())" {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],"rt",encoding="utf-8").read())' {gz_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'Zlib::GzipReader.open(ARGV[0]) {{ |f| print f.read }}' {gz_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::AnyInflate=anyinflate -e 'anyinflate $ARGV[0] => \*STDOUT' {gz_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::AnyInflate=anyinflate -e 'anyinflate $ARGV[0] => \*STDOUT' {zl_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::AnyInflate=anyinflate -e 'anyinflate $ARGV[0] => \*STDOUT' {df_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],"rt").read())' {gz_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.open(ARGV[0]).readlines.join' {gz_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib::GzipReader.open(ARGV[0]).readlines.join' {gz_s})""#
            ),
            format!(r#"eval "$(php -r 'echo implode("", gzfile($argv[1]));' {gz_s})""#),
            format!(
                r#"eval "$(php -r 'echo file_get_contents("compress.zlib://".$argv[1]);' {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,gzip; sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1],"rb").read())' {gz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1],'rb').read())" {gz_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::Inflate.new.inflate(File.binread(ARGV[0]))' {zl_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib::Inflate.new.inflate(File.binread(ARGV[0]))' {zl_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.inflateRawSync(fs.readFileSync(process.argv[1])))' {df_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.inflateRawSync(fs.readFileSync(process.argv[1])))" {df_s})""#
            ),
            format!(r#"eval "$(php -r 'echo gzinflate(file_get_contents($argv[1]));' {df_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1]).read())" {bz_s})""#
            ),
            format!(r#"eval "$(php -r 'echo bzread(bzopen($argv[1],"r"));' {bz_s})""#),
            format!(r#"eval "$(php -r 'echo bzread(bzopen($argv[1],"r"), 4096);' {bz_s})""#),
            format!(
                r#"eval "$(php -r 'echo file_get_contents("compress.bzip2://".$argv[1]);' {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import bz2,sys; sys.stdout.buffer.write(bz2.BZ2Decompressor().decompress(open(sys.argv[1],"rb").read()))' {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import bz2,sys; sys.stdout.buffer.write(bz2.BZ2Decompressor().decompress(open(sys.argv[1],'rb').read()))" {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import lzma,sys; sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],"rb").read()))' {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import lzma,sys; sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],'rb').read()))" {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import lzma,sys; sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],'rb').read()))" {lzma_s})""#
            ),
            format!(r#"eval "$(php -r 'echo stream_get_contents(bzopen($argv[1],"r"));' {bz_s})""#),
            format!(
                r#"eval "$(python3 -c 'import sys,bz2; sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1],"rb").read())' {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1],'rb').read())" {bz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1]).read())" {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1],"rb").read())' {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1],'rb').read())" {xz_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1]).read())" {lzma_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib.inflate(File.binread(ARGV[0]))' {zl_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib.inflate(File.binread(ARGV[0]))' {zl_s})""#
            ),
            format!(r#"eval "$(php -r 'echo gzuncompress(file_get_contents($argv[1]));' {zl_s})""#),
            format!(
                r#"eval "$(perl -MIO::Uncompress::RawInflate=rawinflate -e 'rawinflate $ARGV[0] => \*STDOUT' {df_s})""#
            ),
            format!("eval \"$(gzip -d -c {gz_s})\""),
            format!("eval \"$(gzip -cd {gz_s})\""),
            format!("eval \"$(gunzip -c {gz_s})\""),
            format!("eval \"$(gzcat {gz_s})\""),
            format!("eval \"$(zcat {gz_s})\""),
            format!("eval \"$(bzip2 -dc {bz_s})\""),
            format!("eval \"$(bzip2 -d -c {bz_s})\""),
            format!("eval \"$(bzcat {bz_s})\""),
            format!("eval \"$(bunzip2 -c {bz_s})\""),
            format!("eval \"$(xz -dc {xz_s})\""),
            format!("eval \"$(xz -d -c {xz_s})\""),
            format!("eval \"$(xzcat {xz_s})\""),
            format!("eval \"$(unxz -c {xz_s})\""),
            format!("eval \"$(unlzma -c {lzma_s})\""),
            format!("eval \"$(xz -dc --format=lzma {lzma_s})\""),
            format!("eval \"$(lzcat {lzma_s})\""),
            format!("eval \"$(zstd -dc {zst_s})\""),
            format!("eval \"$(zstd -d -c {zst_s})\""),
            format!("eval \"$(zstdcat {zst_s})\""),
            format!("eval \"$(unzstd -c {zst_s})\""),
            format!("eval \"$(pzstd -dc {zst_s})\""),
            format!("eval \"$(pzstd -dc {pz_s})\""),
            format!("eval \"$(tar -xOf {tar_s})\""),
            format!("eval \"$(tar -xOf {tar_s} git.txt)\""),
            format!("eval \"$(tar -xO -f {tar_s} git.txt)\""),
            format!("eval \"$(gtar -xOf {tar_s} git.txt)\""),
            format!("eval \"$(tar -xzOf {tgz_s} git.txt)\""),
            format!("eval \"$(tar -xOzf {tgz_s} git.txt)\""),
            format!("eval \"$(tar -xOf {tgz_s} git.txt)\""),
            format!("eval \"$(gtar -xzOf {tgz_s} git.txt)\""),
            format!("eval \"$(tar -xJOf {txz_s} git.txt)\""),
            format!("eval \"$(tar -xjOf {tbz_s} git.txt)\""),
            format!("eval \"$(tar --zstd -xOf {tzst_s} git.txt)\""),
            format!("eval \"$(uncompress -c {zed_s})\""),
            format!("eval \"$(gzip -dc {zed_s})\""),
            format!("eval \"$(gzcat {zed_s})\""),
            format!("eval \"$(tar -xOf {tarz_s} git.txt)\""),
            format!("eval \"$(tar -ZxOf {tarz_s} git.txt)\""),
            format!("eval \"$(gtar --compress -xOf {tarz_s} git.txt)\""),
            format!("eval \"$(gtar --use-compress-program=uncompress -xOf {tarz_s} git.txt)\""),
            format!("eval \"$(gtar --use-compress-program uncompress -xOf {tarz_s} git.txt)\""),
            format!("eval \"$(gtar -I uncompress -xOf {tarz_s} git.txt)\""),
            format!("eval \"$(bsdtar -xOf {tar_s} git.txt)\""),
            format!("eval \"$(gtar -I lz4 -xOf {tlz4_s} git.txt)\""),
            format!("eval \"$(gtar -I 'lz4 -d' -xOf {tlz4_s} git.txt)\""),
            format!("eval \"$(gtar -I brotli -xOf {tbr_s} git.txt)\""),
            format!("eval \"$(gtar -xOf {tlzma_s} git.txt)\""),
            format!("eval \"$(tar -xOf {tlzma_s} git.txt)\""),
            format!("eval \"$(gtar -I lzma -xOf {tlzma_s} git.txt)\""),
            format!(
                "eval \"$(python3 -c 'import sys,tarfile; sys.stdout.buffer.write(tarfile.open(sys.argv[1]).extractfile(sys.argv[2]).read())' {tar_s} git.txt)\""
            ),
            format!("eval \"$(python3 {pytar_s} r {tar_s} git.txt)\""),
            format!("eval \"$(python3 {pyzip_s} {zip_s} git.txt)\""),
            format!(
                "eval \"$(python3 -c 'import sys,zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))' {zip_s} git.txt)\""
            ),
            format!(
                r##"eval "$(php -r 'echo file_get_contents("zip://".$argv[1]."#".$argv[2]);' {zip_s} git.txt)""##
            ),
            format!(
                r##"eval "$(php -r 'echo file_get_contents("zip://".$argv[1]."#git.txt");' {zip_s})""##
            ),
            format!(
                "eval \"$(perl -MArchive::Tar -e 'print Archive::Tar->new($ARGV[0])->get_content($ARGV[1])' {tar_s} git.txt)\""
            ),
            format!("eval \"$(unzip -p {zip_s} git.txt)\""),
            format!("eval \"$(unzip -p {zip_s})\""),
            format!("eval \"$(7z e -so {seven_s})\""),
            format!("eval \"$(7z x -so {seven_s})\""),
            format!("eval \"$(lz4 -dc {lz4_s})\""),
            format!("eval \"$(lz4 -d -c {lz4_s})\""),
            format!("eval \"$(lz4 -c {lz4_s})\""),
            format!("eval \"$(lz4cat {lz4_s})\""),
            format!("eval \"$(unlz4 -c {lz4_s})\""),
            format!("eval \"$(brotli -dc {br_s})\""),
            format!("eval \"$(brotli -d -c {br_s})\""),
            format!("eval \"$(brotli --decompress --stdout {br_s})\""),
            format!("eval \"$(python3 -m base64 -u < {coded_s})\""),
            format!("eval \"$(php -r 'system(file_get_contents($argv[1]));' {push_s})\""),
            format!("eval \"$(dd status=none if={push_s})\""),
            format!("eval \"$(cat {push_s} | cat)\""),
            format!("eval \"$(command cat {push_s})\""),
            format!("x=$(cat {push_s}); eval \"$x\""),
            format!("x=$(<{push_s}); eval \"$x\""),
            format!("eval \"$(<{push_s})\""),
            format!("$(cat {push_s})"),
            format!("eval \"$(cat {push_s} {missing_s})\""),
            format!("eval \"$(cat {missing_s} {push_s})\""),
            format!("eval \"$(cat < {push_s})\""),
            format!("eval \"$(cat < {push_s} | cat)\""),
            format!("eval \"$(grep . {push_s})\""),
            format!("eval \"$(grep . {push_s} | cat)\""),
            format!("eval \"$(grep git {push_s})\""),
            format!("eval \"$(grep 'push origin' {push_s})\""),
            format!("eval \"$(grep -h . {push_s})\""),
            format!("eval \"$(grep --no-filename . {push_s})\""),
            format!("eval \"$(grep -i git {push_s})\""),
            format!("eval \"$(grep -i 'push origin' {push_s})\""),
            format!("eval \"$(grep -w git {push_s})\""),
            format!("eval \"$(grep -o '.*' {push_s})\""),
            format!("eval \"$(grep -ho '.*' {push_s})\""),
            format!("eval \"$(grep --only-matching '.*' {push_s})\""),
            format!("eval \"$(grep -o 'git push origin main' {push_s})\""),
            format!("eval \"$(grep -E '.*' {push_s})\""),
            format!("eval \"$(grep -E . {push_s})\""),
            format!("eval \"$(grep -E git {push_s})\""),
            format!("eval \"$(grep -E 'git+' {push_s})\""),
            format!("eval \"$(egrep '.*' {push_s})\""),
            format!("eval \"$(grep -E 'a+' {push_s})\""),
            format!("eval \"$(fgrep git {push_s})\""),
            format!("eval \"$(fgrep 'git push origin main' {push_s})\""),
            format!("eval \"$(grep -G . {push_s})\""),
            format!("eval \"$(grep --basic-regexp . {push_s})\""),
            format!("eval \"$(head -n 1 {push_s})\""),
            format!("eval \"$(head -c 100000 {push_s})\""),
            format!("eval \"$(cat -u {push_s})\""),
            format!("eval \"$(stdbuf -o0 cat {push_s})\""),
            format!("eval \"$(stdbuf -oL cat {push_s})\""),
            format!("eval \"$(gstdbuf -o0 cat {push_s})\""),
            format!("eval \"$(gcat {push_s})\""),
            format!("eval \"$(ghead -c 100000 {push_s})\""),
            format!("eval \"$(gtail -c +1 {push_s})\""),
            format!("eval \"$(gfold {push_s})\""),
            format!("eval \"$(gfmt {push_s})\""),
            format!("eval \"$(gpaste {push_s})\""),
            format!("eval \"$(gsort {push_s})\""),
            format!("eval \"$(gcut -c 1- {push_s})\""),
            format!("eval \"$(gdd if={push_s})\""),
            format!("eval \"$(gtac {push_s})\""),
            format!("eval \"$(guniq {push_s})\""),
            format!("eval \"$(gexpand {push_s})\""),
            format!("eval \"$(gunexpand {push_s})\""),
            format!("eval \"$(gpr -t {push_s})\""),
            format!("eval \"$(gtr a a < {push_s})\""),
            format!("eval \"$(gtee < {push_s})\""),
            format!("eval \"$(gbase64 -d {coded_s})\""),
            format!("eval \"$(timeout 1 cat {push_s})\""),
            format!("eval \"$(gtimeout 1 cat {push_s})\""),
            format!("eval \"$(env cat {push_s})\""),
            format!("eval \"$(env FOO=1 cat {push_s})\""),
            format!("eval \"$(genv cat {push_s})\""),
            format!("eval \"$(env gcat {push_s})\""),
            format!("eval \"$(nice cat {push_s})\""),
            format!("eval \"$(nice -n 0 cat {push_s})\""),
            format!("eval \"$(gnice cat {push_s})\""),
            format!("eval \"$(nohup cat {push_s})\""),
            format!("eval \"$(gnohup cat {push_s})\""),
            format!("eval \"$(time cat {push_s})\""),
            format!("eval \"$(time -p cat {push_s})\""),
            format!("eval \"$(command time cat {push_s})\""),
            format!("eval \"$(exec cat {push_s})\""),
            format!("eval \"$(sudo -n cat {push_s})\""),
            format!("eval \"$(sudo cat {push_s})\""),
            format!("eval \"$(gsplit --filter=cat -l 1 {push_s})\""),
            format!("eval \"$(split --filter=cat -l 1 {push_s})\""),
            format!("eval \"$(gsplit -n 1/1 {push_s})\""),
            format!("eval \"$(gsplit -n 1/2 {push_s})\""),
            format!("eval \"$(gsplit -n l/1/1 {push_s})\""),
            format!("eval \"$(gsplit -n r/1/1 {push_s})\""),
            format!("eval \"$(gsplit -n l/2/3 {multi_s})\""),
            format!("eval \"$(gsplit -n r/3/3 {multi_s})\""),
            format!(r#"eval "$(while read -r line; do printf '%s\n' "$line"; done < {push_s})""#),
            format!(r#"eval "$(while read -r line; do echo "$line"; done < {push_s})""#),
            format!(r#"eval "$(mapfile -t x < {push_s}; echo "${{x[0]}}")""#),
            format!(r#"eval "$(while read -r line; do printf '%s\n' "$line"; done < {multi_s})""#),
            format!(r#"eval "$(mapfile -t x < {push_s}; printf '%s\n' "${{x[0]}}")""#),
            format!(r#"eval "$(read -r line < {push_s}; printf '%s\n' "$line")""#),
            format!(r#"eval "$(read -r line < {push_s} && printf '%s\n' "$line")""#),
            format!(r#"eval "$(read -r line < {push_s} && echo "$line")""#),
            format!(
                r#"eval "$(while read -r line; do printf '%s\n' "$line"; done < {push_s} && true)""#
            ),
            format!(r#"eval "$(read -r line < {push_s} && printf '%s\n' "$line" || true)""#),
            format!(r#"eval "$(read -r line < {push_s} && printf '%s\n' "$line" && false)""#),
            format!(r#"eval "$(read -r line < {push_s} && echo "$line" && :)""#),
            format!(r#"eval "$(mapfile -t x < {push_s} && printf '%s\n' "${{x[0]}}" || true)""#),
            format!(r#"eval "$(read -r line < {push_s} && printf '%s\n' "${{line}}")""#),
            format!(r#"eval "$(read -r line < {push_s}; printf "%s\n" "${{line}}")""#),
            format!(r#"eval "$(read -r line < {push_s} && echo "${{line}}")""#),
            format!(r#"eval "$(read -r line < {push_s} && printf '%s\n' "${{line[0]}}")""#),
            format!(
                r#"eval "$(while read -r line; do printf '%s\n' "${{line}}"; done < {push_s})""#
            ),
            format!(r#"eval "$(while read -r line; do echo "${{line}}"; done < {push_s})""#),
            format!(r#"eval "$(mapfile -t x < {push_s}; echo "$x")""#),
            format!(r#"eval "$(mapfile -t x < {push_s}; echo "${{x}}")""#),
            format!(r#"eval "$(mapfile -t x < {push_s} && printf '%s\n' "$x")""#),
            format!(r#"eval "$(mapfile -t x < {multi_s}; printf '%s\n' "${{x[@]}}")""#),
            format!(r#"eval "$(IFS= read -r line < {push_s} && printf '%s\n' "$line")""#),
            format!(r#"eval "$(builtin read -r line < {push_s} && printf '%s\n' "$line")""#),
            format!(r#"eval "$(command read -r line < {push_s} && printf '%s\n' "$line")""#),
            format!(r#"eval "$(IFS= command read -r line < {push_s} && printf '%s\n' "$line")""#),
            format!(r#"eval "$(builtin mapfile -t x < {push_s} && printf '%s\n' "${{x[0]}}")""#),
            format!(r#"eval "$(command mapfile -t x < {push_s} && echo "$x")""#),
            format!(r#"eval "$(IFS= mapfile -t x < {push_s} && printf '%s\n' "$x")""#),
            format!(r#"eval "$(read line < {push_s}; printf '%s\n' "$line")""#),
            format!(r#"eval "$(read -r line < {push_s}; printf "%s\n" "$line")""#),
            format!(r#"eval "$(read -r line < {push_s}; echo "$line")""#),
            format!("eval \"$(printf '%s\\n' {push_s} | gxargs cat)\""),
            format!("eval \"$(printf '%s\\n' {push_s} | xargs cat)\""),
            format!("eval \"$(gxargs cat {push_s})\""),
            format!("eval \"$(xargs cat {push_s})\""),
            format!("eval \"$(ruby -e 'print ARGF.read' {push_s})\""),
            format!("eval \"$(ruby -e 'puts ARGF.read' {push_s})\""),
            format!("eval \"$(tail -c +1 {push_s})\""),
            format!("eval \"$(tail -c 100 {push_s})\""),
            format!("eval \"$(ruby -e 'IO.copy_stream(ARGV[0], STDOUT)' {push_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; sys.stdout.writelines(open(sys.argv[1]))' {push_s})\""
            ),
            format!("eval \"$(head -n1 {push_s})\""),
            format!("eval \"$(head -1 {push_s})\""),
            format!("eval \"$(head {push_s})\""),
            format!("eval \"$(head -n 1 {push_s} | cat)\""),
            format!("eval \"$(cat {push_s} /dev/null)\""),
            format!("eval \"$(cat {missing_s} || echo 'git push origin main')\""),
            format!("eval \"$(cat {push_s} < {sub_s})\""),
            format!("eval \"$(echo 'git push origin main' < {sub_s})\""),
            format!("eval \"$(tail -n 1 {push_s})\""),
            format!("eval \"$(sort {push_s})\""),
            format!("eval \"$(sort -u {push_s})\""),
            format!("eval \"$(sort -r {push_s})\""),
            format!("eval \"$(sort -n {push_s})\""),
            format!("eval \"$(sort -s {push_s})\""),
            format!("eval \"$(sort -z {push_s})\""),
            format!("eval \"$(sort --stable {push_s})\""),
            format!("eval \"$(sort -k1 {push_s})\""),
            format!("eval \"$(sort -S 1M {push_s})\""),
            format!("eval \"$(sort --parallel=1 {push_s})\""),
            format!("eval \"$(sort {echo_s} {push_s})\""),
            format!("eval \"$(sort --files0-from={names_s})\""),
            format!("eval \"$(uniq {push_s})\""),
            format!("eval \"$(uniq -u {push_s})\""),
            format!("eval \"$(uniq -i {push_s})\""),
            format!("eval \"$(uniq -z {push_s})\""),
            format!("eval \"$(uniq -d {dup_s})\""),
            format!("eval \"$(uniq -D {dup_s})\""),
            format!("eval \"$(tac {push_s})\""),
            format!("eval \"$(tac -b {push_s})\""),
            format!("eval \"$(tac -r {push_s})\""),
            format!("eval \"$(tac -s $'\\n' {multi_s})\""),
            format!("eval \"$(tac -s Z {push_s})\""),
            format!("eval \"$(tac {missing_s} {push_s})\""),
            format!("eval \"$(tac {echo_s} {push_s})\""),
            format!("eval \"$(sort {multi_s})\""),
            format!("eval \"$(base64 -d {coded_s})\""),
            format!("eval \"$(openssl base64 -d -A -in {coded_s})\""),
            format!("eval \"$(openssl base64 -A -d -in {coded_s})\""),
            format!("eval \"$(openssl base64 -d -in {coded_s})\""),
            format!("eval \"$(openssl enc -base64 -d -A -in {coded_s})\""),
            format!("eval \"$(openssl enc -a -d -in {coded_s})\""),
            format!("eval \"$(openssl base64 -d -A < {coded_s})\""),
            format!("eval \"$(base64 --decode {coded_s})\""),
            format!("eval \"$(base64 -d -i {coded_s})\""),
            format!("eval \"$(base64 -d -w0 {coded_s})\""),
            format!("eval \"$(base64 -d --wrap=0 {coded_s})\""),
            format!("eval \"$(base64 -d --ignore-garbage {coded_s})\""),
            format!("eval \"$(base64 -d -w 0 {coded_s})\""),
            format!("eval \"$(base64 -di {coded_s})\""),
            format!("eval \"$(base64 -dw0 {coded_s})\""),
            format!("eval \"$(gbase64 -d -w0 {coded_s})\""),
            format!("eval \"$(base64 -d -w0 < {coded_s})\""),
            format!("eval \"$(paste {push_s})\""),
            format!("eval \"$(paste -s {push_s})\""),
            format!("eval \"$(expand {push_s})\""),
            format!("eval \"$(expand -t 8 {push_s})\""),
            format!("eval \"$(unexpand {push_s})\""),
            format!("eval \"$(unexpand -a {push_s})\""),
            format!("eval \"$(column {push_s})\""),
            format!("eval \"$(column -t {push_s})\""),
            format!("eval \"$(fold {push_s})\""),
            format!("eval \"$(fold -w 80 {push_s})\""),
            format!("eval \"$(fold -s {push_s})\""),
            format!("eval \"$(fmt {push_s})\""),
            format!("eval \"$(fmt -w 80 {push_s})\""),
            format!("eval \"$(pr {push_s})\""),
            format!("eval \"$(pr -t {push_s})\""),
            format!("eval \"$(strings {push_s})\""),
            format!("eval \"$(strings -n 1 {push_s})\""),
            format!("eval \"$(paste -d ' ' {push_s})\""),
            format!("eval \"$(expand -i {push_s})\""),
            format!("eval \"$(strings -a {push_s})\""),
            format!("eval \"$(fmt -s {push_s})\""),
            format!("eval \"$(column -c 80 {push_s})\""),
            format!("eval \"$(fold -b {push_s})\""),
            format!("eval \"$(tail {push_s})\""),
            format!("eval \"$(tail -n1 {push_s})\""),
            format!("eval \"$(tail -1 {push_s})\""),
            format!("eval \"$(tail -n +1 {push_s})\""),
            format!("eval \"$(tail -n+1 {push_s})\""),
            format!("eval \"$(tail -n +0 {push_s})\""),
            format!("eval \"$(tail -n -1 {push_s})\""),
            format!("eval \"$(tail +1 {push_s})\""),
            format!("eval \"$(tail -n 1 {push_s} | cat)\""),
            format!("eval \"$(sed -n 1p {push_s})\""),
            format!("eval \"$(sed -n '1p' {push_s})\""),
            format!("eval \"$(sed -n p {push_s})\""),
            format!("eval \"$(sed '' {push_s})\""),
            format!("eval \"$(sed p {push_s})\""),
            format!("eval \"$(sed 1p {push_s})\""),
            format!("eval \"$(sed 1,1p {push_s})\""),
            format!("eval \"$(sed -n 1,1p {push_s})\""),
            format!("eval \"$(awk '{{print}}' {push_s})\""),
            format!("eval \"$(awk '1' {push_s})\""),
            format!("eval \"$(awk '{{print $0}}' {push_s})\""),
            format!("eval \"$(awk '{{print $0;}}' {push_s})\""),
            format!("eval \"$(cut -c 1- {push_s})\""),
            format!("eval \"$(cut -c1- {push_s})\""),
            format!("eval \"$(tail -n 1 {multi_s})\""),
            format!("eval \"$(sed 1p {multi_s})\""),
            format!("eval \"$(sed -n 3p {multi_s})\""),
            format!("eval \"$(sed -n 2,3p {multi_s})\""),
            format!("eval \"$(sed -n 3,3p {multi_s})\""),
            format!("eval \"$(sed -n '$p' {multi_s})\""),
            format!("eval \"$(awk 'NR==3' {multi_s})\""),
            format!("eval \"$(awk 'NR==1 {{print}}' {push_s})\""),
            format!("eval \"$(awk '{{ print }}' {push_s})\""),
            format!("eval \"$(awk 'NR==1 {{ print }}' {push_s})\""),
            format!("eval \"$(awk '/git/' {push_s})\""),
            format!("eval \"$(awk '/push/' {push_s})\""),
            format!("eval \"$(awk '/push origin/' {push_s})\""),
            format!("eval \"$(awk '/git/ {{print}}' {push_s})\""),
            format!("eval \"$(gawk '/git/' {push_s})\""),
            format!("eval \"$(awk '$0 ~ /git/' {push_s})\""),
            format!("eval \"$(awk '$0 ~ /git/ {{print}}' {push_s})\""),
            format!("eval \"$(awk 'index($0,\"git\")' {push_s})\""),
            format!("eval \"$(awk 'index($0,\"push origin\")' {push_s})\""),
            format!("eval \"$(awk 'index($0,\"git\") {{print}}' {push_s})\""),
            format!("eval \"$(awk '/git./' {push_s})\""),
            format!("eval \"$(awk '/git*/' {push_s})\""),
            format!("eval \"$(awk '/git/; /git/' {push_s})\""),
            format!("eval \"$(awk '//' {push_s})\""),
            format!("eval \"$(awk '/git/; NR==2' {multi_s})\""),
            format!("eval \"$(awk 'gsub(/git/,\"git\")' {push_s})\""),
            format!("eval \"$(awk 'gsub(/push/,\"push\")' {push_s})\""),
            format!("eval \"$(grep -P git {push_s})\""),
            format!("eval \"$(grep -a git {push_s})\""),
            format!("eval \"$(grep --text git {push_s})\""),
            format!("eval \"$(grep -I git {push_s})\""),
            format!("eval \"$(grep -ai git {push_s})\""),
            format!("eval \"$(grep -e git -e zzzz {push_s})\""),
            format!("eval \"$(awk -e '{{print}}' {push_s})\""),
            format!("eval \"$(perl -X -pe '' {push_s})\""),
            format!("eval \"$(ruby -EUTF-8 -ne 'print' {push_s})\""),
            format!("eval \"$(jq -Rr . {push_s})\""),
            format!("eval \"$(jq -R -r . {push_s})\""),
            format!("eval \"$(grep -P 'push origin' {push_s})\""),
            format!("eval \"$(ggrep git {push_s})\""),
            format!("eval \"$(ggrep -P git {push_s})\""),
            format!("eval \"$(gfgrep git {push_s})\""),
            format!("eval \"$(gegrep git {push_s})\""),
            format!("eval \"$(awk 'NR==3{{print}}' {multi_s})\""),
            format!("eval \"$(awk 'NR==2,NR==3' {multi_s})\""),
            format!("eval \"$(sed -n '3p;3p' {multi_s})\""),
            format!("eval \"$(awk 'NR==3; NR==1' {multi_s})\""),
            format!("read x < {push_s}; eval \"$x\""),
            format!("read -r x < {push_s}; eval \"$x\""),
            format!("read < {push_s}; eval \"$REPLY\""),
            format!("mapfile -t x < {push_s}; eval \"${{x[0]}}\""),
            format!("readarray -t x < {push_s}; eval \"${{x[0]}}\""),
            format!("mapfile -t x < {multi_s}; eval \"${{x[2]}}\""),
            format!("mapfile -s 2 -t x < {multi_s}; eval \"${{x[0]}}\""),
            format!("mapfile -O 2 -t x < {push_s}; eval \"${{x[2]}}\""),
            format!("mapfile -O 2 -t x < {multi_s}; eval \"${{x[4]}}\""),
            format!("f() {{ mapfile -t x < {push_s}; }}; f; eval \"${{x[0]}}\""),
            format!(r#"eval "$(perl -e 'print while <>' {push_s})""#),
            format!(r#"eval "$(perl -e 'while (<>) {{ print }}' {push_s})""#),
            format!(r#"eval "$(perl -e 'print $_ while <>' {push_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; print(open(sys.argv[1], encoding='utf-8').read())" {push_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; sys.stdout.write(Path(sys.argv[1]).read_text())" {push_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; sys.stdout.write(Path(sys.argv[1]).read_text(encoding='utf-8'))" {push_s})""#
            ),
            format!(r#"eval "$(ruby -e 'print File.readlines(ARGV[0]).join("")' {push_s})""#),
            format!(r#"eval "$(ruby -e 'File.foreach(ARGV[0]) {{|l| print l}}' {push_s})""#),
            format!(r#"eval "$(ruby -e 'IO.foreach(ARGV[0]) {{|l| print l}}' {push_s})""#),
            format!(r#"eval "$(ruby -e 'puts File.readlines(ARGV[0])' {push_s})""#),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1],'utf-8'))" {push_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); process.stdout.write(fs.readFileSync(process.argv[1],"utf-8"))' {push_s})""#
            ),
            format!(
                r#"eval "$(node -e "process.stdout.write(require('fs').readFileSync(process.argv[1]))" {push_s})""#
            ),
            format!(
                r#"eval "$(node -e "process.stdout.write(require('fs').readFileSync(process.argv[1],'utf8'))" {push_s})""#
            ),
            format!(r#"eval "$(php -r 'echo implode("", file($argv[1]));' {push_s})""#),
            format!(r#"eval "$(php -r 'echo fread(fopen($argv[1], "r"), 4096);' {push_s})""#),
            format!(r#"eval "$(php -r 'echo fread(fopen($argv[1], "r"), 4096);' {early_s})""#),
            format!(
                r#"eval "$(php -r 'echo stream_get_contents(fopen($argv[1], "r"));' {push_s})""#
            ),
            format!(r#"eval "$(php -r 'echo fgets(fopen($argv[1], "r"));' {push_s})""#),
            format!(r#"eval "$(php -r 'echo fgets(fopen($argv[1], "r"));' {beta_last_s})""#),
            format!(r#"eval "$(perl -e 'print while <>;' {push_s})""#),
            format!(
                r#"eval "$(python3 -c "from pathlib import Path; import sys; print(Path(sys.argv[1]).read_text())" {push_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.write(open(sys.argv[1], encoding='utf-8').read())" {push_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; print(''.join(open(sys.argv[1]).readlines()))" {push_s})""#
            ),
            format!(r#"eval "$(ruby -e 'File.read(ARGV[0]).each_line {{|l| print l}}' {push_s})""#),
            format!(r#"eval "$(ruby -e 'print File.read(ARGV[0], encoding: "UTF-8")' {push_s})""#),
            format!(
                r#"eval "$(node -e "const {{readFileSync}}=require('fs'); process.stdout.write(readFileSync(process.argv[1],'utf8'))" {push_s})""#
            ),
            format!(
                r#"eval "$(php -r 'foreach (file($argv[1]) as $line) {{ echo $line; }}' {push_s})""#
            ),
            format!(r#"eval "$(php -r 'echo file_get_contents($argv[1], false);' {push_s})""#),
            format!(r#"eval "$(perl -e 'while (<>) {{ print; }}' {push_s})""#),
            format!(r#"eval "$(ruby -e 'print File.readlines(ARGV[0]).join("\n")' {push_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; print(Path(sys.argv[1]).read_text(encoding='utf-8'))" {push_s})""#
            ),
            format!(
                r#"eval "$(node -e "fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1]))" {push_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1],{{encoding:'utf8'}}))" {push_s})""#
            ),
            format!(r#"eval "$(php -r 'print implode("", file($argv[1]));' {push_s})""#),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); console.log(fs.readFileSync(process.argv[1],'utf8'))" {push_s})""#
            ),
            format!(r#"eval "$(perl -e 'print $_ while <>;' {push_s})""#),
            format!(r#"eval "$(ruby -e 'print File.read(ARGV[0], mode: "r")' {push_s})""#),
            format!(r#"eval "$(php -r 'echo file_get_contents($argv[1], true);' {push_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.writelines(open(sys.argv[1], encoding='utf-8'))" {push_s})""#
            ),
            format!(r#"eval "$(ruby -e 'puts File.readlines(ARGV[0]).join' {push_s})""#),
        ];
        for command in &positives {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command:?}"
            );
        }
        for command in [
            format!("bash {big_s}"),
            format!("source {big_s}"),
            format!("cat {big_s}"),
            format!("head {big_s}"),
            format!("tail {big_s}"),
            format!("cat < {big_s}"),
            format!("head < {big_s}"),
            format!("tail < {big_s}"),
            format!("head - < {big_s}"),
            format!("tail - < {big_s}"),
            format!("grep . {big_s}"),
            format!("sed -n p {big_s}"),
            format!("awk '{{print}}' {big_s}"),
            format!("cut -c 1- {big_s}"),
            format!("sort {big_s}"),
            format!("uniq {big_s}"),
            format!("tac {big_s}"),
            format!("fold {big_s}"),
            format!("fmt {big_s}"),
            format!("paste {big_s}"),
            format!("expand {big_s}"),
            format!("dd status=none if={big_s}"),
            format!("fold < {big_s}"),
            format!("iconv -f utf-8 -t utf-8 {big_s}"),
            format!("perl -pe '' {big_s}"),
            format!("tee out < {big_s}"),
            format!("join {big_s} {big_s}"),
            format!("comm -12 {big_s} {big_s}"),
            format!("jq -Rr . {big_s}"),
            format!("python3 -c 'import sys; print(open(sys.argv[1]).read())' {big_s}"),
            format!(
                "node -e \"fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1]))\" {big_s}"
            ),
            format!("php -r 'print file_get_contents($argv[1]);' {big_s}"),
            format!("split --filter=cat -l 1 {big_s}"),
            format!("tr a-z A-Z < {big_s}"),
            format!("base64 -d {big_s}"),
            format!("base32 -d {big_s}"),
            format!("openssl base64 -d -A -in {big_s}"),
            format!("xxd -r -p {big_s}"),
            format!("uudecode -p {big_s}"),
            format!("uncompress -c {big_s}"),
            format!("unlzma -c {big_s}"),
            format!("basenc --base64 -d {big_s}"),
            format!("gzip -dc {big_s}"),
            format!("bzip2 -dc {big_s}"),
            format!("xz -dc {big_s}"),
            format!("zstd -dc {big_s}"),
            format!("lz4 -dc {big_s}"),
            format!("brotli -dc {big_s}"),
            format!("tar -xOf {big_s} payload.sh"),
            format!("unzip -p {big_s} payload.sh"),
            format!("7z e -so {big_s}"),
            format!("eval \"$(cat {big_s})\""),
            format!("x=$(<{big_s}); eval \"$x\""),
            format!("eval \"$(cat {big_s} {push_s})\""),
            format!("eval \"$(cat {push_s} {big_s})\""),
            format!("eval \"$(cat < {big_s})\""),
            format!("eval \"$(tail -n 1 {big_s})\""),
            format!("eval \"$(sed -n p {big_s})\""),
            format!("eval \"$(awk '{{print}}' {big_s})\""),
            format!("eval \"$(cut -c 1- {big_s})\""),
            format!("read x < {big_s}; eval \"$x\""),
            format!("mapfile -t x < {big_s}; eval \"${{x[0]}}\""),
        ] {
            assert_eq!(
                matched_action(&policy, "Bash", Some(&command)).map(|id| id.0.as_str()),
                Some("shell.deep"),
                "{command:?}"
            );
        }
        let neithers = [
            format!("source {missing_s}"),
            format!(". {missing_s}"),
            format!("source {echo_s}"),
            format!(". {echo_s}"),
            format!("bash {echo_s}"),
            format!("bash -c 'echo hello' {push_s}"),
            format!("bash -s {push_s}"),
            "bash -- -c 'git push origin main'".to_string(),
            format!("source() {{ echo hello; }}; source {push_s}"),
            format!("bash() {{ echo hello; }}; bash {push_s}"),
            "bash /dev/zero".to_string(),
            "source /dev/zero".to_string(),
            format!("echo \"$(cat {push_s})\""),
            format!("eval \"$(cat -n {push_s})\""),
            format!("eval \"$(comm {push_s})\""),
            format!("eval \"$(iconv -f utf-8 -t utf-16 {push_s})\""),
            format!("eval \"$(cat {missing_s})\""),
            format!("x=$(<{missing_s}); eval \"$x\""),
            format!("eval \"$(cat {push_s} | true)\""),
            "eval \"$(cat /dev/zero)\"".to_string(),
            "x=$(</dev/zero); eval \"$x\"".to_string(),
            format!("eval \"$(cat < {missing_s})\""),
            format!("eval \"$(cat {push_s} < {missing_s})\""),
            format!("eval \"$(grep . {missing_s})\""),
            format!("eval \"$(grep -H . {push_s})\""),
            format!("eval \"$(grep -n . {push_s})\""),
            format!("eval \"$(grep -o . {push_s})\""),
            format!("eval \"$(grep -o git {push_s})\""),
            format!("eval \"$(grep -w it {push_s})\""),
            format!("eval \"$(grep -E 'z+' {push_s})\""),
            format!("eval \"$(fgrep 'no such line' {push_s})\""),
            format!("eval \"$(head -n 1 {missing_s})\""),
            format!("eval \"$(cat {push_s} /dev/zero)\""),
            format!("eval \"$(cat {missing_s} {missing2_s})\""),
            format!("eval \"$(cat < {sub_s})\""),
            format!("eval \"$(head -n 0 {push_s})\""),
            format!("eval \"$(head -c 3 {push_s})\""),
            format!("eval \"$(head -c 100000 {echo_s})\""),
            format!("eval \"$(cat -u {echo_s})\""),
            format!("eval \"$(stdbuf -o0 cat {echo_s})\""),
            format!("eval \"$(gstdbuf -o0 cat {echo_s})\""),
            format!("eval \"$(gcat {echo_s})\""),
            format!("eval \"$(ghead -c 100000 {echo_s})\""),
            format!("eval \"$(gtail -c +1 {echo_s})\""),
            format!("eval \"$(gfold {echo_s})\""),
            format!("eval \"$(gfmt {echo_s})\""),
            format!("eval \"$(gpaste {echo_s})\""),
            format!("eval \"$(gsort {echo_s})\""),
            format!("eval \"$(gcut -c 1- {echo_s})\""),
            format!("eval \"$(gdd if={echo_s})\""),
            format!("eval \"$(gtac {echo_s})\""),
            format!("eval \"$(guniq {echo_s})\""),
            format!("eval \"$(gexpand {echo_s})\""),
            format!("eval \"$(gunexpand {echo_s})\""),
            format!("eval \"$(gpr -t {echo_s})\""),
            format!("eval \"$(gtr a a < {echo_s})\""),
            format!("eval \"$(gtee < {echo_s})\""),
            format!("eval \"$(gbase64 -d {echo_s})\""),
            format!("eval \"$(openssl base64 -d -A -in {echo_b64_s})\""),
            format!("eval \"$(openssl base64 -d -A < {echo_b64_s})\""),
            format!(
                "eval \"$(ruby -e 'print File.binread(ARGV[0]).unpack1(\"m\")' {echo_b64_s})\""
            ),
            format!(
                r#"eval "$(python3 -c 'import base64,sys; sys.stdout.buffer.write(base64.b64decode(open(sys.argv[1],"rb").read()))' {echo_b64_s})""#
            ),
            format!(
                "eval \"$(php -r 'echo base64_decode(file_get_contents($argv[1]));' {echo_b64_s})\""
            ),
            format!(
                r#"eval "$(node -e 'process.stdout.write(Buffer.from(require("fs").readFileSync(process.argv[1]).toString(),"base64"))' {echo_b64_s})""#
            ),
            format!(
                "eval \"$(perl -MMIME::Base64 -0777 -e 'print decode_base64(<>)' {echo_b64_s})\""
            ),
            format!("eval \"$(basenc --base64 -d {echo_b64_s})\""),
            format!("eval \"$(gbasenc --base64 -d {echo_b64_s})\""),
            format!("eval \"$(uudecode -p {uu_hello_s})\""),
            format!("eval \"$(xxd -r -p {hex_hello_s})\""),
            format!("eval \"$(xxd -r -p < {hex_hello_s})\""),
            format!("eval \"$(basenc --base16 -d {hex_hello_s})\""),
            format!("eval \"$(gbasenc --base16 -d {hex_hello_s})\""),
            format!("eval \"$(basenc --base32 -d {b32_hello_s})\""),
            format!("eval \"$(gbasenc --base32 -d {b32_hello_s})\""),
            format!("eval \"$(basenc --base64url -d {echo_b64_s})\""),
            format!("eval \"$(basenc --base32hex -d {b32hex_hello_s})\""),
            format!("eval \"$(basenc --base2msbf -d {b2m_hello_s})\""),
            format!("eval \"$(basenc --base2lsbf -d {b2l_hello_s})\""),
            format!("eval \"$(basenc --z85 -d {z85_hello_s})\""),
            format!("eval \"$(basenc --base64 -d -i {echo_b64_s})\""),
            format!("eval \"$(basenc --base64 -d --wrap 0 {echo_b64_s})\""),
            format!("eval \"$(basenc --base58 -d {b58_hello_s})\""),
            format!("eval \"$(base64 -d -w0 {echo_b64_s})\""),
            format!("eval \"$(base32 -d -i {b32_hello_s})\""),
            format!(
                "eval \"$(ruby -rbase64 -e 'print Base64.decode64(File.binread(ARGV[0]))' {echo_b64_s})\""
            ),
            format!(
                r#"eval "$(python3 -c 'import binascii,sys; sys.stdout.buffer.write(binascii.a2b_base64(open(sys.argv[1],"rb").read()))' {echo_b64_s})""#
            ),
            format!("eval \"$(python3 -m base64 -d {echo_b64_s})\""),
            format!("eval \"$(python3 -m base64 -d < {echo_b64_s})\""),
            format!("eval \"$(python3 -m base64 -u {echo_b64_s})\""),
            format!("eval \"$(gzip -dc {gz_hello_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys,gzip; sys.stdout.buffer.write(gzip.open(sys.argv[1],\"rb\").read())' {gz_hello_s})\""
            ),
            format!(
                "eval \"$(python3 -c 'import sys,bz2; sys.stdout.buffer.write(bz2.open(sys.argv[1],\"rb\").read())' {bz_hello_s})\""
            ),
            format!(
                "eval \"$(python3 -c 'import sys,lzma; sys.stdout.buffer.write(lzma.open(sys.argv[1],\"rb\").read())' {xz_hello_s})\""
            ),
            format!(
                "eval \"$(ruby -rzlib -e 'print Zlib::GzipReader.open(ARGV[0]).read' {gz_hello_s})\""
            ),
            format!(
                "eval \"$(node -e 'const fs=require(\"fs\"); const z=require(\"zlib\"); process.stdout.write(z.gunzipSync(fs.readFileSync(process.argv[1])))' {gz_hello_s})\""
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.buffer.write(gzip.open(sys.argv[1],'rb').read())" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.buffer.write(bz2.open(sys.argv[1],'rb').read())" {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.open(sys.argv[1],'rb').read())" {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import gzip,sys; sys.stdout.buffer.write(gzip.decompress(open(sys.argv[1],"rb").read()))' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import gzip,sys; sys.stdout.buffer.write(gzip.decompress(open(sys.argv[1],'rb').read()))" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import bz2,sys; sys.stdout.buffer.write(bz2.decompress(open(sys.argv[1],"rb").read()))' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import bz2,sys; sys.stdout.buffer.write(bz2.decompress(open(sys.argv[1],'rb').read()))" {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import lzma,sys; sys.stdout.buffer.write(lzma.decompress(open(sys.argv[1],"rb").read()))' {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import lzma,sys; sys.stdout.buffer.write(lzma.decompress(open(sys.argv[1],'rb').read()))" {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib.gunzip(File.binread(ARGV[0]))' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib.gunzip(File.binread(ARGV[0]))' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::Gunzip=gunzip -e 'gunzip $ARGV[0] => \*STDOUT' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(php -r 'echo gzdecode(file_get_contents($argv[1]));' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.gunzipSync(fs.readFileSync(process.argv[1])))" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zlib; sys.stdout.buffer.write(zlib.decompress(open(sys.argv[1],"rb").read()))' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,zlib; sys.stdout.buffer.write(zlib.decompress(open(sys.argv[1],'rb').read()))" {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.inflateSync(fs.readFileSync(process.argv[1])))' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.inflateSync(fs.readFileSync(process.argv[1])))" {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))" {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::Bunzip2=bunzip2 -e 'bunzip2 $ARGV[0] => \*STDOUT' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(php -r 'echo bzdecompress(file_get_contents($argv[1]));' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.brotliDecompressSync(fs.readFileSync(process.argv[1])))' {br_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.brotliDecompressSync(fs.readFileSync(process.argv[1])))" {br_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1]).read())" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],'rt').read())" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.write(bz2.open(sys.argv[1],'rt').read())" {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,bz2; sys.stdout.write(bz2.open(sys.argv[1],"rt").read())' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.write(lzma.open(sys.argv[1],'rt').read())" {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,lzma; sys.stdout.write(lzma.open(sys.argv[1],"rt").read())' {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(php -r 'echo stream_get_contents(gzopen($argv[1],"r"));' {gz_hello_s})""#
            ),
            format!(r#"eval "$(php -r 'echo gzread(gzopen($argv[1],"r"), 4096);' {gz_hello_s})""#),
            format!(r#"eval "$(php -r 'echo gzgets(gzopen($argv[1],"r"));' {gz_hello_s})""#),
            format!(r#"eval "$(php -r 'echo gzpassthru(gzopen($argv[1],"r"));' {gz_hello_s})""#),
            format!(r#"eval "$(php -r 'echo fread(gzopen($argv[1],"r"), 4096);' {gz_hello_s})""#),
            format!(r#"eval "$(php -r 'echo readgzfile($argv[1]);' {gz_hello_s})""#),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.wrap(File.open(ARGV[0])) {{ |f| f.read }}' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.new(File.open(ARGV[0])).read' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.open(ARGV[0], &:read)' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).open(sys.argv[2]).read())' {zip_hello_s} echo.txt)""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); sys.stdout.buffer.write(z.read(z.namelist()[0]))' {zip_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,zipfile; sys.stdout.buffer.write(zipfile.Path(sys.argv[1], sys.argv[2]).read_bytes())' {zip_hello_s} echo.txt)""#
            ),
            format!(
                r#"eval "$(perl -MArchive::Zip -e 'print Archive::Zip->new($ARGV[0])->contents($ARGV[1])' {zip_hello_s} echo.txt)""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib::GzipReader.wrap(File.open(ARGV[0])) {{ |f| f.read }}' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],'rt',encoding='utf-8').read())" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],"rt",encoding="utf-8").read())' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'Zlib::GzipReader.open(ARGV[0]) {{ |f| print f.read }}' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::AnyInflate=anyinflate -e 'anyinflate $ARGV[0] => \*STDOUT' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,gzip; sys.stdout.write(gzip.open(sys.argv[1],"rt").read())' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.open(ARGV[0]).readlines.join' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib::GzipReader.open(ARGV[0]).readlines.join' {gz_hello_s})""#
            ),
            format!(r#"eval "$(php -r 'echo implode("", gzfile($argv[1]));' {gz_hello_s})""#),
            format!(
                r#"eval "$(php -r 'echo file_get_contents("compress.zlib://".$argv[1]);' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,gzip; sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1],"rb").read())' {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.buffer.write(gzip.GzipFile(sys.argv[1],'rb').read())" {gz_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::Inflate.new.inflate(File.binread(ARGV[0]))' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib::Inflate.new.inflate(File.binread(ARGV[0]))' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); const z=require("zlib"); process.stdout.write(z.inflateRawSync(fs.readFileSync(process.argv[1])))' {df_hello_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.inflateRawSync(fs.readFileSync(process.argv[1])))" {df_hello_s})""#
            ),
            format!(
                r#"eval "$(php -r 'echo gzinflate(file_get_contents($argv[1]));' {df_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1]).read())" {bz_hello_s})""#
            ),
            format!(r#"eval "$(php -r 'echo bzread(bzopen($argv[1],"r"));' {bz_hello_s})""#),
            format!(r#"eval "$(php -r 'echo bzread(bzopen($argv[1],"r"), 4096);' {bz_hello_s})""#),
            format!(
                r#"eval "$(php -r 'echo file_get_contents("compress.bzip2://".$argv[1]);' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import bz2,sys; sys.stdout.buffer.write(bz2.BZ2Decompressor().decompress(open(sys.argv[1],"rb").read()))' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import bz2,sys; sys.stdout.buffer.write(bz2.BZ2Decompressor().decompress(open(sys.argv[1],'rb').read()))" {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import lzma,sys; sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],"rb").read()))' {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import lzma,sys; sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],'rb').read()))" {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import lzma,sys; sys.stdout.buffer.write(lzma.LZMADecompressor().decompress(open(sys.argv[1],'rb').read()))" {lzma_hello_s})""#
            ),
            format!(
                r#"eval "$(php -r 'echo stream_get_contents(bzopen($argv[1],"r"));' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,bz2; sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1],"rb").read())' {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,bz2; sys.stdout.buffer.write(bz2.BZ2File(sys.argv[1],'rb').read())" {bz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1]).read())" {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1],"rb").read())' {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1],'rb').read())" {xz_hello_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys,lzma; sys.stdout.buffer.write(lzma.LZMAFile(sys.argv[1]).read())" {lzma_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib.inflate(File.binread(ARGV[0]))' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(ruby -r zlib -e 'print Zlib.inflate(File.binread(ARGV[0]))' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(php -r 'echo gzuncompress(file_get_contents($argv[1]));' {zl_hello_s})""#
            ),
            format!(
                r#"eval "$(perl -MIO::Uncompress::RawInflate=rawinflate -e 'rawinflate $ARGV[0] => \*STDOUT' {df_hello_s})""#
            ),
            format!("eval \"$(zcat {gz_hello_s})\""),
            format!(
                r#"eval "$(python3 -c "import sys,gzip; sys.stdout.buffer.write(gzip.open(sys.argv[1],'rb').read())" {zed_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import gzip,sys; sys.stdout.buffer.write(gzip.decompress(open(sys.argv[1],"rb").read()))' {zed_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib::GzipReader.open(ARGV[0]).read' {zed_s})""#
            ),
            format!(
                r#"eval "$(ruby -rzlib -e 'print Zlib.gunzip(File.binread(ARGV[0]))' {zed_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.gunzipSync(fs.readFileSync(process.argv[1])))" {zed_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); const z=require('zlib'); process.stdout.write(z.unzipSync(fs.readFileSync(process.argv[1])))" {zed_s})""#
            ),
            format!(r#"eval "$(php -r 'echo gzdecode(file_get_contents($argv[1]));' {zed_s})""#),
            format!(
                r#"eval "$(perl -MIO::Uncompress::Gunzip=gunzip -e 'gunzip $ARGV[0] => \*STDOUT' {zed_s})""#
            ),
            format!("eval \"$(bzip2 -dc {bz_hello_s})\""),
            format!("eval \"$(bzip2 -d {bz_s})\""),
            format!("eval \"$(xz -dc {xz_hello_s})\""),
            format!("eval \"$(unlzma -c {lzma_hello_s})\""),
            format!("eval \"$(xz -d {xz_s})\""),
            format!("eval \"$(zstd -dc {zst_hello_s})\""),
            format!("eval \"$(pzstd -dc {zst_hello_s})\""),
            format!("eval \"$(tar -xOf {tar_hello_s} echo.txt)\""),
            format!("eval \"$(tar -xzOf {tgz_hello_s} echo.txt)\""),
            format!("eval \"$(uncompress -c {zed_hello_s})\""),
            format!("eval \"$(gzip -dc {zed_hello_s})\""),
            format!("eval \"$(tar -xOf {tarz_hello_s} echo.txt)\""),
            format!("eval \"$(bsdtar -xOf {tar_hello_s} echo.txt)\""),
            format!("eval \"$(gtar -I lz4 -xOf {tlz4_hello_s} echo.txt)\""),
            format!("eval \"$(gtar -I brotli -xOf {tbr_hello_s} echo.txt)\""),
            format!("eval \"$(gtar -xOf {tlzma_hello_s} echo.txt)\""),
            format!(
                "eval \"$(python3 -c 'import sys,tarfile; sys.stdout.buffer.write(tarfile.open(sys.argv[1]).extractfile(sys.argv[2]).read())' {tar_hello_s} echo.txt)\""
            ),
            format!("eval \"$(python3 {pyzip_s} {zip_hello_s} echo.txt)\""),
            format!(
                "eval \"$(python3 -c 'import sys,zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))' {zip_hello_s} echo.txt)\""
            ),
            format!(
                r##"eval "$(php -r 'echo file_get_contents("zip://".$argv[1]."#".$argv[2]);' {zip_hello_s} echo.txt)""##
            ),
            format!(
                r##"eval "$(php -r 'echo file_get_contents("zip://".$argv[1]."#git.txt");' {zip_hello_s})""##
            ),
            format!(
                "eval \"$(perl -MArchive::Tar -e 'print Archive::Tar->new($ARGV[0])->get_content($ARGV[1])' {tar_hello_s} echo.txt)\""
            ),
            format!("eval \"$(tar -xOf {tlzma_hello_s} echo.txt)\""),
            format!("eval \"$(unzip -p {zip_hello_s} echo.txt)\""),
            format!("eval \"$(7z e -so {seven_hello_s})\""),
            format!("eval \"$(7z e {seven_s})\""),
            format!("eval \"$(tar -xOf {tar_s} missing.txt)\""),
            format!("eval \"$(zstd -d {zst_s})\""),
            format!("eval \"$(lz4 -dc {lz4_hello_s})\""),
            format!("eval \"$(lz4 -d {lz4_s})\""),
            format!("eval \"$(brotli -dc {br_hello_s})\""),
            format!("eval \"$(brotli -d {br_s})\""),
            format!("eval \"$(gzip -d {gz_s})\""),
            format!("eval \"$(gzip -c {push_s})\""),
            format!("eval \"$(gbase32 -d {echo_s})\""),
            format!("eval \"$(gjoin {echo_s} {echo_s})\""),
            format!("eval \"$(gcomm -12 {echo_s} {echo_s})\""),
            format!("eval \"$(gcomm -3 {push_s} {push_s})\""),
            format!("eval \"$(gfgrep zzzz {push_s})\""),
            format!("eval \"$(gfgrep echo {echo_s})\""),
            format!("eval \"$(gegrep zzzz {push_s})\""),
            format!("eval \"$(timeout 1 cat {echo_s})\""),
            format!("eval \"$(env cat {echo_s})\""),
            format!("eval \"$(nice cat {echo_s})\""),
            format!("eval \"$(genv cat {echo_s})\""),
            format!("eval \"$(nohup cat {echo_s})\""),
            format!("eval \"$(time cat {echo_s})\""),
            format!("eval \"$(exec cat {echo_s})\""),
            format!("eval \"$(sudo -n cat {echo_s})\""),
            format!("eval \"$(gsplit --filter=echo -l 1 {push_s})\""),
            format!("eval \"$(gsplit --filter=cat -n 1/1 {push_s})\""),
            format!("eval \"$(gsplit -l 1 {push_s})\""),
            format!("eval \"$(gsplit -n 2/2 {push_s})\""),
            format!("eval \"$(gsplit -n 1/2 {echo_s})\""),
            format!("eval \"$(gsplit -n l/1/3 {multi_s})\""),
            format!("eval \"$(gsplit -n r/1/3 {multi_s})\""),
            format!(r#"eval "$(while read -r line; do printf '%s\n' "$line"; done < {echo_s})""#),
            format!(r#"eval "$(while read -r line; do echo "$line"; done < {echo_s})""#),
            format!(r#"eval "$(mapfile -t x < {echo_s}; printf '%s\n' "${{x[0]}}")""#),
            format!(r#"eval "$(mapfile -t x < {echo_s}; echo "${{x[0]}}")""#),
            format!(r#"eval "$(mapfile -t x < {multi_s}; printf '%s\n' "${{x[0]}}")""#),
            format!(r#"eval "$(read -r line < {echo_s}; printf '%s\n' "$line")""#),
            format!(r#"eval "$(read -r line < {echo_s} && printf '%s\n' "$line")""#),
            format!(
                r#"eval "$(while read -r line; do printf '%s\n' "$line"; done < {echo_s} && true)""#
            ),
            format!(r#"eval "$(read -r line < {echo_s} && printf '%s\n' "$line" || true)""#),
            format!(r#"eval "$(mapfile -t x < {echo_s} && printf '%s\n' "${{x[0]}}" || true)""#),
            format!(r#"eval "$(read -r line < {echo_s} && printf '%s\n' "${{line}}")""#),
            format!(r#"eval "$(mapfile -t x < {echo_s}; echo "$x")""#),
            format!(r#"eval "$(while read -r line; do echo "${{line}}"; done < {echo_s})""#),
            format!(r#"eval "$(mapfile -t x < {echo_s}; printf '%s\n' "${{x[@]}}")""#),
            format!(r#"eval "$(IFS= read -r line < {echo_s} && printf '%s\n' "$line")""#),
            format!(r#"eval "$(builtin mapfile -t x < {echo_s} && echo "$x")""#),
            format!(r#"eval "$(read -r line < {echo_s}; echo "$line")""#),
            format!("eval \"$(printf '%s\\n' {echo_s} | gxargs cat)\""),
            format!("eval \"$(gxargs cat {echo_s})\""),
            format!("eval \"$(ruby -e 'print ARGF.read' {echo_s})\""),
            format!("eval \"$(tail -c 3 {push_s})\""),
            format!("eval \"$(tail -c +5 {push_s})\""),
            format!("eval \"$(tail -c +1 {echo_s})\""),
            format!("eval \"$(ruby -e 'IO.copy_stream(ARGV[0], STDOUT)' {echo_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; sys.stdout.writelines(open(sys.argv[1]))' {echo_s})\""
            ),
            format!("eval \"$(sed -n 1p {multi_s})\""),
            format!("eval \"$(sed -n 2p {multi_s})\""),
            format!("eval \"$(awk 'NR==2' {multi_s})\""),
            format!("eval \"$(awk 'NR==2 {{print}}' {multi_s})\""),
            format!("eval \"$(sed -n '2p;2p' {multi_s})\""),
            format!("eval \"$(sed -n 2l {multi_s})\""),
            format!("eval \"$(sed -n '/zzzz/p' {push_s})\""),
            format!("eval \"$(sed -n 's/zzzz/zzzz/p' {push_s})\""),
            format!("eval \"$(sed -n 's/git/echo/p' {push_s})\""),
            format!("eval \"$(awk 'gsub(/zzzz/,\"zzzz\")' {push_s})\""),
            format!("eval \"$(awk '{{printf \"%s\", $0}}' {multi_s})\""),
            format!("eval \"$(awk '{{printf $0}}' {multi_s})\""),
            format!("eval \"$(awk 'END {{print}}' {beta_last_s})\""),
            format!("eval \"$(awk 'END {{print}}' {echo_s})\""),
            format!("eval \"$(ruby -e 'STDOUT.write IO.read(ARGV[0])' {echo_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; print(open(sys.argv[1], encoding=\"utf-8\").read())' {echo_s})\""
            ),
            format!("eval \"$(php -r 'echo readfile($argv[1]);' {echo_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; print(open(sys.argv[1]).read())' {echo_s})\""
            ),
            format!("eval \"$(php -r 'readfile($argv[1]);' {echo_s})\""),
            format!(
                "eval \"$(node -e 'const fs=require(\"fs\"); process.stdout.write(fs.readFileSync(process.argv[1],\"utf8\"))' {echo_s})\""
            ),
            format!("eval \"$(grep -P zzzz {push_s})\""),
            format!("eval \"$(grep -a zzzz {push_s})\""),
            format!("eval \"$(grep -e zzzz -e nope {push_s})\""),
            format!("eval \"$(jq -Rr . {echo_s})\""),
            format!("eval \"$(jq -R . {push_s})\""),
            format!("eval \"$(grep --text zzzz {push_s})\""),
            format!("eval \"$(perl -w -pe '' {echo_s})\""),
            format!("eval \"$(ruby -W0 -ne 'print' {echo_s})\""),
            format!(
                "eval \"$(python3 -I -c 'import sys; print(open(sys.argv[1]).read())' {echo_s})\""
            ),
            format!("eval \"$(awk '/zzzz/' {push_s})\""),
            format!("eval \"$(awk 'index($0,\"zzzz\")' {push_s})\""),
            format!("eval \"$(awk '$0 ~ /zzzz/' {push_s})\""),
            format!("eval \"$(awk '/no.such/' {push_s})\""),
            format!("eval \"$(awk '/zzzz/; NR==2' {multi_s})\""),
            format!("eval \"$(perl -ne '' {push_s})\""),
            format!("eval \"$(ruby -ne '' {push_s})\""),
            format!("eval \"$(perl -ne 'print if 0' {push_s})\""),
            format!("eval \"$(perl -pe '$_=\"\"' {push_s})\""),
            format!("eval \"$(ruby -pe '$_=\"\"' {push_s})\""),
            format!("eval \"$(perl -p {push_s})\""),
            format!("eval \"$(ruby -p {push_s})\""),
            format!("eval \"$(ruby -e 'puts File.read(ARGV[0])' {echo_s})\""),
            format!("eval \"$(perl -lpe '' {echo_s})\""),
            format!("eval \"$(ruby -nle 'print' {echo_s})\""),
            format!("eval \"$(ruby -e 'puts IO.read(ARGV[0])' {echo_s})\""),
            format!(
                "eval \"$(python3 -c 'import sys; from pathlib import Path; print(Path(sys.argv[1]).read_text())' {echo_s})\""
            ),
            format!(
                r#"eval "$(python3 -c "import pathlib,sys; sys.stdout.buffer.write(pathlib.Path(sys.argv[1]).read_bytes())" {echo_s})""#
            ),
            format!(r#"eval "$(ruby -e 'print File.binread(ARGV[0])' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'print IO.binread(ARGV[0])' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'print File.open(ARGV[0]).read' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'print File.readlines(ARGV[0]).join' {echo_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; sys.stdout.buffer.write(Path(sys.argv[1]).read_bytes())" {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1],'utf8'))" {echo_s})""#
            ),
            format!(r#"eval "$(perl -e 'print <>' {echo_s})""#),
            format!(r#"eval "$(perl -e 'print while <>' {echo_s})""#),
            format!(r#"eval "$(perl -e 'while (<>) {{ print }}' {echo_s})""#),
            format!(r#"eval "$(perl -e 'print $_ while <>' {echo_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; print(open(sys.argv[1], encoding='utf-8').read())" {echo_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; sys.stdout.write(Path(sys.argv[1]).read_text())" {echo_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; sys.stdout.write(Path(sys.argv[1]).read_text(encoding='utf-8'))" {echo_s})""#
            ),
            format!(r#"eval "$(ruby -e 'print File.readlines(ARGV[0]).join("")' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'File.foreach(ARGV[0]) {{|l| print l}}' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'IO.foreach(ARGV[0]) {{|l| print l}}' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'puts File.readlines(ARGV[0])' {echo_s})""#),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1],'utf-8'))" {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); process.stdout.write(fs.readFileSync(process.argv[1],"utf-8"))' {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e "process.stdout.write(require('fs').readFileSync(process.argv[1]))" {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e "process.stdout.write(require('fs').readFileSync(process.argv[1],'utf8'))" {echo_s})""#
            ),
            format!(r#"eval "$(php -r 'echo implode("", file($argv[1]));' {echo_s})""#),
            format!(r#"eval "$(php -r 'echo fread(fopen($argv[1], "r"), 4096);' {echo_s})""#),
            format!(r#"eval "$(php -r 'echo fread(fopen($argv[1], "r"), 4096);' {late_s})""#),
            format!(
                r#"eval "$(php -r 'echo stream_get_contents(fopen($argv[1], "r"));' {echo_s})""#
            ),
            format!(r#"eval "$(php -r 'echo fgets(fopen($argv[1], "r"));' {echo_s})""#),
            format!(r#"eval "$(php -r 'echo fgets(fopen($argv[1], "r"));' {multi_s})""#),
            format!(r#"eval "$(perl -e 'print while <>;' {echo_s})""#),
            format!(
                r#"eval "$(python3 -c "from pathlib import Path; import sys; print(Path(sys.argv[1]).read_text())" {echo_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.write(open(sys.argv[1], encoding='utf-8').read())" {echo_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; print(''.join(open(sys.argv[1]).readlines()))" {echo_s})""#
            ),
            format!(r#"eval "$(ruby -e 'File.read(ARGV[0]).each_line {{|l| print l}}' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'print File.read(ARGV[0], encoding: "UTF-8")' {echo_s})""#),
            format!(
                r#"eval "$(node -e "const {{readFileSync}}=require('fs'); process.stdout.write(readFileSync(process.argv[1],'utf8'))" {echo_s})""#
            ),
            format!(
                r#"eval "$(php -r 'foreach (file($argv[1]) as $line) {{ echo $line; }}' {echo_s})""#
            ),
            format!(r#"eval "$(php -r 'echo file_get_contents($argv[1], false);' {echo_s})""#),
            format!(r#"eval "$(perl -e 'while (<>) {{ print; }}' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'print File.readlines(ARGV[0]).join("\n")' {echo_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; from pathlib import Path; print(Path(sys.argv[1]).read_text(encoding='utf-8'))" {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e "fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1]))" {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1],{{encoding:'utf8'}}))" {echo_s})""#
            ),
            format!(r#"eval "$(php -r 'print implode("", file($argv[1]));' {echo_s})""#),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); console.log(fs.readFileSync(process.argv[1],'utf8'))" {echo_s})""#
            ),
            format!(r#"eval "$(perl -e 'print $_ while <>;' {echo_s})""#),
            format!(r#"eval "$(ruby -e 'print File.read(ARGV[0], mode: "r")' {echo_s})""#),
            format!(r#"eval "$(php -r 'echo file_get_contents($argv[1], true);' {echo_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.writelines(open(sys.argv[1], encoding='utf-8'))" {echo_s})""#
            ),
            format!(r#"eval "$(ruby -e 'puts File.readlines(ARGV[0]).join' {echo_s})""#),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.write(open(sys.argv[1],'rb').read().decode())" {echo_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys; sys.stdout.write(open(sys.argv[1],"rb").read().decode())' {echo_s})""#
            ),
            format!(
                r#"eval "$(python3 -c "import sys; sys.stdout.buffer.write(open(sys.argv[1],'rb').read())" {echo_s})""#
            ),
            format!(
                r#"eval "$(python3 -c 'import sys; sys.stdout.buffer.write(open(sys.argv[1],"rb").read())' {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e "const fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1]))" {echo_s})""#
            ),
            format!(
                r#"eval "$(node -e 'const fs=require("fs"); process.stdout.write(fs.readFileSync(process.argv[1]))' {echo_s})""#
            ),
            format!(
                "eval \"$(node -e 'console.log(require(\"fs\").readFileSync(process.argv[1],\"utf8\"))' {echo_s})\""
            ),
            format!("eval \"$(php -r 'print file_get_contents($argv[1]);' {echo_s})\""),
            format!("eval \"$(perl -e 'print File.read(ARGV[0])' {push_s})\""),
            format!("eval \"$(base32 {b32_s})\""),
            format!("eval \"$(cut -d ' ' -f 2 {push_s})\""),
            format!("eval \"$(dd status=none if={push_s} of={tee_s})\""),
            format!("eval \"$(sed -n '' {push_s})\""),
            format!("eval \"$(cut -c 1-5 {push_s})\""),
            format!("eval \"$(cut -c1-5 {push_s})\""),
            format!("eval \"$(tail -n 0 {push_s})\""),
            format!("eval \"$(rev {push_s})\""),
            format!("eval \"$(nl {push_s})\""),
            format!("eval \"$(base64 {coded_s})\""),
            format!("eval \"$(sort -c {push_s})\""),
            format!("eval \"$(sort -C {push_s})\""),
            format!("eval \"$(sort --check {push_s})\""),
            format!("eval \"$(sort -o {echo_s} {push_s})\""),
            format!("eval \"$(sort {missing_s} {push_s})\""),
            format!("eval \"$(sort {push_s} {missing_s})\""),
            format!("eval \"$(uniq -c {push_s})\""),
            format!("eval \"$(uniq -u {dup_s})\""),
            format!("eval \"$(uniq -d {push_s})\""),
            format!("eval \"$(uniq {push_s} {echo_s})\""),
            format!("eval \"$(tac -s ' ' {push_s})\""),
            format!("eval \"$(od -An -c {push_s})\""),
            format!("eval \"$(xxd {push_s})\""),
            format!("mapfile -t x < {multi_s}; eval \"${{x[0]}}\""),
            format!("mapfile -O 2 -t x < {push_s}; eval \"${{x[0]}}\""),
            format!("mapfile -O 2 -t x < {multi_s}; eval \"${{x[2]}}\""),
            format!("read x y < {push_s}; eval \"$x\""),
            format!("read x y < {push_s}; eval \"$y\""),
            format!("read x < {missing_s}; eval \"$x\""),
            format!("f() {{ local x; mapfile -t x < {push_s}; }}; f; eval \"${{x[0]}}\""),
            "bash -c 'echo hi' <(echo 'git push origin main')".to_string(),
            "source <(echo 'echo hello')".to_string(),
        ];
        for command in &neithers {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command:?}"
            );
        }
        #[cfg(unix)]
        {
            let fifo = dir.join(format!("snapif-c29-fifo-{pid}"));
            let link = dir.join(format!("snapif-c29-fifo-link-{pid}"));
            let _ = std::fs::remove_file(&fifo);
            let _ = std::fs::remove_file(&link);
            let made = std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo");
            assert!(made.success(), "mkfifo {fifo:?}");
            std::os::unix::fs::symlink(&fifo, &link).expect("fifo symlink");
            let fifo_s = plain_script_path(&fifo);
            let link_s = plain_script_path(&link);
            for command in [
                format!("bash {fifo_s}"),
                format!("source {fifo_s}"),
                format!("bash {link_s}"),
                format!("source {link_s}"),
                format!("eval \"$(cat {fifo_s})\""),
                format!("x=$(<{fifo_s}); eval \"$x\""),
                format!("eval \"$(cat < {fifo_s})\""),
                format!("eval \"$(echo 'git push origin main' < {fifo_s})\""),
                format!("eval \"$(tail {fifo_s})\""),
                format!("read x < {fifo_s}; eval \"$x\""),
            ] {
                assert!(
                    matched_action(&policy, "Bash", Some(&command)).is_none(),
                    "{command:?}"
                );
            }
            let _ = std::fs::remove_file(&link);
            let _ = std::fs::remove_file(&fifo);
        }
        for path in [
            &push,
            &echo,
            &assign,
            &multi,
            &beta_last,
            &coded,
            &big,
            &fread_early,
            &fread_late,
        ] {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&sub);
    }

    /// `git -c core.hooksPath=dir commit` runs the executable hooks in
    /// that directory. `git status` does not. `--no-verify` skips
    /// `pre-commit` and `commit-msg`.
    #[test]
    fn git_hooks_path_scores_the_commit_hook() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let root = std::env::temp_dir().join(format!("snapif-hooks-{}", std::process::id()));
        let hooks = root.join("hooks");
        std::fs::create_dir_all(&hooks).expect("hooks");
        let write = |name: &str, body: &str| {
            let path = hooks.join(name);
            std::fs::write(&path, body).expect(name);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&path).expect("meta").permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&path, perms).expect("chmod");
            }
        };
        let kind =
            |command: &str| matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str());
        write("pre-commit", "rm -rf /tmp/x\n");
        let dir = hooks.display().to_string();
        let commit = format!("git -c core.hooksPath={dir} commit -m probe");
        assert_eq!(kind(&commit), Some("bash.rm"), "{commit}");
        let status = format!("git -c core.hooksPath={dir} status");
        assert_eq!(kind(&status), None, "{status}");
        let skip = format!("git -c core.hooksPath={dir} commit --no-verify -m probe");
        assert_eq!(kind(&skip), None, "{skip}");
        write("post-commit", "rm -rf /tmp/y\n");
        assert_eq!(kind(&skip), Some("bash.rm"), "{skip}");
        write("pre-merge-commit", "rm -rf /tmp/z\n");
        let merge = format!("git -c core.hooksPath={dir} merge side");
        assert_eq!(kind(&merge), Some("bash.rm"), "{merge}");
        let merge_skip = format!("git -c core.hooksPath={dir} merge --no-verify side");
        assert_eq!(kind(&merge_skip), None, "{merge_skip}");
        write("post-merge", "rm -rf /tmp/w\n");
        assert_eq!(kind(&merge_skip), Some("bash.rm"), "{merge_skip}");
        write("post-checkout", "rm -rf /tmp/co\n");
        let checkout = format!("git -c core.hooksPath={dir} checkout -- a.txt");
        assert_eq!(kind(&checkout), Some("bash.rm"), "{checkout}");
        let switch = format!("git -c core.hooksPath={dir} switch main");
        assert_eq!(kind(&switch), Some("bash.rm"), "{switch}");
        let clone = format!("git -c core.hooksPath={dir} clone src dest");
        assert_eq!(kind(&clone), Some("bash.rm"), "{clone}");
        let clone_n = format!("git -c core.hooksPath={dir} clone -n src dest");
        assert_eq!(kind(&clone_n), None, "{clone_n}");
        let clone_nc = format!("git -c core.hooksPath={dir} clone --no-checkout src dest");
        assert_eq!(kind(&clone_nc), None, "{clone_nc}");
        assert_eq!(kind(&status), None, "{status}");
        write("prepare-commit-msg", "echo ok\n");
        write("post-commit", "echo ok\n");
        write("commit-msg", "rm -rf /tmp/cm\n");
        let cherry = format!("git -c core.hooksPath={dir} cherry-pick side");
        assert_eq!(kind(&cherry), None, "{cherry}");
        write("prepare-commit-msg", "rm -rf /tmp/prep\n");
        assert_eq!(kind(&cherry), Some("bash.rm"), "{cherry}");
        let cherry_n = format!("git -c core.hooksPath={dir} cherry-pick -n side");
        assert_eq!(kind(&cherry_n), None, "{cherry_n}");
        let cherry_nv = format!("git -c core.hooksPath={dir} cherry-pick --no-verify side");
        assert_eq!(kind(&cherry_nv), None, "{cherry_nv}");
        write("post-merge", "rm -rf /tmp/pm\n");
        let pull = format!("git -c core.hooksPath={dir} pull origin main");
        assert_eq!(kind(&pull), Some("bash.rm"), "{pull}");
        write("post-merge", "echo ok\n");
        write("pre-merge-commit", "rm -rf /tmp/pre\n");
        assert_eq!(kind(&pull), None, "{pull}");
        let pull_merge = format!("git -c core.hooksPath={dir} pull --no-rebase origin main");
        assert_eq!(kind(&pull_merge), Some("bash.rm"), "{pull_merge}");
        write("pre-rebase", "echo ok\n");
        write("post-checkout", "echo ok\n");
        write("post-rewrite", "echo ok\n");
        let pull_rebase = format!("git -c core.hooksPath={dir} pull --rebase origin main");
        assert_eq!(kind(&pull_rebase), None, "{pull_rebase}");
        write("pre-rebase", "rm -rf /tmp/prb\n");
        assert_eq!(kind(&pull_rebase), Some("bash.rm"), "{pull_rebase}");
        let pull_r = format!("git -c core.hooksPath={dir} pull -r origin main");
        assert_eq!(kind(&pull_r), Some("bash.rm"), "{pull_r}");
        let pull_merges = format!("git -c core.hooksPath={dir} pull --rebase=merges origin main");
        assert_eq!(kind(&pull_merges), Some("bash.rm"), "{pull_merges}");
        let pull_nv = format!("git -c core.hooksPath={dir} pull --rebase --no-verify origin main");
        assert_eq!(kind(&pull_nv), Some("bash.rm"), "{pull_nv}");
        write("pre-rebase", "echo ok\n");
        write("post-merge", "rm -rf /tmp/pm\n");
        let pull_false = format!("git -c core.hooksPath={dir} pull --rebase=false origin main");
        assert_eq!(kind(&pull_false), Some("bash.rm"), "{pull_false}");
        write("post-merge", "echo ok\n");
        assert_eq!(kind(&pull_false), None, "{pull_false}");
        assert_eq!(kind(&pull_rebase), None, "{pull_rebase}");
        write("pre-rebase", "rm -rf /tmp/prb\n");
        let rebase = format!("git -c core.hooksPath={dir} rebase main");
        assert_eq!(kind(&rebase), Some("bash.rm"), "{rebase}");
        write("pre-rebase", "echo ok\n");
        write("post-checkout", "echo ok\n");
        write("post-rewrite", "echo ok\n");
        assert_eq!(kind(&rebase), None, "{rebase}");
        let rebase_nv = format!("git -c core.hooksPath={dir} rebase --no-verify main");
        write("pre-rebase", "rm -rf /tmp/prb\n");
        assert_eq!(kind(&rebase_nv), None, "{rebase_nv}");
        write("post-rewrite", "rm -rf /tmp/rw\n");
        assert_eq!(kind(&rebase_nv), Some("bash.rm"), "{rebase_nv}");
        write("applypatch-msg", "rm -rf /tmp/am\n");
        let am = format!("git -c core.hooksPath={dir} am patch");
        assert_eq!(kind(&am), Some("bash.rm"), "{am}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With no `core.hooksPath`, `git checkout` runs the executable
    /// `.git/hooks/post-checkout` of the repository `cd` or `-C`
    /// selected. `git status` does not. An explicit `core.hooksPath`
    /// replaces that directory. A stuck `-Cpath` does not select it.
    #[test]
    fn git_checkout_scores_the_default_hook_directory() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let root = std::env::temp_dir().join(format!("snapif-defhook-{}", std::process::id()));
        let repo = root.join("repo");
        let other = root.join("other");
        let empty = root.join("empty");
        let hooks = repo.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).expect("hooks");
        std::fs::create_dir_all(&other).expect("other");
        std::fs::create_dir_all(&empty).expect("empty");
        let path = hooks.join("post-checkout");
        std::fs::write(&path, "rm -rf /tmp/co\n").expect("hook");
        let chmod = |mode: u32| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&path).expect("meta").permissions();
                perms.set_mode(mode);
                std::fs::set_permissions(&path, perms).expect("chmod");
            }
            let _ = mode;
        };
        let kind =
            |command: &str| matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str());
        let repo_s = repo.display().to_string();
        let root_s = root.display().to_string();
        let other_s = other.display().to_string();
        let empty_s = empty.display().to_string();
        let hooks_s = hooks.display().to_string();
        let checkout = format!("git -C {repo_s} checkout side");
        #[cfg(unix)]
        {
            chmod(0o644);
            assert_eq!(kind(&checkout), None, "{checkout}");
        }
        chmod(0o755);
        assert_eq!(kind(&checkout), Some("bash.rm"), "{checkout}");
        let status = format!("git -C {repo_s} status");
        assert_eq!(kind(&status), None, "{status}");
        let from_cd = format!("cd {repo_s} && git checkout side");
        assert_eq!(kind(&from_cd), Some("bash.rm"), "{from_cd}");
        let relative = format!("cd {root_s} && git -C repo checkout side");
        assert_eq!(kind(&relative), Some("bash.rm"), "{relative}");
        let into_repo = format!("git -C {other_s} -C ../repo checkout side");
        assert_eq!(kind(&into_repo), Some("bash.rm"), "{into_repo}");
        let into_other = format!("git -C {repo_s} -C ../other checkout side");
        assert_eq!(kind(&into_other), None, "{into_other}");
        let forced = format!("git -C {repo_s} -c core.hooksPath={other_s} checkout side");
        assert_eq!(kind(&forced), None, "{forced}");
        let explicit = format!("git -c core.hooksPath={hooks_s} -C {other_s} checkout side");
        assert_eq!(kind(&explicit), Some("bash.rm"), "{explicit}");
        let stuck = format!("cd {empty_s} && git -C{repo_s} checkout side");
        assert_eq!(kind(&stuck), None, "{stuck}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `npx` and `npm exec` run a `PATH` program before a local bin.
    /// `pnpm exec` and `bunx` run the local bin first. `pnpm` does
    /// nothing without a `package.json`. `-c` and `--call` run that
    /// string. A bin that is not executable does not run.
    #[test]
    fn package_exec_scores_the_command_the_host_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let root = std::env::temp_dir().join(format!("snapif-exec-{}", std::process::id()));
        let proj = root.join("proj");
        let bins = proj.join("node_modules").join(".bin");
        let sub = proj.join("sub");
        let empty = root.join("empty");
        std::fs::create_dir_all(&bins).expect("bins");
        std::fs::create_dir_all(&sub).expect("sub");
        std::fs::create_dir_all(&empty).expect("empty");
        std::fs::write(proj.join("package.json"), "{\"name\":\"p\"}\n").expect("pkg");
        let write = |name: &str, mode: u32| {
            let path = bins.join(name);
            std::fs::write(&path, "#!/bin/sh\ngit push origin main\n").expect(name);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&path).expect("meta").permissions();
                perms.set_mode(mode);
                std::fs::set_permissions(&path, perms).expect("chmod");
            }
            let _ = mode;
        };
        write("hostprobe", 0o755);
        write("echo", 0o755);
        write("quiet", 0o644);
        let kind =
            |command: &str| matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str());
        let root_s = root.display().to_string();
        let proj_s = proj.display().to_string();
        let empty_s = empty.display().to_string();
        let push = "git push origin main";
        let cases = [
            (
                format!("cd {proj_s} && npx --no-install hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {proj_s}/sub && npx --no-install hostprobe"),
                Some("git.push"),
            ),
            (format!("cd {proj_s} && npx --no-install echo hi"), None),
            (
                format!("cd {empty_s} && npx --no-install {push}"),
                Some("git.push"),
            ),
            (format!("cd {proj_s} && npx --no-install quiet"), None),
            (format!("cd {proj_s} && npx --no-install nosuchbin"), None),
            (
                format!("cd {proj_s} && npm exec --offline -- hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {proj_s} && npm x --offline -- hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {root_s} && npm exec --prefix proj --offline -- hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {root_s} && npm exec --prefix=proj --offline -- hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {empty_s} && npm exec --offline -c '{push}'"),
                Some("git.push"),
            ),
            (
                format!("cd {empty_s} && npm exec --offline --call='{push}'"),
                Some("git.push"),
            ),
            (
                format!("cd {empty_s} && npx --no-install -c '{push}'"),
                Some("git.push"),
            ),
            (format!("npm exec --offline"), None),
            (
                format!("cd {proj_s} && pnpm exec hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {proj_s} && pnpm exec echo hi"),
                Some("git.push"),
            ),
            (format!("cd {proj_s} && pnpm exec {push}"), Some("git.push")),
            (format!("cd {proj_s}/sub && pnpm exec hostprobe"), None),
            (
                format!("cd {proj_s}/sub && pnpm exec {push}"),
                Some("git.push"),
            ),
            (format!("cd {empty_s} && pnpm exec {push}"), None),
            (format!("cd {proj_s} && pnpm exec quiet"), None),
            (format!("cd {proj_s} && pnpm exec nosuchbin"), None),
            (
                format!("cd {root_s} && pnpm --dir proj exec hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {root_s} && pnpm --dir=proj exec hostprobe"),
                Some("git.push"),
            ),
            (format!("cd {proj_s} && bunx hostprobe"), Some("git.push")),
            (format!("cd {proj_s} && bun x hostprobe"), Some("git.push")),
            (format!("cd {proj_s} && bunx echo hi"), Some("git.push")),
            (format!("cd {proj_s}/sub && bunx echo hi"), Some("git.push")),
            (format!("cd {empty_s} && bunx {push}"), Some("git.push")),
            (
                format!("cd {empty_s} && bunx --cwd {proj_s} hostprobe"),
                Some("git.push"),
            ),
            (
                format!("cd {root_s} && bunx --cwd=proj hostprobe"),
                Some("git.push"),
            ),
            (format!("cd {proj_s} && bunx quiet"), None),
        ];
        for (command, want) in cases {
            assert_eq!(kind(&command), want, "{command}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `ninja` runs each scheduled `command`. With no target, `default`
    /// picks the roots; otherwise every build runs. A dependency runs
    /// too. `-n` and an unknown target do not.
    #[test]
    fn ninja_scores_the_builds_it_runs() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let root = std::env::temp_dir().join(format!("snapif-ninja-{}", std::process::id()));
        let proj = root.join("proj");
        let alt = root.join("alt");
        std::fs::create_dir_all(&proj).expect("proj");
        std::fs::create_dir_all(&alt).expect("alt");
        std::fs::write(
            proj.join("build.ninja"),
            "rule top\n  command = echo ok\nrule dep\n  command = git push origin main\nbuild mid: dep\nbuild out: top mid\nbuild other: top\n",
        )
        .expect("ninja");
        std::fs::write(
            alt.join("custom.ninja"),
            "rule one\n  command = git push origin main\nbuild out: one\n",
        )
        .expect("custom");
        let kind =
            |command: &str| matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str());
        let root_s = root.display().to_string();
        let proj_s = proj.display().to_string();
        let all = format!("cd {proj_s} && ninja");
        assert_eq!(kind(&all), Some("git.push"), "{all}");
        let named = format!("cd {proj_s} && ninja out");
        assert_eq!(kind(&named), Some("git.push"), "{named}");
        let only = format!("cd {proj_s} && ninja other");
        assert_eq!(kind(&only), None, "{only}");
        let dry = format!("cd {proj_s} && ninja -n");
        assert_eq!(kind(&dry), None, "{dry}");
        let missing = format!("cd {proj_s} && ninja absent");
        assert_eq!(kind(&missing), None, "{missing}");
        let switched = format!("cd {root_s} && ninja -C proj");
        assert_eq!(kind(&switched), Some("git.push"), "{switched}");
        let file = format!("cd {root_s} && ninja -f alt/custom.ninja");
        assert_eq!(kind(&file), Some("git.push"), "{file}");
        let both = format!("cd {root_s} && ninja -C alt -f custom.ninja");
        assert_eq!(kind(&both), Some("git.push"), "{both}");
        std::fs::write(
            proj.join("build.ninja"),
            "rule top\n  command = echo ok\nrule side\n  command = git push origin main\nbuild out: top\nbuild other: side\ndefault out\n# git push origin main\n",
        )
        .expect("default");
        assert_eq!(kind(&all), None, "{all}");
        let side = format!("cd {proj_s} && ninja other");
        assert_eq!(kind(&side), Some("git.push"), "{side}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `vim -c '!cmd'`, `--cmd`, `+!cmd`, and `-S` run `cmd`. `qa!`
    /// and `:echo` do not.
    #[test]
    fn vim_bang_scores_the_shell_command() {
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let root = std::env::temp_dir().join(format!("snapif-vim-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::write(root.join("go.ex"), "!git push origin main\nqa!\n").expect("ex");
        std::fs::write(root.join("quiet.ex"), "qa!\n").expect("quiet");
        let kind =
            |command: &str| matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str());
        let root_s = root.display().to_string();
        let push = "git push origin main";
        let cases = [
            (
                format!("vim -n -es -c '!{push}' -c 'qa!'"),
                Some("git.push"),
            ),
            (format!("vim -c ':!{push}' -c qa!"), Some("git.push")),
            (format!("vim -c '  !{push}' -c qa!"), Some("git.push")),
            (format!("vim -c 'echo {push}' -c qa!"), None),
            (format!("vim -c qa!"), None),
            (format!("vim --cmd '!{push}' -c qa!"), Some("git.push")),
            (format!("vim '+!{push}' '+qa!'"), Some("git.push")),
            (format!("cd {root_s} && vim -S go.ex"), Some("git.push")),
            (format!("cd {root_s} && vim -S quiet.ex"), None),
            (format!("cd {root_s} && vim -S missing.ex"), None),
        ];
        for (command, want) in cases {
            assert_eq!(kind(&command), want, "{command}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The host ran these interpreters and package scripts. A file that
    /// only prints the same words does not run them. A missing file
    /// stays unmatched. `python3 -c` does not read a redirected file.
    #[test]
    fn host_programs_score_the_command_they_run() {
        let _lock = drive_letter_lock();
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let root = std::env::temp_dir().join(format!("snapif-host-prog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let write = |name: &str, body: &str| {
            std::fs::write(root.join(name), body).expect(name);
        };
        let kind =
            |command: &str| matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str());
        let show = root.display().to_string();
        write(
            "system.py",
            "import os\nos.system(\"git push origin main\")\n",
        );
        write(
            "popen.py",
            "import os\nprint(os.popen(\"git push origin main\").read())\n",
        );
        write(
            "run.py",
            "import subprocess\nsubprocess.run([\"git\", \"push\", \"origin\", \"main\"])\n",
        );
        write(
            "call.py",
            "import subprocess\nsubprocess.check_call([\"git\", \"push\", \"origin\", \"main\"])\n",
        );
        write(
            "shell.py",
            "import subprocess\nsubprocess.run(\"git push origin main\", shell=True)\n",
        );
        write(
            "plain.py",
            "import subprocess\nsubprocess.run(\"git push origin main\")\n",
        );
        write("print.py", "print(\"git push origin main\")\n");
        write(
            "comment.py",
            "# os.system(\"git push origin main\")\nprint(\"ok\")\n",
        );
        write(
            "exec.js",
            "const {execSync} = require(\"child_process\");\nexecSync(\"git push origin main\");\n",
        );
        write(
            "spawn.js",
            "const {spawnSync} = require(\"child_process\");\nspawnSync(\"git\", [\"push\", \"origin\", \"main\"]);\n",
        );
        write("log.js", "console.log(\"git push origin main\");\n");
        write("system.rb", "system(\"git push origin main\")\n");
        write(
            "argv.rb",
            "system(\"git\", \"push\", \"origin\", \"main\")\n",
        );
        write("tick.rb", "print `git push origin main`\n");
        write("puts.rb", "puts \"git push origin main\"\n");
        write("system.pl", "system(\"git push origin main\");\n");
        write("tick.pl", "print `git push origin main`;\n");
        write("print.pl", "print \"git push origin main\\n\";\n");
        write("system.php", "<?php\nsystem(\"git push origin main\");\n");
        write("tick.php", "<?php\necho `git push origin main`;\n");
        write("echo.php", "<?php\necho \"git push origin main\\n\";\n");
        write(
            "execvp.py",
            "import os\nos.execvp(\"git\", [\"git\", \"push\", \"origin\", \"main\"])\n",
        );
        write(
            "execl.py",
            "import os\nos.execl(\"/bin/git\", \"git\", \"push\", \"origin\", \"main\")\n",
        );
        write(
            "shell1.py",
            "import subprocess\nsubprocess.run(\"git push origin main\", shell=1)\n",
        );
        write(
            "shell2.py",
            "import subprocess\nsubprocess.run(\"git push origin main\", shell=2)\n",
        );
        write(
            "shell0.py",
            "import subprocess\nsubprocess.run(\"git push origin main\", shell=0)\n",
        );
        write(
            "false.py",
            "import subprocess\nsubprocess.run(\"git push origin main\", shell=False)\n",
        );
        write("run.lua", "os.execute(\"git push origin main\")\n");
        write(
            "comment.lua",
            "-- os.execute(\"git push origin main\")\nprint(1)\n",
        );
        write("run.R", "system(\"git push origin main\")\n");
        std::fs::write(root.join("justfile"), "push:\n\tgit push origin main\n").expect("justfile");
        write(
            "run.swift",
            "import Foundation\nlet task = Process()\ntask.executableURL = URL(fileURLWithPath: \"/usr/bin/env\")\ntask.arguments = [\"git\", \"push\", \"origin\", \"main\"]\ntry? task.run()\ntask.waitUntilExit()\n",
        );
        write(
            "preload.js",
            "require(\"child_process\").execSync(\"git push origin main\");\n",
        );
        write(
            "need.rb",
            "system(\"git\", \"push\", \"origin\", \"main\")\n",
        );
        write("run.awk", "BEGIN { system(\"git push origin main\") }\n");
        write("push.awk", "BEGIN { system(\"git push origin main\") }\n");
        write("push.sh", "git push origin main\n");
        write("Makefile", "push:\n\tgit push origin main\n");
        write("print.awk", "BEGIN { print \"git push origin main\" }\n");
        write("run.scpt", "do shell script \"git push origin main\"\n");
        write("mod.py", "import os\nos.system(\"git push origin main\")\n");
        std::fs::create_dir_all(root.join("pkg")).expect("pkg");
        write(
            "pkg/__main__.py",
            "import os\nos.system(\"git push origin main\")\n",
        );
        write("Need.pm", "system(\"git push origin main\");\n1;\n");
        write("run.exp", "spawn git push origin main\nexpect eof\n");
        write("eof.exp", "expect eof\n");
        write("run.tcl", "exec git push origin main\n");
        write("puts.tcl", "puts [exec git push origin main]\n");
        write("hello.tcl", "puts hello\n");
        write("args.tcl", "puts hello exec git push origin main\n");
        write("push.csh", "git push origin main\n");
        write(
            "main.go",
            "package main\nimport (\n\t\"os\"\n\t\"os/exec\"\n)\nfunc main() {\n\tcmd := exec.Command(\"git\", \"push\", \"origin\", \"main\")\n\tcmd.Stdout = os.Stdout\n\t_ = cmd.Run()\n}\n",
        );
        write(
            "print.go",
            "package main\nimport \"fmt\"\nfunc main() { fmt.Println(\"git push origin main\") }\n",
        );
        write(
            "run.cmake",
            "execute_process(COMMAND git push origin main)\n",
        );
        write("quiet.cmake", "message(STATUS \"git push origin main\")\n");
        let app = root.join("app/src");
        std::fs::create_dir_all(&app).expect("app");
        std::fs::write(
            root.join("app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("toml");
        std::fs::write(
            app.join("main.rs"),
            "fn main() {\n    let _ = std::process::Command::new(\"git\")\n        .args([\"push\", \"origin\", \"main\"])\n        .output();\n}\n",
        )
        .expect("cargo main");
        write(
            "Main.java",
            "public class Main {\n  public static void main(String[] args) throws Exception {\n    Process p = new ProcessBuilder(\"git\", \"push\", \"origin\", \"main\").inheritIO().start();\n    p.waitFor();\n  }\n}\n",
        );
        write(
            "run.kts",
            "ProcessBuilder(\"git\", \"push\", \"origin\", \"main\").inheritIO().start().waitFor()\n",
        );
        std::fs::create_dir_all(root.join("pkg")).expect("pkg");
        std::fs::create_dir_all(root.join("buntest")).expect("buntest");
        std::fs::create_dir_all(root.join("bunquiet")).expect("bunquiet");
        std::fs::write(
            root.join("pkg/package.json"),
            "{\"scripts\":{\"pretest\":\"rm -rf /tmp/x\",\"test\":\"git push origin main\"}}",
        )
        .expect("pkg json");
        std::fs::write(
            root.join("buntest/push.test.js"),
            "require(\"child_process\").execSync(\"git push origin main\");\n",
        )
        .expect("bun test file");
        std::fs::write(
            root.join("buntest/other.js"),
            "require(\"child_process\").execSync(\"rm -rf /tmp/x\");\n",
        )
        .expect("bun other");
        std::fs::write(
            root.join("bunquiet/other.js"),
            "require(\"child_process\").execSync(\"git push origin main\");\n",
        )
        .expect("bun quiet");
        write("run.m", "system(\"git push origin main\");\n");
        write("quiet.m", "disp(1);\n");
        write("run.jl", "run(`git push origin main`)\n");
        write("quiet.jl", "println(1)\n");
        write(
            "run.exs",
            "System.cmd(\"git\", [\"push\", \"origin\", \"main\"])\n",
        );
        write("quiet.exs", "IO.puts(\"git push origin main\")\n");
        write("cmds.sql", ".shell git push origin main\n");
        write("quiet.sql", "select 1;\n");
        write(
            "run.coffee",
            "child_process = require('child_process')\nchild_process.execSync('git push origin main', {stdio: 'inherit'})\n",
        );
        write(
            "Run.hs",
            "import System.Process\nmain = do\n  out <- readCreateProcess (proc \"git\" [\"push\", \"origin\", \"main\"]) \"\"\n  putStr out\n",
        );
        write("quiet.hs", "main = putStrLn \"git push origin main\"\n");
        write("push.lua", "os.execute(\"git push origin main\")\n");
        write("push.rb", "system(\"git push origin main\")\n");
        write(
            "push.js",
            "require(\"child_process\").execSync(\"git push origin main\", {stdio:\"inherit\"});\n",
        );
        write(
            "push.py",
            "import os\nos.system(\"git push origin main\")\n",
        );
        write("push.pl", "system(\"git push origin main\");\n");
        write("Push.pm", "system(\"git push origin main\");\n1;\n");
        write("push.php", "<?php\nsystem(\"git push origin main\");\n");
        write("push.R", "system(\"git push origin main\")\n");
        write("push.jl", "run(`git push origin main`)\n");
        write("push.m", "system(\"git push origin main\");\n");
        std::fs::create_dir_all(root.join("empty-lib")).expect("empty-lib");
        std::fs::create_dir_all(root.join("gorun")).expect("gorun");
        std::fs::write(
            root.join("gorun/main.go"),
            "package main\nimport \"os/exec\"\nfunc main() {\n\t_ = exec.Command(\"git\", \"push\", \"origin\", \"main\").Run()\n}\n",
        )
        .expect("gorun");
        std::fs::create_dir_all(root.join("gogen")).expect("gogen");
        std::fs::write(
            root.join("gogen/main.go"),
            "package main\n//go:generate git push origin main\nfunc main() {}\n",
        )
        .expect("gogen");
        std::fs::write(
            root.join("gogen/quiet.go"),
            "package main\nfunc main() { println(\"//go:generate git push origin main\") }\n",
        )
        .expect("gogen quiet");
        write("print.swift", "print(\"git push origin main\")\n");
        write(
            "note.swift",
            "// task.arguments = [\"git\", \"push\", \"origin\", \"main\"]\n// task.run()\nprint(1)\n",
        );
        write(
            "package.json",
            "{\"scripts\":{\"pretest\":\"git push origin pretest\",\"test\":\"echo ok\",\"posttest\":\"echo ok\",\"prestart\":\"echo ok\",\"start\":\"git push origin start\",\"poststart\":\"echo ok\",\"preunused\":\"echo ok\",\"unused\":\"git push origin unused\",\"postunused\":\"echo ok\"}}",
        );
        let review = [
            format!("python3 {show}/system.py"),
            format!("python3 {show}/popen.py"),
            format!("python3 {show}/run.py"),
            format!("python3 {show}/call.py"),
            format!("python3 {show}/shell.py"),
            format!("python3 < {show}/system.py"),
            format!("python3 - < {show}/run.py"),
            "python3 -c 'import os; os.system(\"git push origin main\")'".to_string(),
            format!("node {show}/exec.js"),
            format!("node {show}/spawn.js"),
            format!("node < {show}/exec.js"),
            "node -e 'require(\"child_process\").execSync(\"git push origin main\")'".to_string(),
            format!("ruby {show}/system.rb"),
            format!("ruby {show}/argv.rb"),
            format!("ruby {show}/tick.rb"),
            format!("ruby < {show}/system.rb"),
            "ruby -e 'system(\"git push origin main\")'".to_string(),
            format!("perl {show}/system.pl"),
            format!("perl {show}/tick.pl"),
            format!("perl < {show}/system.pl"),
            "perl -e 'system(\"git push origin main\")'".to_string(),
            format!("php {show}/system.php"),
            format!("php {show}/tick.php"),
            format!("php < {show}/system.php"),
            "php -r 'system(\"git push origin main\");'".to_string(),
            format!("npm test --prefix {show}"),
            format!("npm start --prefix {show}"),
            format!("npm run unused --prefix {show}"),
            format!("python3 {show}/execvp.py"),
            format!("python3 {show}/execl.py"),
            format!("python3 {show}/shell1.py"),
            format!("python3 {show}/shell2.py"),
            format!("lua {show}/run.lua"),
            format!("Rscript {show}/run.R"),
            format!("just --justfile {show}/justfile push"),
            format!("uv run --no-project {show}/system.py"),
            format!("swift {show}/run.swift"),
            format!("node -r {show}/preload.js -e '0'"),
            format!("node --require {show}/preload.js -e '0'"),
            format!("ruby -r {show}/need.rb -e '0'"),
            format!("ruby -r{show}/need.rb -e '0'"),
            format!("awk -f {show}/run.awk"),
            "awk 'BEGIN { system(\"git push origin main\") }'".to_string(),
            format!("osascript {show}/run.scpt"),
            "osascript -e 'do shell script \"git push origin main\"'".to_string(),
            format!("bun {show}/preload.js"),
            format!("bun run {show}/preload.js"),
            format!("node --import {show}/preload.js -e '0'"),
            format!("perl -I {show} -MNeed -e 0"),
            format!("expect {show}/run.exp"),
            "expect -c 'spawn git push origin main'".to_string(),
            format!("tclsh {show}/run.tcl"),
            format!("tclsh {show}/puts.tcl"),
            "csh -c 'git push origin main'".to_string(),
            "tcsh -c 'git push origin main'".to_string(),
            format!("csh {show}/push.csh"),
            format!("tcsh {show}/push.csh"),
            "python3 -c 'import subprocess; subprocess.run(\"git push origin main\", shell=\"yes\")'"
                .to_string(),
            "python3 -c 'import subprocess; subprocess.run(\"git push origin main\", shell=\"False\")'"
                .to_string(),
            "python3 -c 'import subprocess; subprocess.run(\"git push origin main\", shell=\"0\")'"
                .to_string(),
            "lua -e 'os.execute(\"git push origin main\")'".to_string(),
            "R --vanilla -e 'system(\"git push origin main\")'".to_string(),
            "Rscript --vanilla -e 'system(\"git push origin main\")'".to_string(),
            format!("awk -f{show}/run.awk"),
            format!("just -f {show}/justfile push"),
            format!("cd {show}/empty-lib && just -f ../justfile push"),
            format!("cd {show}/empty-lib && cmake -P ../run.cmake"),
            format!("cd {show}/empty-lib && swift ../run.swift"),
            format!("cd {show}/empty-lib && kotlinc -script ../run.kts"),
            format!("cd {show}/empty-lib && expect ../run.exp"),
            format!("cd {show}/empty-lib && bun ../preload.js"),
            format!("cd {show}/empty-lib && go run ../gorun/main.go"),
            format!("cd {show}/empty-lib && coffee ../run.coffee"),
            format!("cd {show}/empty-lib && runghc ../Run.hs"),
            format!("cd {show}/empty-lib && uv run --no-project ../push.py"),
            format!("cd {show}/empty-lib && julia --startup-file=no ../push.jl"),
            format!("cd {show}/empty-lib && elixir ../run.exs"),
            format!("cd {show}/empty-lib && osascript ../run.scpt"),
            "sqlite3 :memory: '.shell git push origin main'".to_string(),
            "octave --quiet --eval 'system(\"git push origin main\");'".to_string(),
            "julia --startup-file=no -e 'run(`git push origin main`)'".to_string(),
            "elixir -e 'System.cmd(\"git\", [\"push\", \"origin\", \"main\"])'".to_string(),
            "elixir -e 'System.shell(\"git push origin main\")'".to_string(),
            "erl -noshell -eval 'os:cmd(\"git push origin main\"), halt().'".to_string(),
            format!("go run {show}/main.go"),
            format!("cmake -P {show}/run.cmake"),
            format!("cargo run --quiet --manifest-path {show}/app/Cargo.toml"),
            format!("cd {show} && javac Main.java && java Main"),
            format!("cd {show}/pkg && node --run test"),
            format!("cd {show}/buntest && bun test"),
            format!("kotlinc -script {show}/run.kts"),
            "ghc -e 'System.Process.system \"git push origin main\"'".to_string(),
            format!("cd {show} && lua -l push -e '0'"),
            format!("cd {show} && lua -lpush -e '0'"),
            format!("cd {show} && lua -e 'dofile(\"push.lua\")'"),
            format!("cd {show} && ruby -e 'load \"push.rb\"'"),
            format!("cd {show} && node -e 'require(\"./push.js\")'"),
            format!("cd {show} && python3 -c 'exec(open(\"push.py\").read())'"),
            format!("cd {show} && perl -e 'do \"./push.pl\"'"),
            format!("cd {show} && python3 -c 'import push'"),
            format!("cd {show} && ruby -e 'require \"./push.rb\"'"),
            format!("cd {show} && php -r 'include \"push.php\";'"),
            format!("cd {show} && perl -e 'require \"./push.pl\";'"),
            format!("cd {show} && python3 -c 'from push import os'"),
            format!("cd {show} && php -r 'require \"push.php\";'"),
            format!("cd {show} && ruby -e 'require_relative \"push.rb\"'"),
            format!("cd {show} && R --vanilla -e 'source(\"push.R\")'"),
            format!("cd {show} && julia --startup-file=no -e 'include(\"push.jl\")'"),
            format!("cd {show} && python3 -c 'import importlib; importlib.import_module(\"push\")'"),
            format!("cd {show} && python3 -c 'import runpy; runpy.run_path(\"push.py\")'"),
            format!("cd {show} && php -r 'include_once \"push.php\";'"),
            format!("cd {show} && php -r 'require_once \"push.php\";'"),
            format!("cd {show} && octave --quiet --eval 'source(\"push.m\");'"),
            format!("cd {show}/gogen && go generate"),
            format!("cd {show} && RUBYOPT='-r./push.rb' ruby -e '0'"),
            format!("cd {show} && NODE_OPTIONS='--require ./push.js' node -e '0'"),
            format!("cd {show} && PERL5OPT='-I. -MPush' perl -e '0'"),
            format!("cd {show} && PERL5LIB=. perl -MPush -e 0"),
            format!("cd {show}/empty-lib && perl -I.. -MPush -e 0"),
            format!("cd {show}/empty-lib && ruby -I.. -e 'require \"push\"'"),
            format!("cd {show}/empty-lib && AWKPATH=.. awk -f push.awk"),
            format!("cd {show}/empty-lib && bash ../push.sh"),
            format!("cd {show}/empty-lib && sh ../push.sh"),
            format!("cd {show}/empty-lib && awk -f ../push.awk"),
            format!("cd {show}/empty-lib && make -f ../Makefile push"),
            format!("cd {show}/empty-lib && ruby -r../push.rb -e 0"),
            format!("cd {show}/empty-lib && lua ../push.lua"),
            format!("cd {show}/empty-lib && perl ../push.pl"),
            format!("cd {show}/empty-lib && python3 ../push.py"),
            format!("cd {show}/empty-lib && R_PROFILE_USER=../push.R R --no-save -e '1'"),
            format!("PYTHONPATH='{show}' python3 -c 'import push'"),
            format!("RUBYLIB='{show}' ruby -e 'require \"push\"'"),
            format!("cd {show}/empty-lib && LUA_PATH='{show}/?.lua' lua -l push -e '0'"),
            format!("cd {show}/empty-lib && PYTHONPATH='{show}' python3 -m push"),
            format!("cd {show}/empty-lib && NODE_PATH='{show}' node -e 'require(\"push\")'"),
            format!(
                "cd {show}/empty-lib && php -d include_path='{show}' -r 'include \"push.php\";'"
            ),
            format!(
                "cd {show} && python3 -c 'exec(compile(open(\"push.py\").read(), \"push.py\", \"exec\"))'"
            ),
            "swift -e 'import Foundation; let t = Process(); t.executableURL = URL(fileURLWithPath: \"/usr/bin/env\"); t.arguments = [\"git\", \"push\", \"origin\", \"main\"]; try? t.run(); t.waitUntilExit()'".to_string(),
            format!("coffee {show}/run.coffee"),
            format!("runghc {show}/Run.hs"),
            format!("runhaskell {show}/Run.hs"),
            "ghci -e 'System.Process.system \"git push origin main\"' -e ':quit'".to_string(),
            "mix run --no-mix-exs -e 'System.cmd(\"git\", [\"push\", \"origin\", \"main\"], into: IO.stream(:stdio, :line))'".to_string(),
            "clj -M -e '(let [p (.exec (Runtime/getRuntime) \"git push origin main\")] (println 1))'".to_string(),
            format!("octave --quiet {show}/run.m"),
            format!("julia --startup-file=no {show}/run.jl"),
            format!("elixir {show}/run.exs"),
            format!("sqlite3 :memory: '.read {show}/cmds.sql'"),
            "osascript -l JavaScript -e '$.system(\"git push origin main\")'".to_string(),
            "sbcl --noinform --non-interactive --eval '(sb-ext:run-program \"git\" (list \"push\" \"origin\" \"main\") :search t :output *standard-output*)' --quit".to_string(),
        ];
        for command in &review {
            assert_eq!(kind(command), Some("git.push"), "{command}");
        }
        let quiet = [
            format!("python3 {show}/print.py"),
            format!("python3 {show}/comment.py"),
            format!("python3 {show}/plain.py"),
            format!("python3 {show}/missing.py"),
            format!("python3 -c 'print(1)' < {show}/system.py"),
            format!("node {show}/log.js"),
            format!("ruby {show}/puts.rb"),
            format!("perl {show}/print.pl"),
            format!("php {show}/echo.php"),
            format!("npm run --prefix {show}"),
            format!("python3 {show}/shell0.py"),
            format!("python3 {show}/false.py"),
            format!("lua {show}/comment.lua"),
            format!("just --justfile {show}/justfile absent"),
            format!("swift {show}/print.swift"),
            format!("swift {show}/note.swift"),
            format!("awk -f {show}/print.awk"),
            "osascript -e 'display dialog \"git push origin main\"'".to_string(),
            format!("ruby -r {show}/missing.rb -e '0'"),
            "node -e '0'".to_string(),
            format!("expect {show}/eof.exp"),
            "expect -c 'expect eof'".to_string(),
            format!("tclsh {show}/hello.tcl"),
            format!("tclsh {show}/args.tcl"),
            format!("perl -MNeed -e 0"),
            "bun run test".to_string(),
            "csh -c 'echo hi'".to_string(),
            "python3 -c 'import subprocess; subprocess.run(\"git push origin main\", shell=\"\")'"
                .to_string(),
            "lua -e 'print(1)'".to_string(),
            "R --vanilla -e 'print(1)'".to_string(),
            "sqlite3 :memory: 'select 1'".to_string(),
            "octave --quiet --eval 'disp(1)'".to_string(),
            "julia --startup-file=no -e 'println(1)'".to_string(),
            "elixir -e 'IO.puts(\"git push origin main\")'".to_string(),
            "erl -noshell -eval 'halt().'".to_string(),
            format!("awk -f{show}/print.awk"),
            format!("just -f {show}/justfile absent"),
            format!("cd {show}/empty-lib && just -f ../missing.just push"),
            format!("cd {show}/empty-lib && cmake -P ../missing.cmake"),
            format!("cd {show}/empty-lib && swift ../missing.swift"),
            format!("cd {show}/empty-lib && expect ../missing.exp"),
            format!("cd {show}/empty-lib && coffee ../missing.coffee"),
            format!("cd {show}/empty-lib && osascript ../missing.scpt"),
            format!("go run {show}/print.go"),
            format!("cmake -P {show}/quiet.cmake"),
            format!("cargo build --quiet --manifest-path {show}/app/Cargo.toml"),
            format!("cd {show}/bunquiet && bun test"),
            format!("cd {show}/pkg && node --run absent"),
            format!("cd {show} && java Missing"),
            format!("cd {show} && lua -l missing -e '0'"),
            format!("cd {show} && lua -e 'dofile(\"missing.lua\")'"),
            format!("cd {show} && ruby -e 'print 1'"),
            format!("cd {show} && node -e 'require(\"fs\")'"),
            format!("cd {show} && python3 -c 'open(\"push.py\").read()'"),
            format!("cd {show} && perl -e 'do \"push.pl\"'"),
            format!("cd {show} && python3 -c 'import os'"),
            format!("cd {show} && ruby -e 'require \"json\"'"),
            format!("cd {show} && php -r 'echo 1;'"),
            format!("cd {show} && perl -e 'require \"push.pl\";'"),
            format!("cd {show} && python3 -c 'from missing import os'"),
            format!("cd {show} && julia --startup-file=no -e 'include(\"quiet.jl\")'"),
            format!(
                "cd {show} && python3 -c 'import importlib; importlib.import_module(\"os.path\")'"
            ),
            format!("cd {show}/gogen && go build"),
            format!("cd {show} && RUBYOPT='-r./missing.rb' ruby -e '0'"),
            format!("cd {show} && NODE_OPTIONS='--require ./missing.js' node -e '0'"),
            format!("cd {show} && PERL5OPT='-I. -MMissing' perl -e '0'"),
            format!("cd {show} && PERL5LIB=. perl -MMissing -e 0"),
            format!("cd {show}/empty-lib && perl -I.. -MMissing -e 0"),
            format!("cd {show}/empty-lib && ruby -I.. -e 'require \"missing\"'"),
            format!("cd {show}/empty-lib && AWKPATH=.. awk -f missing.awk"),
            format!("cd {show}/empty-lib && bash ../missing.sh"),
            format!("cd {show}/empty-lib && make -n -f ../Makefile push"),
            format!("cd {show}/empty-lib && lua ../missing.lua"),
            format!("cd {show}/empty-lib && R_PROFILE_USER=../push.R R --vanilla --no-save -e '1'"),
            format!("PYTHONPATH='{show}/empty-lib' python3 -c 'import push'"),
            format!("RUBYLIB='{show}/empty-lib' ruby -e 'require \"push\"'"),
            format!("cd {show}/empty-lib && LUA_PATH='{show}/?.lua' lua -l missing -e '0'"),
            format!("cd {show}/empty-lib && PYTHONPATH='{show}/empty-lib' python3 -m push"),
            format!(
                "cd {show}/empty-lib && NODE_PATH='{show}/empty-lib' node -e 'require(\"push\")'"
            ),
            format!(
                "cd {show}/empty-lib && php -d include_path='{show}/empty-lib' -r 'include \"missing.php\";'"
            ),
            "swift -e 'print(1)'".to_string(),
            format!("coffee -c {show}/run.coffee"),
            format!("runghc {show}/quiet.hs"),
            "ghci -e '1'".to_string(),
            "mix -e 'System.cmd(\"git\", [\"push\", \"origin\", \"main\"])'".to_string(),
            "clj -M -e '(println \"git push origin main\")'".to_string(),
            format!("octave --quiet {show}/quiet.m"),
            format!("julia --startup-file=no {show}/quiet.jl"),
            format!("elixir {show}/quiet.exs"),
            format!("sqlite3 :memory: '.read {show}/quiet.sql'"),
            "osascript -l JavaScript -e '1'".to_string(),
        ];
        for command in &quiet {
            assert_eq!(kind(command), None, "{command}");
        }
        let previous = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&root).expect("cd");
        assert_eq!(kind("pnpm test"), Some("git.push"));
        assert_eq!(kind("pnpm run unused"), Some("git.push"));
        assert_eq!(kind("bun run test"), Some("git.push"));
        assert_eq!(kind("python3 -m mod"), Some("git.push"));
        assert_eq!(kind("python3 -m pkg"), Some("git.push"));
        assert_eq!(kind("python3 -m missingmod"), None);
        std::env::set_current_dir(previous).expect("back");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// These are the command strings a Windows host ran. The backslashes
    /// stay in the string. Replacing them with `/` before the call is a
    /// different spelling, and a missing `script.ps1` does not prove the
    /// reader is absent.
    #[test]
    fn windows_host_script_files_score_the_body() {
        let _drive = drive_letter_lock();
        struct HostFiles {
            root: std::path::PathBuf,
            previous: Option<std::path::PathBuf>,
        }
        impl Drop for HostFiles {
            fn drop(&mut self) {
                if let Some(prev) = self.previous.take() {
                    let _ = std::env::set_current_dir(prev);
                }
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }

        let pid = std::process::id();
        let body = "git push origin main\n";
        let names = ["payload.sh", "payload.ps1", "payload.bat", "payload.cmd"];
        let (files, dir_shown) = if cfg!(windows) {
            let dir = std::env::temp_dir().join(format!("snapif-host-{pid}"));
            std::fs::create_dir_all(&dir).expect("dir");
            for name in names {
                std::fs::write(dir.join(name), body).expect(name);
            }
            std::fs::write(dir.join("payload.mk"), "all:\n\tgit push origin main\n").expect("mk");
            let zdot = dir.join("zdot");
            std::fs::create_dir_all(&zdot).expect("zdot");
            std::fs::write(zdot.join(".zshenv"), body).expect("zshenv");
            let spaced = dir.join("Program Files");
            std::fs::create_dir_all(&spaced).expect("spaced");
            std::fs::write(spaced.join("payload.ps1"), body).expect("spaced ps1");
            let shown = dir.display().to_string().replace('/', "\\");
            (
                HostFiles {
                    root: dir,
                    previous: None,
                },
                shown,
            )
        } else {
            let root = std::env::temp_dir().join(format!("snapif-host-root-{pid}"));
            let rel = format!("C:/Users/sebta/AppData/Local/Temp/snapif-host-{pid}");
            let dir = root.join(&rel);
            std::fs::create_dir_all(&dir).expect("dir");
            for name in names {
                std::fs::write(dir.join(name), body).expect(name);
            }
            std::fs::write(dir.join("payload.mk"), "all:\n\tgit push origin main\n").expect("mk");
            let zdot = dir.join("zdot");
            std::fs::create_dir_all(&zdot).expect("zdot");
            std::fs::write(zdot.join(".zshenv"), body).expect("zshenv");
            let spaced = dir.join("Program Files");
            std::fs::create_dir_all(&spaced).expect("spaced");
            std::fs::write(spaced.join("payload.ps1"), body).expect("spaced ps1");
            let previous = std::env::current_dir().expect("cwd");
            std::env::set_current_dir(&root).expect("chdir");
            (
                HostFiles {
                    root,
                    previous: Some(previous),
                },
                rel.replace('/', "\\"),
            )
        };
        let path = |name: &str| format!("{dir_shown}\\{name}");
        let sh = path("payload.sh");
        let ps1 = path("payload.ps1");
        let bat = path("payload.bat");
        let cmd_file = path("payload.cmd");
        let mk = path("payload.mk");
        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let kind = |command: &str| -> &'static str {
            let id = matched_action(&policy, "Bash", Some(command))
                .cloned()
                .unwrap_or_else(|| ActionId::new("Bash"));
            let verdict =
                verdict_with_blocks(&policy, &id, 1.0, Some(HarmClass::Read), &[]).expect("gates");
            match verdict {
                Verdict::Auto(_) => "auto",
                Verdict::Review(_) => "review",
                Verdict::Escalate(_) => "escalate",
            }
        };
        let review = [
            format!("bash {sh}"),
            format!("make -f {mk}"),
            format!("make -f '{mk}' all"),
            format!("gmake --file {mk}"),
            format!("make -f{}", mk.replace('\\', "/")),
            format!("ZDOTDIR='{}' zsh -c 'echo hi'", path("zdot")),
            format!("BASH_ENV='{sh}' bash -c 'echo hi'"),
            format!("bash --rcfile '{sh}' -ic 'echo hi'"),
            format!("bash --init-file '{sh}' -ic 'echo hi'"),
            format!("bash < '{sh}'"),
            format!("bash -s < '{sh}'"),
            format!("sh < '{sh}'"),
            format!("bash -c 'source /dev/stdin' < '{sh}'"),
            format!("bash -c 'bash -s' < '{sh}'"),
            format!("bash -c 'sh -s' < '{sh}'"),
            format!("sh -c 'sh -s' < '{sh}'"),
            format!("zsh -c 'zsh -s' < '{sh}'"),
            format!("bash -c 'exec bash -s' < '{sh}'"),
            format!("bash -c bash < '{sh}'"),
            format!("command bash < '{sh}'"),
            format!("env bash < '{sh}'"),
            format!("nice bash < '{sh}'"),
            format!("nohup bash < '{sh}'"),
            format!("zsh < '{sh}'"),
            format!("dash < '{sh}'"),
            format!("ksh < '{sh}'"),
            format!("zsh -c 'source /dev/stdin' < '{sh}'"),
            format!("source {sh}"),
            format!("cat {sh}"),
            format!("bash '{sh}'"),
            format!("bash \"{sh}\""),
            format!("bash {}", sh.replace('\\', "/")),
            format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -File {ps1}"),
            format!("powershell -File {ps1}"),
            format!("pwsh -NoProfile -ExecutionPolicy Bypass -File {ps1}"),
            format!("pwsh -NoProfile -ExecutionPolicy Bypass {ps1}"),
            format!("pwsh -f {ps1} arg"),
            format!("powershell -File {ps1} -Command rm -rf /tmp/x"),
            format!("pwsh {ps1} -Command rm -rf /tmp/x"),
            format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& '{ps1}'\""),
            format!("powershell -Command \"Invoke-Expression (Get-Content -Raw '{ps1}')\""),
            format!("powershell -Command \"Invoke-Expression (Get-Content '{ps1}' -Raw)\""),
            format!("powershell -Command \"Get-Content -Raw '{ps1}' | Invoke-Expression\""),
            format!("powershell -Command \"Get-Content '{ps1}' -Raw | Invoke-Expression\""),
            format!("powershell {ps1}"),
            format!("powershell -Command {ps1}"),
            format!("powershell -Command '{ps1}'"),
            format!("powershell -Command \"{ps1}\""),
            format!("pwsh -NoProfile -Command {ps1}"),
            format!("pwsh -NoProfile -Command {ps1} arg"),
            format!(
                "powershell -Command '{}'",
                path("Program Files\\payload.ps1")
            ),
            format!(
                "powershell -Command \"{}\"",
                path("Program Files\\payload.ps1")
            ),
            format!("pwsh -NoProfile -Command - < '{ps1}'"),
            format!("powershell -Command - < '{ps1}'"),
            format!("cmd.exe /d /c {bat}"),
            format!("cmd /c {bat}"),
            format!("cmd.exe /d /c \"{bat}\""),
            format!("cmd.exe /d /c call {bat}"),
            format!("cmd /k {cmd_file}"),
            format!("cmd /c call {cmd_file} arg"),
        ];
        for command in &review {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("git.push"),
                "{command}"
            );
            assert_eq!(kind(command), "review", "{command}");
        }
        let quiet = [
            format!("bash {sh}.missing"),
            format!("make -f {mk}.missing"),
            format!("make -n -f {mk}"),
            format!("make -f {mk} -n"),
            format!("ZDOTDIR='{}' zsh -f -c 'echo hi'", path("zdot")),
            format!("bash < '{sh}.missing'"),
            format!("bash -s < '{sh}.missing'"),
            format!("source < '{sh}'"),
            format!("bash -c 'echo hi' < '{sh}'"),
            format!("bash --rcfile '{sh}' -c 'echo hi'"),
            format!("BASH_ENV='{sh}' bash -ic 'echo hi'"),
            format!("BASH_ENV='{sh}.missing' bash -c 'echo hi'"),
            format!("source {sh}.missing"),
            format!("cat {sh}.missing"),
            format!("powershell -File {ps1}.missing"),
            format!("pwsh {ps1}.missing"),
            format!("powershell -File {ps1}.missing -Command rm -rf /tmp/x"),
            format!("powershell -Command \"& '{ps1}.missing'\""),
            format!("powershell -Command \"Invoke-Expression (Get-Content -Raw '{ps1}.missing')\""),
            format!("powershell -Command \"Invoke-Expression (Get-Content '{ps1}')\""),
            format!("powershell -Command \"iex (Get-Content -Raw '{ps1}')\""),
            format!("cmd /c {bat}.missing"),
            format!("cmd /c call {bat}.missing"),
            format!("powershell -Command \"{ps1}.missing\""),
            format!("powershell -Command \"Get-Content -Raw '{ps1}'\""),
            format!("powershell -Command \"Get-Content -Raw '{ps1}.missing' | Invoke-Expression\""),
            format!("powershell -Command \"Invoke-Expression (Get-Content '{ps1}.missing' -Raw)\""),
            format!("pwsh -Command - < '{ps1}.missing'"),
            "pwsh rm -rf /tmp/x".to_string(),
        ];
        for command in &quiet {
            assert!(
                matched_action(&policy, "Bash", Some(command)).is_none(),
                "{command}"
            );
            assert_eq!(kind(command), "auto", "{command}");
        }
        for command in [
            "powershell -Command rm -rf /tmp/x",
            "cmd /c rm -rf /tmp/x",
            "cmd /c \"rm -rf /tmp/x\"",
            "cmd /c \"rm -rf /tmp/x.bat\"",
        ] {
            assert_eq!(
                matched_action(&policy, "Bash", Some(command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command}"
            );
        }
        assert_eq!(
            matched_action(&policy, "Bash", Some("cmd /c \"git push origin main.bat\""))
                .map(|id| id.0.as_str()),
            Some("git.push")
        );
        drop(files);
    }

    /// `cat`, `head`, and `tail` of a drive-letter path score the file
    /// text. A missing path stays unmatched. On Unix the `C:/...` string
    /// is a relative path under a temporary root. On Windows it is the
    /// real temp path with forward slashes.
    #[test]
    fn cat_of_a_drive_letter_path_scores_the_file() {
        let _drive = drive_letter_lock();
        struct CwdGuard(Option<std::path::PathBuf>);
        impl Drop for CwdGuard {
            fn drop(&mut self) {
                if let Some(prev) = self.0.take() {
                    let _ = std::env::set_current_dir(prev);
                }
            }
        }

        let policy = Policy::shipped("tool-gate").expect("tool-gate");
        let pid = std::process::id();
        let mut guard = CwdGuard(None);
        let (path, root) = if cfg!(windows) {
            let dir = std::env::temp_dir().join(format!("snapif-cat-{pid}"));
            std::fs::create_dir_all(&dir).expect("dir");
            let file = dir.join("payload.sh");
            std::fs::write(&file, "rm -rf /tmp/x\n").expect("payload");
            let mut shown = file.display().to_string().replace('\\', "/");
            if let Some(rest) = shown.strip_prefix("//?/") {
                shown = rest.to_string();
            }
            (shown, dir)
        } else {
            let root = std::env::temp_dir().join(format!("snapif-drive-root-{pid}"));
            let rel = "C:/Users/sebta/AppData/Local/Temp/snapif-cat-39492/payload.sh";
            let file = root.join(rel);
            std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
            std::fs::write(&file, "rm -rf /tmp/x\n").expect("payload");
            guard.0 = Some(std::env::current_dir().expect("cwd"));
            std::env::set_current_dir(&root).expect("chdir");
            (rel.to_string(), root)
        };
        assert!(
            path.as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphabetic())
                && path.as_bytes().get(1) == Some(&b':')
                && path.as_bytes().get(2) == Some(&b'/'),
            "{path}"
        );
        let coded = format!("{path}.b64");
        std::fs::write(&coded, "cm0gLXJmIC90bXAveAo=\n").expect("coded");
        let b32 = format!("{path}.b32");
        std::fs::write(&b32, "OJWSALLSMYQC65DNOAXXQCQ=\n").expect("b32");
        let hex = format!("{path}.hex");
        std::fs::write(&hex, "726d202d7266202f746d702f780a\n").expect("hex");
        let uu = format!("{path}.uu");
        std::fs::write(
            &uu,
            "begin 644 payload.sh\n.<FT@+7)F(\"]T;7 O> H \n`\nend\n",
        )
        .expect("uu");
        let zed = format!("{path}.Z");
        std::fs::write(
            &zed,
            [
                31, 157, 144, 114, 218, 128, 104, 33, 199, 12, 136, 23, 116, 218, 192, 121, 129,
                71, 1,
            ],
        )
        .expect("Z");
        let lzma_path = format!("{path}.lzma");
        {
            let mut input = std::io::Cursor::new(b"rm -rf /tmp/x\n".as_slice());
            let mut out = Vec::new();
            lzma_rs::lzma_compress(&mut input, &mut out).expect("lzma");
            std::fs::write(&lzma_path, out).expect("lzma write");
        }
        let gz = format!("{path}.gz");
        std::fs::write(
            &gz,
            [
                31, 139, 8, 0, 0, 0, 0, 0, 0, 255, 43, 202, 85, 208, 45, 74, 83, 208, 47, 201, 45,
                208, 175, 224, 2, 0, 5, 108, 22, 117, 14, 0, 0, 0,
            ],
        )
        .expect("gz");
        let bz = format!("{path}.bz2");
        std::fs::write(
            &bz,
            [
                66, 90, 104, 57, 49, 65, 89, 38, 83, 89, 99, 126, 202, 34, 0, 0, 5, 209, 128, 0,
                16, 64, 2, 129, 2, 84, 64, 32, 0, 33, 160, 52, 200, 64, 12, 25, 56, 57, 7, 169, 49,
                226, 238, 72, 167, 10, 18, 12, 111, 217, 68, 64,
            ],
        )
        .expect("bz");
        let xz = format!("{path}.xz");
        std::fs::write(
            &xz,
            [
                253, 55, 122, 88, 90, 0, 0, 4, 230, 214, 180, 70, 2, 0, 33, 1, 22, 0, 0, 0, 116,
                47, 229, 163, 1, 0, 13, 114, 109, 32, 45, 114, 102, 32, 47, 116, 109, 112, 47, 120,
                10, 0, 0, 0, 109, 2, 0, 252, 142, 104, 100, 55, 0, 1, 38, 14, 8, 27, 224, 4, 31,
                182, 243, 125, 1, 0, 0, 0, 0, 4, 89, 90,
            ],
        )
        .expect("xz");
        let zst = format!("{path}.zst");
        std::fs::write(
            &zst,
            [
                40, 181, 47, 253, 36, 14, 113, 0, 0, 114, 109, 32, 45, 114, 102, 32, 47, 116, 109,
                112, 47, 120, 10, 123, 150, 56, 186,
            ],
        )
        .expect("zst");
        let lz = format!("{path}.lz4");
        std::fs::write(
            &lz,
            [
                4, 34, 77, 24, 100, 64, 167, 14, 0, 0, 128, 114, 109, 32, 45, 114, 102, 32, 47,
                116, 109, 112, 47, 120, 10, 0, 0, 0, 0, 80, 220, 136, 211,
            ],
        )
        .expect("lz4");
        let br = format!("{path}.br");
        std::fs::write(
            &br,
            [
                33, 52, 0, 4, 114, 109, 32, 45, 114, 102, 32, 47, 116, 109, 112, 47, 120, 10, 3,
            ],
        )
        .expect("br");
        let tar_path = format!("{path}.tar");
        {
            let text = "rm -rf /tmp/x\n";
            let mut header = tar::Header::new_gnu();
            header.set_path("payload.sh").expect("tar path");
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            let mut builder = tar::Builder::new(Vec::new());
            builder
                .append(&header, text.as_bytes())
                .expect("tar append");
            std::fs::write(&tar_path, builder.into_inner().expect("tar finish")).expect("tar");
        }
        let zip_path = format!("{path}.zip");
        {
            use std::io::Write;
            let text = "rm -rf /tmp/x\n";
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut writer = zip::ZipWriter::new(&mut cursor);
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                writer.start_file("payload.sh", options).expect("zip start");
                writer.write_all(text.as_bytes()).expect("zip write");
                writer.finish().expect("zip finish");
            }
            std::fs::write(&zip_path, cursor.into_inner()).expect("zip");
        }
        let seven_src = format!("{path}-7zsrc");
        let seven = format!("{path}.7z");
        std::fs::create_dir_all(&seven_src).expect("7z dir");
        std::fs::write(format!("{seven_src}/payload.sh"), "rm -rf /tmp/x\n").expect("7z src");
        let seven_cursor =
            sevenz_rust2::compress(&seven_src, std::io::Cursor::new(Vec::new())).expect("7z");
        std::fs::write(&seven, seven_cursor.into_inner()).expect("7z write");
        std::fs::remove_dir_all(&seven_src).ok();
        let missing = format!("{path}-missing");
        for command in [
            format!("bash '{path}'"),
            format!("source '{path}'"),
            format!("cat '{path}'"),
            format!("cat {path}"),
            format!("cat < {path}"),
            format!("cat < '{path}'"),
            format!("cat < \"{path}\""),
            format!("head < \"{path}\""),
            format!("tail < \"{path}\""),
            format!("grep . {path}"),
            format!("sed -n p {path}"),
            format!("awk '{{print}}' {path}"),
            format!("cut -c 1- {path}"),
            format!("sort {path}"),
            format!("uniq {path}"),
            format!("tac {path}"),
            format!("fold {path}"),
            format!("fold -w 80 {path}"),
            format!("fold < {path}"),
            format!("gfold {path}"),
            format!("fmt {path}"),
            format!("fmt -w 80 {path}"),
            format!("paste {path}"),
            format!("paste -s {path}"),
            format!("expand {path}"),
            format!("unexpand {path}"),
            format!("column {path}"),
            format!("pr -t {path}"),
            format!("strings -n 1 {path}"),
            format!("dd status=none if={path}"),
            format!("gdd if={path}"),
            format!("iconv -f utf-8 -t utf-8 {path}"),
            format!("iconv -f UTF-8 -t utf8 {path}"),
            format!("perl -pe '' {path}"),
            format!("ruby -pe '' {path}"),
            format!("perl -ne 'print' {path}"),
            format!("tee out < {path}"),
            format!("tee -a out < {path}"),
            format!("gtee out < {path}"),
            format!("join {path} {path}"),
            format!("gjoin {path} {path}"),
            format!("comm -12 {path} {path}"),
            format!("gcomm -12 {path} {path}"),
            format!("jq -Rr . {path}"),
            format!("jq -R -r . {path}"),
            format!("python3 -c 'import sys; print(open(sys.argv[1]).read())' {path}"),
            format!("python -c 'import sys; print(open(sys.argv[1]).read())' {path}"),
            format!(
                "node -e \"fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1]))\" {path}"
            ),
            format!("php -r 'print file_get_contents($argv[1]);' {path}"),
            format!("split --filter=cat -l 1 {path}"),
            format!("gsplit --filter=cat -l 1 {path}"),
            format!("tr a-z A-Z < {path}"),
            format!("gtr a-z A-Z < {path}"),
            format!("base64 -d {coded}"),
            format!("openssl base64 -d -A -in {coded}"),
            format!("xxd -r -p {hex}"),
            format!("xxd -p -r {hex}"),
            format!("xxd -r -ps {hex}"),
            format!("xxd -r -p < {hex}"),
            format!("uudecode -p {uu}"),
            format!("uncompress -c {zed}"),
            format!("unlzma -c {lzma_path}"),
            format!("lzcat {lzma_path}"),
            format!("lzma -dc {lzma_path}"),
            format!("openssl base64 -d -in {coded}"),
            format!("openssl base64 -d -A < {coded}"),
            format!("gbase64 -d {coded}"),
            format!("base32 -d {b32}"),
            format!("base32 -d -i {b32}"),
            format!("gbase32 -d {b32}"),
            format!("basenc --base64 -d {coded}"),
            format!("gbasenc --base64 -d {coded}"),
            format!("gzip -dc {gz}"),
            format!("bzip2 -dc {bz}"),
            format!("bunzip2 -c {bz}"),
            format!("bzcat {bz}"),
            format!("xz -dc {xz}"),
            format!("xz -d -c {xz}"),
            format!("xzcat {xz}"),
            format!("unxz -c {xz}"),
            format!("zstd -dc {zst}"),
            format!("zstd -d -c {zst}"),
            format!("zstdcat {zst}"),
            format!("unzstd -c {zst}"),
            format!("pzstd -dc {zst}"),
            format!("lz4 -dc {lz}"),
            format!("lz4 -d -c {lz}"),
            format!("lz4cat {lz}"),
            format!("unlz4 -c {lz}"),
            format!("brotli -dc {br}"),
            format!("brotli -d -c {br}"),
            format!("brotli --decompress --stdout {br}"),
            format!("tar -xOf {tar_path} payload.sh"),
            format!("tar -xOf {tar_path}"),
            format!("gtar -xOf {tar_path} payload.sh"),
            format!("bsdtar -xOf {tar_path} payload.sh"),
            format!("unzip -p {zip_path} payload.sh"),
            format!("unzip -p {zip_path}"),
            format!("7z e -so {seven}"),
            format!("7z x -so {seven}"),
            format!("7za e -so {seven}"),
            format!("gunzip -c {gz}"),
            format!("gzcat {gz}"),
            format!("zcat {gz}"),
            format!("gcat < {path}"),
            format!("head '{path}'"),
            format!("head {path}"),
            format!("head < {path}"),
            format!("ghead < {path}"),
            format!("head -n 1 < {path}"),
            format!("head - < {path}"),
            format!("head -n 1 - < {path}"),
            format!("tail - < {path}"),
            format!("tail '{path}'"),
            format!("tail {path}"),
            format!("tail < {path}"),
            format!("gtail < {path}"),
            format!("cat {path} < {missing}"),
        ] {
            assert_eq!(
                matched_action(&policy, "Bash", Some(&command)).map(|id| id.0.as_str()),
                Some("bash.rm"),
                "{command:?}"
            );
        }
        for command in [
            format!("cat '{missing}'"),
            format!("cat {missing}"),
            format!("cat < {missing}"),
            format!("cat < \"{path}$(echo)\""),
            format!("grep zzzz {path}"),
            format!("grep . {missing}"),
            format!("fold {missing}"),
            format!("dd status=none if={missing}"),
            format!("iconv -f utf-8 -t utf-8 {missing}"),
            format!("iconv -f utf-8 -t utf-16 {path}"),
            format!("perl -pe '' {missing}"),
            format!("tee out < {missing}"),
            format!("tee --output-error=exit < {path}"),
            format!("join {path} {missing}"),
            format!("comm -12 {path} {missing}"),
            format!("comm -3 {path} {path}"),
            format!("jq . {path}"),
            format!("jq -Rr . {missing}"),
            format!("python3 -c 'print(1)' {path}"),
            format!("python3 -c 'import sys; print(open(sys.argv[1]).read())' {missing}"),
            format!("node -e 'console.log(1)' {path}"),
            format!(
                "node -e \"fs=require('fs'); process.stdout.write(fs.readFileSync(process.argv[1]))\" {missing}"
            ),
            format!("php -r 'echo 1;' {path}"),
            format!("php -r 'print file_get_contents($argv[1]);' {missing}"),
            format!("split --filter=cat -l 1 {missing}"),
            format!("tr < {path}"),
            format!("tr a-z A-Z < {missing}"),
            format!("base64 -d {path}"),
            format!("base64 {coded}"),
            format!("base32 -d {path}"),
            format!("base32 {b32}"),
            format!("openssl base64 -A -in {coded}"),
            format!("openssl base64 -d -A -in {path}"),
            format!("xxd -r -p {path}"),
            format!("xxd {hex}"),
            format!("uudecode {uu}"),
            format!("uudecode -p {path}"),
            format!("uncompress {zed}"),
            format!("uncompress -c {path}"),
            format!("lzma {lzma_path}"),
            format!("basenc --base64 -d {path}"),
            format!("gzip -dc {path}"),
            format!("bzip2 -dc {path}"),
            format!("xz -dc {path}"),
            format!("zstd -dc {path}"),
            format!("lz4 -dc {path}"),
            format!("brotli -dc {path}"),
            format!("tar -xOf {tar_path} missing.sh"),
            format!("tar -xOf {path} payload.sh"),
            format!("unzip -p {zip_path} missing.sh"),
            format!("unzip {zip_path} payload.sh"),
            format!("7z e {seven}"),
            format!("head {missing}"),
            format!("head < {missing}"),
            "head -".to_string(),
            "tail -".to_string(),
            format!("tail {missing}"),
            format!("tail < {missing}"),
            format!("bash {missing}"),
            format!("source {missing}"),
        ] {
            assert!(
                matched_action(&policy, "Bash", Some(&command)).is_none(),
                "{command:?}"
            );
        }
        drop(guard);
        let _ = std::fs::remove_dir_all(root);
    }

    fn plain_script_path(path: &std::path::Path) -> String {
        let text = path.display().to_string();
        assert!(
            text.chars().all(|ch| {
                ch.is_ascii_graphic()
                    && !matches!(
                        ch,
                        '\'' | '"'
                            | '\\'
                            | '$'
                            | '`'
                            | '*'
                            | '?'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '~'
                            | ';'
                            | '&'
                            | '|'
                            | '<'
                            | '>'
                            | '('
                            | ')'
                            | '#'
                            | ' '
                    )
            }),
            "{text}"
        );
        text
    }
}
