//! The rules (ADR-0016): the targets of a call, and the rule that decides
//! each one.
//!
//! A call has one target per path it names (or the runtime's working
//! directory, when it names none) and, for tools that run commands, one per
//! simple command. Every combination is judged by the rule list — the
//! first rule that matches decides it — and the most restrictive decision
//! wins.

use orchestrator_core::{Decision, PolicyRule, RuleAccess, RuleWhere};
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Rules the user may write.
pub const MAX_RULES: usize = 100;
/// Characters of a command or path pattern.
pub const MAX_PATTERN: usize = 200;
/// Characters of a rule's note.
pub const MAX_NOTE: usize = 200;
/// Paths of one call that are judged; more than this is judged as the
/// first ones plus "outside" (a call does not escape by naming many).
const MAX_TARGETS: usize = 64;

/// Tools judged by command patterns, and the argument with the command.
const COMMANDS: [(&str, &str); 3] = [
    ("shell.execute", "command"),
    ("process.start", "command"),
    ("terminal.write", "data"),
];
/// Arguments that name a path, in any tool.
const PATH_KEYS: [&str; 4] = ["path", "from", "to", "cwd"];
/// Arguments that name several paths.
const PATH_LISTS: [&str; 1] = ["roots"];

/// The fixed rules of the Assisted mode.
pub fn assisted_rules() -> Vec<PolicyRule> {
    vec![
        PolicyRule::tools(&["filesystem.*"], Decision::Ask)
            .with_path(".env*")
            .with_note("arquivos de ambiente costumam ter segredos"),
        PolicyRule::tools(&[], Decision::Ask)
            .with_location(RuleWhere::Outside)
            .with_note("o que está fora da pasta do projeto"),
        PolicyRule::tools(&[], Decision::Allow)
            .with_access(RuleAccess::Read)
            .with_note("consultas não mudam nada"),
        PolicyRule::tools(&["agent.finish"], Decision::Allow)
            .with_note("só entrega o resultado para a sua revisão"),
        PolicyRule::tools(&[], Decision::Ask).with_note("toda ação pede autorização"),
    ]
}

/// The rules a new installation starts with in the Autonomous mode:
/// visible, editable and restorable.
pub fn default_rules() -> Vec<PolicyRule> {
    vec![
        PolicyRule::tools(&["filesystem.*"], Decision::Ask)
            .with_path(".env*")
            .with_note("arquivos de ambiente costumam ter segredos"),
        PolicyRule::tools(&[], Decision::Ask)
            .with_location(RuleWhere::Outside)
            .with_note("o que está fora da pasta do projeto"),
        PolicyRule::tools(&[], Decision::Allow)
            .with_access(RuleAccess::Read)
            .with_note("consultas não mudam nada"),
        PolicyRule::tools(&["filesystem.delete"], Decision::Ask).with_note("apagar não tem volta"),
        PolicyRule::tools(&["git.push", "git.reset"], Decision::Ask)
            .with_note("mexe no remoto ou descarta trabalho"),
        PolicyRule::tools(&["package.install"], Decision::Ask)
            .with_note("muda as dependências do projeto"),
        PolicyRule::tools(&[], Decision::Ask)
            .with_command("rm *")
            .with_note("apaga arquivos"),
        PolicyRule::tools(&[], Decision::Ask)
            .with_command("sudo *")
            .with_note("age como administrador"),
        PolicyRule::tools(&[], Decision::Ask)
            .with_command("git push*")
            .with_note("envia para o remoto"),
        PolicyRule::tools(&["terminal.write"], Decision::Ask)
            .with_note("o que se digita num terminal não se analisa inteiro"),
        PolicyRule::tools(&["github.*"], Decision::Ask)
            .with_access(RuleAccess::Write)
            .with_note("publica no GitHub em nome da sua conta"),
        PolicyRule::tools(&[], Decision::Allow).with_note("o resto o agente faz sozinho"),
    ]
}

