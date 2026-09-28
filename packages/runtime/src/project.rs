//! Project discovery and PROJECT PROFILE detection (ADR-0008).
//!
//! Tools: `project.discover`, `project.profile` (and `project.open`, which
//! lives in the dispatcher because it changes the runtime's base directory).
//!
//! Detection is heuristic and evidence based: every conclusion adds an entry
//! to `markers`. The contents of `.env` files are never read.

use crate::platform::{home_dir, io_error};
use chrono::Utc;
use orchestrator_core::{
    DockerInfo, GitRemote, GitSummary, ProjectCandidate, ProjectProfile, RuntimeRequirement,
    ToolError,
};
use orchestrator_git::{ChangeKind, Git};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

/// Files whose presence marks a folder as a project.
const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "package.json",
    "deno.json",
    "deno.jsonc",
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "Pipfile",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "Gemfile",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

/// Directories never descended into during discovery (besides hidden ones).
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "venv",
    "env",
    "__pycache__",
    "site-packages",
    "Library",
    "AppData",
    "Applications",
    "Windows",
    "Program Files",
    "Program Files (x86)",
    "ProgramData",
    "$Recycle.Bin",
    "System Volume Information",
];

pub const DEFAULT_MAX_DEPTH: usize = 4;
pub const DEFAULT_MAX_DIRS: usize = 20_000;

// ------------------------------------------------------------- discover ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverArgs {
    /// Folders to scan. Default: the home directory and common project
    /// folders (`C:\Projetos`, `D:\dev`, …) that exist.
    #[serde(default)]
    pub roots: Option<Vec<String>>,
    #[serde(default)]
    pub max_depth: Option<usize>,
    /// Stop after visiting this many directories.
    #[serde(default)]
    pub max_dirs: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverOutput {
    pub roots: Vec<String>,
    pub projects: Vec<ProjectCandidate>,
    pub scanned_dirs: usize,
    /// True when `maxDirs` stopped the scan early.
    pub truncated: bool,
}

/// Roots scanned when the caller gives none.
pub fn default_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = home_dir().into_iter().collect();
    if cfg!(windows) {
        for drive in ['C', 'D', 'E', 'F'] {
            for name in [
                "Projetos",
                "Projects",
                "dev",
                "src",
                "code",
                "repos",
                "workspace",
            ] {
                let candidate = PathBuf::from(format!("{drive}:\\{name}"));
                if candidate.is_dir() {
                    roots.push(candidate);
                }
            }
        }
    }
    roots
}

fn candidate_markers(names: &BTreeSet<String>) -> Vec<String> {
    let mut markers: Vec<String> = PROJECT_MARKERS
        .iter()
        .filter(|m| names.contains(**m))
        .map(|m| (*m).to_owned())
        .collect();
    if let Some(solution) = names
        .iter()
        .find(|n| n.ends_with(".sln") || n.ends_with(".csproj"))
    {
        markers.push(solution.clone());
    }
    markers
}

fn dir_names(dir: &Path) -> Option<BTreeSet<String>> {
    let entries = fs::read_dir(dir).ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
    )
}

