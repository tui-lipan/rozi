//! Built-in Rozi agent skill: one embedded `SKILL.md`, installed under `.agents/skills/rozi`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const SKILL_MD: &str = include_str!("SKILL.md");

#[cfg(windows)]
mod windows;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillPaths {
    pub scope_root: PathBuf,
    pub canonical_dir: PathBuf,
    pub skill_file: PathBuf,
    pub claude_path: PathBuf,
}

impl SkillPaths {
    pub fn project(cwd: impl AsRef<Path>) -> Self {
        Self::at(absolute(cwd.as_ref()))
    }

    pub fn global(home: impl AsRef<Path>) -> Self {
        Self::at(absolute(home.as_ref()))
    }

    fn at(scope_root: PathBuf) -> Self {
        let canonical_dir = scope_root.join(".agents").join("skills").join("rozi");
        let skill_file = canonical_dir.join("SKILL.md");
        let claude_path = scope_root.join(".claude").join("skills").join("rozi");
        Self {
            scope_root,
            canonical_dir,
            skill_file,
            claude_path,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalStatus {
    Installed,
    Outdated,
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeStatus {
    Linked,
    Copied,
    Missing,
    Conflict,
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusReport {
    pub canonical: CanonicalStatus,
    pub claude: ClaudeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallReport {
    pub skill_file: PathBuf,
    pub claude: ClaudeInstall,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeInstall {
    Linked { path: PathBuf, target: PathBuf },
    Copied { path: PathBuf },
    Failed { path: PathBuf },
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninstallReport {
    pub removed: Vec<PathBuf>,
}

pub fn default_paths(global: bool) -> Result<SkillPaths, String> {
    if global {
        Ok(SkillPaths::global(resolve_home()?))
    } else {
        let cwd = std::env::current_dir().map_err(|err| format!("could not read cwd: {err}"))?;
        Ok(SkillPaths::project(cwd))
    }
}

pub fn print_skill() {
    print!("{SKILL_MD}");
}

pub fn install(paths: &SkillPaths, claude_available: bool) -> Result<InstallReport, String> {
    install_canonical(paths)?;
    let claude = if claude_available {
        match install_claude(paths) {
            Ok(installed) => installed,
            Err(_) => ClaudeInstall::Failed {
                path: paths.claude_path.clone(),
            },
        }
    } else {
        ClaudeInstall::Skipped
    };
    Ok(InstallReport {
        skill_file: paths.skill_file.clone(),
        claude,
    })
}

pub fn uninstall(paths: &SkillPaths) -> Result<UninstallReport, String> {
    let mut removed = Vec::new();
    if let Some(path) = uninstall_claude(paths)? {
        removed.push(path);
    }
    if let Some(path) = uninstall_canonical(paths)? {
        removed.push(path);
    }
    Ok(UninstallReport { removed })
}

pub fn status(paths: &SkillPaths, claude_available: bool) -> StatusReport {
    StatusReport {
        canonical: canonical_status(paths),
        claude: claude_status(paths, claude_available),
    }
}

pub fn format_install(report: &InstallReport, paths: &SkillPaths) -> String {
    let mut out = String::from("Installed Rozi skill\n");
    out.push_str(&format!("  {}\n", display_path(&report.skill_file, paths)));
    match &report.claude {
        ClaudeInstall::Linked { path, target } => {
            out.push_str(&format!(
                "  {} -> {}\n",
                display_path(path, paths),
                target.display()
            ));
        }
        ClaudeInstall::Copied { path } => {
            out.push_str(&format!("  {}\n", display_path(path, paths)));
        }
        ClaudeInstall::Failed { path } => {
            out.push_str(&format!(
                "\nClaude compatibility could not be installed:\n  {}\n",
                display_path(path, paths)
            ));
        }
        ClaudeInstall::Skipped => {}
    }
    out
}

pub fn format_uninstall(report: &UninstallReport, paths: &SkillPaths) -> String {
    if report.removed.is_empty() {
        return "Rozi skill is not installed\n".to_string();
    }
    let mut out = String::from("Removed Rozi skill\n");
    for path in &report.removed {
        out.push_str(&format!("  {}\n", display_path(path, paths)));
    }
    out
}

pub fn format_status(report: &StatusReport, paths: &SkillPaths) -> String {
    format!(
        "Rozi skill\n\n  canonical   {:<15} {}\n  claude      {:<15} {}\n",
        canonical_label(report.canonical),
        display_path(&paths.skill_file, paths),
        claude_label(report.claude),
        display_path(&paths.claude_path, paths),
    )
}

pub fn display_path(path: &Path, paths: &SkillPaths) -> String {
    if let Ok(relative) = path.strip_prefix(&paths.scope_root) {
        return relative.to_string_lossy().into_owned();
    }
    crate::platform::paths::compress_home(&path.to_string_lossy())
}

fn resolve_home() -> Result<PathBuf, String> {
    // Prefer PlatformEnv so tests under isolation never escape into the real profile. On Windows,
    // PlatformEnv::home is unused; fall through to the USERPROFILE helper.
    let env = crate::platform::paths::PlatformEnv::from_process();
    if let Some(home) = env.home {
        return Ok(home);
    }
    crate::platform::paths::home_directory()
        .map(PathBuf::from)
        .ok_or_else(|| "could not resolve home directory".to_string())
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn canonical_status(paths: &SkillPaths) -> CanonicalStatus {
    match fs::read(&paths.skill_file) {
        Ok(bytes) if bytes == SKILL_MD.as_bytes() => CanonicalStatus::Installed,
        Ok(_) => CanonicalStatus::Outdated,
        Err(_) => CanonicalStatus::Missing,
    }
}

fn claude_status(paths: &SkillPaths, claude_available: bool) -> ClaudeStatus {
    if !claude_available {
        return ClaudeStatus::NotApplicable;
    }
    match inspect_claude(paths) {
        ClaudeEntry::Missing => ClaudeStatus::Missing,
        ClaudeEntry::Link { ours: true, .. } => ClaudeStatus::Linked,
        ClaudeEntry::Copy { ours: true } => ClaudeStatus::Copied,
        ClaudeEntry::Link { ours: false, .. } | ClaudeEntry::Copy { ours: false } => {
            ClaudeStatus::Conflict
        }
        ClaudeEntry::Other => ClaudeStatus::Conflict,
    }
}

#[derive(Debug)]
enum ClaudeEntry {
    Missing,
    Link { ours: bool, target: PathBuf },
    Copy { ours: bool },
    Other,
}

fn inspect_claude(paths: &SkillPaths) -> ClaudeEntry {
    let meta = match fs::symlink_metadata(&paths.claude_path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return ClaudeEntry::Missing,
        Err(_) => return ClaudeEntry::Other,
        Ok(meta) => meta,
    };
    if is_link(&meta) {
        let target = read_compat_target(&paths.claude_path).unwrap_or_default();
        let ours = compat_link_is_ours(paths, &target);
        return ClaudeEntry::Link { ours, target };
    }
    if meta.is_dir() {
        return ClaudeEntry::Copy {
            ours: managed_copy_dir(&paths.claude_path),
        };
    }
    ClaudeEntry::Other
}

fn install_canonical(paths: &SkillPaths) -> Result<(), String> {
    ensure_parent_dir(&paths.canonical_dir)?;
    ensure_real_dir(&paths.canonical_dir)?;
    match fs::symlink_metadata(&paths.skill_file) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Ok(meta) if meta.is_file() && !is_link(&meta) => {}
        Ok(_) => {
            return Err(format!(
                "refusing to overwrite {}",
                paths.skill_file.display()
            ));
        }
        Err(err) => {
            return Err(format!(
                "could not inspect {}: {err}",
                paths.skill_file.display()
            ));
        }
    }
    replace_file(&paths.skill_file, SKILL_MD)
        .map_err(|err| format!("could not write {}: {err}", paths.skill_file.display()))
}

fn install_claude(paths: &SkillPaths) -> Result<ClaudeInstall, String> {
    match inspect_claude(paths) {
        ClaudeEntry::Missing => create_claude(paths),
        ClaudeEntry::Link { ours: true, target } if link_points_at_canonical(paths, &target) => {
            Ok(ClaudeInstall::Linked {
                path: paths.claude_path.clone(),
                target: displayed_link_target(paths, &target),
            })
        }
        ClaudeEntry::Link { ours: true, .. } => {
            remove_link(&paths.claude_path).map_err(|err| {
                format!("could not replace {}: {err}", paths.claude_path.display())
            })?;
            create_claude(paths)
        }
        ClaudeEntry::Copy { ours: true } => {
            let copy = paths.claude_path.join("SKILL.md");
            replace_file(&copy, SKILL_MD)
                .map_err(|err| format!("could not write {}: {err}", copy.display()))?;
            Ok(ClaudeInstall::Copied {
                path: paths.claude_path.clone(),
            })
        }
        ClaudeEntry::Link { ours: false, .. }
        | ClaudeEntry::Copy { ours: false }
        | ClaudeEntry::Other => Err(format!(
            "refusing to overwrite {}",
            paths.claude_path.display()
        )),
    }
}

fn create_claude(paths: &SkillPaths) -> Result<ClaudeInstall, String> {
    ensure_parent_dir(&paths.claude_path)?;
    #[cfg(unix)]
    {
        create_unix_symlink(paths)
            .map_err(|err| format!("could not link {}: {err}", paths.claude_path.display()))?;
        Ok(ClaudeInstall::Linked {
            path: paths.claude_path.clone(),
            target: relative_from(
                paths.claude_path.parent().unwrap_or(&paths.claude_path),
                &paths.canonical_dir,
            ),
        })
    }
    #[cfg(windows)]
    {
        match windows::create_junction(&paths.claude_path, &paths.canonical_dir) {
            Ok(()) => Ok(ClaudeInstall::Linked {
                path: paths.claude_path.clone(),
                target: relative_from(
                    paths.claude_path.parent().unwrap_or(&paths.claude_path),
                    &paths.canonical_dir,
                ),
            }),
            Err(_) => create_windows_copy(paths),
        }
    }
}

#[cfg(unix)]
fn create_unix_symlink(paths: &SkillPaths) -> io::Result<()> {
    let parent = paths
        .claude_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "claude path has no parent"))?;
    let target = relative_from(parent, &paths.canonical_dir);
    std::os::unix::fs::symlink(target, &paths.claude_path)
}

#[cfg(windows)]
fn create_windows_copy(paths: &SkillPaths) -> Result<ClaudeInstall, String> {
    ensure_real_dir(&paths.claude_path)?;
    let copy = paths.claude_path.join("SKILL.md");
    replace_file(&copy, SKILL_MD)
        .map_err(|err| format!("could not write {}: {err}", copy.display()))?;
    Ok(ClaudeInstall::Copied {
        path: paths.claude_path.clone(),
    })
}

fn uninstall_canonical(paths: &SkillPaths) -> Result<Option<PathBuf>, String> {
    let meta = match fs::symlink_metadata(&paths.skill_file) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            remove_empty_real_dir(&paths.canonical_dir)?;
            return Ok(None);
        }
        Err(err) => {
            return Err(format!(
                "could not inspect {}: {err}",
                paths.skill_file.display()
            ));
        }
        Ok(meta) => meta,
    };
    if is_link(&meta) || !meta.is_file() {
        return Err(format!("refusing to remove {}", paths.skill_file.display()));
    }
    fs::remove_file(&paths.skill_file)
        .map_err(|err| format!("could not remove {}: {err}", paths.skill_file.display()))?;
    remove_empty_real_dir(&paths.canonical_dir)?;
    Ok(Some(paths.skill_file.clone()))
}

fn uninstall_claude(paths: &SkillPaths) -> Result<Option<PathBuf>, String> {
    match inspect_claude(paths) {
        ClaudeEntry::Missing => Ok(None),
        ClaudeEntry::Link { ours: true, .. } => {
            remove_link(&paths.claude_path).map_err(|err| {
                format!("could not remove {}: {err}", paths.claude_path.display())
            })?;
            Ok(Some(paths.claude_path.clone()))
        }
        ClaudeEntry::Copy { ours: true } => {
            remove_managed_copy(&paths.claude_path)?;
            Ok(Some(paths.claude_path.clone()))
        }
        ClaudeEntry::Link { ours: false, .. }
        | ClaudeEntry::Copy { ours: false }
        | ClaudeEntry::Other => Ok(None),
    }
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent)
        .map_err(|err| format!("could not create {}: {err}", parent.display()))
}

fn ensure_real_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => fs::create_dir(path)
            .map_err(|err| format!("could not create {}: {err}", path.display())),
        Ok(meta) if meta.is_dir() && !is_link(&meta) => Ok(()),
        Ok(_) => Err(format!(
            "refusing to use {}: not a directory",
            path.display()
        )),
        Err(err) => Err(format!("could not inspect {}: {err}", path.display())),
    }
}

fn replace_file(path: &Path, contents: &str) -> io::Result<()> {
    if fs::read(path).is_ok_and(|existing| existing == contents.as_bytes()) {
        return Ok(());
    }
    let tmp = path.with_file_name("SKILL.md.rozi-tmp");
    fs::write(&tmp, contents)?;
    if cfg!(windows) {
        let _ = fs::remove_file(path);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        windows::is_junction(meta)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn remove_link(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        fs::remove_file(path)
    } else {
        #[cfg(windows)]
        {
            if windows::is_junction(&meta) {
                return fs::remove_dir(path);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a compatibility link",
        ))
    }
}

fn remove_empty_real_dir(path: &Path) -> Result<(), String> {
    let meta = match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("could not inspect {}: {err}", path.display())),
        Ok(meta) => meta,
    };
    if is_link(&meta) || !meta.is_dir() {
        return Ok(());
    }
    let empty = fs::read_dir(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?
        .next()
        .is_none();
    if empty {
        fs::remove_dir(path)
            .map_err(|err| format!("could not remove {}: {err}", path.display()))?;
    }
    Ok(())
}

fn managed_copy_dir(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    let mut saw_skill = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == "SKILL.md" {
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                return false;
            };
            if !meta.is_file() || is_link(&meta) {
                return false;
            }
            saw_skill = true;
        } else {
            return false;
        }
    }
    saw_skill
}

