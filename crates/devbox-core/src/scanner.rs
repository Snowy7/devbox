use crate::PolicyDecision;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct ProjectScanner;

impl ProjectScanner {
    pub fn scan_path(&self, root: impl AsRef<Path>) -> Result<ProjectScan, ScanError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(ScanError::RootNotFound {
                path: root.to_path_buf(),
            });
        }

        if !root.is_dir() {
            return Err(ScanError::RootNotDirectory {
                path: root.to_path_buf(),
            });
        }

        let root = absolute_root(root)?;

        let mut scan = ProjectScan::new(root.clone());
        scan_directory(&root, &root, &mut scan)?;
        Ok(scan)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectScan {
    root: PathBuf,
    projects: Vec<DetectedProject>,
    policy_evaluations: Vec<PathPolicyEvaluation>,
}

impl ProjectScan {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            projects: Vec::new(),
            policy_evaluations: Vec::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn projects(&self) -> &[DetectedProject] {
        &self.projects
    }

    pub fn policy_evaluations(&self) -> &[PathPolicyEvaluation] {
        &self.policy_evaluations
    }

    pub fn excluded_paths(&self) -> impl Iterator<Item = &PathPolicyEvaluation> {
        self.policy_evaluations
            .iter()
            .filter(|evaluation| matches!(evaluation.decision(), PolicyDecision::Exclude { .. }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedProject {
    relative_path: PathBuf,
    kind: ProjectKind,
    signals: Vec<ProjectSignal>,
    rehydration_hints: Vec<RehydrationHint>,
}

impl DetectedProject {
    fn new(
        relative_path: PathBuf,
        kind: ProjectKind,
        signals: Vec<ProjectSignal>,
        rehydration_hints: Vec<RehydrationHint>,
    ) -> Self {
        Self {
            relative_path,
            kind,
            signals,
            rehydration_hints,
        }
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub fn kind(&self) -> &ProjectKind {
        &self.kind
    }

    pub fn signals(&self) -> &[ProjectSignal] {
        &self.signals
    }

    pub fn rehydration_hints(&self) -> &[RehydrationHint] {
        &self.rehydration_hints
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectKind {
    Node,
    Rust,
    Python,
}

impl fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node => f.write_str("Node"),
            Self::Rust => f.write_str("Rust"),
            Self::Python => f.write_str("Python"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSignal {
    path: PathBuf,
}

impl ProjectSignal {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RehydrationHint {
    command: String,
}

impl RehydrationHint {
    fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }

    pub fn command(&self) -> &str {
        &self.command
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPolicyEvaluation {
    relative_path: PathBuf,
    decision: PolicyDecision,
}

impl PathPolicyEvaluation {
    fn new(relative_path: PathBuf, decision: PolicyDecision) -> Self {
        Self {
            relative_path,
            decision,
        }
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub fn decision(&self) -> &PolicyDecision {
        &self.decision
    }
}

#[derive(Debug)]
pub enum ScanError {
    RootNotFound { path: PathBuf },
    RootNotDirectory { path: PathBuf },
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotFound { path } => {
                write!(f, "scan root does not exist: {}", path.display())
            }
            Self::RootNotDirectory { path } => {
                write!(f, "scan root is not a directory: {}", path.display())
            }
            Self::Io { path, source } => write!(f, "could not scan {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn scan_directory(root: &Path, path: &Path, scan: &mut ProjectScan) -> Result<(), ScanError> {
    let relative_path = relative_to(root, path);
    let decision = evaluate_policy(&relative_path);

    if matches!(decision, PolicyDecision::Exclude { .. }) {
        scan.policy_evaluations
            .push(PathPolicyEvaluation::new(relative_path, decision));
        return Ok(());
    }

    if let Some(project) = detect_project(&relative_path, path) {
        scan.projects.push(project);
    }

    let mut entries = fs::read_dir(path)
        .map_err(|source| ScanError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ScanError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let file_type = entry.file_type().map_err(|source| ScanError::Io {
            path: entry.path(),
            source,
        })?;

        if file_type.is_dir() {
            scan_directory(root, &entry.path(), scan)?;
        }
    }

    Ok(())
}

fn detect_project(relative_path: &Path, path: &Path) -> Option<DetectedProject> {
    if path.join("package.json").is_file() {
        let mut signals = vec![ProjectSignal::new("package.json")];
        let command = if path.join("pnpm-lock.yaml").is_file() {
            signals.push(ProjectSignal::new("pnpm-lock.yaml"));
            "pnpm install"
        } else if path.join("yarn.lock").is_file() {
            signals.push(ProjectSignal::new("yarn.lock"));
            "yarn install"
        } else if path.join("package-lock.json").is_file() {
            signals.push(ProjectSignal::new("package-lock.json"));
            "npm install"
        } else {
            "install Node dependencies with the project package manager"
        };

        return Some(DetectedProject::new(
            relative_path.to_path_buf(),
            ProjectKind::Node,
            signals,
            vec![RehydrationHint::new(command)],
        ));
    }

    if path.join("Cargo.toml").is_file() {
        return Some(DetectedProject::new(
            relative_path.to_path_buf(),
            ProjectKind::Rust,
            vec![ProjectSignal::new("Cargo.toml")],
            vec![RehydrationHint::new("cargo fetch")],
        ));
    }

    if path.join("pyproject.toml").is_file()
        || path.join("requirements.txt").is_file()
        || path.join("setup.py").is_file()
    {
        let mut signals = Vec::new();
        if path.join("pyproject.toml").is_file() {
            signals.push(ProjectSignal::new("pyproject.toml"));
        }
        if path.join("requirements.txt").is_file() {
            signals.push(ProjectSignal::new("requirements.txt"));
        }
        if path.join("setup.py").is_file() {
            signals.push(ProjectSignal::new("setup.py"));
        }

        return Some(DetectedProject::new(
            relative_path.to_path_buf(),
            ProjectKind::Python,
            signals,
            vec![RehydrationHint::new(
                "create a virtualenv and install dependencies",
            )],
        ));
    }

    None
}

fn evaluate_policy(relative_path: &Path) -> PolicyDecision {
    let Some(name) = relative_path.file_name().and_then(|name| name.to_str()) else {
        return PolicyDecision::Include;
    };

    let reason = match name {
        ".git" => Some("Git metadata is handled by the Git adapter"),
        "node_modules" => Some("generated Node dependency directory"),
        ".next" => Some("generated Next.js build directory"),
        "dist" => Some("generated distribution output"),
        "build" => Some("generated build output"),
        "target" => Some("generated Rust build output"),
        ".venv" | "venv" => Some("generated Python virtual environment"),
        "__pycache__" | ".pytest_cache" => Some("generated Python cache directory"),
        ".turbo" => Some("generated Turborepo cache directory"),
        ".gradle" => Some("generated Gradle cache directory"),
        ".cache" => Some("generated tool cache directory"),
        "coverage" => Some("generated coverage output"),
        _ => None,
    };

    match reason {
        Some(reason) => PolicyDecision::Exclude {
            reason: reason.to_string(),
        },
        None => PolicyDecision::Include,
    }
}

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn absolute_root(root: &Path) -> Result<PathBuf, ScanError> {
    if root.is_absolute() {
        return Ok(root.to_path_buf());
    }

    let current_dir = std::env::current_dir().map_err(|source| ScanError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    Ok(current_dir.join(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn detects_node_projects_and_excludes_generated_artifacts() {
        let scan = ProjectScanner
            .scan_path(fixture_path("node"))
            .expect("node fixture scans");

        assert_eq!(scan.projects().len(), 1);
        let project = &scan.projects()[0];
        assert_eq!(project.kind(), &ProjectKind::Node);
        assert_eq!(project.relative_path(), Path::new(""));
        assert_eq!(
            project
                .signals()
                .iter()
                .map(ProjectSignal::path)
                .collect::<Vec<_>>(),
            vec![Path::new("package.json"), Path::new("package-lock.json")]
        );
        assert_eq!(project.rehydration_hints()[0].command(), "npm install");

        let excluded = excluded_path_strings(&scan);
        assert!(excluded.contains(&"dist".to_string()));
        assert!(excluded.contains(&"node_modules".to_string()));
    }

    #[test]
    fn detects_rust_projects_and_excludes_target() {
        let scan = ProjectScanner
            .scan_path(fixture_path("rust"))
            .expect("rust fixture scans");

        assert_eq!(scan.projects().len(), 1);
        let project = &scan.projects()[0];
        assert_eq!(project.kind(), &ProjectKind::Rust);
        assert_eq!(
            project
                .signals()
                .iter()
                .map(ProjectSignal::path)
                .collect::<Vec<_>>(),
            vec![Path::new("Cargo.toml")]
        );
        assert_eq!(project.rehydration_hints()[0].command(), "cargo fetch");

        let excluded = excluded_path_strings(&scan);
        assert!(excluded.contains(&"target".to_string()));
    }

    #[test]
    fn detects_python_projects_and_excludes_virtualenv_and_cache() {
        let scan = ProjectScanner
            .scan_path(fixture_path("python"))
            .expect("python fixture scans");

        assert_eq!(scan.projects().len(), 1);
        let project = &scan.projects()[0];
        assert_eq!(project.kind(), &ProjectKind::Python);
        assert_eq!(
            project
                .signals()
                .iter()
                .map(ProjectSignal::path)
                .collect::<Vec<_>>(),
            vec![Path::new("pyproject.toml"), Path::new("requirements.txt")]
        );

        let excluded = excluded_path_strings(&scan);
        assert!(excluded.contains(&".venv".to_string()));
        assert!(excluded.contains(&"__pycache__".to_string()));
    }

    #[test]
    fn scans_nested_projects_but_does_not_descend_into_excluded_paths() {
        let scan = ProjectScanner
            .scan_path(fixture_path("workspace"))
            .expect("workspace fixture scans");

        let projects = scan
            .projects()
            .iter()
            .map(|project| {
                (
                    project.relative_path().to_path_buf(),
                    project.kind().to_owned(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            projects,
            vec![
                (PathBuf::from("api"), ProjectKind::Python),
                (PathBuf::from("frontend"), ProjectKind::Node),
                (PathBuf::from("tools").join("cli"), ProjectKind::Rust),
            ]
        );

        let excluded = excluded_path_strings(&scan);
        assert!(excluded.contains(
            &PathBuf::from("frontend")
                .join("node_modules")
                .display()
                .to_string()
        ));
        assert!(!projects
            .iter()
            .any(|(path, _)| path.starts_with("frontend/node_modules")));
    }

    #[test]
    fn rejects_missing_and_file_roots() {
        let missing = fixture_path("missing");
        assert!(matches!(
            ProjectScanner.scan_path(&missing),
            Err(ScanError::RootNotFound { .. })
        ));

        let file = fixture_path("node").join("package.json");
        assert!(matches!(
            ProjectScanner.scan_path(&file),
            Err(ScanError::RootNotDirectory { .. })
        ));
    }

    fn excluded_path_strings(scan: &ProjectScan) -> Vec<String> {
        scan.excluded_paths()
            .map(|evaluation| evaluation.relative_path().display().to_string())
            .collect()
    }
}