/// Breadth-first scan for project folders. A project folder is reported and
/// not descended into.
pub fn discover(roots: &[PathBuf], max_depth: usize, max_dirs: usize) -> DiscoverOutput {
    let mut projects = Vec::new();
    let mut seen = BTreeSet::new();
    let mut scanned = 0usize;
    let mut truncated = false;
    let mut queue: VecDeque<(PathBuf, usize)> =
        roots.iter().map(|root| (root.clone(), 0)).collect();

    while let Some((dir, depth)) = queue.pop_front() {
        if scanned >= max_dirs {
            truncated = true;
            break;
        }
        if !seen.insert(dir.clone()) {
            continue;
        }
        scanned += 1;
        let Some(names) = dir_names(&dir) else {
            continue;
        };
        let markers = candidate_markers(&names);
        if !markers.is_empty() {
            projects.push(ProjectCandidate {
                name: folder_name(&dir),
                path: dir.display().to_string(),
                is_git_repo: markers.iter().any(|m| m == ".git"),
                markers,
            });
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        for name in names {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let child = dir.join(&name);
            // Do not follow symlinks (avoids cycles and huge detours).
            let is_dir = fs::symlink_metadata(&child)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if is_dir {
                queue.push_back((child, depth + 1));
            }
        }
    }

    projects.sort_by_key(|p| p.path.to_lowercase());
    DiscoverOutput {
        roots: roots.iter().map(|r| r.display().to_string()).collect(),
        projects,
        scanned_dirs: scanned,
        truncated,
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// -------------------------------------------------------------- profile ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathArgs {
    /// Default: the open project (runtime base directory).
    #[serde(default)]
    pub path: Option<String>,
}

/// Accumulates findings without duplicates, keeping insertion order.
#[derive(Default)]
struct Findings {
    languages: Vec<String>,
    frameworks: Vec<String>,
    package_managers: Vec<String>,
    runtimes: Vec<RuntimeRequirement>,
    databases: Vec<String>,
    tools: Vec<String>,
    markers: Vec<String>,
}

fn add(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|v| v == value) {
        list.push(value.to_owned());
    }
}

impl Findings {
    fn runtime(&mut self, name: &str, version: Option<String>) {
        match self.runtimes.iter_mut().find(|r| r.name == name) {
            Some(existing) if existing.version.is_none() => existing.version = version,
            Some(_) => {}
            None => self.runtimes.push(RuntimeRequirement {
                name: name.to_owned(),
                version,
            }),
        }
    }

    fn marker(&mut self, marker: impl Into<String>) {
        let marker = marker.into();
        if !self.markers.contains(&marker) {
            self.markers.push(marker);
        }
    }
}

/// Reads a small text file (profiles never need large files).
fn read_text(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Lower-cased words of a manifest (split on anything that cannot be part of
/// a package name).
fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@')))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn profile(root: &Path, git: Option<&Git>) -> Result<ProjectProfile, ToolError> {
    let metadata = fs::metadata(root).map_err(|e| io_error("cannot profile", root, e))?;
    if !metadata.is_dir() {
        return Err(ToolError::invalid_args(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let names = dir_names(root).unwrap_or_default();
    let has = |name: &str| names.contains(name);
    let mut f = Findings::default();
    let mut scripts = BTreeMap::new();
    let mut monorepo = false;

    detect_node(root, &names, &mut f, &mut scripts, &mut monorepo);
    detect_python(root, &names, &mut f);
    detect_rust(root, &names, &mut f, &mut monorepo);
    detect_go(root, &has, &mut f);
    detect_jvm_dotnet_php_ruby(root, &names, &mut f);
    let docker = detect_docker(root, &names, &mut f);

    if has("prisma") {
        if let Some(schema) = read_text(&root.join("prisma/schema.prisma")) {
            add(&mut f.tools, "Prisma");
            f.marker("prisma/schema.prisma");
            if let Some(provider) = prisma_provider(&schema) {
                if let Some(db) = database_name(&provider) {
                    add(&mut f.databases, db);
                    f.marker(format!("prisma/schema.prisma: provider {provider}"));
                }
            }
        }
    }
    for monorepo_file in ["pnpm-workspace.yaml", "lerna.json", "turbo.json", "nx.json"] {
        if has(monorepo_file) {
            monorepo = true;
            f.marker(monorepo_file);
        }
    }

    Ok(ProjectProfile {
        name: folder_name(root),
        path: root.display().to_string(),
        git: git.and_then(|git| git_summary(git, root)),
        languages: f.languages,
        frameworks: f.frameworks,
        package_managers: f.package_managers,
        runtimes: f.runtimes,
        docker,
        databases: f.databases,
        tools: f.tools,
        important_files: important_files(root, &names),
        scripts,
        monorepo,
        markers: f.markers,
        detected_at: Utc::now(),
    })
}

fn git_summary(git: &Git, root: &Path) -> Option<GitSummary> {
    // Not a repository (or git unavailable): the profile has no Git section.
    let status = git.status(root).ok()?;
    let remotes = git
        .remotes(root)
        .unwrap_or_default()
        .into_iter()
        .map(|r| GitRemote {
            name: r.name,
            url: r.url,
        })
        .collect();
    let has = |kind: ChangeKind| {
        move |c: &orchestrator_git::FileChange| c.unstaged == Some(kind) || c.staged == Some(kind)
    };
    Some(GitSummary {
        root: status.root.clone(),
        branch: status.branch.clone(),
        head: status.head.as_ref().map(|h| h.chars().take(7).collect()),
        upstream: status.upstream.clone(),
        ahead: status.ahead,
        behind: status.behind,
        remotes,
        staged: status.count(|c| c.staged.is_some()),
        modified: status.count(has(ChangeKind::Modified)),
        deleted: status.count(has(ChangeKind::Deleted)),
        untracked: status.count(has(ChangeKind::Untracked)),
        conflicted: status.count(|c| c.conflicted),
        clean: status.clean,
    })
}

const NODE_FRAMEWORKS: &[(&str, &str)] = &[
    ("next", "Next.js"),
    ("nuxt", "Nuxt"),
    ("@remix-run/react", "Remix"),
    ("astro", "Astro"),
    ("@sveltejs/kit", "SvelteKit"),
    ("svelte", "Svelte"),
    ("@angular/core", "Angular"),
    ("vue", "Vue"),
    ("expo", "Expo"),
    ("react-native", "React Native"),
    ("react", "React"),
    ("@nestjs/core", "NestJS"),
    ("express", "Express"),
    ("fastify", "Fastify"),
    ("hono", "Hono"),
    ("koa", "Koa"),
    ("electron", "Electron"),
    ("@tauri-apps/api", "Tauri"),
    ("vite", "Vite"),
];

const NODE_TOOLS: &[(&str, &str)] = &[
    ("prisma", "Prisma"),
    ("@prisma/client", "Prisma"),
    ("drizzle-orm", "Drizzle"),
    ("typeorm", "TypeORM"),
    ("sequelize", "Sequelize"),
    ("mongoose", "Mongoose"),
    ("jest", "Jest"),
    ("vitest", "Vitest"),
    ("@playwright/test", "Playwright"),
    ("cypress", "Cypress"),
    ("eslint", "ESLint"),
    ("prettier", "Prettier"),
    ("tailwindcss", "Tailwind CSS"),
    ("turbo", "Turborepo"),
    ("nx", "Nx"),
];

const NODE_DATABASES: &[(&str, &str)] = &[
    ("pg", "PostgreSQL"),
    ("postgres", "PostgreSQL"),
    ("@neondatabase/serverless", "PostgreSQL"),
    ("@supabase/supabase-js", "Supabase"),
    ("mysql", "MySQL"),
    ("mysql2", "MySQL"),
    ("sqlite3", "SQLite"),
    ("better-sqlite3", "SQLite"),
    ("mongodb", "MongoDB"),
    ("mongoose", "MongoDB"),
    ("redis", "Redis"),
    ("ioredis", "Redis"),
];

fn detect_node(
    root: &Path,
    names: &BTreeSet<String>,
    f: &mut Findings,
    scripts: &mut BTreeMap<String, String>,
    monorepo: &mut bool,
) {
    let deno = names.contains("deno.json") || names.contains("deno.jsonc");
    if deno {
        add(&mut f.languages, "TypeScript");
        f.runtime("Deno", None);
        f.marker(if names.contains("deno.json") {
            "deno.json"
        } else {
            "deno.jsonc"
        });
    }
    if !names.contains("package.json") {
        return;
    }
    f.marker("package.json");
    let package: Value = read_text(&root.join("package.json"))
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);

    let mut deps = BTreeSet::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(map) = package.get(section).and_then(Value::as_object) {
            deps.extend(map.keys().cloned());
        }
    }

    let typescript = names.contains("tsconfig.json") || deps.contains("typescript");
    add(
        &mut f.languages,
        if typescript {
            "TypeScript"
        } else {
            "JavaScript"
        },
    );
    if names.contains("tsconfig.json") {
        f.marker("tsconfig.json");
    }

    // Package manager: lockfiles first, then the `packageManager` field.
    for (lockfile, manager) in [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("package-lock.json", "npm"),
        ("bun.lockb", "bun"),
        ("bun.lock", "bun"),
    ] {
        if names.contains(lockfile) {
            add(&mut f.package_managers, manager);
            f.marker(lockfile);
        }
    }
    if let Some(declared) = package.get("packageManager").and_then(Value::as_str) {
        let manager = declared.split('@').next().unwrap_or(declared);
        if !manager.is_empty() {
            add(&mut f.package_managers, manager);
            f.marker(format!("package.json: packageManager {declared}"));
        }
    }
    if !f
        .package_managers
        .iter()
        .any(|m| matches!(m.as_str(), "pnpm" | "yarn" | "npm" | "bun"))
    {
        add(&mut f.package_managers, "npm");
    }

    let node_version = package
        .pointer("/engines/node")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            [".nvmrc", ".node-version"]
                .iter()
                .find_map(|file| read_text(&root.join(file)))
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        });
    if f.package_managers.iter().any(|m| m == "bun") {
        f.runtime("Bun", None);
    }
    f.runtime("Node.js", node_version);

    for (dep, framework) in NODE_FRAMEWORKS {
        if deps.contains(*dep) {
            add(&mut f.frameworks, framework);
            f.marker(format!("dependency: {dep}"));
        }
    }
    if names.iter().any(|n| n.starts_with("next.config.")) {
        add(&mut f.frameworks, "Next.js");
    }
    for (dep, tool) in NODE_TOOLS {
        if deps.contains(*dep) {
            add(&mut f.tools, tool);
        }
    }
    for (dep, db) in NODE_DATABASES {
        if deps.contains(*dep) {
            add(&mut f.databases, db);
            f.marker(format!("dependency: {dep}"));
        }
    }

    if let Some(map) = package.get("scripts").and_then(Value::as_object) {
        for (name, command) in map {
            if let Some(command) = command.as_str() {
                scripts.insert(name.clone(), command.to_owned());
            }
        }
    }
    if package.get("workspaces").is_some() {
        *monorepo = true;
        f.marker("package.json: workspaces");
    }
}

fn detect_python(root: &Path, names: &BTreeSet<String>, f: &mut Findings) {
    let manifests: Vec<&String> = names
        .iter()
        .filter(|n| {
            matches!(
                n.as_str(),
                "pyproject.toml" | "setup.py" | "Pipfile" | "setup.cfg"
            ) || (n.starts_with("requirements") && n.ends_with(".txt"))
        })
        .collect();
    if manifests.is_empty() && !names.contains("manage.py") {
        return;
    }
    add(&mut f.languages, "Python");
    let mut text = String::new();
    for manifest in &manifests {
        f.marker(manifest.as_str());
        if let Some(content) = read_text(&root.join(manifest.as_str())) {
            text.push_str(&content);
            text.push('\n');
        }
    }
    let words = words(&text);
    let pyproject = read_text(&root.join("pyproject.toml")).unwrap_or_default();

    if names.contains("poetry.lock") || pyproject.contains("[tool.poetry]") {
        add(&mut f.package_managers, "poetry");
        f.marker("poetry");
    }
    if names.contains("uv.lock") {
        add(&mut f.package_managers, "uv");
        f.marker("uv.lock");
    }
    if names.contains("Pipfile") {
        add(&mut f.package_managers, "pipenv");
    }
    add(&mut f.package_managers, "pip");

    let version = read_text(&root.join(".python-version"))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            // `requires-python = ">=3.11"` → `>=3.11`
            pyproject
                .lines()
                .find(|l| l.trim_start().starts_with("requires-python"))
                .and_then(|l| l.split_once('='))
                .map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'').to_owned())
        });
    f.runtime("Python", version);

    if names.contains("manage.py") || words.contains("django") {
        add(&mut f.frameworks, "Django");
        f.marker(if names.contains("manage.py") {
            "manage.py"
        } else {
            "dependency: django"
        });
    }
    for (dep, framework) in [
        ("fastapi", "FastAPI"),
        ("flask", "Flask"),
        ("streamlit", "Streamlit"),
    ] {
        if words.contains(dep) {
            add(&mut f.frameworks, framework);
            f.marker(format!("dependency: {dep}"));
        }
    }
    for (dep, tool) in [
        ("sqlalchemy", "SQLAlchemy"),
        ("pytest", "Pytest"),
        ("alembic", "Alembic"),
    ] {
        if words.contains(dep) {
            add(&mut f.tools, tool);
        }
    }
    let has_prefix = |prefix: &str| words.iter().any(|w| w.starts_with(prefix));
    for (prefix, db) in [
        ("psycopg", "PostgreSQL"),
        ("asyncpg", "PostgreSQL"),
        ("pymysql", "MySQL"),
        ("mysqlclient", "MySQL"),
        ("pymongo", "MongoDB"),
        ("motor", "MongoDB"),
        ("redis", "Redis"),
    ] {
        if has_prefix(prefix) {
            add(&mut f.databases, db);
            f.marker(format!("dependency: {prefix}"));
        }
    }
}