fn remove_managed_copy(path: &Path) -> Result<(), String> {
    let skill = path.join("SKILL.md");
    match fs::symlink_metadata(&skill) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Ok(meta) if meta.is_file() && !is_link(&meta) => {
            fs::remove_file(&skill)
                .map_err(|err| format!("could not remove {}: {err}", skill.display()))?;
        }
        Ok(_) => {
            return Err(format!("refusing to remove {}", skill.display()));
        }
        Err(err) => {
            return Err(format!("could not inspect {}: {err}", skill.display()));
        }
    }
    remove_empty_real_dir(path)
}

fn read_compat_target(link: &Path) -> Option<PathBuf> {
    if let Ok(target) = fs::read_link(link) {
        return Some(resolve_against(link, &target));
    }
    fs::canonicalize(link).ok()
}

fn resolve_against(link: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        link.parent().unwrap_or(link).join(target)
    }
}

fn compat_link_is_ours(paths: &SkillPaths, target: &Path) -> bool {
    link_points_at_canonical(paths, target) || looks_like_agents_rozi_skill(target)
}

fn link_points_at_canonical(paths: &SkillPaths, target: &Path) -> bool {
    let resolved = resolve_against(&paths.claude_path, target);
    same_path(&resolved, &paths.canonical_dir)
        || fs::canonicalize(&resolved)
            .ok()
            .zip(fs::canonicalize(&paths.canonical_dir).ok())
            .is_some_and(|(left, right)| left == right)
}