/// Where a call lands.
#[derive(Debug, Clone)]
pub struct Scope {
    /// Root of the project of the session; `None`: no project, so every
    /// target is outside.
    pub project_root: Option<PathBuf>,
    /// Working directory of the runtime: relative paths, and calls that
    /// name no path, land here.
    pub workdir: PathBuf,
}

/// A path of a call, resolved as the runtime resolves it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetPath {
    /// Relative to the project (`/`-separated, `.` for the root) when
    /// inside; the absolute path otherwise.
    pub shown: String,
    pub inside: bool,
    /// Named by the call; `false`: the runtime's working directory.
    pub explicit: bool,
    #[serde(skip)]
    file_name: String,
    /// What path patterns are matched against (without the `.`).
    #[serde(skip)]
    matched: String,
}

/// One combination of path and simple command, and what decided it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unit {
    pub path: TargetPath,
    pub command: Option<String>,
    pub decision: Decision,
    /// Index of the rule that decided; `None`: no rule matched (ask).
    pub rule: Option<usize>,
}

/// The decision about a call.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub decision: Decision,
    /// Rule behind the decision (the first target that got it).
    pub rule: Option<usize>,
    pub units: Vec<Unit>,
    /// The command has `$(…)` or backticks, so command patterns did not
    /// apply to it.
    pub opaque: bool,
}

impl Verdict {
    /// Targets whose decision is to ask.
    pub fn asking(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| u.decision == Decision::Ask)
    }

    /// Whether any target is outside the project.
    pub fn outside(&self) -> bool {
        self.units.iter().any(|u| !u.path.inside)
    }
}

/// Judges a call with a rule list.
pub fn evaluate(
    rules: &[PolicyRule],
    tool: &str,
    read_only: bool,
    args: &Value,
    scope: &Scope,
) -> Verdict {
    let paths = targets(args, scope);
    let (parts, opaque) = command_parts(tool, args);
    let mut units = Vec::new();
    for path in &paths {
        match &parts {
            None => units.push(decide(rules, tool, read_only, path, None, opaque)),
            Some(parts) => {
                for part in parts {
                    units.push(decide(rules, tool, read_only, path, Some(part), opaque));
                }
            }
        }
    }
    let decision = units
        .iter()
        .map(|u| u.decision)
        .fold(Decision::Allow, Decision::strictest);
    let rule = units
        .iter()
        .find(|u| u.decision == decision)
        .and_then(|u| u.rule);
    Verdict {
        decision,
        rule,
        units,
        opaque,
    }
}

fn decide(
    rules: &[PolicyRule],
    tool: &str,
    read_only: bool,
    path: &TargetPath,
    command: Option<&String>,
    opaque: bool,
) -> Unit {
    let found = rules
        .iter()
        .enumerate()
        .find(|(_, rule)| matches(rule, tool, read_only, path, command, opaque));
    Unit {
        path: path.clone(),
        command: command.cloned(),
        decision: found.map_or(Decision::Ask, |(_, rule)| rule.decision),
        rule: found.map(|(index, _)| index),
    }
}