fn detect_rust(root: &Path, names: &BTreeSet<String>, f: &mut Findings, monorepo: &mut bool) {
    if !names.contains("Cargo.toml") {
        return;
    }
    add(&mut f.languages, "Rust");
    add(&mut f.package_managers, "cargo");
    f.marker("Cargo.toml");
    let manifest = read_text(&root.join("Cargo.toml")).unwrap_or_default();
    if manifest.contains("[workspace]") {
        *monorepo = true;
    }
    let version = read_text(&root.join("rust-toolchain.toml"))
        .and_then(|t| toml_value(&t, "channel"))
        .or_else(|| toml_value(&manifest, "rust-version"));
    f.runtime("Rust", version);
    let words = words(&manifest);
    for (dep, framework) in [
        ("tauri", "Tauri"),
        ("axum", "Axum"),
        ("actix-web", "Actix Web"),
        ("rocket", "Rocket"),
    ] {
        if words.contains(dep) {
            add(&mut f.frameworks, framework);
        }
    }
    for (dep, tool) in [
        ("sqlx", "SQLx"),
        ("diesel", "Diesel"),
        ("sea-orm", "SeaORM"),
    ] {
        if words.contains(dep) {
            add(&mut f.tools, tool);
        }
    }
    if words.contains("rusqlite") {
        add(&mut f.databases, "SQLite");
    }
}