fn displayed_link_target(paths: &SkillPaths, target: &Path) -> PathBuf {
    if target.is_absolute() {
        relative_from(
            paths.claude_path.parent().unwrap_or(&paths.claude_path),
            &paths.canonical_dir,
        )
    } else {
        target.to_path_buf()
    }
}

fn looks_like_agents_rozi_skill(path: &Path) -> bool {
    let mut parts = path
        .components()
        .rev()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        });
    parts.next().is_some_and(|name| name == "rozi")
        && parts.next().is_some_and(|name| name == "skills")
        && parts.next().is_some_and(|name| name == ".agents")
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let left_components: Vec<_> = left.components().collect();
    let right_components: Vec<_> = right.components().collect();
    left_components == right_components
}

fn relative_from(from_dir: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return to.iter().collect();
    }
    let mut relative = PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in &to[common..] {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
}

fn canonical_label(status: CanonicalStatus) -> &'static str {
    match status {
        CanonicalStatus::Installed => "installed",
        CanonicalStatus::Outdated => "outdated",
        CanonicalStatus::Missing => "missing",
    }
}

fn claude_label(status: ClaudeStatus) -> &'static str {
    match status {
        ClaudeStatus::Linked => "linked",
        ClaudeStatus::Copied => "copied",
        ClaudeStatus::Missing => "missing",
        ClaudeStatus::Conflict => "conflict",
        ClaudeStatus::NotApplicable => "not applicable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "rozi-skill-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time")
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).expect("scratch");
            Self(dir)
        }

        fn paths(&self) -> SkillPaths {
            SkillPaths::project(&self.0)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("read")
    }

    #[test]
    fn embedded_skill_is_a_well_formed_skill_document() {
        assert!(SKILL_MD.starts_with("---\nname: rozi"));
        assert!(SKILL_MD.contains("ROZI=1"));
        assert!(SKILL_MD.contains("rozi list-panes"));
    }

    #[test]
    fn project_and_global_paths_stay_under_the_given_root() {
        let project = SkillPaths::project(PathBuf::from("proj"));
        let home = SkillPaths::global(PathBuf::from("home"));
        assert_eq!(
            project.skill_file,
            project
                .scope_root
                .join(".agents")
                .join("skills")
                .join("rozi")
                .join("SKILL.md")
        );
        assert_eq!(
            project.claude_path,
            project
                .scope_root
                .join(".claude")
                .join("skills")
                .join("rozi")
        );
        assert_ne!(project.skill_file, home.skill_file);
        assert!(home.skill_file.starts_with(&home.scope_root));
    }

    #[test]
    fn conflict_leaves_status_conflict_after_a_failed_compat_install() {
        let scratch = Scratch::new("conflict-status");
        let paths = scratch.paths();
        fs::create_dir_all(&paths.claude_path).unwrap();
        fs::write(paths.claude_path.join("other.md"), "user").unwrap();
        let report = install(&paths, true).unwrap();
        assert!(matches!(report.claude, ClaudeInstall::Failed { .. }));
        assert_eq!(status(&paths, true).claude, ClaudeStatus::Conflict);
        assert_eq!(canonical_status(&paths), CanonicalStatus::Installed);
    }

    #[test]
    fn project_install_writes_the_embedded_skill() {
        let scratch = Scratch::new("install");
        let paths = scratch.paths();
        let report = install(&paths, false).expect("install");
        assert_eq!(read(&paths.skill_file), SKILL_MD);
        assert!(!paths.claude_path.exists());
        assert!(matches!(report.claude, ClaudeInstall::Skipped));
        assert_eq!(
            status(&paths, false),
            StatusReport {
                canonical: CanonicalStatus::Installed,
                claude: ClaudeStatus::NotApplicable,
            }
        );
    }

    #[test]
    fn global_install_uses_the_injected_home() {
        let scratch = Scratch::new("global");
        let paths = SkillPaths::global(&scratch.0);
        install(&paths, false).expect("install");
        assert_eq!(read(&paths.skill_file), SKILL_MD);
        assert!(paths.skill_file.starts_with(&scratch.0));
    }

    #[test]
    fn reinstall_is_idempotent_when_contents_match() {
        let scratch = Scratch::new("idempotent");
        let paths = scratch.paths();
        install(&paths, false).expect("first");
        let first = fs::metadata(&paths.skill_file)
            .expect("meta")
            .modified()
            .ok();
        install(&paths, false).expect("second");
        assert_eq!(read(&paths.skill_file), SKILL_MD);
        let second = fs::metadata(&paths.skill_file)
            .expect("meta")
            .modified()
            .ok();
        assert_eq!(first, second);
    }

    #[test]
    fn outdated_canonical_skill_is_replaced() {
        let scratch = Scratch::new("outdated");
        let paths = scratch.paths();
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        fs::write(&paths.skill_file, "stale\n").unwrap();
        assert_eq!(canonical_status(&paths), CanonicalStatus::Outdated);
        install(&paths, false).expect("install");
        assert_eq!(read(&paths.skill_file), SKILL_MD);
        assert_eq!(canonical_status(&paths), CanonicalStatus::Installed);
    }

    #[test]
    fn status_reports_missing_before_install() {
        let scratch = Scratch::new("missing");
        let paths = scratch.paths();
        assert_eq!(
            status(&paths, true),
            StatusReport {
                canonical: CanonicalStatus::Missing,
                claude: ClaudeStatus::Missing,
            }
        );
    }

    #[test]
    fn uninstall_removes_the_skill_and_is_idempotent() {
        let scratch = Scratch::new("uninstall");
        let paths = scratch.paths();
        install(&paths, false).expect("install");
        let first = uninstall(&paths).expect("uninstall");
        assert_eq!(first.removed, vec![paths.skill_file.clone()]);
        assert!(!paths.skill_file.exists());
        assert!(!paths.canonical_dir.exists());
        assert!(scratch.0.join(".agents").exists());
        let second = uninstall(&paths).expect("again");
        assert!(second.removed.is_empty());
    }

    #[test]
    fn project_and_global_installs_do_not_share_files() {
        let scratch = Scratch::new("isolate");
        let project_root = scratch.0.join("proj");
        let home_root = scratch.0.join("home");
        fs::create_dir_all(&project_root).unwrap();
        fs::create_dir_all(&home_root).unwrap();
        let project = SkillPaths::project(&project_root);
        let global = SkillPaths::global(&home_root);
        install(&project, false).unwrap();
        install(&global, false).unwrap();
        assert_ne!(project.skill_file, global.skill_file);
        assert!(project.skill_file.exists());
        assert!(global.skill_file.exists());
        uninstall(&project).unwrap();
        assert!(!project.skill_file.exists());
        assert!(global.skill_file.exists());
    }

    #[test]
    fn partial_install_recovers_a_missing_canonical_file() {
        let scratch = Scratch::new("partial");
        let paths = scratch.paths();
        install(&paths, true).unwrap();
        fs::remove_file(&paths.skill_file).unwrap();
        install(&paths, true).unwrap();
        assert_eq!(read(&paths.skill_file), SKILL_MD);
        assert_eq!(canonical_status(&paths), CanonicalStatus::Installed);
    }

    #[test]
    fn canonical_install_survives_a_claude_compat_failure() {
        let scratch = Scratch::new("compat-fail");
        let paths = scratch.paths();
        fs::write(scratch.0.join(".claude"), "not a directory").unwrap();
        let report = install(&paths, true).expect("canonical still installs");
        assert_eq!(read(&paths.skill_file), SKILL_MD);
        assert!(matches!(report.claude, ClaudeInstall::Failed { .. }));
    }

    #[test]
    fn print_skill_emits_only_the_embedded_document() {
        let mut buffer = Vec::new();
        {
            use std::io::Write;
            write!(&mut buffer, "{SKILL_MD}").unwrap();
        }
        assert_eq!(buffer, SKILL_MD.as_bytes());
        assert!(!SKILL_MD.contains("Installed Rozi skill"));
    }

    #[test]
    fn claude_detected_creates_a_compatibility_entry() {
        let scratch = Scratch::new("claude-yes");
        let paths = scratch.paths();
        let report = install(&paths, true).expect("install");
        assert!(matches!(
            report.claude,
            ClaudeInstall::Linked { .. } | ClaudeInstall::Copied { .. }
        ));
        assert!(
            status(&paths, true).claude == ClaudeStatus::Linked
                || status(&paths, true).claude == ClaudeStatus::Copied
        );
    }

    #[test]
    fn claude_not_detected_skips_compatibility() {
        let scratch = Scratch::new("claude-no");
        let paths = scratch.paths();
        install(&paths, false).unwrap();
        assert!(!paths.claude_path.exists());
        assert_eq!(status(&paths, false).claude, ClaudeStatus::NotApplicable);
    }

    #[test]
    fn existing_correct_compatibility_entry_is_left_alone() {
        let scratch = Scratch::new("claude-ok");
        let paths = scratch.paths();
        install(&paths, true).unwrap();
        let first = inspect_claude(&paths);
        install(&paths, true).unwrap();
        match (first, inspect_claude(&paths)) {
            (ClaudeEntry::Link { target: a, .. }, ClaudeEntry::Link { target: b, .. }) => {
                assert_eq!(a, b);
            }
            (ClaudeEntry::Copy { ours: true }, ClaudeEntry::Copy { ours: true }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn stale_rozi_managed_link_is_repaired() {
        let scratch = Scratch::new("stale");
        let paths = scratch.paths();
        install(&paths, false).unwrap();
        fs::create_dir_all(paths.claude_path.parent().unwrap()).unwrap();
        let old = scratch
            .0
            .join("old")
            .join(".agents")
            .join("skills")
            .join("rozi");
        fs::create_dir_all(&old).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&old, &paths.claude_path).unwrap();
        #[cfg(windows)]
        windows::create_junction(&paths.claude_path, &old)
            .or_else(|_| {
                fs::create_dir_all(&paths.claude_path)?;
                fs::write(paths.claude_path.join("SKILL.md"), "stale")
            })
            .unwrap();
        install(&paths, true).unwrap();
        assert!(
            status(&paths, true).claude == ClaudeStatus::Linked
                || status(&paths, true).claude == ClaudeStatus::Copied
        );
        assert!(paths.canonical_dir.exists());
    }

    #[test]
    fn unrelated_symlink_is_a_conflict() {
        let scratch = Scratch::new("unrelated");
        let paths = scratch.paths();
        fs::create_dir_all(paths.claude_path.parent().unwrap()).unwrap();
        let other = scratch.0.join("elsewhere");
        fs::create_dir_all(&other).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&other, &paths.claude_path).unwrap();
        #[cfg(windows)]
        {
            if windows::create_junction(&paths.claude_path, &other).is_err() {
                fs::create_dir_all(&paths.claude_path).unwrap();
                fs::write(paths.claude_path.join("notes.md"), "mine").unwrap();
            }
        }
        let report = install(&paths, true).unwrap();
        assert_eq!(read(&paths.skill_file), SKILL_MD);
        assert!(matches!(report.claude, ClaudeInstall::Failed { .. }));
        assert_eq!(status(&paths, true).claude, ClaudeStatus::Conflict);
    }

    #[test]
    fn real_claude_skill_directory_is_a_conflict() {
        let scratch = Scratch::new("real-dir");
        let paths = scratch.paths();
        fs::create_dir_all(&paths.claude_path).unwrap();
        fs::write(paths.claude_path.join("other.md"), "user").unwrap();
        let report = install(&paths, true).unwrap();
        assert!(matches!(report.claude, ClaudeInstall::Failed { .. }));
        assert_eq!(status(&paths, true).claude, ClaudeStatus::Conflict);
        assert!(paths.claude_path.join("other.md").exists());
        uninstall(&paths).unwrap();
        assert!(paths.claude_path.join("other.md").exists());
    }

    #[test]
    fn uninstall_removes_only_a_verified_compat_entry() {
        let scratch = Scratch::new("uninstall-compat");
        let paths = scratch.paths();
        install(&paths, true).unwrap();
        assert!(paths.claude_path.exists() || fs::symlink_metadata(&paths.claude_path).is_ok());
        uninstall(&paths).unwrap();
        assert!(fs::symlink_metadata(&paths.claude_path).is_err());
        assert!(!paths.skill_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unix_compat_is_a_directory_symlink_and_unlink_is_not_recursive() {
        let scratch = Scratch::new("unix-link");
        let paths = scratch.paths();
        install(&paths, true).unwrap();
        let meta = fs::symlink_metadata(&paths.claude_path).unwrap();
        assert!(meta.file_type().is_symlink());
        let target = fs::read_link(&paths.claude_path).unwrap();
        assert_eq!(
            target,
            Path::new("..")
                .join("..")
                .join(".agents")
                .join("skills")
                .join("rozi")
        );
        fs::remove_file(&paths.claude_path).unwrap();
        assert!(paths.skill_file.exists());
        assert_eq!(read(&paths.skill_file), SKILL_MD);
    }

    #[test]
    fn path_construction_uses_pathbuf_joins() {
        let paths = SkillPaths::project(PathBuf::from("scope"));
        assert_eq!(
            paths.skill_file,
            paths
                .scope_root
                .join(".agents")
                .join("skills")
                .join("rozi")
                .join("SKILL.md")
        );
        assert_eq!(
            paths.claude_path,
            paths.scope_root.join(".claude").join("skills").join("rozi")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_copy_refreshes_when_embedded_contents_would_change() {
        let scratch = Scratch::new("win-copy");
        let paths = scratch.paths();
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        fs::write(&paths.skill_file, SKILL_MD).unwrap();
        fs::create_dir_all(&paths.claude_path).unwrap();
        fs::write(paths.claude_path.join("SKILL.md"), "stale").unwrap();
        assert_eq!(status(&paths, true).claude, ClaudeStatus::Copied);
        install(&paths, true).unwrap();
        assert_eq!(read(&paths.claude_path.join("SKILL.md")), SKILL_MD);
    }

    #[cfg(windows)]
    #[test]
    fn uninstalling_a_windows_compat_copy_does_not_delete_the_canonical_skill() {
        let scratch = Scratch::new("win-un");
        let paths = scratch.paths();
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        fs::write(&paths.skill_file, SKILL_MD).unwrap();
        fs::create_dir_all(&paths.claude_path).unwrap();
        fs::write(paths.claude_path.join("SKILL.md"), SKILL_MD).unwrap();
        remove_managed_copy(&paths.claude_path).unwrap();
        assert!(!paths.claude_path.exists());
        assert_eq!(read(&paths.skill_file), SKILL_MD);
    }

    #[test]
    fn relative_link_from_claude_skills_points_at_agents_skills() {
        let from = Path::new("/proj/.claude/skills");
        let to = Path::new("/proj/.agents/skills/rozi");
        assert_eq!(
            relative_from(from, to),
            Path::new("..")
                .join("..")
                .join(".agents")
                .join("skills")
                .join("rozi")
        );
    }
}