fn matches(
    rule: &PolicyRule,
    tool: &str,
    read_only: bool,
    path: &TargetPath,
    command: Option<&String>,
    opaque: bool,
) -> bool {
    if !rule.tools.is_empty() && !rule.tools.iter().any(|p| tool_matches(p, tool)) {
        return false;
    }
    match rule.access {
        Some(RuleAccess::Read) if !read_only => return false,
        Some(RuleAccess::Write) if read_only => return false,
        _ => {}
    }
    match rule.location {
        Some(RuleWhere::Inside) if !path.inside => return false,
        Some(RuleWhere::Outside) if path.inside => return false,
        _ => {}
    }
    if let Some(pattern) = &rule.command {
        match command {
            Some(command) if !opaque => {
                if !glob(&one_line(pattern), command, false, false) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    if let Some(pattern) = &rule.path {
        if !path.explicit || !path_matches(pattern, path) {
            return false;
        }
    }
    true
}

/// `*`, `grupo.*` or the exact name.
pub fn tool_matches(pattern: &str, tool: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" {
        return true;
    }
    match pattern.strip_suffix(".*") {
        Some(group) => tool
            .strip_prefix(group)
            .is_some_and(|rest| rest.starts_with('.')),
        None => pattern == tool,
    }
}

fn path_matches(pattern: &str, path: &TargetPath) -> bool {
    let pattern = pattern.trim();
    let fold = cfg!(any(windows, target_os = "macos"));
    if !pattern.contains('/') {
        return glob(pattern, &path.file_name, true, fold);
    }
    if path.inside {
        glob(pattern.trim_start_matches('/'), &path.matched, true, fold)
    } else {
        let expanded = match pattern.strip_prefix("~/") {
            Some(rest) => home_dir()
                .map(|home| format!("{}/{rest}", shown(&home)))
                .unwrap_or_else(|| pattern.to_owned()),
            None => pattern.to_owned(),
        };
        glob(&expanded, &path.matched, true, fold)
    }
}

// ---- commands -------------------------------------------------------

/// The simple commands of a call (`None` for tools without a command),
/// and whether it is opaque (`$(…)`, backticks, `<(…)`).
fn command_parts(tool: &str, args: &Value) -> (Option<Vec<String>>, bool) {
    let Some((_, key)) = COMMANDS.iter().find(|(name, _)| *name == tool) else {
        return (None, false);
    };
    let mut text = args
        .get(*key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // What a shell reads from stdin runs like the command itself.
    if tool == "shell.execute" {
        if let Some(stdin) = args.get("stdin").and_then(Value::as_str) {
            text.push('\n');
            text.push_str(stdin);
        }
    }
    let opaque = ["$(", "`", "<(", ">("]
        .iter()
        .any(|marker| text.contains(marker));
    let parts = split_command(&text);
    (if parts.is_empty() { None } else { Some(parts) }, opaque)
}

/// Splits on `;`, `&&`, `||`, `|`, `&` and line breaks, keeping
/// redirections such as `2>&1` and `&>` whole. Quotes are not parsed: a
/// separator inside quotes makes an extra part, which can only make the
/// decision stricter.
pub fn split_command(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let separator = match c {
            ';' | '\n' | '\r' | '|' => true,
            '&' => {
                let before = i.checked_sub(1).map(|j| chars[j]);
                let after = chars.get(i + 1).copied();
                !(matches!(before, Some('>') | Some('<')) || after == Some('>'))
            }
            _ => false,
        };
        if separator {
            push_part(&mut parts, &current);
            current.clear();
        } else {
            current.push(c);
        }
    }
    push_part(&mut parts, &current);
    parts
}

fn push_part(parts: &mut Vec<String>, text: &str) {
    let part = one_line(text);
    if !part.is_empty() {
        parts.push(part);
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- paths ----------------------------------------------------------

/// The paths a call names, resolved; the working directory when it names
/// none.
pub fn targets(args: &Value, scope: &Scope) -> Vec<TargetPath> {
    let mut given: Vec<String> = PATH_KEYS
        .iter()
        .filter_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    for key in PATH_LISTS {
        if let Some(list) = args.get(key).and_then(Value::as_array) {
            given.extend(list.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    given.retain(|path| !path.trim().is_empty());
    if given.is_empty() {
        return vec![target(scope, None)];
    }
    let many = given.len() > MAX_TARGETS;
    let mut targets: Vec<TargetPath> = given
        .iter()
        .take(MAX_TARGETS)
        .map(|path| target(scope, Some(path)))
        .collect();
    if many {
        // What was not looked at is treated as outside.
        targets.push(TargetPath {
            shown: format!("(+{} caminhos)", given.len() - MAX_TARGETS),
            inside: false,
            explicit: true,
            file_name: String::new(),
            matched: String::new(),
        });
    }
    targets
}

fn target(scope: &Scope, given: Option<&String>) -> TargetPath {
    let absolute = match given {
        Some(path) => resolve(&scope.workdir, path),
        None => scope.workdir.clone(),
    };
    let real = real_path(&absolute);
    let relative = scope
        .project_root
        .as_ref()
        .and_then(|root| relative_to(&real, &real_path(root)));
    let file_name = real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match relative {
        Some(relative) => TargetPath {
            shown: if relative.is_empty() {
                ".".to_owned()
            } else {
                relative.clone()
            },
            inside: true,
            explicit: given.is_some(),
            file_name,
            matched: relative,
        },
        None => {
            let absolute = shown(&real);
            TargetPath {
                shown: absolute.clone(),
                inside: false,
                explicit: given.is_some(),
                file_name,
                matched: absolute,
            }
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// As the runtime does: `~` is the home, relative paths are joined to the
/// working directory.
fn resolve(workdir: &Path, input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workdir.join(path)
    }
}

/// The path with what exists of it resolved on disk (links included), and
/// the rest applied by hand.
pub fn real_path(path: &Path) -> PathBuf {
    let mut components: Vec<Component> = path.components().collect();
    let mut rest: Vec<OsString> = Vec::new();
    while !components.is_empty() {
        let candidate: PathBuf = components.iter().collect();
        if let Ok(real) = std::fs::canonicalize(&candidate) {
            let mut out = real;
            for part in rest.iter().rev() {
                if part == ".." {
                    out.pop();
                } else if part != "." {
                    out.push(part);
                }
            }
            return out;
        }
        if let Some(last) = components.pop() {
            rest.push(last.as_os_str().to_owned());
        }
    }
    lexical(path)
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` relative to `root` (`/`-separated, empty for the root itself),
/// when inside it.
fn relative_to(path: &Path, root: &Path) -> Option<String> {
    let fold = cfg!(any(windows, target_os = "macos"));
    let same = |a: &std::ffi::OsStr, b: &std::ffi::OsStr| {
        if fold {
            a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
        } else {
            a == b
        }
    };
    let mut components = path.components();
    for expected in root.components() {
        let found = components.next()?;
        if !same(found.as_os_str(), expected.as_os_str()) {
            return None;
        }
    }
    Some(
        components
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// An absolute path as people read it: `/`-separated, without Windows'
/// `\\?\` prefix.
fn shown(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| text.into_owned());
    text.replace('\\', "/")
}

// ---- patterns -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Literal(char),
    /// `?`
    One,
    /// `*`
    Star,
    /// `**` (paths): anything, across folders.
    Any,
    /// `**/` (paths): nothing, or whole folders.
    Folders,
}

fn tokens(pattern: &str, paths: bool) -> Vec<Token> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if paths && chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    out.push(Token::Folders);
                    i += 3;
                } else {
                    out.push(Token::Any);
                    i += 2;
                }
                continue;
            }
            '*' => out.push(Token::Star),
            '?' => out.push(Token::One),
            c => out.push(Token::Literal(c)),
        }
        i += 1;
    }
    out
}

/// Whole-text glob. In commands `*` matches anything; in paths it stays
/// inside a folder, and `**` crosses folders.
pub fn glob(pattern: &str, text: &str, paths: bool, fold: bool) -> bool {
    let (pattern, text) = if fold {
        (pattern.to_lowercase(), text.to_lowercase())
    } else {
        (pattern.to_owned(), text.to_owned())
    };
    let tokens = tokens(&pattern, paths);
    let text: Vec<char> = text.chars().collect();
    let n = text.len();
    // next[j]: tokens[i + 1..] match text[j..].
    let mut next = vec![false; n + 1];
    next[n] = true;
    for token in tokens.iter().rev() {
        let mut row = vec![false; n + 1];
        match *token {
            Token::Folders => {
                // Nothing, or any text ending in `/`.
                let mut after_slash = false;
                for j in (0..=n).rev() {
                    if j < n && text[j] == '/' && next[j + 1] {
                        after_slash = true;
                    }
                    row[j] = next[j] || after_slash;
                }
            }
            _ => {
                for j in (0..=n).rev() {
                    row[j] = match *token {
                        Token::Literal(c) => j < n && text[j] == c && next[j + 1],
                        Token::One => j < n && (!paths || text[j] != '/') && next[j + 1],
                        Token::Star => {
                            next[j] || (j < n && (!paths || text[j] != '/') && row[j + 1])
                        }
                        Token::Any => next[j] || (j < n && row[j + 1]),
                        Token::Folders => unreachable!(),
                    };
                }
            }
        }
        next = row;
    }
    next[0]
}

// ---- validation -----------------------------------------------------

fn valid_tool_pattern(pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let name = pattern.strip_suffix(".*").unwrap_or(pattern);
    !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
}

/// Checks the user's rules and returns them tidied (trimmed, empty fields
/// removed).
pub fn validate(rules: &[PolicyRule]) -> Result<Vec<PolicyRule>, String> {
    if rules.len() > MAX_RULES {
        return Err(format!("no máximo {MAX_RULES} regras"));
    }
    let tidy = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let mut out = Vec::with_capacity(rules.len());
    for (index, rule) in rules.iter().enumerate() {
        let number = index + 1;
        let tools: Vec<String> = rule
            .tools
            .iter()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        if let Some(bad) = tools.iter().find(|t| !valid_tool_pattern(t)) {
            return Err(format!(
                "regra {number}: \"{bad}\" não é um nome de ferramenta (use git.push, git.* ou *)"
            ));
        }
        let command = tidy(&rule.command);
        let path = tidy(&rule.path);
        let note = tidy(&rule.note);
        for (label, value, max) in [
            ("o padrão de comando", &command, MAX_PATTERN),
            ("o padrão de caminho", &path, MAX_PATTERN),
            ("a nota", &note, MAX_NOTE),
        ] {
            if value.as_ref().is_some_and(|v| v.chars().count() > max) {
                return Err(format!(
                    "regra {number}: {label} tem mais de {max} caracteres"
                ));
            }
        }
        out.push(PolicyRule {
            tools,
            access: rule.access,
            location: rule.location,
            command,
            path,
            decision: rule.decision,
            note,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope(root: &Path) -> Scope {
        Scope {
            project_root: Some(root.to_path_buf()),
            workdir: root.to_path_buf(),
        }
    }

    #[test]
    fn globs_follow_commands_and_gitignore() {
        assert!(glob("rm *", "rm -rf build/out", false, false));
        assert!(!glob("rm *", "npm rm x", false, false));
        assert!(glob("npm test*", "npm test", false, false));
        assert!(glob("git push*", "git push origin main", false, false));
        assert!(glob("?s", "ls", false, false));

        assert!(glob(".env*", ".env.local", true, false));
        assert!(!glob(".env*", "environment.ts", true, false));
        assert!(glob("src/*.ts", "src/app.ts", true, false));
        assert!(!glob("src/*.ts", "src/lib/app.ts", true, false));
        assert!(glob("src/**", "src/lib/app.ts", true, false));
        assert!(glob("**/secret.txt", "secret.txt", true, false));
        assert!(glob("**/secret.txt", "a/b/secret.txt", true, false));
        assert!(!glob("**/secret.txt", "a/bsecret.txt", true, false));
        assert!(glob("docs/**/*.md", "docs/a/b/c.md", true, false));
        assert!(glob("docs/**/*.md", "docs/c.md", true, false));
        assert!(glob("SRC/*.TS", "src/app.ts", true, true));
    }

    #[test]
    fn commands_are_split_into_simple_commands() {
        assert_eq!(
            split_command("npm test && rm -rf build; echo ok | tee log"),
            ["npm test", "rm -rf build", "echo ok", "tee log"]
        );
        assert_eq!(split_command("cargo test 2>&1"), ["cargo test 2>&1"]);
        assert_eq!(split_command("make &> out.txt"), ["make &> out.txt"]);
        assert_eq!(split_command("sleep 5 &\necho  a"), ["sleep 5", "echo a"]);
        assert_eq!(split_command("ls\r"), ["ls"]);
        assert!(split_command("  ;; ").is_empty());
    }

    #[test]
    fn the_first_matching_rule_decides_and_the_strictest_target_wins() {
        let dir = tempfile::tempdir().unwrap();
        let scope = scope(dir.path());
        let rules = vec![
            PolicyRule::tools(&[], Decision::Ask).with_command("rm *"),
            PolicyRule::tools(&[], Decision::Allow).with_command("npm test*"),
            PolicyRule::tools(&["shell.execute"], Decision::Deny),
        ];
        let run = |command: &str| {
            evaluate(
                &rules,
                "shell.execute",
                false,
                &json!({ "command": command }),
                &scope,
            )
        };
        let plain = run("npm test");
        assert_eq!((plain.decision, plain.rule), (Decision::Allow, Some(1)));

        // A chained command does not ride on the allowed part.
        let chained = run("npm test && rm -rf build");
        assert_eq!((chained.decision, chained.rule), (Decision::Ask, Some(0)));
        assert_eq!(chained.units.len(), 2);

        let other = run("npm test; curl example.com");
        assert_eq!((other.decision, other.rule), (Decision::Deny, Some(2)));

        // Command patterns do not vouch for what they cannot read.
        let opaque = run("npm test $(cat list)");
        assert!(opaque.opaque);
        assert_eq!((opaque.decision, opaque.rule), (Decision::Deny, Some(2)));

        // What a shell reads from stdin counts.
        let stdin = evaluate(
            &rules,
            "shell.execute",
            false,
            &json!({ "command": "npm test", "stdin": "rm -rf ~" }),
            &scope,
        );
        assert_eq!(stdin.decision, Decision::Ask);

        // No rule matched: ask.
        let none = evaluate(&rules, "git.push", false, &json!({}), &scope);
        assert_eq!((none.decision, none.rule), (Decision::Ask, None));
    }

    #[test]
    fn paths_are_judged_where_the_runtime_would_put_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let scope = scope(&root);
        let judge = |args: Value| targets(&args, &scope);

        let inside = judge(json!({"path": "src/new/app.ts"}));
        assert_eq!(inside[0].shown, "src/new/app.ts");
        assert!(inside[0].inside && inside[0].explicit);

        let escape = judge(json!({"path": "src/../../elsewhere.txt"}));
        assert!(!escape[0].inside, "{escape:?}");

        let absolute = judge(json!({"path": dir.path().join("x").to_string_lossy()}));
        assert!(!absolute[0].inside);

        let moved = judge(json!({"from": "src/a.ts", "to": "../a.ts"}));
        assert_eq!(
            moved.iter().map(|t| t.inside).collect::<Vec<_>>(),
            [true, false]
        );

        let none = judge(json!({"command": "ls"}));
        assert_eq!(none[0].shown, ".");
        assert!(none[0].inside && !none[0].explicit);

        let no_project = targets(
            &json!({"path": "src/app.ts"}),
            &Scope {
                project_root: None,
                workdir: root.clone(),
            },
        );
        assert!(!no_project[0].inside);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_project_that_points_outside_counts_as_outside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let found = targets(&json!({"path": "link/new.txt"}), &scope(&root));
        assert!(!found[0].inside, "{found:?}");
        assert!(found[0].shown.ends_with("outside/new.txt"));
    }

    #[test]
    fn assisted_asks_for_actions_and_for_what_is_outside_or_secret() {
        let dir = tempfile::tempdir().unwrap();
        let scope = scope(dir.path());
        let rules = assisted_rules();
        let decide = |tool: &str, read_only: bool, args: Value| {
            evaluate(&rules, tool, read_only, &args, &scope).decision
        };
        assert_eq!(
            decide("filesystem.read", true, json!({"path": "src/a.ts"})),
            Decision::Allow
        );
        assert_eq!(
            decide("filesystem.read", true, json!({"path": ".env.local"})),
            Decision::Ask
        );
        assert_eq!(
            decide("filesystem.read", true, json!({"path": "/etc/hosts"})),
            Decision::Ask
        );
        assert_eq!(
            decide("filesystem.write", false, json!({"path": "a.ts"})),
            Decision::Ask
        );
        assert_eq!(
            decide("agent.finish", false, json!({"result": "ok"})),
            Decision::Allow
        );
        assert_eq!(decide("git.status", true, json!({})), Decision::Allow);
    }

    #[test]
    fn the_default_rules_leave_the_routine_alone_and_ask_for_the_dangerous() {
        let dir = tempfile::tempdir().unwrap();
        let scope = scope(dir.path());
        let rules = default_rules();
        assert_eq!(validate(&rules).unwrap(), rules);
        let decide = |tool: &str, read_only: bool, args: Value| {
            evaluate(&rules, tool, read_only, &args, &scope).decision
        };
        let shell = |command: &str| decide("shell.execute", false, json!({"command": command}));
        assert_eq!(shell("npm test"), Decision::Allow);
        assert_eq!(shell("cargo build --release"), Decision::Allow);
        assert_eq!(shell("rm -rf target"), Decision::Ask);
        assert_eq!(shell("cd a && sudo make install"), Decision::Ask);
        assert_eq!(shell("git push origin main"), Decision::Ask);
        assert_eq!(
            decide("filesystem.write", false, json!({"path": "src/a.ts"})),
            Decision::Allow
        );
        assert_eq!(
            decide("filesystem.write", false, json!({"path": "../a.ts"})),
            Decision::Ask
        );
        assert_eq!(
            decide("filesystem.delete", false, json!({"path": "a.ts"})),
            Decision::Ask
        );
        assert_eq!(decide("git.push", false, json!({})), Decision::Ask);
        assert_eq!(
            decide("git.commit", false, json!({"message": "x"})),
            Decision::Allow
        );
        assert_eq!(
            decide("terminal.write", false, json!({"id": "t", "data": "ls\r"})),
            Decision::Ask
        );
        assert_eq!(
            decide("agent.delegate", false, json!({"title": "x"})),
            Decision::Allow
        );
        // GitHub (ADR-0017): reading is routine, publishing asks.
        assert_eq!(decide("github.pr.list", true, json!({})), Decision::Allow);
        assert_eq!(decide("github.checks", true, json!({})), Decision::Allow);
        let create = evaluate(
            &rules,
            "github.pr.create",
            false,
            &json!({"title": "x"}),
            &scope,
        );
        // Index 10: "regra 11" on screen.
        assert_eq!((create.decision, create.rule), (Decision::Ask, Some(10)));
        assert_eq!(
            decide("github.pr.merge", false, json!({"number": 3})),
            Decision::Ask
        );
        assert_eq!(decide("git.fetch", false, json!({})), Decision::Allow);
    }

    #[test]
    fn tool_patterns_and_validation() {
        assert!(tool_matches("git.*", "git.push"));
        assert!(!tool_matches("git.*", "gitx.push"));
        assert!(tool_matches("*", "memory.save"));
        assert!(!tool_matches("git.push", "git.pull"));

        let tidy = validate(&[PolicyRule {
            tools: vec![" git.push ".into(), "".into()],
            access: None,
            location: None,
            command: Some("  ".into()),
            path: None,
            decision: Decision::Ask,
            note: Some(" remoto ".into()),
        }])
        .unwrap();
        assert_eq!(tidy[0].tools, ["git.push"]);
        assert_eq!(tidy[0].command, None);
        assert_eq!(tidy[0].note.as_deref(), Some("remoto"));

        let err = validate(&[PolicyRule::tools(&["Git Push"], Decision::Ask)]).unwrap_err();
        assert!(err.contains("regra 1"), "{err}");
        let long = "x".repeat(MAX_PATTERN + 1);
        assert!(validate(&[PolicyRule::tools(&[], Decision::Ask).with_command(&long)]).is_err());
        assert!(validate(&vec![PolicyRule::tools(&[], Decision::Ask); MAX_RULES + 1]).is_err());
    }
}