fn detect_go(root: &Path, has: &dyn Fn(&str) -> bool, f: &mut Findings) {
    if !has("go.mod") {
        return;
    }
    add(&mut f.languages, "Go");
    add(&mut f.package_managers, "go");
    f.marker("go.mod");
    let module = read_text(&root.join("go.mod")).unwrap_or_default();
    let version = module
        .lines()
        .find_map(|l| l.trim().strip_prefix("go ").map(|v| v.trim().to_owned()));
    f.runtime("Go", version);
    for (dep, framework) in [
        ("github.com/gin-gonic/gin", "Gin"),
        ("github.com/labstack/echo", "Echo"),
        ("github.com/gofiber/fiber", "Fiber"),
    ] {
        if module.contains(dep) {
            add(&mut f.frameworks, framework);
        }
    }
}

fn detect_jvm_dotnet_php_ruby(root: &Path, names: &BTreeSet<String>, f: &mut Findings) {
    if names.contains("pom.xml") {
        add(&mut f.languages, "Java");
        add(&mut f.package_managers, "maven");
        f.runtime("JVM", None);
        f.marker("pom.xml");
        if read_text(&root.join("pom.xml")).is_some_and(|t| t.contains("spring-boot")) {
            add(&mut f.frameworks, "Spring Boot");
        }
    }
    for gradle in ["build.gradle", "build.gradle.kts"] {
        if names.contains(gradle) {
            add(
                &mut f.languages,
                if gradle.ends_with(".kts") {
                    "Kotlin"
                } else {
                    "Java"
                },
            );
            add(&mut f.package_managers, "gradle");
            f.runtime("JVM", None);
            f.marker(gradle);
            if read_text(&root.join(gradle)).is_some_and(|t| t.contains("org.springframework.boot"))
            {
                add(&mut f.frameworks, "Spring Boot");
            }
        }
    }
    if let Some(dotnet) = names
        .iter()
        .find(|n| n.ends_with(".sln") || n.ends_with(".csproj"))
    {
        add(&mut f.languages, "C#");
        add(&mut f.package_managers, "dotnet");
        f.runtime(".NET", None);
        f.marker(dotnet.as_str());
    }
    if names.contains("composer.json") {
        add(&mut f.languages, "PHP");
        add(&mut f.package_managers, "composer");
        f.runtime("PHP", None);
        f.marker("composer.json");
        let composer = read_text(&root.join("composer.json")).unwrap_or_default();
        if composer.contains("laravel/framework") {
            add(&mut f.frameworks, "Laravel");
        }
        if composer.contains("symfony/") {
            add(&mut f.frameworks, "Symfony");
        }
    }
    if names.contains("Gemfile") {
        add(&mut f.languages, "Ruby");
        add(&mut f.package_managers, "bundler");
        f.runtime("Ruby", None);
        f.marker("Gemfile");
        if read_text(&root.join("Gemfile")).is_some_and(|t| words(&t).contains("rails")) {
            add(&mut f.frameworks, "Rails");
        }
    }
}

fn detect_docker(root: &Path, names: &BTreeSet<String>, f: &mut Findings) -> DockerInfo {
    let mut docker = DockerInfo::default();
    for name in names {
        let lower = name.to_lowercase();
        if lower == "dockerfile"
            || lower.starts_with("dockerfile.")
            || lower.ends_with(".dockerfile")
        {
            docker.dockerfiles.push(name.clone());
            f.marker(name.as_str());
        }
        let compose = (lower.starts_with("docker-compose") || lower.starts_with("compose."))
            && (lower.ends_with(".yml") || lower.ends_with(".yaml"));
        if compose {
            docker.compose_files.push(name.clone());
            f.marker(name.as_str());
            for image in compose_images(&read_text(&root.join(name)).unwrap_or_default()) {
                if let Some(db) = image_database(&image) {
                    add(&mut f.databases, db);
                    f.marker(format!("{name}: image {image}"));
                }
                add(&mut docker.images, &image);
            }
        }
    }
    if docker.is_used() {
        add(&mut f.tools, "Docker");
    }
    docker
}

/// `image:` values of a compose file (line based, no YAML parser needed).
fn compose_images(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("image:"))
        .map(|image| image.trim().trim_matches('"').trim_matches('\'').to_owned())
        .filter(|image| !image.is_empty())
        .collect()
}

fn image_database(image: &str) -> Option<&'static str> {
    let name = image.rsplit('/').next().unwrap_or(image);
    let name = name.split(':').next().unwrap_or(name);
    Some(match name {
        "postgres" | "postgis" => "PostgreSQL",
        "mysql" => "MySQL",
        "mariadb" => "MariaDB",
        "mongo" => "MongoDB",
        "redis" | "valkey" => "Redis",
        "mssql" | "mssql-server" => "SQL Server",
        _ if image.contains("mssql") => "SQL Server",
        _ => return None,
    })
}

/// `provider` of the `datasource` block of a Prisma schema.
fn prisma_provider(schema: &str) -> Option<String> {
    let mut in_datasource = false;
    for line in schema.lines() {
        let line = line.trim();
        if line.starts_with("datasource") {
            in_datasource = true;
        } else if in_datasource && line.starts_with('}') {
            in_datasource = false;
        } else if in_datasource && line.starts_with("provider") {
            return line
                .split_once('=')
                .map(|(_, v)| v.trim().trim_matches('"').to_owned());
        }
    }
    None
}

fn database_name(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "postgresql" | "postgres" => "PostgreSQL",
        "mysql" => "MySQL",
        "sqlite" => "SQLite",
        "sqlserver" => "SQL Server",
        "mongodb" => "MongoDB",
        "cockroachdb" => "CockroachDB",
        _ => return None,
    })
}

/// Value of a simple `key = "value"` TOML line.
fn toml_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
    })
}

/// Well-known files present at the project root (names only; `.env`
/// contents are never read).
fn important_files(root: &Path, names: &BTreeSet<String>) -> Vec<String> {
    const EXACT: &[&str] = &[
        "ARCHITECTURE.md",
        "CONTRIBUTING.md",
        "CLAUDE.md",
        "AGENTS.md",
        "package.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "package-lock.json",
        "bun.lock",
        "bun.lockb",
        "pnpm-workspace.yaml",
        "tsconfig.json",
        "deno.json",
        "pyproject.toml",
        "requirements.txt",
        "Pipfile",
        "poetry.lock",
        "uv.lock",
        "manage.py",
        "Cargo.toml",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "composer.json",
        "Gemfile",
        "Makefile",
        ".env",
        ".env.example",
        ".gitlab-ci.yml",
        ".nvmrc",
        ".python-version",
    ];
    let mut files: Vec<String> = Vec::new();
    for name in names {
        let lower = name.to_lowercase();
        let wanted = EXACT.contains(&name.as_str())
            || lower.starts_with("readme")
            || lower.starts_with("license")
            || lower.starts_with("next.config.")
            || lower.starts_with("vite.config.")
            || lower.starts_with("nuxt.config.")
            || lower == "dockerfile"
            || ((lower.starts_with("docker-compose") || lower.starts_with("compose."))
                && (lower.ends_with(".yml") || lower.ends_with(".yaml")));
        if wanted {
            files.push(name.clone());
        }
    }
    for nested in ["prisma/schema.prisma", ".github/workflows"] {
        if root.join(nested).exists() {
            files.push(nested.to_owned());
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, content: &str) {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }

    #[test]
    fn profiles_a_next_prisma_docker_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{
              "name": "meu-saas",
              "packageManager": "pnpm@9.1.0",
              "engines": { "node": ">=20" },
              "scripts": { "dev": "next dev", "build": "next build" },
              "dependencies": { "next": "15", "react": "19", "@prisma/client": "5", "pg": "8" },
              "devDependencies": { "typescript": "5", "vitest": "2", "prisma": "5" }
            }"#,
        );
        write(root, "pnpm-lock.yaml", "");
        write(root, "tsconfig.json", "{}");
        write(
            root,
            "prisma/schema.prisma",
            "generator client {\n  provider = \"prisma-client-js\"\n}\n\ndatasource db {\n  provider = \"postgresql\"\n  url = env(\"DATABASE_URL\")\n}\n",
        );
        write(
            root,
            "docker-compose.yml",
            "services:\n  db:\n    image: \"postgres:16\"\n  cache:\n    image: redis:7-alpine\n",
        );
        write(root, "Dockerfile", "FROM node:20");
        write(root, ".env", "SECRET=nao-deve-ser-lido");
        write(root, "README.md", "# Meu SaaS");

        let profile = profile(root, None).unwrap();
        assert_eq!(profile.languages, vec!["TypeScript"]);
        assert_eq!(profile.package_managers[0], "pnpm");
        assert!(profile.frameworks.starts_with(&["Next.js".to_owned()]));
        assert!(profile.frameworks.contains(&"React".to_owned()));
        assert!(profile.tools.contains(&"Prisma".to_owned()));
        assert!(profile.tools.contains(&"Vitest".to_owned()));
        assert!(profile.tools.contains(&"Docker".to_owned()));
        assert_eq!(profile.databases, vec!["PostgreSQL", "Redis"]);
        assert_eq!(profile.runtimes[0].name, "Node.js");
        assert_eq!(profile.runtimes[0].version.as_deref(), Some(">=20"));
        assert_eq!(profile.docker.dockerfiles, vec!["Dockerfile"]);
        assert_eq!(profile.docker.images, vec!["postgres:16", "redis:7-alpine"]);
        assert_eq!(
            profile.scripts.get("dev").map(String::as_str),
            Some("next dev")
        );
        assert!(profile.important_files.contains(&".env".to_owned()));
        assert!(profile
            .important_files
            .contains(&"prisma/schema.prisma".to_owned()));
        assert!(profile
            .markers
            .contains(&"prisma/schema.prisma: provider postgresql".to_owned()));
        assert!(profile.git.is_none());
        // `.env` content never shows up anywhere in the profile.
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("nao-deve-ser-lido"));
    }

    #[test]
    fn profiles_a_django_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "manage.py", "");
        write(
            root,
            "requirements.txt",
            "Django>=5.0\npsycopg2-binary==2.9\npytest\n",
        );
        write(root, ".python-version", "3.12\n");
        let profile = profile(root, None).unwrap();
        assert_eq!(profile.languages, vec!["Python"]);
        assert_eq!(profile.frameworks, vec!["Django"]);
        assert_eq!(profile.package_managers, vec!["pip"]);
        assert_eq!(profile.databases, vec!["PostgreSQL"]);
        assert_eq!(profile.runtimes[0].version.as_deref(), Some("3.12"));
        assert!(profile.tools.contains(&"Pytest".to_owned()));
    }

    #[test]
    fn profiles_poetry_and_rust_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pyproject.toml",
            "[tool.poetry]\nname = \"x\"\n[project]\nrequires-python = \">=3.11\"\ndependencies = [\"fastapi\"]\n",
        );
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = []\n[workspace.dependencies]\ntauri = \"2\"\n",
        );
        let profile = profile(root, None).unwrap();
        assert_eq!(profile.languages, vec!["Python", "Rust"]);
        assert_eq!(profile.package_managers[0], "poetry");
        assert!(profile.frameworks.contains(&"FastAPI".to_owned()));
        assert!(profile.frameworks.contains(&"Tauri".to_owned()));
        assert!(profile.monorepo);
        let python = profile
            .runtimes
            .iter()
            .find(|r| r.name == "Python")
            .unwrap();
        assert_eq!(python.version.as_deref(), Some(">=3.11"));
    }

    #[test]
    fn empty_folder_has_an_empty_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = profile(dir.path(), None).unwrap();
        assert!(profile.languages.is_empty());
        assert!(profile.markers.is_empty());
        assert!(!profile.docker.is_used());
    }

    #[test]
    fn profile_of_a_file_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "");
        assert!(profile(&dir.path().join("a.txt"), None).is_err());
    }

    #[test]
    fn discovery_finds_projects_and_skips_heavy_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "work/app/package.json", "{}");
        write(root, "work/app/node_modules/dep/package.json", "{}");
        write(root, "work/api/go.mod", "module x");
        write(root, "work/site/.git/HEAD", "ref: refs/heads/main");
        write(root, "node_modules/ignored/package.json", "{}");
        write(root, ".hidden/secret/package.json", "{}");
        write(root, "too/deep/a/b/c/package.json", "{}");

        let out = discover(&[root.to_path_buf()], 4, 10_000);
        let names: Vec<_> = out.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["api", "app", "site"]);
        let site = out.projects.iter().find(|p| p.name == "site").unwrap();
        assert!(site.is_git_repo);
        assert!(!out.truncated);

        let limited = discover(&[root.to_path_buf()], 4, 2);
        assert!(limited.truncated);
    }

    #[test]
    fn discovery_reports_the_root_itself_when_it_is_a_project() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[package]");
        write(dir.path(), "sub/package.json", "{}");
        let out = discover(&[dir.path().to_path_buf()], 4, 100);
        assert_eq!(out.projects.len(), 1);
        assert_eq!(out.projects[0].markers, vec!["Cargo.toml"]);
    }
}
